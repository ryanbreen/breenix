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
        let thread_name = thread.name.clone();
        let is_user = thread.privilege == super::thread::ThreadPrivilege::User;
        self.threads.push(thread);
        self.ready_queue.push_back(thread_id);
        log::info!("Added thread {} '{}' to scheduler (user: {}, ready_queue: {:?})", 
                  thread_id, thread_name, is_user, self.ready_queue);
    }
    
    
    /// Get a mutable thread by ID
    pub fn get_thread_mut(&mut self, id: u64) -> Option<&mut Thread> {
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
        // Only log scheduling events when something interesting happens
        // log::trace!("Schedule called: current={:?}, ready_queue={:?}", 
        //            self.current_thread, self.ready_queue);
        
        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.current_thread {
            if current_id != self.idle_thread {
                if let Some(current) = self.get_thread_mut(current_id) {
                    if current.is_runnable() {
                        current.set_ready();
                        self.ready_queue.push_back(current_id);
                        // log::trace!("Put thread {} back in ready queue", current_id);
                    }
                }
            }
        }
        
        // Get next thread from ready queue
        let mut next_thread_id = self.ready_queue.pop_front()
            .or(Some(self.idle_thread))?; // Use idle thread if nothing ready
        
        // Important: Don't skip if it's the same thread when there are other threads waiting
        // This was causing the issue where yielding wouldn't switch to other ready threads
        if Some(next_thread_id) == self.current_thread && !self.ready_queue.is_empty() {
            // Put current thread back and get the next one
            self.ready_queue.push_back(next_thread_id);
            next_thread_id = self.ready_queue.pop_front()?;
            log::debug!("Forced switch from {} to {} (other threads waiting)", 
                       self.current_thread.unwrap_or(0), next_thread_id);
        } else if Some(next_thread_id) == self.current_thread {
            // No other threads ready, stay with current
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
pub fn with_scheduler<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Scheduler) -> R,
{
    let mut scheduler_lock = SCHEDULER.lock();
    scheduler_lock.as_mut().map(f)
}

/// Get mutable access to a specific thread (for timer interrupt handler)
pub fn with_thread_mut<F, R>(thread_id: u64, f: F) -> Option<R>
where
    F: FnOnce(&mut super::thread::Thread) -> R,
{
    let mut scheduler_lock = SCHEDULER.lock();
    scheduler_lock.as_mut().and_then(|sched| {
        sched.get_thread_mut(thread_id).map(f)
    })
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