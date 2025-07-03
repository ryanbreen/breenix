//! Process-Task Integration
//!
//! This module bridges the gap between the Process Manager and the Task Scheduler,
//! allowing processes to be scheduled as tasks.

use crate::process::{ProcessId};
use crate::task::thread::{Thread, ThreadPrivilege};
use crate::task::scheduler;
use alloc::boxed::Box;

/// Integration functions for scheduling processes as tasks
pub struct ProcessScheduler;

impl ProcessScheduler {
    
    /// Handle process exit from scheduler context
    /// Called when a userspace thread exits
    pub fn handle_thread_exit(thread_id: u64, exit_code: i32) {
        log::debug!("Thread {} exited with code {}", thread_id, exit_code);
        
        // Find which process this thread belongs to
        if let Some(ref mut manager) = *crate::process::manager() {
            if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                log::info!("Process {} (thread {}) exited with code {}", 
                         pid.as_u64(), thread_id, exit_code);
                process.terminate(exit_code);
                
                // TODO: Clean up process resources
                // TODO: Notify parent process
            }
        }
    }
    
    /// Get the current process ID from the scheduler context
    pub fn current_pid() -> Option<ProcessId> {
        // Get current thread from scheduler
        let thread_id = scheduler::current_thread_id()?;
        
        // Find process that owns this thread
        crate::process::manager().as_ref().and_then(|manager| {
            manager.find_process_by_thread(thread_id)
                .map(|(pid, _)| pid)
        })
    }
}

/// Extension trait for Thread to support process operations
pub trait ProcessThread {
    /// Check if this thread belongs to a userspace process
    fn is_process_thread(&self) -> bool;
    
    /// Get the process ID if this is a process thread
    fn process_id(&self) -> Option<ProcessId>;
}

impl ProcessThread for Thread {
    fn is_process_thread(&self) -> bool {
        self.privilege == ThreadPrivilege::User
    }
    
    fn process_id(&self) -> Option<ProcessId> {
        if !self.is_process_thread() {
            return None;
        }
        
        // Find process that owns this thread
        crate::process::manager().as_ref().and_then(|manager| {
            manager.find_process_by_thread(self.id)
                .map(|(pid, _)| pid)
        })
    }
}