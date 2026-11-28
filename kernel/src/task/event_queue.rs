//! The event queue: the queue half of the kernel's sleep-and-wake primitive.
//!
//! Clean-room implementation (E2.1, rulings DEV-36(2), DEV-37(1a), DEV-38(4)),
//! written from brief B1 in `docs/cleanroom/derivation-assessment.md`, the E2
//! design (`docs/plans/e2-sleepable-contexts.md` §3.3-§3.4), the public
//! interface the callers use, and the memory-ordering chapters of the Intel SDM
//! (Vol. 3A §9.2) and the Arm ARM (DDI 0487, B2). A wake is a hint and the
//! waiter rechecks its condition (Lampson and Redell, CACM 23(2), 1980).
//!
//! This file has no kernel dependencies. Everything it needs from the
//! scheduler and the CPU comes through [`Hooks`], so the host model in
//! `tools/waitqueue-host-tests` drives this same code against a recording
//! scheduler. `task::waitqueue` binds it to the real scheduler.
//!
//! # The protocol
//!
//! A queue is a list of thread IDs, oldest first, under one lock that masks
//! local interrupts while held, and a counter of wakes owed by interrupt
//! handlers that found the lock busy.
//!
//! - A waiter registers and publishes itself blocked under the queue lock, and
//!   for a checked wait evaluates its condition in the same critical section.
//!   A waker makes the condition true before it takes the lock. So either the
//!   waker's critical section comes first, and the waiter's condition check
//!   sees the change, or the waiter's comes first, and the waker finds it
//!   registered and already published blocked (B1 requirements 1-2).
//! - A multi-queue waiter registers on each queue without publishing, then
//!   publishes once under the scheduler lock, then rechecks. A wake that finds
//!   it registered but not yet published goes through the scheduler lock
//!   before the waiter's publication, so the recheck after the publication
//!   sees the condition.
//! - A task-context waker hands each chosen thread to the scheduler and
//!   unlinks it afterwards (B1 requirement 7). A deferred or interrupt waker
//!   hands it to the scheduler's lock-free inbox instead, without taking the
//!   scheduler lock (B1 requirements 4-5).
//! - An interrupt waker does not spin on the lock. When its try-lock fails it
//!   adds its count to `irq_wakes_owed`, issues a full barrier and tries once
//!   more. Each unlock issues a full barrier after releasing the lock and then
//!   reads the counter. That is the store-buffering shape of design §3.4: with
//!   a full barrier on both sides, either the second try-lock succeeds or the
//!   holder that made it fail reads the count, so the owed wake is paid. The
//!   payer hands the threads to the lock-free inbox, which is safe whatever
//!   context the unlocking caller is in. claim-lint:ok: the held-lock and
//!   in-section cases are tools/waitqueue-host-tests/src/tests.rs
//!   `interrupt_wake_under_a_held_lock_is_paid_by_the_holder` and
//!   `interrupt_inside_the_waiters_critical_section_is_not_lost`.

use alloc::vec::Vec;
use core::sync::atomic::{fence, AtomicU32, Ordering};
use spin::{Mutex, MutexGuard};

/// What the queue needs from the scheduler and the CPU.
///
/// The kernel's implementation is `task::waitqueue::KernelHooks`; the host
/// model's is a recording scheduler.
pub trait Hooks {
    /// The saved local interrupt state.
    type Irq;

    /// Save the local interrupt state and mask interrupts.
    fn mask_irqs(&self) -> Self::Irq;

    /// Restore a state saved by `mask_irqs`.
    fn restore_irqs(&self, saved: Self::Irq);

    /// The current thread, if there is one.
    fn current_tid(&self) -> Option<u64>;

    /// Publish thread `tid`, the current thread, blocked for I/O with an
    /// optional absolute deadline in nanoseconds. `queue` names the queue for
    /// diagnostics. Called with the queue lock held. False when the thread may
    /// not sleep; the thread's state is then unchanged.
    fn publish_blocked(&self, tid: u64, deadline_ns: Option<u64>, queue: usize) -> bool;

    /// End `tid`'s wait: if it is still published blocked, it runs again, and
    /// it is no longer marked blocked in a syscall. A no-op for a thread that
    /// is already running.
    fn end_wait(&self, tid: u64);

    /// Task-context wake: make `tid` runnable if it is published blocked. May
    /// take the scheduler lock, but not a queue lock.
    fn wake_task(&self, tid: u64);

    /// Lock-free wake through the scheduler's per-task inbox. Safe from hard
    /// interrupt context and while holding locks the scheduler path takes.
    fn wake_from_interrupt(&self, tid: u64);
}

/// The result of a checked registration (`EventQueue::prepare_checked`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareOutcome {
    /// The condition did not hold. The thread was not registered or published.
    Mismatch,
    /// The thread is registered and published blocked.
    Queued,
    /// The thread could not be published blocked. It is not registered.
    PublishFailed,
    /// Capacity for the registration could not be reserved. The thread was not
    /// registered or published.
    OutOfMemory,
}

/// A queue of threads waiting for one event.
pub struct EventQueue {
    /// Registered thread IDs, oldest first. One entry per thread.
    waiters: Mutex<Vec<u64>>,
    /// Wakes requested by interrupt handlers that found the lock busy, paid
    /// by whoever releases the lock next.
    irq_wakes_owed: AtomicU32,
}

/// The queue lock, held with local interrupts masked. Dropping it releases
/// the lock, restores the interrupt state, and pays any owed wakes.
struct Locked<'a, H: Hooks> {
    queue: &'a EventQueue,
    hooks: &'a H,
    held: Option<(MutexGuard<'a, Vec<u64>>, H::Irq)>,
}

impl<H: Hooks> Locked<'_, H> {
    fn list(&mut self) -> &mut Vec<u64> {
        match self.held.as_mut() {
            Some((list, _)) => list,
            None => unreachable!("queue lock released before its guard"),
        }
    }
}

impl<H: Hooks> Drop for Locked<'_, H> {
    fn drop(&mut self) {
        if let Some((list, irq)) = self.held.take() {
            drop(list);
            self.hooks.restore_irqs(irq);
        }
        self.queue.pay_owed_wakes(self.hooks);
    }
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl EventQueue {
    /// An empty queue, usable in a `static`.
    pub const fn new() -> Self {
        Self {
            waiters: Mutex::new(Vec::new()),
            irq_wakes_owed: AtomicU32::new(0),
        }
    }

    /// The identity diagnostics record for a wait on this queue.
    pub fn id(&self) -> usize {
        self as *const Self as usize
    }

    fn lock<'a, H: Hooks>(&'a self, hooks: &'a H) -> Locked<'a, H> {
        let irq = hooks.mask_irqs();
        let list = self.waiters.lock();
        Locked { queue: self, hooks, held: Some((list, irq)) }
    }

    fn try_lock<'a, H: Hooks>(&'a self, hooks: &'a H) -> Option<Locked<'a, H>> {
        let irq = hooks.mask_irqs();
        match self.waiters.try_lock() {
            Some(list) => Some(Locked { queue: self, hooks, held: Some((list, irq)) }),
            None => {
                hooks.restore_irqs(irq);
                None
            }
        }
    }

    /// Pay owed interrupt wakes. Runs after each unlock. Takes the lock only
    /// by try-lock, so it does not spin: if another holder has the lock, that
    /// holder's own unlock reads the count (module doc).
    fn pay_owed_wakes<H: Hooks>(&self, hooks: &H) {
        loop {
            fence(Ordering::SeqCst);
            if self.irq_wakes_owed.load(Ordering::SeqCst) == 0 {
                return;
            }
            let irq = hooks.mask_irqs();
            let Some(mut list) = self.waiters.try_lock() else {
                hooks.restore_irqs(irq);
                return;
            };
            let owed = self.irq_wakes_owed.swap(0, Ordering::SeqCst);
            let n = (owed as usize).min(list.len());
            for &tid in &list[..n] {
                hooks.wake_from_interrupt(tid);
            }
            list.drain(..n);
            drop(list);
            hooks.restore_irqs(irq);
        }
    }

    /// Register `tid` if it is not registered yet. `Ok(true)` when this call
    /// added it, `Ok(false)` when it was already there, `Err(())` when there
    /// was no memory for the entry; the list is then unchanged.
    fn register(list: &mut Vec<u64>, tid: u64) -> Result<bool, ()> {
        if list.contains(&tid) {
            return Ok(false);
        }
        list.try_reserve(1).map_err(|_| ())?;
        list.push(tid);
        Ok(true)
    }

    fn unregister(list: &mut Vec<u64>, tid: u64) -> bool {
        match list.iter().position(|&queued| queued == tid) {
            Some(index) => {
                list.remove(index);
                true
            }
            None => false,
        }
    }

    /// Register the current thread and publish it blocked, with no
    /// condition. Returns its TID; without a current thread, memory for the
    /// entry, or permission to sleep, it returns no TID and is not registered.
    pub fn prepare<H: Hooks>(&self, hooks: &H, deadline_ns: Option<u64>) -> Option<u64> {
        let tid = hooks.current_tid()?;
        let mut locked = self.lock(hooks);
        Self::register(locked.list(), tid).ok()?;
        if hooks.publish_blocked(tid, deadline_ns, self.id()) {
            Some(tid)
        } else {
            Self::unregister(locked.list(), tid);
            None
        }
    }

    /// Evaluate `cond`, register the current thread and publish it blocked, in
    /// one critical section with respect to this queue's wakers. Registers and
    /// publishes only if `cond` holds (B1 requirement 2).
    pub fn prepare_checked<H: Hooks, F: FnOnce() -> bool>(
        &self,
        hooks: &H,
        deadline_ns: Option<u64>,
        cond: F,
    ) -> PrepareOutcome {
        let Some(tid) = hooks.current_tid() else {
            return PrepareOutcome::PublishFailed;
        };
        let mut locked = self.lock(hooks);
        if !cond() {
            return PrepareOutcome::Mismatch;
        }
        if Self::register(locked.list(), tid).is_err() {
            return PrepareOutcome::OutOfMemory;
        }
        if hooks.publish_blocked(tid, deadline_ns, self.id()) {
            PrepareOutcome::Queued
        } else {
            Self::unregister(locked.list(), tid);
            PrepareOutcome::PublishFailed
        }
    }

    /// End the current thread's wait on this queue (`finish_for`).
    pub fn finish<H: Hooks>(&self, hooks: &H) {
        if let Some(tid) = hooks.current_tid() {
            self.finish_for(hooks, tid);
        }
    }

    /// End `tid`'s wait however it ended: unlink it, then return it to
    /// running if it is still published blocked (B1 requirement 6).
    pub fn finish_for<H: Hooks>(&self, hooks: &H, tid: u64) {
        {
            let mut locked = self.lock(hooks);
            Self::unregister(locked.list(), tid);
        }
        hooks.end_wait(tid);
    }

    /// Task-context wake of the registered threads. Returns the count.
    pub fn wake_all<H: Hooks>(&self, hooks: &H) -> u32 {
        self.wake_n(hooks, u32::MAX)
    }

    /// Task-context wake of up to `max` registered threads, oldest first.
    /// Each is handed to the scheduler before it is unlinked. Returns the
    /// count.
    pub fn wake_n<H: Hooks>(&self, hooks: &H, max: u32) -> u32 {
        let mut locked = self.lock(hooks);
        let list = locked.list();
        let n = list.len().min(max as usize);
        for &tid in &list[..n] {
            hooks.wake_task(tid);
        }
        list.drain(..n);
        n as u32
    }

    /// Wake the registered threads through the lock-free inbox, for callers
    /// that may not enter the scheduler lock (they hold the process manager,
    /// for example). Returns the count.
    pub fn wake_all_deferred<H: Hooks>(&self, hooks: &H) -> u32 {
        let mut locked = self.lock(hooks);
        let list = locked.list();
        for &tid in list.iter() {
            hooks.wake_from_interrupt(tid);
        }
        let n = list.len();
        list.clear();
        n as u32
    }

    /// Wake up to `max` registered threads from hard interrupt context,
    /// without spinning on the queue lock. When the lock is busy, returns 0, leaves the
    /// list unchanged, and records the wakes as owed; the lock's holder pays
    /// them when it unlocks (module doc).
    pub fn wake_n_from_irq<H: Hooks>(&self, hooks: &H, max: u32) -> u32 {
        if max == 0 {
            return 0;
        }
        if let Some(mut locked) = self.try_lock(hooks) {
            let list = locked.list();
            let n = list.len().min(max as usize);
            for &tid in &list[..n] {
                hooks.wake_from_interrupt(tid);
            }
            list.drain(..n);
            return n as u32;
        }
        let _ = self
            .irq_wakes_owed
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |owed| Some(owed.saturating_add(max)));
        // The second attempt: pay_owed_wakes issues the full barrier, then
        // try-locks. If that fails too, the holder that made it fail pays.
        self.pay_owed_wakes(hooks);
        0
    }

    /// Register `tid` without publishing it (multi-queue waits). True when it
    /// is registered afterwards; false when there was no memory.
    pub fn add<H: Hooks>(&self, hooks: &H, tid: u64) -> bool {
        let mut locked = self.lock(hooks);
        Self::register(locked.list(), tid).is_ok()
    }

    /// Unlink `tid` without waking it. True when it was registered.
    pub fn take<H: Hooks>(&self, hooks: &H, tid: u64) -> bool {
        let mut locked = self.lock(hooks);
        Self::unregister(locked.list(), tid)
    }

    /// Unlink up to `out.len()` registered threads, oldest first, into `out`,
    /// without waking them. Returns the count.
    pub fn take_oldest<H: Hooks>(&self, hooks: &H, out: &mut [u64]) -> usize {
        let mut locked = self.lock(hooks);
        let list = locked.list();
        let n = list.len().min(out.len());
        out[..n].copy_from_slice(&list[..n]);
        list.drain(..n);
        n
    }

    /// Whether any thread is registered.
    pub fn has_waiters<H: Hooks>(&self, hooks: &H) -> bool {
        let mut locked = self.lock(hooks);
        !locked.list().is_empty()
    }

    /// Whether `tid` is registered.
    pub fn contains<H: Hooks>(&self, hooks: &H, tid: u64) -> bool {
        let mut locked = self.lock(hooks);
        locked.list().contains(&tid)
    }

    /// Interrupt wakes recorded and not yet paid.
    pub fn irq_wakes_owed(&self) -> u32 {
        self.irq_wakes_owed.load(Ordering::SeqCst)
    }

    /// Hold the queue lock for the duration of `f`, as a contending task
    /// would. The host model uses it to place an interrupt wake inside a held
    /// lock; releasing the lock afterwards pays what the interrupt owed.
    pub fn with_lock_held<H: Hooks, R>(&self, hooks: &H, f: impl FnOnce() -> R) -> R {
        let _locked = self.lock(hooks);
        f()
    }
}
