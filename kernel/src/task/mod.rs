use alloc::boxed::Box;
use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU64, Ordering},
    task::{Context, Poll},
};

// Core task/thread modules - shared across architectures
pub mod executor;
pub mod thread;

// Architecture-specific context switching
#[cfg(target_arch = "x86_64")]
pub mod context;

// Scheduler and preemption - requires per_cpu which is x86_64 only for now
#[cfg(target_arch = "x86_64")]
pub mod scheduler;

// Kernel threads and workqueues - depend on scheduler
#[cfg(target_arch = "x86_64")]
pub mod kthread;
#[cfg(target_arch = "x86_64")]
pub mod workqueue;
#[cfg(target_arch = "x86_64")]
pub mod softirqd;

// Process-related modules - depend on process module which is x86_64 only
#[cfg(target_arch = "x86_64")]
pub mod process_context;
#[cfg(target_arch = "x86_64")]
pub mod process_task;
#[cfg(target_arch = "x86_64")]
pub mod spawn;

// Re-export kthread public API for kernel-wide use (x86_64 only)
// These are intentionally available but may not be called yet
#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use kthread::{
    kthread_exit, kthread_join, kthread_park, kthread_run, kthread_should_stop, kthread_stop,
    kthread_unpark, KthreadError, KthreadHandle,
};

// Re-export workqueue public API for kernel-wide use (x86_64 only)
#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use workqueue::{
    flush_system_workqueue, init_workqueue, schedule_work, schedule_work_fn, Work, Workqueue,
    WorkqueueFlags,
};

// Re-export softirqd public API for kernel-wide use (x86_64 only)
#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use softirqd::{
    init_softirq, raise_softirq, register_softirq_handler, shutdown_softirq, SoftirqHandler,
    SoftirqType,
};

#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId(u64);

impl TaskId {
    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        TaskId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
pub struct Task {
    id: TaskId,
    future: Pin<Box<dyn Future<Output = ()>>>,
}

impl Task {
    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    pub fn new(future: impl Future<Output = ()> + 'static) -> Task {
        Task {
            id: TaskId::new(),
            future: Box::pin(future),
        }
    }

    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    fn poll(&mut self, context: &mut Context) -> Poll<()> {
        self.future.as_mut().poll(context)
    }
}
