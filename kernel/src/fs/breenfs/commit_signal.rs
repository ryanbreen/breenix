//! Worker requests and generation completion, separate from mount exclusion.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub struct CommitSignal {
    pub requested: AtomicBool,
    pub pressure: AtomicBool,
    pub started: AtomicBool,
    epoch: AtomicU64,
    #[cfg(not(test))]
    queue: crate::task::waitqueue::WaitQueueHead,
}
impl CommitSignal {
    pub fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
            pressure: AtomicBool::new(false),
            started: AtomicBool::new(false),
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
    pub fn wait(&self, observed: u64, deadline: Option<u64>) {
        #[cfg(test)]
        {
            let _ = (observed, deadline);
            std::thread::yield_now();
        }
        #[cfg(not(test))]
        {
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
            if self
                .queue
                .prepare_to_wait_checked(ThreadState::BlockedOnIO, deadline, || {
                    self.epoch() == observed
                })
                == PrepareOutcome::Queued
            {
                crate::task::waitqueue::schedule_current_wait();
                self.queue.finish_wait();
            }
            if add_brake {
                cpu::preempt_enable();
            }
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
