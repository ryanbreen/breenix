//! Task-context inode finalization. Unlink reclaims an unobserved orphan
//! itself; this service handles orphans whose last descriptor closes later
//! and retries failed reclamation. A handle drop only sets an atomic hint, so
//! the service takes no filesystem guard unless an orphan is queued.

use crate::task::thread::ThreadState;
use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};
use core::sync::atomic::Ordering;

pub fn init() -> Result<(), &'static str> {
    crate::task::kthread::kthread_run(service, "ext2-writeback")
        .map(|_| ())
        .map_err(|_| "Failed to start ext2 finalization service")
}

fn service() {
    let waiters = WaitQueueHead::new();
    loop {
        // ext2/device wait helpers expect the same single scheduling brake as
        // a syscall. They release it when blocking and restore it on return.
        // This kthread arrives with count zero and must restore that count at
        // the end of each iteration. IRQs remain enabled throughout the work.
        #[cfg(target_arch = "aarch64")]
        crate::per_cpu_aarch64::preempt_disable();
        #[cfg(target_arch = "x86_64")]
        crate::per_cpu::preempt_disable();

        if super::live_inode::FINALIZATION_PENDING.swap(false, Ordering::AcqRel) {
            for home in [false, true] {
                let mut guard = if home {
                    super::home_fs_write()
                } else {
                    super::root_fs_write()
                };
                if let Some(fs) = guard.as_mut() {
                    if fs.finalize_inactive().is_err() {
                        super::live_inode::FINALIZATION_PENDING.store(true, Ordering::Release);
                    }
                }
            }
        }

        // No FS/index guard crosses this timed scheduler wait. Completion's
        // count-zero fallback is boot polling, so it is not an idle-kthread
        // timer. Parking here also gives persistent I/O failures a retry delay.
        let (seconds, nanos) = crate::time::get_monotonic_time_ns();
        let deadline = (seconds as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(nanos as u64)
            .saturating_add(100_000_000);
        let prepared =
            waiters.prepare_to_wait_checked(ThreadState::BlockedOnIO, Some(deadline), || true);
        assert_eq!(
            prepared,
            PrepareOutcome::Queued,
            "Finalizer lost its scheduler thread"
        );
        crate::task::waitqueue::schedule_current_wait();
        waiters.finish_wait();

        #[cfg(target_arch = "aarch64")]
        crate::per_cpu_aarch64::preempt_enable();
        #[cfg(target_arch = "x86_64")]
        crate::per_cpu::preempt_enable();
    }
}
