//! Task-context suffix and orphan finalization. Idle waits are event driven;
//! only a spent guard budget or an I/O failure needs a timed retry.

use crate::task::thread::ThreadState;
use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};
use core::sync::atomic::{AtomicBool, Ordering};

static PENDING: AtomicBool = AtomicBool::new(false);
static WORK: WaitQueueHead = WaitQueueHead::new();

/// Publish before waking. Handle drops can run under the process manager, so
/// deliver the wake through the deferred I/O path rather than locking the
/// scheduler inline. No filesystem guard or disk I/O belongs in this path.
pub(super) fn request() {
    PENDING.store(true, Ordering::Release);
    WORK.wake_up_deferred();
}

pub fn init() -> Result<(), &'static str> {
    crate::task::kthread::kthread_run(service, "ext2-writeback")
        .map(|_| ())
        .map_err(|_| "Failed to start ext2 finalization service")
}

fn wait(waiters: &WaitQueueHead, deadline: Option<u64>, condition: impl FnOnce() -> bool) {
    match waiters.prepare_to_wait_checked(ThreadState::BlockedOnIO, deadline, condition) {
        PrepareOutcome::Mismatch => return,
        PrepareOutcome::PublishFailed => panic!("Finalizer lost its scheduler thread"),
        PrepareOutcome::Queued => {}
    }
    #[cfg(target_arch = "x86_64")]
    crate::task::scheduler::yield_current();
    crate::task::waitqueue::schedule_current_wait();
    waiters.finish_wait();
}

/// Called with the same single preemption brake as a syscall. Every caller
/// has released its filesystem guard before giving another writer a turn.
pub(super) fn pause(nanoseconds: u64) {
    let waiters = WaitQueueHead::new();
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    let deadline = (seconds as u64).saturating_mul(1_000_000_000)
        .saturating_add(nanos as u64).saturating_add(nanoseconds);
    wait(&waiters, Some(deadline), || true);
}

fn service() {
    loop {
        // Device waits release this syscall-style brake while blocked and
        // restore it on return. Restore the kthread's zero count each pass.
        #[cfg(target_arch = "aarch64")]
        crate::per_cpu_aarch64::preempt_disable();
        #[cfg(target_arch = "x86_64")]
        crate::per_cpu::preempt_disable();

        let mut more = false;
        let mut retry = false;
        if PENDING.swap(false, Ordering::AcqRel) {
            for home in [false, true] {
                let mut guard = super::fs_write_raw(home);
                if let Some(fs) = guard.as_mut() {
                    match fs.finalize_inactive() {
                        super::Finalize::Idle => {}
                        super::Finalize::More => more = true,
                        super::Finalize::Retry => retry = true,
                    }
                }
            }
        }
        if more || retry {
            PENDING.store(true, Ordering::Release);
            pause(if more { 1_000_000 } else { 100_000_000 });
        } else {
            // The condition and publication are serialized with request's
            // wake under WORK's lock, closing both sides of the sleep race.
            wait(&WORK, None, || !PENDING.load(Ordering::Acquire));
        }

        #[cfg(target_arch = "aarch64")]
        crate::per_cpu_aarch64::preempt_enable();
        #[cfg(target_arch = "x86_64")]
        crate::per_cpu::preempt_enable();
    }
}
