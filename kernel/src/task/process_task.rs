//! Process-Task Integration
//!
//! This module bridges the gap between the Process Manager and the Task Scheduler,
//! allowing processes to be scheduled as tasks.

use crate::ipc::fd::FileDescriptor;
use crate::process::ProcessId;
use crate::task::scheduler;
use crate::task::thread::{Thread, ThreadPrivilege};
#[cfg(target_arch = "aarch64")]
use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_arch = "aarch64")]
const DEFERRED_FAULT_EXIT_SLOTS: usize = 16;
#[cfg(target_arch = "aarch64")]
const DEFERRED_FAULT_EXIT_EMPTY: u64 = 0;

#[cfg(target_arch = "aarch64")]
struct DeferredFaultExitBuffer {
    slots: [AtomicU64; DEFERRED_FAULT_EXIT_SLOTS],
}

#[cfg(target_arch = "aarch64")]
unsafe impl Sync for DeferredFaultExitBuffer {}

#[cfg(target_arch = "aarch64")]
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

    fn take_one(&self) -> Option<u64> {
        for slot in &self.slots {
            let tid = slot.swap(DEFERRED_FAULT_EXIT_EMPTY, Ordering::AcqRel);
            if tid != DEFERRED_FAULT_EXIT_EMPTY {
                return Some(tid);
            }
        }
        None
    }
}

#[cfg(target_arch = "aarch64")]
pub static FAULT_EXIT_INTENT_DROPPED: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static DEFERRED_FAULT_EXIT_BUFFERS: [DeferredFaultExitBuffer; 8] =
    [const { DeferredFaultExitBuffer::new() }; 8];
/// Close one process-owned file descriptor outside the PM lock.
///
/// CRITICAL: No PM lock is held when this runs.
#[cfg(target_arch = "aarch64")]
pub fn close_owned_fd(fd_entry: FileDescriptor) {
    use crate::ipc::FdKind;

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

#[cfg(target_arch = "x86_64")]
fn release_process_resources(process: &mut crate::process::Process) {
    process.cleanup_cow_frames();
    process.drain_old_page_tables();
    drop(process.page_table.take());
    drop(process.stack.take());
    process.pending_old_page_tables.clear();
}

#[cfg(target_arch = "x86_64")]
fn close_extracted_fds(entries: alloc::vec::Vec<(usize, FileDescriptor)>) {
    use crate::ipc::FdKind;

    for (_fd, fd_entry) in entries {
        match fd_entry.kind {
            FdKind::PipeRead(buffer) => buffer.lock().close_read(),
            FdKind::PipeWrite(buffer) => buffer.lock().close_write(),
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
            FdKind::UnixStream(socket) => socket.lock().close(),
            FdKind::FifoRead(path, buffer) => {
                crate::ipc::fifo::close_fifo_read(&path);
                buffer.lock().close_read();
            }
            FdKind::FifoWrite(path, buffer) => {
                crate::ipc::fifo::close_fifo_write(&path);
                buffer.lock().close_write();
            }
            _ => {}
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
    /// Phase 1 (under PM lock): Commit the process grave, set SIGCHLD on the
    ///   parent, and collect the durable worker obligations. No logging, FD
    ///   close, allocation, scheduler call, or heavy destructor runs here.
    ///
    /// Phase 2 (no PM lock): Complete the receipt and wake the reclaimer.
    ///
    /// This prevents a system-wide hang on ARM64 SMP where the PM lock (acquired
    /// with interrupts disabled on all CPUs) combined with logging (which acquires
    /// SERIAL and framebuffer locks) creates an unbreakable deadlock.
    #[cfg(target_arch = "aarch64")]
    pub fn handle_thread_exit(thread_id: u64, exit_code: i32) {
        let exit = {
            let mut manager_guard = crate::process::manager();
            manager_guard.as_mut().and_then(|manager| {
                let pid = manager.find_process_by_thread(thread_id).map(|(pid, _)| pid)?;
                Some((pid, manager.retire_process(pid, exit_code)))
            })
        };
        if let Some(exit) = exit {
            #[cfg(feature = "btrt")]
            {
                let (pid, receipt) = exit;
                receipt.complete();
                crate::test_framework::btrt::on_process_exit(pid.as_u64(), exit_code);
            }
            #[cfg(not(feature = "btrt"))]
            {
                let (_, receipt) = exit;
                receipt.complete();
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    pub fn handle_thread_exit(thread_id: u64, exit_code: i32) {
        let phase1_result = {
            if let Some(ref mut manager) = *crate::process::manager() {
                if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                    let parent_pid = process.parent;
                    let process_name = process.name.clone();
                    let children = core::mem::take(&mut process.children);

                    process.terminate_minimal(exit_code);
                    let fd_entries = process.take_fd_entries();
                    release_process_resources(process);

                    #[cfg(feature = "btrt")]
                    crate::test_framework::btrt::on_process_exit(pid.as_u64(), exit_code);

                    let parent_tid = if let Some(parent_pid) = parent_pid {
                        if let Some(parent_process) = manager.get_process_mut(parent_pid) {
                            use crate::signal::constants::SIGCHLD;
                            parent_process.signals.set_pending(SIGCHLD);
                            parent_process.main_thread.as_ref().map(|thread| thread.id)
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let init_pid = ProcessId::new(1);
                    // Guard against init reparenting its own children to itself.
                    // If `pid == init_pid` here, init itself is exiting (or being
                    // torn down) with live children still attached; without this
                    // check the loop below would set `child.parent = Some(init_pid)`
                    // for children that already have init as their parent and then
                    // re-append them onto `init.children`, corrupting the child
                    // list with duplicates and creating a self-referential
                    // reparent that never terminates the (already degenerate)
                    // parent chain. This mirrors the identical `pid != init_pid`
                    // guard in `ProcessManager`'s exit paths (see
                    // `process/manager.rs`), which apply the same constraint to
                    // the exact reparent-to-init operation. Missing the
                    // equivalent guard on aarch64's retire path was flagged as a
                    // blocking r13 review finding (reparent livelock); do not
                    // remove this without re-deriving that analysis.
                    if pid != init_pid && !children.is_empty() {
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
        };

        if let Some((pid, process_name, fd_entries, parent_tid)) = phase1_result {
            close_extracted_fds(fd_entries);

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
#[cfg(target_arch = "aarch64")]
pub fn defer_fault_sigsegv_exit(thread_id: u64) -> bool {
    let cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;

    let idx = cpu.min(DEFERRED_FAULT_EXIT_BUFFERS.len().saturating_sub(1));
    let queued = DEFERRED_FAULT_EXIT_BUFFERS[idx].push(thread_id);
    if queued {
        crate::task::reclaim::kreclaim_wake();
    }
    queued
}

/// Claim one deferred kernel-fault exit without allocating.
#[cfg(target_arch = "aarch64")]
pub fn take_deferred_fault_sigsegv_exit() -> Option<u64> {
    for buf in &DEFERRED_FAULT_EXIT_BUFFERS {
        if let Some(tid) = buf.take_one() {
            return Some(tid);
        }
    }
    None
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
