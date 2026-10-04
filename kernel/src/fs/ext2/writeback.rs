//! Task-context inode finalization. Metadata drops only set an atomic hint;
//! the service owns retries and never runs inside process/root reclamation.

use core::sync::atomic::Ordering;

pub fn init() -> Result<(), &'static str> {
    crate::task::kthread::kthread_run(service, "ext2-writeback")
        .map(|_| ())
        .map_err(|_| "Failed to start ext2 finalization service")
}

fn service() {
    let timer = crate::task::completion::Completion::new();
    loop {
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
        // The timed wake is also the backstop for drops under PM and failed I/O.
        let _ = timer.wait_timeout_uninterruptible(1, 100_000_000);
    }
}
