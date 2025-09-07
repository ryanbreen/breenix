//! Preemptive scheduler implementation
//!
//! This module implements a round-robin scheduler for kernel threads.

use super::thread::{Thread, ThreadState};
use alloc::{boxed::Box, collections::VecDeque};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Global scheduler instance
static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Global need_resched flag for timer interrupt
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

/// The kernel scheduler
pub struct Scheduler {
    /// All threads in the system
    threads: alloc::vec::Vec<Box<Thread>>,

    /// Ready queue (thread IDs)
    ready_queue: VecDeque<u64>,

    /// Currently running thread ID
    current_thread: Option<u64>,

    /// Idle thread ID (runs when no other threads are ready)
    idle_thread: u64,
}

impl Scheduler {
    /// Create a new scheduler with an idle thread
    pub fn new(idle_thread: Box<Thread>) -> Self {
        let idle_id = idle_thread.id();
        let scheduler = Self {
            threads: alloc::vec![idle_thread],
            ready_queue: VecDeque::new(),
            current_thread: Some(idle_id),
            idle_thread: idle_id,
        };

        // Don't put idle thread in ready queue
        // It runs only when nothing else is ready

        scheduler
    }

    /// Add a new thread to the scheduler
    pub fn add_thread(&mut self, thread: Box<Thread>) {
        let thread_id = thread.id();
        let thread_name = thread.name.clone();
        let is_user = thread.privilege == super::thread::ThreadPrivilege::User;
        self.threads.push(thread);
        self.ready_queue.push_back(thread_id);
        log::info!(
            "Added thread {} '{}' to scheduler (user: {}, ready_queue: {:?})",
            thread_id,
            thread_name,
            is_user,
            self.ready_queue
        );
    }

    /// Get a mutable thread by ID
    pub fn get_thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
        self.threads
            .iter_mut()
            .find(|t| t.id() == id)
            .map(|t| t.as_mut())
    }

    /// Get the current running thread
    pub fn current_thread(&self) -> Option<&Thread> {
        self.current_thread.and_then(|id| self.get_thread(id))
    }

    /// Get the current running thread mutably
    pub fn current_thread_mut(&mut self) -> Option<&mut Thread> {
        self.current_thread
            .and_then(move |id| self.get_thread_mut(id))
    }

    /// Schedule the next thread to run
    /// Returns (old_thread, new_thread) for context switching
    pub fn schedule(&mut self) -> Option<(&mut Thread, &Thread)> {
        // Always log the first few scheduling decisions
        static SCHEDULE_COUNT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let count = SCHEDULE_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        // Log the first few scheduling decisions
        if count < 10 {
            log::info!(
                "schedule() called #{}: current={:?}, ready_queue={:?}, idle_thread={}",
                count,
                self.current_thread,
                self.ready_queue,
                self.idle_thread
            );
        }

        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.current_thread {
            if current_id != self.idle_thread {
                // First check the state and update it
                let (is_terminated, prev_state) =
                    if let Some(current) = self.get_thread_mut(current_id) {
                        let was_terminated = current.state == ThreadState::Terminated;
                        let prev = current.state;
                        if !was_terminated {
                            current.set_ready();
                        }
                        (was_terminated, prev)
                    } else {
                        (true, ThreadState::Terminated)
                    };

                // Then modify the ready queue
                if !is_terminated {
                    self.ready_queue.push_back(current_id);
                    if count < 10 {
                        log::info!(
                            "Put thread {} back in ready queue, state was {:?}",
                            current_id,
                            prev_state
                        );
                    }
                } else {
                    log::info!(
                        "Thread {} is terminated, not putting back in ready queue",
                        current_id
                    );
                }
            }
        }

        // Get next thread from ready queue
        let mut next_thread_id = if let Some(n) = self.ready_queue.pop_front() {
            n
        } else {
            self.idle_thread
        };

        if count < 10 {
            log::info!(
                "Next thread from queue: {}, ready_queue after pop: {:?}",
                next_thread_id,
                self.ready_queue
            );
        }

        // Important: Don't skip if it's the same thread when there are other threads waiting
        // This was causing the issue where yielding wouldn't switch to other ready threads
        if Some(next_thread_id) == self.current_thread && !self.ready_queue.is_empty() {
            // Put current thread back and get the next one
            self.ready_queue.push_back(next_thread_id);
            next_thread_id = self.ready_queue.pop_front()?;
            log::info!(
                "Forced switch from {} to {} (other threads waiting)",
                self.current_thread.unwrap_or(0),
                next_thread_id
            );
        } else if Some(next_thread_id) == self.current_thread {
            // No other threads ready, stay with current
            if count < 10 {
                log::info!(
                    "Staying with current thread {} (no other threads ready)",
                    next_thread_id
                );
            }
            return None;
        }

        // If current is idle and we have a real next thread, allow switch even if idle
        let old_thread_id = self.current_thread.unwrap_or(self.idle_thread);
        self.current_thread = Some(next_thread_id);

        if count < 10 {
            log::info!(
                "Switching from thread {} to thread {}",
                old_thread_id,
                next_thread_id
            );
        }

        // Mark new thread as running
        if let Some(next) = self.get_thread_mut(next_thread_id) {
            next.set_running();
        }

        // Get mutable reference to old thread and immutable to new
        // This is safe because we know they're different threads
        unsafe {
            let threads_ptr = self.threads.as_mut_ptr();
            let old_idx = self.threads.iter().position(|t| t.id() == old_thread_id)?;
            let new_idx = self.threads.iter().position(|t| t.id() == next_thread_id)?;

            let old_thread = &mut *(*threads_ptr.add(old_idx)).as_mut();
            let new_thread = &*(*threads_ptr.add(new_idx)).as_ref();

            Some((old_thread, new_thread))
        }
    }

    /// Block the current thread
    pub fn block_current(&mut self) {
        if let Some(current) = self.current_thread_mut() {
            current.set_blocked();
        }
    }

    /// Unblock a thread by ID
    pub fn unblock(&mut self, thread_id: u64) {
        if let Some(thread) = self.get_thread_mut(thread_id) {
            if thread.state == ThreadState::Blocked {
                thread.set_ready();
                if thread_id != self.idle_thread {
                    self.ready_queue.push_back(thread_id);
                }
            }
        }
    }

    /// Terminate the current thread
    pub fn terminate_current(&mut self) {
        if let Some(current) = self.current_thread_mut() {
            current.set_terminated();
            // Don't put back in ready queue
        }
        self.current_thread = None;
    }

    /// Check if scheduler has any runnable threads
    pub fn has_runnable_threads(&self) -> bool {
        !self.ready_queue.is_empty()
            || self.current_thread.map_or(false, |id| {
                self.get_thread(id).map_or(false, |t| t.is_runnable())
            })
    }

    /// Check if scheduler has any userspace threads (ready, running, or blocked)
    pub fn has_userspace_threads(&self) -> bool {
        self.threads.iter().any(|t| {
            t.id() != self.idle_thread
                && t.privilege == super::thread::ThreadPrivilege::User
                && t.state != super::thread::ThreadState::Terminated
        })
    }

    /// Get a thread by ID (public for timer.rs)
    pub fn get_thread(&self, id: u64) -> Option<&Thread> {
        self.threads
            .iter()
            .find(|t| t.id() == id)
            .map(|t| t.as_ref())
    }

    /// Get the idle thread ID
    pub fn idle_thread(&self) -> u64 {
        self.idle_thread
    }

    /// Set the current thread (used by spawn mechanism)
    pub fn set_current_thread(&mut self, thread_id: u64) {
        self.current_thread = Some(thread_id);
    }
}

/// Initialize the global scheduler
pub fn init(idle_thread: Box<Thread>) {
    let mut scheduler_lock = SCHEDULER.lock();
    *scheduler_lock = Some(Scheduler::new(idle_thread));
    log::info!("Scheduler initialized");
}

/// Initialize scheduler with the current thread as the idle task (Linux-style)
/// This is used during boot where the boot thread becomes the idle task
pub fn init_with_current(current_thread: Box<Thread>) {
    let mut scheduler_lock = SCHEDULER.lock();
    let thread_id = current_thread.id();
    
    // Create scheduler with current thread as both idle and current
    let mut scheduler = Scheduler::new(current_thread);
    scheduler.current_thread = Some(thread_id);
    
    *scheduler_lock = Some(scheduler);
    log::info!("Scheduler initialized with current thread {} as idle task", thread_id);
}

/// Add a thread to the scheduler
pub fn spawn(thread: Box<Thread>) {
    // Disable interrupts to prevent timer interrupt deadlock
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.add_thread(thread);
            // Ensure a switch happens ASAP (especially in CI smoke runs)
            NEED_RESCHED.store(true, Ordering::Relaxed);
            // Mirror to per-CPU flag so IRQ-exit path sees it
            crate::per_cpu::set_need_resched(true);
        } else {
            panic!("Scheduler not initialized");
        }
    });
}

/// Perform scheduling and return threads to switch between
pub fn schedule() -> Option<(u64, u64)> {
    // Check if interrupts are already disabled (i.e., we're in interrupt context)
    let interrupts_enabled = x86_64::instructions::interrupts::are_enabled();

    let result = if interrupts_enabled {
        // Normal case: disable interrupts to prevent deadlock
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut scheduler_lock = SCHEDULER.lock();
            if let Some(scheduler) = scheduler_lock.as_mut() {
                scheduler.schedule().map(|(old, new)| (old.id(), new.id()))
            } else {
                None
            }
        })
    } else {
        // Already in interrupt context - don't try to disable interrupts again
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.schedule().map(|(old, new)| (old.id(), new.id()))
        } else {
            None
        }
    };

    result
}

/// Special scheduling point called from IRQ exit path
/// This is safe to call from IRQ context when returning to user or idle
pub fn preempt_schedule_irq() {
    // This is the Linux-style preempt_schedule_irq equivalent
    // It's called from irq_exit when:
    // 1. HARDIRQ count is going to 0
    // 2. need_resched is set
    // 3. We're about to return to a preemptible context
    
    // Linux-style loop: keep scheduling while need_resched is set
    // This prevents lost wakeups
    loop {
        // Check need_resched at the start of each iteration
        if !crate::per_cpu::need_resched() {
            break;
        }
        
        // Clear need_resched only AFTER checking it
        crate::per_cpu::set_need_resched(false);
        
        // Try non-blocking schedule since we're in IRQ exit path
        if let Some((old_tid, new_tid)) = try_schedule() {
            log::info!("preempt_schedule_irq: Scheduled {} -> {}", old_tid, new_tid);
            // Context switch will happen on return from interrupt
        }
        
        // Loop will check need_resched again in case it was set during scheduling
    }
}

/// Non-blocking scheduling attempt (for interrupt context). Returns None if lock is busy.
pub fn try_schedule() -> Option<(u64, u64)> {
    // Do not disable interrupts; we only attempt a non-blocking lock here
    if let Some(mut scheduler_lock) = SCHEDULER.try_lock() {
        if let Some(scheduler) = scheduler_lock.as_mut() {
            return scheduler.schedule().map(|(old, new)| (old.id(), new.id()));
        }
    }
    None
}

/// Get access to the scheduler
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn with_scheduler<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Scheduler) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        scheduler_lock.as_mut().map(f)
    })
}

/// Get mutable access to a specific thread (for timer interrupt handler)
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn with_thread_mut<F, R>(thread_id: u64, f: F) -> Option<R>
where
    F: FnOnce(&mut super::thread::Thread) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        scheduler_lock
            .as_mut()
            .and_then(|sched| sched.get_thread_mut(thread_id).map(f))
    })
}

/// Get the current thread ID
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn current_thread_id() -> Option<u64> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let scheduler_lock = SCHEDULER.lock();
        scheduler_lock.as_ref().and_then(|s| s.current_thread)
    })
}

/// Yield the current thread
pub fn yield_current() {
    // This will be called from timer interrupt or sys_yield
    // The actual context switch happens in the interrupt handler
    if let Some((old_id, new_id)) = schedule() {
        log::debug!("Scheduling: {} -> {}", old_id, new_id);
        // Context switch will be performed by caller
    }
}

/// Get pending context switch if any
/// Returns Some((old_thread_id, new_thread_id)) if a switch is pending
pub fn get_pending_switch() -> Option<(u64, u64)> {
    // For now, just call schedule to see if we would switch
    // In a real implementation, we might cache this decision
    schedule()
}

/// Allocate a new thread ID
pub fn allocate_thread_id() -> Option<u64> {
    Some(super::thread::allocate_thread_id())
}

/// Set the need_resched flag (called from timer interrupt)
pub fn set_need_resched() {
    NEED_RESCHED.store(true, Ordering::Relaxed);
    crate::per_cpu::set_need_resched(true);
}

/// Check and clear the need_resched flag (called from interrupt return path)
pub fn check_and_clear_need_resched() -> bool {
    let need = crate::per_cpu::need_resched();
    if need { crate::per_cpu::set_need_resched(false); }
    let _ = NEED_RESCHED.swap(false, Ordering::Relaxed);
    need
}
