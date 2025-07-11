//! Process-Task Integration
//!
//! This module bridges the gap between the Process Manager and the Task Scheduler,
//! allowing processes to be scheduled as tasks.


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
}

