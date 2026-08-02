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

pub static FAULT_EXIT_INTENT_DROPPED: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static DEFERRED_FAULT_EXIT_BUFFERS: [DeferredFaultExitBuffer; 8] =
    [const { DeferredFaultExitBuffer::new() }; 8];
#[cfg(not(target_arch = "aarch64"))]
static DEFERRED_FAULT_EXIT_BUFFERS: [DeferredFaultExitBuffer; 1] =
    [const { DeferredFaultExitBuffer::new() }];

/// Close one process-owned file descriptor outside the PM lock.
///
/// CRITICAL: No PM lock is held when this runs.
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
    pub fn handle_thread_exit(thread_id: u64, exit_code: i32) {
        let receipt = {
            let mut manager_guard = crate::process::manager();
            manager_guard.as_mut().and_then(|manager| {
                let pid = manager.find_process_by_thread(thread_id).map(|(pid, _)| pid)?;
                Some(manager.retire_process(pid, exit_code))
            })
        };
        if let Some(receipt) = receipt {
            receipt.complete();
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

/// Claim one deferred kernel-fault exit without allocating.
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
