//! Process-Task Integration
//!
//! This module bridges the gap between the Process Manager and the Task Scheduler,
//! allowing processes to be scheduled as tasks.

use crate::ipc::fd::FileDescriptor;
use crate::process::ProcessId;
use crate::task::scheduler;
use crate::task::thread::{Thread, ThreadPrivilege};
use core::sync::atomic::{AtomicU64, Ordering};

const DEFERRED_FAULT_EXIT_SLOTS: usize = 16;
const DEFERRED_FAULT_EXIT_EMPTY: u64 = 0;

struct DeferredFaultExitBuffer {
    slots: [AtomicU64; DEFERRED_FAULT_EXIT_SLOTS],
}

unsafe impl Sync for DeferredFaultExitBuffer {}

impl DeferredFaultExitBuffer {
    const fn new() -> Self {
        Self {
            slots: [const { AtomicU64::new(DEFERRED_FAULT_EXIT_EMPTY) };
                DEFERRED_FAULT_EXIT_SLOTS],
        }
    }

    fn push(&self, tid: u64) -> bool {
        for slot in &self.slots {
            if slot
                .compare_exchange(
                    DEFERRED_FAULT_EXIT_EMPTY,
                    tid,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return true;
            }
        }
        false
    }

    fn drain(&self, out: &mut alloc::vec::Vec<u64>) {
        for slot in &self.slots {
            let tid = slot.swap(DEFERRED_FAULT_EXIT_EMPTY, Ordering::AcqRel);
            if tid != DEFERRED_FAULT_EXIT_EMPTY {
                out.push(tid);
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
static DEFERRED_FAULT_EXIT_BUFFERS: [DeferredFaultExitBuffer; 8] =
    [const { DeferredFaultExitBuffer::new() }; 8];
#[cfg(not(target_arch = "aarch64"))]
static DEFERRED_FAULT_EXIT_BUFFERS: [DeferredFaultExitBuffer; 1] =
    [const { DeferredFaultExitBuffer::new() }];

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
            FdKind::PipeRead(buffer) => {
                buffer.lock().close_read();
            }
            FdKind::PipeWrite(buffer) => {
                buffer.lock().close_write();
            }
            FdKind::TcpListener(port) => {
                crate::net::tcp::tcp_listener_ref_dec(port);
            }
            FdKind::TcpConnection(conn_id) => {
                let _ = crate::net::tcp::tcp_close(&conn_id);
            }
            FdKind::PtyMaster(pty_num) => {
                if let Some(pair) = crate::tty::pty::get(pty_num) {
                    let old_count = pair
                        .master_refcount
                        .fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
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
            FdKind::UnixStream(socket) => {
                socket.lock().close();
            }
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
                    let children = core::mem::take(&mut process.children);

                    // Mark terminated and extract FDs without closing them
                    process.terminate_minimal(exit_code);
                    let fd_entries = process.take_fd_entries();
                    // CoW cleanup is fast (no logging, no locks besides frame allocator)
                    process.cleanup_cow_frames();
                    process.drain_old_page_tables();

                    // Free heavy resources immediately (CoW refcounts already decremented)
                    process.page_table.take();
                    process.stack.take();
                    process.pending_old_page_tables.clear();

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

                    // Reparent children to init (PID 1)
                    if !children.is_empty() {
                        use crate::process::ProcessId;
                        let init_pid = ProcessId::new(1);
                        for &child_pid in &children {
                            if let Some(child) = manager.get_process_mut(child_pid) {
                                child.parent = Some(init_pid);
                            }
                        }
                        if let Some(init) = manager.get_process_mut(init_pid) {
                            init.children.extend(children.iter());
                        }
                    }

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

            // Clean up window buffers so the compositor stops reading freed pages
            #[cfg(target_arch = "aarch64")]
            crate::syscall::graphics::cleanup_windows_for_pid(pid.as_u64());

            // Wake parent thread if blocked on waitpid or pause()
            if let Some(parent_tid) = parent_tid {
                scheduler::with_scheduler(|sched| {
                    sched.unblock_for_child_exit(parent_tid);
                    sched.unblock_for_signal(parent_tid);
                });
                crate::tracing::providers::process::trace_waitpid_wake(
                    parent_tid as u16,
                    pid.as_u64() as u16,
                );
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

/// Defer a SIGSEGV-style process exit for a user thread that faulted in kernel mode.
pub fn defer_fault_sigsegv_exit(thread_id: u64) -> bool {
    #[cfg(target_arch = "aarch64")]
    let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    #[cfg(not(target_arch = "aarch64"))]
    let cpu = 0usize;

    let idx = cpu.min(DEFERRED_FAULT_EXIT_BUFFERS.len().saturating_sub(1));
    DEFERRED_FAULT_EXIT_BUFFERS[idx].push(thread_id)
}

/// Drain deferred kernel-fault exits from a normal scheduling context.
pub fn drain_deferred_fault_sigsegv_exits() {
    let mut tids = alloc::vec::Vec::new();
    for buf in &DEFERRED_FAULT_EXIT_BUFFERS {
        buf.drain(&mut tids);
    }
    for tid in tids {
        ProcessScheduler::handle_thread_exit(tid, -11);
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
