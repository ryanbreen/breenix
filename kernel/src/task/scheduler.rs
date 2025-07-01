//! Preemptive scheduler implementation
//!
//! This module implements a round-robin scheduler for kernel threads.

use super::thread::{Thread, ThreadState};
use alloc::{collections::VecDeque, boxed::Box};
use spin::Mutex;

/// Global scheduler instance
static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

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
        self.threads.push(thread);
        self.ready_queue.push_back(thread_id);
        log::debug!("Added thread {} to scheduler", thread_id);
    }
    
    /// Get a thread by ID
    fn get_thread(&self, id: u64) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id() == id).map(|t| t.as_ref())
    }
    
    /// Get a mutable thread by ID
    fn get_thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id() == id).map(|t| t.as_mut())
    }
    
    /// Get the current running thread
    pub fn current_thread(&self) -> Option<&Thread> {
        self.current_thread.and_then(|id| self.get_thread(id))
    }
    
    /// Get the current running thread mutably
    pub fn current_thread_mut(&mut self) -> Option<&mut Thread> {
        self.current_thread.and_then(move |id| self.get_thread_mut(id))
    }
    
    /// Schedule the next thread to run
    /// Returns (old_thread, new_thread) for context switching
    pub fn schedule(&mut self) -> Option<(&mut Thread, &Thread)> {
        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.current_thread {
            if current_id != self.idle_thread {
                if let Some(current) = self.get_thread_mut(current_id) {
                    if current.is_runnable() {
                        current.set_ready();
                        self.ready_queue.push_back(current_id);
                    }
                }
            }
        }
        
        // Get next thread from ready queue
        let next_thread_id = self.ready_queue.pop_front()
            .or(Some(self.idle_thread))?; // Use idle thread if nothing ready
        
        // Skip if it's the same thread
        if Some(next_thread_id) == self.current_thread {
            return None;
        }
        
        let old_thread_id = self.current_thread?;
        self.current_thread = Some(next_thread_id);
        
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
        !self.ready_queue.is_empty() || 
        self.current_thread.map_or(false, |id| {
            self.get_thread(id).map_or(false, |t| t.is_runnable())
        })
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
    let mut scheduler_lock = SCHEDULER.lock();
    if let Some(scheduler) = scheduler_lock.as_mut() {
        scheduler.add_thread(thread);
    } else {
        panic!("Scheduler not initialized");
    }
}

/// Perform scheduling and return threads to switch between
pub fn schedule() -> Option<(u64, u64)> {
    let mut scheduler_lock = SCHEDULER.lock();
    if let Some(scheduler) = scheduler_lock.as_mut() {
        scheduler.schedule().map(|(old, new)| (old.id(), new.id()))
    } else {
        None
    }
}

/// Get access to the scheduler
pub fn with_scheduler<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Scheduler) -> R,
{
    let mut scheduler_lock = SCHEDULER.lock();
    scheduler_lock.as_mut().map(f)
}

/// Get the current thread ID
pub fn current_thread_id() -> Option<u64> {
    let scheduler_lock = SCHEDULER.lock();
    scheduler_lock.as_ref().and_then(|s| s.current_thread)
}

/// Yield the current thread
pub fn yield_current() {
    // This will be called from timer interrupt or sys_yield
    // The actual context switch happens in the interrupt handler
    if let Some((old_id, new_id)) = schedule() {
        log::trace!("Scheduling: {} -> {}", old_id, new_id);
        // Context switch will be performed by caller
    }
}