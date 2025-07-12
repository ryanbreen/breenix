//! Preemptive scheduler implementation
//!
//! This module implements a round-robin scheduler for kernel threads.

use super::thread::{Thread, ThreadState, BlockedReason};
use alloc::{collections::VecDeque, boxed::Box, vec::Vec};
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::process::ProcessId;

/// Global scheduler instance
static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Global need_resched flag for timer interrupt
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

/// Mode of waiting for child processes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaitMode {
    /// Wait for any child
    AnyChild,
    /// Wait for specific child
    SpecificChild(ProcessId),
}

/// Represents a waiting parent
#[derive(Debug)]
pub struct Waiter {
    /// Thread ID of the waiting parent
    pub thread_id: u64,
    /// Process ID of the parent process
    pub parent_pid: ProcessId,
    /// What we're waiting for
    pub mode: WaitMode,
}

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
    
    /// List of threads waiting for child processes
    waiters: Vec<Waiter>,
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
            waiters: Vec::new(),
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
        let thread_state = thread.state;
        self.threads.push(thread);
        self.ready_queue.push_back(thread_id);
        log::info!("Added thread {} '{}' to scheduler (user: {}, state: {:?}, ready_queue: {:?})", 
                  thread_id, thread_name, is_user, thread_state, self.ready_queue);
        
        // Child threads are now added to the ready queue successfully
    }
    
    
    /// Get a mutable thread by ID
    pub fn get_thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id() == id).map(|t| t.as_mut())
    }
    
    /// Get the current running thread mutably
    pub fn current_thread_mut(&mut self) -> Option<&mut Thread> {
        self.current_thread.and_then(move |id| self.get_thread_mut(id))
    }
    
    /// Schedule the next thread to run
    /// Returns (old_thread, new_thread) for context switching
    pub fn schedule(&mut self) -> Option<(&mut Thread, &Thread)> {
        // Scheduler entry point
        
        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.current_thread {
            if current_id != self.idle_thread {
                // First check the state and update it
                let (should_requeue, prev_state) = if let Some(current) = self.get_thread_mut(current_id) {
                    let prev = current.state;
                    match current.state {
                        ThreadState::Terminated => (false, prev),
                        ThreadState::Blocked(_) => (false, prev), // Keep blocked threads out of ready queue
                        _ => {
                            current.set_ready();
                            (true, prev)
                        }
                    }
                } else {
                    (false, ThreadState::Terminated)
                };
                
                // Then modify the ready queue
                if should_requeue {
                    self.ready_queue.push_back(current_id);
                    if count < 10 {
                        log::info!("Put thread {} back in ready queue, state was {:?}", current_id, prev_state);
                    }
                } else {
                    log::info!("Thread {} is {:?}, not putting back in ready queue", current_id, prev_state);
                }
            }
        }
        
        // Get next thread from ready queue
        log::info!("DEBUG: ready_queue before pop: {:?}", self.ready_queue);
        let mut next_thread_id = self.ready_queue.pop_front()
            .or(Some(self.idle_thread))?; // Use idle thread if nothing ready
        
        log::info!("DEBUG: Next thread from queue: {}, ready_queue after pop: {:?}", 
                  next_thread_id, self.ready_queue);
        
        if count < 10 {
            log::info!("Next thread from queue: {}, ready_queue after pop: {:?}", 
                      next_thread_id, self.ready_queue);
        }
        
        // Important: Don't skip if it's the same thread when there are other threads waiting
        // This was causing the issue where yielding wouldn't switch to other ready threads
        if Some(next_thread_id) == self.current_thread && !self.ready_queue.is_empty() {
            // Put current thread back and get the next one
            self.ready_queue.push_back(next_thread_id);
            next_thread_id = self.ready_queue.pop_front()?;
            log::info!("Forced switch from {} to {} (other threads waiting)", 
                       self.current_thread.unwrap_or(0), next_thread_id);
        } else if Some(next_thread_id) == self.current_thread {
            // No other threads ready, stay with current
            if count < 10 {
                log::info!("Staying with current thread {} (no other threads ready)", next_thread_id);
            }
            return None;
        }
        
        let old_thread_id = self.current_thread?;
        self.current_thread = Some(next_thread_id);
        
        if count < 10 {
            log::info!("Switching from thread {} to thread {}", old_thread_id, next_thread_id);
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
    
    /// Check if scheduler has any runnable threads
    pub fn has_runnable_threads(&self) -> bool {
        !self.ready_queue.is_empty() || 
        self.current_thread.map_or(false, |id| {
            self.get_thread(id).map_or(false, |t| t.is_runnable())
        })
    }
    
    /// Check if scheduler has any userspace threads (ready, running, or blocked)
    pub fn has_userspace_threads(&self) -> bool {
        self.threads.iter().any(|t| {
            t.id() != self.idle_thread && 
            t.privilege == super::thread::ThreadPrivilege::User &&
            t.state != super::thread::ThreadState::Terminated
        })
    }
    
    /// Get a thread by ID (public for timer.rs)
    pub fn get_thread(&self, id: u64) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id() == id).map(|t| t.as_ref())
    }
    
    /// Get the idle thread ID
    pub fn idle_thread(&self) -> u64 {
        self.idle_thread
    }
    
    /// Add a waiter for a child process
    pub fn add_waiter(&mut self, waiter: Waiter) {
        log::info!("Adding waiter: thread {} waiting for {:?}", waiter.thread_id, waiter.mode);
        
        // Mark the thread as blocked
        if let Some(thread) = self.get_thread_mut(waiter.thread_id) {
            thread.set_blocked(BlockedReason::Wait);
        }
        
        self.waiters.push(waiter);
    }
    
    /// Wake up waiters that match the given child process
    pub fn wake_waiters(&mut self, child_pid: ProcessId, parent_pid: Option<ProcessId>) {
        log::info!("Waking waiters for child {} (parent: {:?})", child_pid.as_u64(), parent_pid);
        
        // Find waiters to wake
        let mut threads_to_wake = Vec::new();
        
        self.waiters.retain(|waiter| {
            let should_wake = match (&waiter.mode, &parent_pid) {
                // If we know the parent, only wake if it matches
                (_, Some(parent)) if waiter.parent_pid != *parent => false,
                // Wake if waiting for any child
                (WaitMode::AnyChild, _) => true,
                // Wake if waiting for this specific child
                (WaitMode::SpecificChild(pid), _) => *pid == child_pid,
            };
            
            if should_wake {
                threads_to_wake.push(waiter.thread_id);
                false // Remove from waiters list
            } else {
                true // Keep in waiters list
            }
        });
        
        // Wake the threads
        for thread_id in threads_to_wake {
            log::info!("Waking thread {} from wait", thread_id);
            if let Some(thread) = self.get_thread_mut(thread_id) {
                thread.set_ready();
                self.ready_queue.push_back(thread_id);
            }
        }
    }
    
    /// Remove all waiters for a given parent (e.g., when parent dies)
    #[allow(dead_code)]
    pub fn remove_waiters_for_parent(&mut self, parent_pid: ProcessId) {
        self.waiters.retain(|waiter| waiter.parent_pid != parent_pid);
    }
}

/// Initialize the global scheduler
pub fn init(idle_thread: Box<Thread>) {
    let mut scheduler_lock = SCHEDULER.lock();
    *scheduler_lock = Some(Scheduler::new(idle_thread));
    log::info!("Scheduler initialized");
}

/// Add a thread to the scheduler
pub fn spawn(thread: Box<Thread>) {
    // Disable interrupts to prevent timer interrupt deadlock
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.add_thread(thread);
        } else {
            panic!("Scheduler not initialized");
        }
    });
}

/// Perform scheduling and return threads to switch between
pub fn schedule() -> Option<(u64, u64)> {
    // Note: This is called from timer interrupt, so interrupts are already disabled
    // But we'll be explicit about it to prevent nested timer deadlocks
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.schedule().map(|(old, new)| (old.id(), new.id()))
        } else {
            None
        }
    })
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

/// Get mutable access to the scheduler (alias for with_scheduler)
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn with_scheduler_mut<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Scheduler) -> R,
{
    with_scheduler(f)
}

/// Get mutable access to a specific thread (for timer interrupt handler)
/// This function disables interrupts to prevent deadlock with timer interrupt
pub fn with_thread_mut<F, R>(thread_id: u64, f: F) -> Option<R>
where
    F: FnOnce(&mut super::thread::Thread) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        scheduler_lock.as_mut().and_then(|sched| {
            sched.get_thread_mut(thread_id).map(f)
        })
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

/// Set the need_resched flag (called from timer interrupt)
pub fn set_need_resched() {
    NEED_RESCHED.store(true, Ordering::Relaxed);
}

/// Check and clear the need_resched flag (called from interrupt return path)
pub fn check_and_clear_need_resched() -> bool {
    NEED_RESCHED.swap(false, Ordering::Relaxed)
}