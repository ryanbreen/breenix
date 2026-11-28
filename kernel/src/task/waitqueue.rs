//! Wait queues: the kernel binding of the event queue (`task::event_queue`).
//!
//! Clean-room implementation (E2.1; rulings DEV-36(2), DEV-37(1a)), written
//! from brief B1 in `docs/cleanroom/derivation-assessment.md` and the E2
//! design. `event_queue` holds the queue and its protocol; this module supplies
//! its hooks from the scheduler, keeps the caller-facing names, and holds the
//! sleep itself.
//!
//! A wait has four steps: register and publish (`prepare_to_wait_checked`,
//! or `add_waiter` plus the caller's own publication for a multi-queue wait),
//! check, sleep (`schedule_current_wait`), and finish (`finish_wait`). A wake
//! is a hint: whoever returns from the sleep rechecks its condition.

use super::event_queue::{EventQueue, Hooks};
use super::thread::ThreadState;

pub use super::event_queue::PrepareOutcome;

/// The scheduler and CPU operations the event queue uses.
struct KernelHooks;

impl Hooks for KernelHooks {
    type Irq = crate::irq_safe_mutex::IrqState;

    #[inline(always)]
    fn mask_irqs(&self) -> Self::Irq {
        crate::irq_safe_mutex::IrqState::save_and_mask()
    }

    #[inline(always)]
    fn restore_irqs(&self, saved: Self::Irq) {
        saved.restore();
    }

    fn current_tid(&self) -> Option<u64> {
        crate::task::scheduler::current_thread_id()
    }

    /// ARM64 records `queue` in the thread's wait-site record for the lockup
    /// capture.
    #[cfg(target_arch = "aarch64")]
    fn publish_blocked(&self, tid: u64, deadline_ns: Option<u64>, queue: usize) -> bool {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current_for_io_publish(deadline_ns, queue as u64) == Some(tid)
        })
        .unwrap_or(false)
    }

    /// x86-64 publishes under this CPU's run-queue lock. It keeps no
    /// wait-site record, so the queue identity has no use here.
    #[cfg(not(target_arch = "aarch64"))]
    fn publish_blocked(&self, tid: u64, deadline_ns: Option<u64>, _queue: usize) -> bool {
        crate::task::scheduler::with_current(|sched| {
            sched.current_thread_id_inner() == Some(tid) && sched.block_current_for_io_with_timeout(deadline_ns)
        })
        .unwrap_or(false)
    }

    fn end_wait(&self, tid: u64) {
        crate::task::scheduler::with_thread_mut(tid, |thread| {
            thread.blocked_in_syscall = false;
            if thread.state == ThreadState::BlockedOnIO {
                thread.set_running();
                thread.wake_time_ns = None;
            }
        });
    }

    fn wake_task(&self, tid: u64) {
        crate::task::scheduler::wake_waitqueue_thread(tid);
    }

    fn wake_from_interrupt(&self, tid: u64) {
        crate::task::scheduler::isr_unblock_for_io(tid);
    }
}

/// A queue of threads waiting for one event.
pub struct WaitQueueHead {
    queue: EventQueue,
}

impl Default for WaitQueueHead {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitQueueHead {
    /// An empty queue, usable in a `static`.
    pub const fn new() -> Self {
        Self { queue: EventQueue::new() }
    }

    /// Register the current thread and publish it in `state`, which must be
    /// `BlockedOnIO`. Returns its TID; if it could not be registered and
    /// published, it returns no TID and is not registered.
    pub fn prepare_to_wait(&self, state: ThreadState) -> Option<u64> {
        if state != ThreadState::BlockedOnIO {
            return None;
        }
        self.queue.prepare(&KernelHooks, None)
    }

    /// Evaluate `cond`, register the current thread and publish it in
    /// `state` (`BlockedOnIO`) with the optional absolute deadline
    /// `wake_time_ns`, in one critical section with respect to this queue's
    /// wakers. Registers and publishes only if `cond` holds.
    pub(crate) fn prepare_to_wait_checked<F: FnOnce() -> bool>(
        &self,
        state: ThreadState,
        wake_time_ns: Option<u64>,
        cond: F,
    ) -> PrepareOutcome {
        if state != ThreadState::BlockedOnIO {
            return PrepareOutcome::PublishFailed;
        }
        self.queue.prepare_checked(&KernelHooks, wake_time_ns, cond)
    }

    /// End the current thread's wait on this queue, however it ended.
    pub fn finish_wait(&self) {
        self.queue.finish(&KernelHooks);
    }

    /// End thread `tid`'s wait on this queue, however it ended. `tid` is the
    /// calling thread, named by the caller so its identity is the one it
    /// registered with.
    pub(crate) fn finish_wait_for(&self, tid: u64) {
        self.queue.finish_for(&KernelHooks, tid);
    }

    /// Wake the registered threads.
    pub fn wake_up(&self) {
        self.queue.wake_all(&KernelHooks);
    }

    /// Wake the registered threads without entering the scheduler lock, for
    /// callers that hold the process manager or another lock the scheduler
    /// path takes. The owner CPU's inbox drain makes each one runnable.
    pub fn wake_up_deferred(&self) {
        self.queue.wake_all_deferred(&KernelHooks);
    }

    /// Wake the oldest registered thread.
    pub fn wake_up_one(&self) {
        self.queue.wake_n(&KernelHooks, 1);
    }

    /// Wake up to `max` registered threads, oldest first. Returns the count.
    pub fn wake_up_n(&self, max: u32) -> u32 {
        self.queue.wake_n(&KernelHooks, max)
    }

    /// From hard interrupt context: wake up to `max` registered threads
    /// without spinning on the queue lock. Returns 0 when the lock is busy;
    /// the queue is then unchanged, and its holder pays the wakes when it
    /// releases the lock.
    pub(crate) fn try_wake_up_n_from_irq(&self, max: u32) -> u32 {
        self.queue.wake_n_from_irq(&KernelHooks, max)
    }

    /// Register `tid` without publishing it, for a wait on several queues.
    /// False when there was no memory for the entry.
    pub(crate) fn add_waiter(&self, tid: u64) -> bool {
        self.queue.add(&KernelHooks, tid)
    }

    /// Unlink up to `out.len()` registered threads, oldest first, without
    /// waking them. Returns the count.
    pub(crate) fn take_waiters(&self, out: &mut [u64]) -> usize {
        self.queue.take_oldest(&KernelHooks, out)
    }

    /// Unlink `tid` without waking it. True when it was registered.
    pub fn take_waiter(&self, tid: u64) -> bool {
        self.queue.take(&KernelHooks, tid)
    }

    /// Whether any thread is registered.
    pub fn has_waiters(&self) -> bool {
        self.queue.has_waiters(&KernelHooks)
    }

    /// Whether `tid` is registered.
    #[cfg(feature = "boot_tests")]
    pub(crate) fn contains_waiter(&self, tid: u64) -> bool {
        self.queue.contains(&KernelHooks, tid)
    }
}

/// x86-64: halt a thread that is published blocked, letting another thread
/// queued on this CPU run first. The blocked check is redone with interrupts
/// masked before the halt, so a wake that lands after it ends the halt
/// (B1 requirement 11). Callers loop until they are no longer blocked.
#[cfg(not(target_arch = "aarch64"))]
pub(crate) fn halt_blocked_current() {
    let others_ready = crate::task::scheduler::with_current(|sched| sched.has_queued_work_here()).unwrap_or(false);
    if others_ready {
        crate::task::scheduler::yield_current();
    }
    crate::arch_halt_while_blocked();
}

/// Sleep while the current thread is published blocked, and return once it
/// is runnable. The caller holds one preemption level (a syscall's entry
/// level, or one it took); it is released for the sleep and taken back before
/// returning, and the entry interrupt state is restored (B1 requirement 9).
///
/// ARM64 enters through this stub so the caller's return address reaches
/// `sleep_from` before a Rust prologue can change it; the lockup capture then
/// names the blocked call site (B1 requirement 10).
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
pub extern "C" fn schedule_current_wait() {
    core::arch::naked_asm!("mov x0, x30", "b {}", sym sleep_from);
}

#[cfg(target_arch = "aarch64")]
extern "C" fn sleep_from(return_pc: u64) {
    sleep_while_blocked(return_pc);
}

/// Sleep while the current thread is published blocked (see the ARM64 entry).
#[cfg(not(target_arch = "aarch64"))]
pub fn schedule_current_wait() {
    sleep_while_blocked();
}

fn current_still_blocked() -> bool {
    crate::task::scheduler::with_current(|sched| {
        sched
            .current_thread_mut()
            .is_some_and(|thread| thread.state == ThreadState::BlockedOnIO)
    })
    .unwrap_or(false)
}

/// ARM64: name the blocked site in the wait-site record the publication made,
/// and return how many times the scheduler has saved this thread while it
/// was blocked on I/O, so the return can tell whether the wait parked.
#[cfg(target_arch = "aarch64")]
fn note_wait_site(return_pc: u64) -> u64 {
    crate::task::scheduler::with_scheduler(|sched| {
        sched.current_thread_mut().map(|thread| {
            thread.wait_site.set_pc(return_pc);
            thread.io_park_epoch.load(core::sync::atomic::Ordering::Relaxed)
        })
    })
    .flatten()
    .unwrap_or(0)
}

/// ARM64: count the wait's return in the thread's progress record, as parked
/// or as returning without a park.
#[cfg(target_arch = "aarch64")]
fn note_wait_return(parks_before: u64) {
    // SAFETY: the current thread's record stays live while it runs.
    if let Some(thread) = unsafe { super::thread::arm_current_thread_ptr().as_ref() } {
        let parks = thread.io_park_epoch.load(core::sync::atomic::Ordering::Relaxed);
        thread.lockup_progress.record_wait_return(parks != parks_before);
    }
}

fn sleep_while_blocked(#[cfg(target_arch = "aarch64")] return_pc: u64) {
    #[cfg(target_arch = "aarch64")]
    let parks_before = note_wait_site(return_pc);
    let irqs_were_enabled = crate::arch_interrupts_enabled();
    let release = crate::per_cpu::preempt_count() & crate::arch_impl::current::constants::PREEMPT_MASK != 0;
    if release {
        crate::per_cpu::preempt_enable();
    }
    loop {
        if !current_still_blocked() {
            break;
        }
        #[cfg(target_arch = "aarch64")]
        crate::arch_halt_while_blocked();
        #[cfg(not(target_arch = "aarch64"))]
        halt_blocked_current();
    }
    if release {
        crate::per_cpu::preempt_disable();
    }
    if !irqs_were_enabled {
        // SAFETY: the caller entered with interrupts masked and expects them
        // masked on return.
        unsafe { crate::arch_disable_interrupts() };
    }
    #[cfg(target_arch = "aarch64")]
    note_wait_return(parks_before);
}
