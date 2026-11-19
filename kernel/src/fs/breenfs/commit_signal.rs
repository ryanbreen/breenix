//! Worker requests and generation completion, separate from mount exclusion.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::fs::errno::Errno;

pub struct CommitSignal {
    pub requested: AtomicBool,
    pub pressure: AtomicBool,
    pub mappings: AtomicBool,
    pub started: AtomicBool,
    pub stop: AtomicBool,
    // Worker requests do not change admission or complete an fsync.
    pub worker: WakeSignal,
    waiters: WakeSignal,
}

pub struct WakeSignal {
    epoch: AtomicU64,
    #[cfg(not(test))]
    queue: crate::task::waitqueue::WaitQueueHead,
}

impl WakeSignal {
    fn new() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            #[cfg(not(test))]
            queue: crate::task::waitqueue::WaitQueueHead::new(),
        }
    }
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    pub fn wake(&self) {
        self.epoch.fetch_add(1, Ordering::Release);
        #[cfg(not(test))]
        self.queue.wake_up_deferred();
    }
    #[cfg(not(test))]
    pub fn wait(&self, observed: u64, deadline: Option<u64>) -> Result<(), Errno> {
        use crate::task::{thread::ThreadState, waitqueue::PrepareOutcome};
        crate::block::cache_kernel::assert_io_context();
        #[cfg(target_arch = "x86_64")]
        use crate::per_cpu as cpu;
        #[cfg(target_arch = "aarch64")]
        use crate::per_cpu_aarch64 as cpu;
        let add_brake = cpu::preempt_count() == 0;
        if add_brake {
            cpu::preempt_disable();
        }
        assert_eq!(
            cpu::preempt_count(),
            1,
            "breenfs wait has nested preemption guards"
        );
        let outcome = self
            .queue
            .prepare_to_wait_checked(ThreadState::BlockedOnIO, deadline, || {
                self.epoch() == observed
            });
        let result = match outcome {
            PrepareOutcome::Queued => {
                crate::task::waitqueue::schedule_current_wait();
                self.queue.finish_wait();
                Ok(())
            }
            PrepareOutcome::Mismatch => Ok(()),
            PrepareOutcome::OutOfMemory => {
                if deadline.is_none_or(|limit| monotonic_ns() < limit) {
                    crate::block::cache_kernel::park_for_wait_retry();
                }
                Ok(())
            }
            // A kill can refuse a blocked publication after the caller
            // checked its condition. Unwind instead of retrying that wait.
            PrepareOutcome::PublishFailed => Err(Errno(crate::syscall::errno::EINTR)),
        };
        if add_brake {
            cpu::preempt_enable();
        }
        result
    }
}

impl CommitSignal {
    pub fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
            pressure: AtomicBool::new(false),
            mappings: AtomicBool::new(false),
            started: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            worker: WakeSignal::new(),
            waiters: WakeSignal::new(),
        }
    }
    pub fn epoch(&self) -> u64 {
        self.waiters.epoch()
    }
    pub fn wake_waiters(&self) {
        self.waiters.wake();
    }
    // An install or failure makes both publication and waiter work runnable.
    pub fn wake_all(&self) {
        self.wake_waiters();
        self.worker.wake();
    }
    pub fn wait(&self, observed: u64, deadline: Option<u64>) -> Result<(), Errno> {
        #[cfg(not(test))]
        {
            self.waiters.wait(observed, deadline)
        }
        #[cfg(test)]
        {
            super::preempt::context::observe(super::preempt::context::Event::CommitWait {
                signal: self as *const Self as usize,
                epoch: observed,
            });
            let _ = deadline;
            std::thread::yield_now();
            Ok(())
        }
    }
}

pub fn monotonic_ns() -> u64 {
    #[cfg(test)]
    {
        0
    }
    #[cfg(not(test))]
    {
        let (seconds, nanos) = crate::time::get_monotonic_time_ns();
        (seconds as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(nanos as u64)
    }
}
