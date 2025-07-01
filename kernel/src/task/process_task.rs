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
    /// Add a process to the scheduler
    /// This extracts the main thread from the process and adds it to the scheduler
    pub fn schedule_process(pid: ProcessId) -> Result<(), &'static str> {
        // Get process manager
        let mut manager_lock = crate::process::manager();
        let manager = manager_lock.as_mut()
            .ok_or("Process manager not initialized")?;
        
        // Get the process
        let process = manager.get_process_mut(pid)
            .ok_or("Process not found")?;
        
        // Extract the main thread
        let thread = process.main_thread.take()
            .ok_or("Process has no main thread")?;
        
        // Verify it's a userspace thread
        if thread.privilege != ThreadPrivilege::User {
            return Err("Process thread is not userspace");
        }
        
        log::info!("Scheduling process {} (thread {}) on kernel scheduler", 
                  pid.as_u64(), thread.id);
        
        // Add to scheduler
        scheduler::spawn(Box::new(thread));
        
        Ok(())
    }
    
    /// Create a process and immediately schedule it
    pub fn create_and_schedule_process(
        name: alloc::string::String,
        elf_data: &[u8]
    ) -> Result<ProcessId, &'static str> {
        // Create the process
        let pid = {
            let mut manager_lock = crate::process::manager();
            let manager = manager_lock.as_mut()
                .ok_or("Process manager not initialized")?;
            manager.create_process(name, elf_data)?
        };
        
        // Schedule it
        Self::schedule_process(pid)?;
        
        Ok(pid)
    }
    
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