//! Mount exclusion that sleeps when contended (E24 coordinator ledger 309, C1).
//!
//! A mount's holder reads nodes and data through the buffer cache and sleeps
//! on block I/O with the exclusion held. Under a spin lock its contenders keep
//! their CPUs with preemption disabled (x86 syscalls also mask interrupts)
//! while the holder waits for one to finish on: ext2's #728. Contenders here
//! park on a wait queue instead, as ext2's #728 park path does
//! (fs/ext2/mod.rs, ext2_acquire), and spin only where the context cannot
//! sleep. Holding the exclusion changes no preemption count, so a holder's
//! block waits keep the single preemption brake they assert.
use core::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// One park round. A missed wake becomes a retry at this granularity rather
/// than a hang, as ext2's #728 park bounds its rounds.
#[cfg(not(test))]
const PARK_ROUND_NS: u64 = 200_000_000;

pub struct MountLock<T> {
    held: AtomicBool,
    // Contenders between announcing a park and finishing it. Release wakes
    // only when this is nonzero. The release's store-then-load and the
    // contender's increment-then-recheck are SeqCst, so either the release
    // sees the contender or the contender's recheck sees the lock free.
    parked: AtomicUsize,
    #[cfg(not(test))]
    queue: crate::task::waitqueue::WaitQueueHead,
    #[cfg(test)]
    host: HostPark,
    value: UnsafeCell<T>,
}

// The exclusion hands out one guard at a time, as spin::Mutex does.
unsafe impl<T: Send> Send for MountLock<T> {}
unsafe impl<T: Send> Sync for MountLock<T> {}

#[must_use]
pub struct MountGuard<'a, T> {
    lock: &'a MountLock<T>,
}

impl<T> MountLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            held: AtomicBool::new(false),
            parked: AtomicUsize::new(0),
            #[cfg(not(test))]
            queue: crate::task::waitqueue::WaitQueueHead::new(),
            #[cfg(test)]
            host: HostPark::default(),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> MountGuard<'_, T> {
        loop {
            if let Some(guard) = self.try_lock() {
                return guard;
            }
            self.park();
        }
    }

    pub fn try_lock(&self) -> Option<MountGuard<'_, T>> {
        self.held
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .ok()
            .map(|_| {
                #[cfg(test)]
                super::preempt::context::observe(super::preempt::context::Event::Acquire(self as *const Self as usize));
                MountGuard { lock: self }
            })
    }

    // Release first, then wake: no mount state is held across the wake.
    fn release(&self) {
        #[cfg(test)]
        super::preempt::context::observe(super::preempt::context::Event::Release(self as *const Self as usize));
        self.held.store(false, Ordering::SeqCst);
        if self.parked.load(Ordering::SeqCst) != 0 {
            self.wake();
        }
    }

    #[cfg(not(test))]
    fn park(&self) {
        use crate::task::{
            thread::ThreadState,
            waitqueue::{schedule_current_wait, PrepareOutcome},
        };
        #[cfg(target_arch = "x86_64")]
        use crate::per_cpu as cpu;
        #[cfg(target_arch = "aarch64")]
        use crate::per_cpu_aarch64 as cpu;
        if !can_sleep() {
            core::hint::spin_loop();
            return;
        }
        // schedule_current_wait releases exactly one preemption brake.
        let add_brake = cpu::preempt_count() == 0;
        if add_brake {
            cpu::preempt_disable();
        }
        self.parked.fetch_add(1, Ordering::SeqCst);
        let deadline = super::commit_signal::monotonic_ns().saturating_add(PARK_ROUND_NS);
        // The recheck runs under the queue lock, atomically with publishing
        // the wait, so a release before the enqueue is seen here.
        match self
            .queue
            .prepare_to_wait_checked(ThreadState::BlockedOnIO, Some(deadline), || {
                self.held.load(Ordering::SeqCst)
            }) {
            PrepareOutcome::Queued => {
                schedule_current_wait();
                self.queue.finish_wait();
            }
            PrepareOutcome::Mismatch => {}
            PrepareOutcome::PublishFailed | PrepareOutcome::OutOfMemory => {
                crate::block::cache_kernel::park_for_wait_retry();
            }
        }
        self.parked.fetch_sub(1, Ordering::SeqCst);
        if add_brake {
            cpu::preempt_enable();
        }
    }

    #[cfg(not(test))]
    fn wake(&self) {
        // The exclusion precedes PROCESS_MANAGER in the design 1.1 lock
        // order. Its nested PROCESS_MANAGER section (Mapping::publish's bind
        // from syscall/mmap.rs) ends before the guard drops.
        self.queue.wake_up();
    }

    #[cfg(test)]
    fn park(&self) {
        self.parked.fetch_add(1, Ordering::SeqCst);
        self.host.parks.fetch_add(1, Ordering::SeqCst);
        let mut sleep = self.host.sleep.lock().unwrap();
        while self.held.load(Ordering::SeqCst) {
            sleep = self
                .host
                .wake
                .wait_timeout(sleep, std::time::Duration::from_millis(50))
                .unwrap()
                .0;
        }
        drop(sleep);
        self.parked.fetch_sub(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn wake(&self) {
        let _sleep = self.host.sleep.lock().unwrap();
        self.host.wake.notify_all();
    }

    /// Host checks: contended acquisitions that parked.
    #[cfg(test)]
    pub fn parks(&self) -> u64 {
        self.host.parks.load(Ordering::SeqCst)
    }

    /// Host checks: whether a guard is live.
    #[cfg(test)]
    pub fn is_held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }
}

/// A contended acquisition may park only from a thread, outside interrupt
/// context, with at most the one preemption brake a syscall or the commit
/// worker holds and, on aarch64, with IRQs unmasked once the timer runs (the
/// conditions of ext2's #728 park).
#[cfg(not(test))]
fn can_sleep() -> bool {
    if crate::task::scheduler::current_thread_id().is_none() {
        return false;
    }
    #[cfg(target_arch = "aarch64")]
    {
        !crate::per_cpu_aarch64::in_interrupt()
            && crate::arch_impl::aarch64::cpu::interrupts_enabled()
            && crate::per_cpu_aarch64::preempt_count() <= 1
            && crate::arch_impl::aarch64::timer_interrupt::is_initialized()
    }
    #[cfg(target_arch = "x86_64")]
    {
        !crate::per_cpu::in_interrupt() && crate::per_cpu::preempt_count() <= 1
    }
}

#[cfg(test)]
#[derive(Default)]
struct HostPark {
    parks: core::sync::atomic::AtomicU64,
    sleep: std::sync::Mutex<()>,
    wake: std::sync::Condvar,
}

impl<T> Deref for MountGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        #[cfg(test)]
        super::preempt::context::observe(super::preempt::context::Event::Access(self.lock as *const MountLock<T> as usize));
        // SAFETY: a live guard is the only holder of the exclusion.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for MountGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        #[cfg(test)]
        super::preempt::context::observe(super::preempt::context::Event::Access(self.lock as *const MountLock<T> as usize));
        // SAFETY: a live guard is the only holder of the exclusion.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for MountGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.release();
    }
}
