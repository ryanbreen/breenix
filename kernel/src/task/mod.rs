use alloc::boxed::Box;
use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU64, Ordering},
    task::{Context, Poll},
};

pub mod context;
pub mod executor;
pub mod kthread;
pub mod process_context;
pub mod process_task;
pub mod scheduler;
pub mod spawn;
pub mod thread;
pub mod workqueue;

// Re-export kthread public API for kernel-wide use
// These are intentionally available but may not be called yet
#[allow(unused_imports)]
pub use kthread::{
    kthread_exit, kthread_join, kthread_park, kthread_run, kthread_should_stop, kthread_stop,
    kthread_unpark, KthreadError, KthreadHandle,
};

// Re-export workqueue public API for kernel-wide use
#[allow(unused_imports)]
pub use workqueue::{
    flush_system_workqueue, init_workqueue, schedule_work, schedule_work_fn, Work, Workqueue,
    WorkqueueFlags,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId(u64);

impl TaskId {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        TaskId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

pub struct Task {
    id: TaskId,
    future: Pin<Box<dyn Future<Output = ()>>>,
}

impl Task {
    pub fn new(future: impl Future<Output = ()> + 'static) -> Task {
        Task {
            id: TaskId::new(),
            future: Box::pin(future),
        }
    }

    fn poll(&mut self, context: &mut Context) -> Poll<()> {
        self.future.as_mut().poll(context)
    }
}
