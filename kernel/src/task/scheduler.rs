//! Preemptive scheduler implementation
//!
//! This module implements a round-robin scheduler for kernel threads.

use super::thread::{Thread, ThreadState, BlockedReason};
use alloc::{collections::VecDeque, boxed::Box, vec::Vec, sync::Arc};
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
    /// All threads in the system (Arc<Mutex<>> prevents use-after-free)
    threads: alloc::vec::Vec<Arc<Mutex<Thread>>>,
    
    /// Ready queue (thread IDs)
    ready_queue: VecDeque<u64>,
    
    /// Currently running thread ID
    current_thread: Option<u64>,
    
    /// Idle thread ID (runs when no other threads are ready)
    idle_thread: u64,
    
    /// List of threads waiting for child processes
    waiters: Vec<Waiter>,
    
    /// Deferred drop list - prevents Arc drops during interrupt context
    retire_list: Vec<Arc<Mutex<Thread>>>,
}

impl Scheduler {
    /// Create a new scheduler with an idle thread  
    pub fn new(idle_thread: Box<Thread>) -> Self {
        let idle_id = idle_thread.id();
        let mut threads = Vec::new();
        
        // Reserve capacity to prevent reallocation while interrupts are enabled
        const EXPECTED_THREADS: usize = 128;
        threads.reserve_exact(EXPECTED_THREADS);
        threads.push(Arc::new(Mutex::new(*idle_thread)));
        
        let scheduler = Self {
            threads,
            ready_queue: VecDeque::new(),
            current_thread: Some(idle_id),
            idle_thread: idle_id,
            waiters: Vec::new(),
            retire_list: Vec::new(),
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
        
        // HEAP CORRUPTION GUARD: Log thread allocation 
        #[cfg(feature = "heap_trace")]
        {
            let ptr = thread.as_ref() as *const Thread as u64;
            crate::serial_println!("SCHED_ADD: ptr={:#x} id={} state={:?}", ptr, thread_id, thread_state);
        }
        
        // Debug assert to catch Vec reallocation issues
        debug_assert!(
            self.threads.len() < self.threads.capacity(),
            "threads Vec would reallocate with interrupts enabled"
        );
        
        let new_arc = Arc::new(Mutex::new(*thread));
        
        // ARC GUARD: Log Arc value before push
        #[cfg(feature = "arc_guard")]
        {
            let arc_ptr = &new_arc as *const _ as u64;
            let arc_inner = Arc::as_ptr(&new_arc) as u64;
            crate::serial_println!("ARC_GUARD: Before push - Arc at {:#x} points to {:#x}", arc_ptr, arc_inner);
        }
        
        self.threads.push(new_arc);
        
        // ARC GUARD: Check all Arc slots after push
        #[cfg(feature = "arc_guard")]
        {
            self.debug_arc_slots("After push");
        }
        
        self.ready_queue.push_back(thread_id);
        crate::serial_println!("SCHED_PUSH: tid={} ready_queue={:?}", thread_id, self.ready_queue);
        log::info!("Added thread {} '{}' to scheduler (user: {}, state: {:?}, ready_queue: {:?})", 
                  thread_id, thread_name, is_user, thread_state, self.ready_queue);
        
        // Child threads are now added to the ready queue successfully
    }
    
    
    /// Get a mutable thread by ID (returns MutexGuard to prevent use-after-free)
    pub fn get_thread_mut(&self, id: u64) -> Option<spin::MutexGuard<'_, Thread>> {
        #[cfg(feature = "arc_guard")]
        {
            // Check for corruption before accessing
            for (i, arc) in self.threads.iter().enumerate() {
                let arc_inner = Arc::as_ptr(arc) as u64;
                if arc_inner == 0x444444441748 || arc_inner < 0x100000 {
                    crate::serial_println!("ARC_GUARD: CORRUPTION in get_thread_mut! Slot [{}] Arc points to {:#x}", i, arc_inner);
                    panic!("Arc corruption detected before access!");
                }
            }
        }
        
        self.threads.iter().find(|t| t.lock().id() == id).map(|t| {
            let guard = t.lock();
            
            // HEAP CORRUPTION GUARD: Log access for debugging  
            #[cfg(feature = "heap_trace")]
            {
                crate::serial_println!("ARC ACCESS id={} state={:?}", id, guard.state);
            }
            
            guard
        })
    }
    
    /// Get the current running thread mutably
    pub fn current_thread_mut(&self) -> Option<spin::MutexGuard<'_, Thread>> {
        self.current_thread.and_then(|id| self.get_thread_mut(id))
    }
    
    /// Schedule the next thread to run
    /// Returns (old_thread_id, new_thread_id) for context switching
    pub fn schedule(&mut self) -> Option<(u64, u64)> {
        // Scheduler entry point
        
        #[cfg(feature = "arc_guard")]
        {
            self.debug_arc_slots("schedule() entry");
        }
        
        // If current thread is still runnable, put it back in ready queue
        if let Some(current_id) = self.current_thread {
            if current_id != self.idle_thread {
                // First check the state and update it
                let (should_requeue, prev_state) = if let Some(mut current) = self.get_thread_mut(current_id) {
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
                    crate::serial_println!("SCHED_REQUEUE: tid={} prev_state={:?}", current_id, prev_state);
                    log::debug!("Put thread {} back in ready queue, state was {:?}", current_id, prev_state);
                } else {
                    crate::serial_println!("SCHED_NO_REQUEUE: tid={} state={:?}", current_id, prev_state);
                    log::info!("Thread {} is {:?}, not putting back in ready queue", current_id, prev_state);
                }
            }
        }
        
        // Get next thread from ready queue
        crate::serial_println!("SCHED_LOOP: ready={:?}", self.ready_queue);
        log::info!("DEBUG: ready_queue before pop: {:?}", self.ready_queue);
        let next_thread_id = self.ready_queue.pop_front();
        if let Some(tid) = next_thread_id {
            crate::serial_println!("SCHED_REMOVE: tid={} reason=schedule_pop", tid);
        }
        let mut next_thread_id = next_thread_id.or(Some(self.idle_thread))?; // Use idle thread if nothing ready
        
        log::info!("DEBUG: Next thread from queue: {}, ready_queue after pop: {:?}", 
                  next_thread_id, self.ready_queue);
        
        log::debug!("Next thread from queue: {}, ready_queue after pop: {:?}", 
                  next_thread_id, self.ready_queue);
        
        // 3.1 Process-state tracer: Log thread state transitions
        #[cfg(feature = "sched_debug")]
        {
            if let Some(current) = self.current_thread {
                if let Some(thread) = self.get_thread(current) {
                    crate::serial_println!("SCHED_STATE: tid={} state_before={:?} -> tid={} ready_queue={:?}", 
                        current, thread.state, next_thread_id, self.ready_queue);
                }
            } else {
                crate::serial_println!("SCHED_STATE: no_current -> tid={} ready_queue={:?}", 
                    next_thread_id, self.ready_queue);
            }
        }
        
        // Important: Don't skip if it's the same thread when there are other threads waiting
        // This was causing the issue where yielding wouldn't switch to other ready threads
        if Some(next_thread_id) == self.current_thread && !self.ready_queue.is_empty() {
            // Put current thread back and get the next one
            self.ready_queue.push_back(next_thread_id);
            next_thread_id = self.ready_queue.pop_front()?;
            crate::serial_println!("SCHED_REMOVE: tid={} reason=forced_switch", next_thread_id);
            log::info!("Forced switch from {} to {} (other threads waiting)", 
                       self.current_thread.unwrap_or(0), next_thread_id);
        } else if Some(next_thread_id) == self.current_thread {
            // No other threads ready, stay with current
            log::debug!("Staying with current thread {} (no other threads ready)", next_thread_id);
            return None;
        }
        
        let old_thread_id = self.current_thread.unwrap_or_else(|| {
            crate::serial_println!("SCHED_BUG: current_thread is None! Using idle thread {}", self.idle_thread);
            self.idle_thread
        });
        self.current_thread = Some(next_thread_id);
        
        crate::serial_println!("SCHED_SWITCH: {} -> {}", old_thread_id, next_thread_id);
        log::debug!("Switching from thread {} to thread {}", old_thread_id, next_thread_id);
        
        // Mark new thread as running
        if let Some(mut next) = self.get_thread_mut(next_thread_id) {
            next.set_running();
        }
        
        // Return thread IDs for context switching (caller will get MutexGuards)
        Some((old_thread_id, next_thread_id))
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
            let thread = t.lock();
            thread.id() != self.idle_thread && 
            thread.privilege == super::thread::ThreadPrivilege::User &&
            thread.state != super::thread::ThreadState::Terminated
        })
    }
    
    /// Get a thread by ID (public for timer.rs)
    pub fn get_thread(&self, id: u64) -> Option<spin::MutexGuard<'_, Thread>> {
        self.threads.iter().find(|t| t.lock().id() == id).map(|t| t.lock())
    }
    
    /// Get the idle thread ID
    pub fn idle_thread(&self) -> u64 {
        self.idle_thread
    }
    
    /// Debug function to check Arc slots for corruption
    #[cfg(feature = "arc_guard")]
    fn debug_arc_slots(&self, context: &str) {
        crate::serial_println!("ARC_GUARD: {} - checking {} slots", context, self.threads.len());
        for (i, arc) in self.threads.iter().enumerate() {
            let arc_ptr = arc as *const _ as u64;
            let arc_inner = Arc::as_ptr(arc) as u64;
            let vec_start = self.threads.as_ptr() as u64;
            
            // Check if Arc points to the Vec buffer itself (corruption indicator)
            if arc_inner == vec_start || arc_inner == 0x444444441748 {
                crate::serial_println!("ARC_GUARD: CORRUPTION! Slot [{}] Arc at {:#x} points to Vec buffer {:#x}!", 
                                      i, arc_ptr, arc_inner);
                panic!("Arc corruption detected in scheduler Vec!");
            }
            
            crate::serial_println!("ARC_GUARD:   [{}] Arc at {:#x} -> {:#x}", i, arc_ptr, arc_inner);
        }
    }
    
    /// Add a waiter for a child process
    pub fn add_waiter(&mut self, waiter: Waiter) {
        log::info!("Adding waiter: thread {} waiting for {:?}", waiter.thread_id, waiter.mode);
        
        // Mark the thread as blocked
        if let Some(mut thread) = self.get_thread_mut(waiter.thread_id) {
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
            // Set thread to ready state
            {
                if let Some(mut thread) = self.get_thread_mut(thread_id) {
                    thread.set_ready();
                }
            }
            // Add to ready queue after dropping MutexGuard
            self.ready_queue.push_back(thread_id);
        }
    }
    
    /// Remove all waiters for a given parent (e.g., when parent dies)
    #[allow(dead_code)]
    pub fn remove_waiters_for_parent(&mut self, parent_pid: ProcessId) {
        self.waiters.retain(|waiter| waiter.parent_pid != parent_pid);
    }
    
    /// Retire a thread for deferred dropping (prevents drop during interrupt context)
    pub fn retire_thread(&mut self, thread_id: u64) {
        // Find and remove the thread from the active list
        if let Some(pos) = self.threads.iter().position(|t| t.lock().id() == thread_id) {
            #[cfg(feature = "arc_guard")]
            {
                crate::serial_println!("ARC_GUARD: retire_thread removing tid={} at pos={}", thread_id, pos);
                self.debug_arc_slots("Before retire_thread remove");
            }
            
            let retired_thread = self.threads.remove(pos);
            
            #[cfg(feature = "arc_guard")]
            {
                self.debug_arc_slots("After retire_thread remove");
                let arc_inner = Arc::as_ptr(&retired_thread) as u64;
                crate::serial_println!("ARC_GUARD: Retired thread Arc points to {:#x}", arc_inner);
            }
            
            #[cfg(feature = "heap_trace")]
            {
                crate::serial_println!("RETIRE_THREAD: tid={} moved to retire_list", thread_id);
            }
            
            self.retire_list.push(retired_thread);
        }
    }
    
    /// Process retire list (should be called from idle thread)
    pub fn process_retire_list(&mut self) {
        if !self.retire_list.is_empty() {
            let _count = self.retire_list.len();
            
            #[cfg(feature = "heap_trace")]
            {
                crate::serial_println!("RETIRE_DRAIN: processing {} threads", _count);
            }
            
            // Debug assert to catch reference count issues
            for thread_arc in &self.retire_list {
                debug_assert_eq!(Arc::strong_count(thread_arc), 1, 
                    "Thread being retired still has active references");
            }
            
            // Clear the retire list - this will drop all the Arc<Mutex<Thread>>s
            self.retire_list.clear();
            
            #[cfg(feature = "heap_trace")]
            {
                crate::serial_println!("RETIRE_COMPLETE: dropped {} threads", count);
            }
        }
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
    let tid = thread.id;
    // Disable interrupts to prevent timer interrupt deadlock
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            crate::serial_println!("SCHED_PUSH: tid={} queue_len_before={}", tid, scheduler.ready_queue.len());
            scheduler.add_thread(thread);
            crate::serial_println!("SCHED_PUSH: tid={} queue_len_after={}", tid, scheduler.ready_queue.len());
        } else {
            panic!("Scheduler not initialized");
        }
    });
}

/// Perform scheduling and return thread IDs to switch between
pub fn schedule() -> Option<(u64, u64)> {
    // Note: This is called from timer interrupt, so interrupts are already disabled
    // But we'll be explicit about it to prevent nested timer deadlocks
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            let result = scheduler.schedule();
            if let Some((old_id, new_id)) = result {
                crate::serial_println!("SCHED_GLOBAL: returning Some({}, {})", old_id, new_id);
                Some((old_id, new_id))
            } else {
                crate::serial_println!("SCHED_GLOBAL: scheduler.schedule() returned None");
                None
            }
        } else {
            crate::serial_println!("SCHED_GLOBAL: SCHEDULER lock is None!");
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
        let scheduler_lock = SCHEDULER.lock();
        scheduler_lock.as_ref().and_then(|sched| {
            sched.get_thread_mut(thread_id).map(|mut guard| f(&mut *guard))
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

/// Retire a thread for deferred dropping (prevents Arc drops during interrupt context)
pub fn retire_thread(thread_id: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.retire_thread(thread_id);
        }
    })
}

/// Process retired threads (should be called from idle thread)
pub fn process_retire_list() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut scheduler_lock = SCHEDULER.lock();
        if let Some(scheduler) = scheduler_lock.as_mut() {
            scheduler.process_retire_list();
        }
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