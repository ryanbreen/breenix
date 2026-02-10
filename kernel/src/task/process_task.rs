//! Process-Task Integration
//!
//! This module bridges the gap between the Process Manager and the Task Scheduler,
//! allowing processes to be scheduled as tasks.

use crate::process::ProcessId;
use crate::task::scheduler;
use crate::task::thread::{Thread, ThreadPrivilege};

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
                log::info!(
                    "Process {} (thread {}) exited with code {}",
                    pid.as_u64(),
                    thread_id,
                    exit_code
                );

                // Get parent PID before terminating (needed for SIGCHLD)
                let parent_pid = process.parent;

                process.terminate(exit_code);

                #[cfg(feature = "btrt")]
                crate::test_framework::btrt::on_process_exit(pid.as_u64(), exit_code);

                // Send SIGCHLD to the parent process and wake it if blocked on waitpid
                if let Some(parent_pid) = parent_pid {
                    if let Some(parent_process) = manager.get_process_mut(parent_pid) {
                        use crate::signal::constants::SIGCHLD;
                        parent_process.signals.set_pending(SIGCHLD);

                        // Get parent's main thread ID to wake it if blocked on waitpid
                        let parent_thread_id = parent_process.main_thread.as_ref().map(|t| t.id);

                        log::debug!(
                            "Sent SIGCHLD to parent process {} for child {} exit",
                            parent_pid.as_u64(),
                            pid.as_u64()
                        );

                        // Wake up the parent thread if it's blocked on waitpid or pause()
                        if let Some(parent_tid) = parent_thread_id {
                            scheduler::with_scheduler(|sched| {
                                // Wake if blocked on waitpid (BlockedOnChildExit)
                                sched.unblock_for_child_exit(parent_tid);
                                // Also wake if blocked on pause() or other signal wait (BlockedOnSignal)
                                sched.unblock_for_signal(parent_tid);
                            });
                        }
                    }
                }

                // TODO: Clean up process resources
            }
        }
    }

    /// Get the current process ID from the scheduler context
    #[allow(dead_code)]
    pub fn current_pid() -> Option<ProcessId> {
        // Get current thread from scheduler
        let thread_id = scheduler::current_thread_id()?;

        // Find process that owns this thread
        crate::process::manager().as_ref().and_then(|manager| {
            manager
                .find_process_by_thread(thread_id)
                .map(|(pid, _)| pid)
        })
    }
}

/// Extension trait for Thread to support process operations
#[allow(dead_code)]
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
        crate::process::manager()
            .as_ref()
            .and_then(|manager| manager.find_process_by_thread(self.id).map(|(pid, _)| pid))
    }
}
