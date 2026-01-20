//! Linux-style work queues for deferred execution.
//!
//! Work queues allow kernel code to schedule work to run in process context
//! (i.e., in a kernel thread that can sleep), rather than interrupt context.
//!
//! # Architecture
//!
//! - `Work`: A unit of deferred work containing a closure
//! - `Workqueue`: Manages a queue of work items and a worker thread
//! - System workqueue: A global default workqueue for general use
//!
//! # Example
//!
//! ```rust,ignore
//! use kernel::task::workqueue::{schedule_work_fn, Work};
//!
//! // Schedule work on the system workqueue
//! let work = schedule_work_fn(|| {
//!     log::info!("Deferred work executing!");
//! }, "example_work");
//!
//! // Optionally wait for completion
//! work.wait();
//! ```

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use spin::Mutex;

use super::kthread::{kthread_join, kthread_park, kthread_run, kthread_should_stop, kthread_stop, kthread_unpark, KthreadHandle};
use super::scheduler;

/// Work states
const WORK_IDLE: u8 = 0;
const WORK_PENDING: u8 = 1;
const WORK_RUNNING: u8 = 2;

/// A unit of deferred work.
///
/// Work items are created with a closure that will be executed by a worker thread.
/// The work can be queued to a workqueue and waited on for completion.
pub struct Work {
    /// The function to execute (wrapped in Option for take() semantics)
    func: UnsafeCell<Option<Box<dyn FnOnce() + Send + 'static>>>,
    /// Current state: Idle -> Pending -> Running -> Idle
    state: AtomicU8,
    /// Set to true after func returns
    completed: AtomicBool,
    /// Thread ID waiting for completion (NO_WAITER = u64::MAX means no waiter)
    waiter: AtomicU64,
    /// Debug name for this work item
    name: &'static str,
}

// SAFETY: Work is Send because:
// - func is only accessed by the worker thread (via take())
// - All other fields are atomic or immutable
unsafe impl Send for Work {}
// SAFETY: Work is Sync because:
// - func access is serialized (queued once, executed once)
// - All other fields are atomic or immutable
unsafe impl Sync for Work {}

/// Sentinel value for "no waiter" - u64::MAX instead of 0
/// because TID 0 is a valid thread ID (the idle thread)
const NO_WAITER: u64 = u64::MAX;

impl Work {
    /// Create a new work item with the given function and debug name.
    pub fn new<F>(func: F, name: &'static str) -> Arc<Work>
    where
        F: FnOnce() + Send + 'static,
    {
        Arc::new(Work {
            func: UnsafeCell::new(Some(Box::new(func))),
            state: AtomicU8::new(WORK_IDLE),
            completed: AtomicBool::new(false),
            waiter: AtomicU64::new(NO_WAITER),
            name,
        })
    }

    /// Check if this work item has completed execution.
    #[allow(dead_code)] // Part of public API for callers to poll completion status
    pub fn is_completed(&self) -> bool {
        self.completed.load(Ordering::Acquire)
    }

    /// Wait for this work item to complete.
    ///
    /// If the work is already complete, returns immediately.
    /// Otherwise, yields to allow the worker thread to run until completion.
    ///
    /// This uses a spin-yield pattern similar to kthread_park():
    /// - Set need_resched flag to hint scheduler
    /// - HLT to wait for timer interrupt (allows context switch)
    /// - Check completion after each wakeup
    ///
    /// Wait for this work item to complete using proper block/wake semantics.
    ///
    /// This follows the Linux wait_for_completion() pattern:
    /// 1. Register as waiter
    /// 2. Check condition under interrupt lock
    /// 3. If not complete: mark Blocked, remove from ready queue
    /// 4. Atomically enable interrupts and halt
    /// 5. When woken by execute()'s unblock(), check condition again
    pub fn wait(&self) {
        // Fast path: already completed
        if self.completed.load(Ordering::Acquire) {
            return;
        }

        // Get our thread ID - handle early boot case where scheduler isn't ready
        let tid = match scheduler::current_thread_id() {
            Some(id) => id,
            None => {
                // No scheduler yet (early boot): spin loop is acceptable
                while !self.completed.load(Ordering::Acquire) {
                    core::hint::spin_loop();
                }
                return;
            }
        };

        // Register as waiter so execute() can wake us via unblock()
        self.waiter.store(tid, Ordering::Release);

        // Block/wake loop - proper Linux-style blocking
        loop {
            // CRITICAL: Disable interrupts for the check-and-block sequence.
            // This prevents the race where:
            //   1. We check completed (false)
            //   2. Worker sets completed=true and calls unblock()
            //   3. We mark ourselves Blocked (missed wakeup!)
            let completed = x86_64::instructions::interrupts::without_interrupts(|| {
                // Check if completed while interrupts disabled
                if self.completed.load(Ordering::Acquire) {
                    return true;
                }

                // Not complete - mark ourselves as Blocked
                // The scheduler will see this and switch to another thread
                scheduler::with_scheduler(|sched| {
                    if let Some(thread) = sched.current_thread_mut() {
                        thread.state = super::thread::ThreadState::Blocked;
                    }
                    // Remove from ready queue (we're blocked, not runnable)
                    sched.remove_from_ready_queue(tid);
                });

                false
            });

            if completed {
                break;
            }

            // Now we're Blocked and removed from ready queue.
            // Atomically enable interrupts and halt - this is critical!
            // If we enabled then halted separately, an interrupt could fire
            // between them and we'd miss it.
            //
            // When timer fires, can_schedule() sees we're Blocked and allows
            // scheduling. schedule() picks the worker thread. Worker runs,
            // completes work, calls unblock(tid) which sets us Ready and
            // adds us back to ready queue. Eventually we get scheduled again.
            x86_64::instructions::interrupts::enable_and_hlt();

            // We've been woken up - loop back and check if completed
        }

        // Clear waiter
        self.waiter.store(NO_WAITER, Ordering::Release);
    }

    /// Get the debug name of this work item.
    #[allow(dead_code)] // Part of public API for debugging and logging
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Transition from Idle to Pending. Returns false if not Idle.
    fn try_set_pending(&self) -> bool {
        self.state
            .compare_exchange(WORK_IDLE, WORK_PENDING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Execute this work item (called by worker thread).
    fn execute(&self) {
        // Transition to Running
        self.state.store(WORK_RUNNING, Ordering::Release);

        // Take and execute the function
        // SAFETY: Only the worker thread calls execute(), and only once
        let func = unsafe { (*self.func.get()).take() };
        if let Some(func) = func {
            func();
        }

        // Mark complete and transition back to Idle
        self.state.store(WORK_IDLE, Ordering::Release);
        self.completed.store(true, Ordering::Release);

        // Wake waiter if any
        // Use NO_WAITER (u64::MAX) as sentinel since TID 0 is valid
        let waiter = self.waiter.load(Ordering::Acquire);
        if waiter != NO_WAITER {
            scheduler::with_scheduler(|sched| {
                sched.unblock(waiter);
            });
        }
    }
}

/// Flags for workqueue creation (reserved for future use).
#[derive(Default)]
pub struct WorkqueueFlags {
    // Future: max_workers, priority, cpu_affinity, etc.
}

/// A workqueue manages a queue of work items and a worker thread.
pub struct Workqueue {
    /// Queue of pending work items
    queue: Mutex<VecDeque<Arc<Work>>>,
    /// Worker thread handle (created on first queue)
    worker: Mutex<Option<KthreadHandle>>,
    /// Shutdown flag - signals worker to exit
    shutdown: AtomicBool,
    /// Debug name for this workqueue
    name: &'static str,
}

impl Workqueue {
    /// Create a new workqueue with the given name.
    pub fn new(name: &'static str, _flags: WorkqueueFlags) -> Arc<Workqueue> {
        Arc::new(Workqueue {
            queue: Mutex::new(VecDeque::new()),
            worker: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            name,
        })
    }

    /// Queue work for execution. Returns false if work is already pending.
    ///
    /// The work item must be in the Idle state to be queued.
    pub fn queue(self: &Arc<Self>, work: Arc<Work>) -> bool {
        // Reject if already pending
        if !work.try_set_pending() {
            log::warn!(
                "workqueue({}): work '{}' already pending, rejecting",
                self.name,
                work.name
            );
            return false;
        }

        // Add to queue
        self.queue.lock().push_back(work);

        // Ensure worker thread exists and wake it
        self.ensure_worker();

        true
    }

    /// Wait for all pending work to complete (flush the queue).
    pub fn flush(&self) {
        // Create a sentinel work item
        let sentinel = Work::new(|| {}, "flush_sentinel");

        // Queue and wait for sentinel - all work before it will be complete
        if sentinel.try_set_pending() {
            self.queue.lock().push_back(Arc::clone(&sentinel));
            self.wake_worker();
            sentinel.wait();
        }
    }

    /// Destroy this workqueue, stopping the worker thread.
    ///
    /// All pending work will be completed before destruction.
    pub fn destroy(&self) {
        // First, flush all pending work to ensure completion
        self.flush();

        // Signal shutdown
        self.shutdown.store(true, Ordering::Release);

        // Stop worker thread
        let worker = self.worker.lock().take();
        if let Some(handle) = worker {
            // Wake worker so it sees shutdown flag
            kthread_unpark(&handle);
            // Signal stop
            let _ = kthread_stop(&handle);
            // Wait for worker thread to actually exit
            let _ = kthread_join(&handle);
        }
    }

    /// Ensure worker thread exists, creating it if needed.
    fn ensure_worker(self: &Arc<Self>) {
        let mut worker_guard = self.worker.lock();
        if worker_guard.is_none() {
            let wq = Arc::clone(self);
            let thread_name = self.name;
            match kthread_run(
                move || {
                    worker_thread_fn(wq);
                },
                thread_name,
            ) {
                Ok(handle) => {
                    log::info!("KWORKER_SPAWN: {} started", thread_name);
                    *worker_guard = Some(handle);
                }
                Err(e) => {
                    log::error!("workqueue({}): failed to create worker: {:?}", self.name, e);
                }
            }
        } else {
            // Worker exists, just wake it
            if let Some(ref handle) = *worker_guard {
                kthread_unpark(handle);
            }
        }
    }

    /// Wake the worker thread (if it exists).
    fn wake_worker(&self) {
        if let Some(ref handle) = *self.worker.lock() {
            kthread_unpark(handle);
        }
    }
}

impl Drop for Workqueue {
    fn drop(&mut self) {
        self.destroy();
    }
}

/// Worker thread main function.
fn worker_thread_fn(wq: Arc<Workqueue>) {
    // Enable interrupts for preemption
    x86_64::instructions::interrupts::enable();

    log::debug!("workqueue({}): worker thread started", wq.name);

    while !wq.shutdown.load(Ordering::Acquire) && !kthread_should_stop() {
        // Try to get work from queue
        let work = wq.queue.lock().pop_front();

        match work {
            Some(work) => {
                log::debug!("workqueue({}): executing work '{}'", wq.name, work.name);
                work.execute();
            }
            None => {
                // No work available, park until woken
                kthread_park();
            }
        }
    }

    log::debug!("workqueue({}): worker thread exiting", wq.name);
}

// =============================================================================
// System Workqueue (Global Default)
// =============================================================================

/// Global system workqueue for general use.
static SYSTEM_WQ: Mutex<Option<Arc<Workqueue>>> = Mutex::new(None);

/// Initialize the workqueue subsystem.
///
/// Creates the system workqueue. Must be called during boot after kthread
/// infrastructure is ready.
pub fn init_workqueue() {
    let wq = Workqueue::new("kworker/0", WorkqueueFlags::default());
    *SYSTEM_WQ.lock() = Some(wq);
    log::info!("WORKQUEUE_INIT: workqueue system initialized");
}

/// Schedule work on the system workqueue.
///
/// Returns true if work was queued, false if already pending.
pub fn schedule_work(work: Arc<Work>) -> bool {
    if let Some(ref wq) = *SYSTEM_WQ.lock() {
        wq.queue(work)
    } else {
        log::error!("schedule_work: system workqueue not initialized");
        false
    }
}

/// Create and schedule a work item on the system workqueue.
///
/// Convenience function that creates a Work item and queues it in one step.
/// Returns the Work handle for waiting on completion.
pub fn schedule_work_fn<F>(func: F, name: &'static str) -> Arc<Work>
where
    F: FnOnce() + Send + 'static,
{
    let work = Work::new(func, name);
    if !schedule_work(Arc::clone(&work)) {
        log::warn!("schedule_work_fn: failed to queue work '{}'", name);
    }
    work
}

/// Flush the system workqueue, waiting for all pending work to complete.
pub fn flush_system_workqueue() {
    if let Some(ref wq) = *SYSTEM_WQ.lock() {
        wq.flush();
    }
}
