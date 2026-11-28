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
    /// Sleep until the epoch moves past `observed` or `deadline`. A claimed
    /// kill ends the wait with EINTR: the kill's wake returns the thread to
    /// the park, and its publication is then refused (`task::park`). A
    /// context that may not park gets a receipt and EINTR, so its caller
    /// unwinds. Since E2.4 the boot continuation reads no storage.
    #[cfg(not(test))]
    pub fn wait(&self, observed: u64, deadline: Option<u64>) -> Result<(), Errno> {
        use crate::task::{park::{self, WaitEnd, WaitMode}, thread::current_wait_must_unwind};
        const INTERRUPTED: Errno = Errno(crate::syscall::errno::EINTR);
        if current_wait_must_unwind() { return Err(INTERRUPTED); }
        crate::block::cache_kernel::assert_io_context();
        let end = park::wait(&self.queue, || self.epoch() != observed, deadline, WaitMode::Uninterruptible);
        let result = match end {
            Ok(WaitEnd::Condition | WaitEnd::Woken | WaitEnd::Timeout | WaitEnd::Interrupted(_)) => Ok(()),
            Ok(WaitEnd::NoMemory) => {
                if !current_wait_must_unwind() && deadline.is_none_or(|limit| monotonic_ns() < limit) {
                    crate::block::cache_kernel::park_for_wait_retry();
                }
                Ok(())
            }
            // A kill can refuse a blocked publication after the caller
            // checked its condition. Unwind instead of retrying that wait.
            Ok(WaitEnd::Killed) | Err(_) => Err(INTERRUPTED),
        };
        if current_wait_must_unwind() { Err(INTERRUPTED) } else { result }
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
