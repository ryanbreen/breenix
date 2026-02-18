//! Process-Task Integration
//!
//! This module bridges the gap between the Process Manager and the Task Scheduler,
//! allowing processes to be scheduled as tasks.

use crate::ipc::fd::FileDescriptor;
use crate::process::ProcessId;
use crate::task::scheduler;
use crate::task::thread::{Thread, ThreadPrivilege};

/// Close extracted file descriptor entries outside the PM lock.
///
/// This performs the same cleanup as Process::close_all_fds() but operates on
/// a Vec of entries that were extracted from the FD table under PM lock via
/// Process::take_fd_entries(). This avoids holding PM lock during pipe wakeups,
/// PTY refcounting, TCP close, etc.
///
/// CRITICAL: No PM lock is held when this runs.
fn close_extracted_fds(entries: alloc::vec::Vec<(usize, FileDescriptor)>) {
    use crate::ipc::FdKind;

    for (_fd, fd_entry) in entries {
        match fd_entry.kind {
            FdKind::PipeRead(buffer) => { buffer.lock().close_read(); }
            FdKind::PipeWrite(buffer) => { buffer.lock().close_write(); }
            FdKind::TcpListener(port) => { crate::net::tcp::tcp_listener_ref_dec(port); }
            FdKind::TcpConnection(conn_id) => { let _ = crate::net::tcp::tcp_close(&conn_id); }
            FdKind::PtyMaster(pty_num) => {
                if let Some(pair) = crate::tty::pty::get(pty_num) {
                    let old_count = pair.master_refcount.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
                    if old_count == 1 {
                        crate::tty::pty::release(pty_num);
                    }
                }
            }
            FdKind::PtySlave(pty_num) => {
                if let Some(pair) = crate::tty::pty::get(pty_num) {
                    pair.slave_close();
                }
            }
            FdKind::UnixStream(socket) => { socket.lock().close(); }
            FdKind::FifoRead(path, buffer) => {
                crate::ipc::fifo::close_fifo_read(&path);
                buffer.lock().close_read();
            }
            FdKind::FifoWrite(path, buffer) => {
                crate::ipc::fifo::close_fifo_write(&path);
                buffer.lock().close_write();
            }
            _ => {} // StdIo, RegularFile, Directory, Device, etc. — no action needed
        }
    }
}

/// Integration functions for scheduling processes as tasks
pub struct ProcessScheduler;

impl ProcessScheduler {
    /// Handle process exit from scheduler context.
    ///
    /// Two-phase design to minimize PM lock hold time and prevent deadlocks:
    ///
    /// Phase 1 (under PM lock): Mark process terminated, extract FD entries,
    ///   set SIGCHLD on parent, collect parent thread ID for wakeup.
    ///   No logging, no pipe wakeups, no scheduler calls.
    ///
    /// Phase 2 (no PM lock): Close extracted FDs (pipe wakeups, PTY cleanup),
    ///   wake parent thread via scheduler, log the exit.
    ///
    /// This prevents a system-wide hang on ARM64 SMP where the PM lock (acquired
    /// with interrupts disabled on all CPUs) combined with logging (which acquires
    /// SERIAL and framebuffer locks) creates an unbreakable deadlock.
    pub fn handle_thread_exit(thread_id: u64, exit_code: i32) {
        // Phase 1: Under PM lock — minimal work only
        let phase1_result = {
            if let Some(ref mut manager) = *crate::process::manager() {
                if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                    let parent_pid = process.parent;
                    let process_name = process.name.clone();

                    // Mark terminated and extract FDs without closing them
                    process.terminate_minimal(exit_code);
                    let fd_entries = process.take_fd_entries();
                    // CoW cleanup is fast (no logging, no locks besides frame allocator)
                    process.cleanup_cow_frames();
                    process.drain_old_page_tables();

                    #[cfg(feature = "btrt")]
                    crate::test_framework::btrt::on_process_exit(pid.as_u64(), exit_code);

                    // Set SIGCHLD on parent and get parent thread ID for wakeup
                    let parent_tid = if let Some(parent_pid) = parent_pid {
                        if let Some(parent_process) = manager.get_process_mut(parent_pid) {
                            use crate::signal::constants::SIGCHLD;
                            parent_process.signals.set_pending(SIGCHLD);
                            parent_process.main_thread.as_ref().map(|t| t.id)
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    Some((pid, process_name, fd_entries, parent_tid))
                } else {
                    None
                }
            } else {
                None
            }
        }; // PM lock dropped here

        // Phase 2: No PM lock — safe to do pipe wakeups, scheduler calls, logging
        if let Some((pid, process_name, fd_entries, parent_tid)) = phase1_result {
            // Close FDs outside PM lock (pipe close_write wakes readers, etc.)
            close_extracted_fds(fd_entries);

            // Wake parent thread if blocked on waitpid or pause()
            if let Some(parent_tid) = parent_tid {
                scheduler::with_scheduler(|sched| {
                    sched.unblock_for_child_exit(parent_tid);
                    sched.unblock_for_signal(parent_tid);
                });
            }

            log::debug!(
                "Process {} '{}' (thread {}) exited with code {}",
                pid.as_u64(),
                process_name,
                thread_id,
                exit_code
            );
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
