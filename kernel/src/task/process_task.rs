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

#[cfg(target_arch = "x86_64")]
const DEFERRED_SIGNAL_EXIT_SLOTS: usize = 16;
#[cfg(target_arch = "x86_64")]
const SIGNAL_EXIT_EMPTY: u64 = 0;
#[cfg(target_arch = "x86_64")]
const SIGNAL_EXIT_WRITING: u64 = 1;
#[cfg(target_arch = "x86_64")]
const SIGNAL_EXIT_PENDING: u64 = 2;
#[cfg(target_arch = "x86_64")]
const SIGNAL_EXIT_READY: u64 = 3;
#[cfg(target_arch = "x86_64")]
const SIGNAL_EXIT_READING: u64 = 4;

#[cfg(target_arch = "x86_64")]
struct DeferredSignalExitSlot {
    state: AtomicU64,
    owner_cpu: AtomicU64,
    child_pid: AtomicU64,
    exit_code: AtomicU64,
    victim_tid: AtomicU64,
}

#[cfg(target_arch = "x86_64")]
impl DeferredSignalExitSlot {
    const fn new() -> Self {
        Self {
            state: AtomicU64::new(SIGNAL_EXIT_EMPTY),
            owner_cpu: AtomicU64::new(0),
            child_pid: AtomicU64::new(0),
            exit_code: AtomicU64::new(0),
            victim_tid: AtomicU64::new(0),
        }
    }
}

#[cfg(target_arch = "x86_64")]
static DEFERRED_SIGNAL_EXITS: [DeferredSignalExitSlot; DEFERRED_SIGNAL_EXIT_SLOTS] =
    [const { DeferredSignalExitSlot::new() }; DEFERRED_SIGNAL_EXIT_SLOTS];

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn x86_cpu_id() -> u64 {
    <crate::arch_impl::x86_64::X86PerCpu as crate::arch_impl::PerCpuOps>::cpu_id()
}

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
static DEFERRED_FAULT_EXIT_COUNT: AtomicU64 = AtomicU64::new(0);

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
    let queued = DEFERRED_FAULT_EXIT_BUFFERS[idx].push(thread_id);
    #[cfg(target_arch = "aarch64")]
    if queued {
        DEFERRED_FAULT_EXIT_COUNT.fetch_add(1, Ordering::Release);
        crate::task::reclaim::kreclaim_wake();
    }
    queued
}

/// Claim one deferred kernel-fault exit without allocating.
pub fn take_deferred_fault_sigsegv_exit() -> Option<u64> {
    for buf in &DEFERRED_FAULT_EXIT_BUFFERS {
        if let Some(tid) = buf.take_one() {
            #[cfg(target_arch = "aarch64")]
            DEFERRED_FAULT_EXIT_COUNT.fetch_sub(1, Ordering::AcqRel);
            return Some(tid);
        }
    }
    None
}

/// Publish a fatal x86 signal exit before returning the notification to a
/// caller. Normal consumers cancel their copy before completing it; the
/// syscall-return consumer that discards the notification leaves this durable
/// intent for the off-stack idle context.
#[cfg(target_arch = "x86_64")]
pub(crate) fn defer_signal_exit(notification: &crate::signal::delivery::ParentNotification) -> bool {
    for slot in &DEFERRED_SIGNAL_EXITS {
        if slot
            .state
            .compare_exchange(
                SIGNAL_EXIT_EMPTY,
                SIGNAL_EXIT_WRITING,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            slot.owner_cpu.store(x86_cpu_id(), Ordering::Relaxed);
            slot.child_pid
                .store(notification.child_pid.as_u64(), Ordering::Relaxed);
            slot.exit_code
                .store(notification.exit_code as i64 as u64, Ordering::Relaxed);
            slot.victim_tid
                .store(notification.victim_tid.unwrap_or(0), Ordering::Relaxed);
            slot.state.store(SIGNAL_EXIT_PENDING, Ordering::Release);
            return true;
        }
    }
    false
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn cancel_deferred_signal_exit(child_pid: ProcessId) {
    for slot in &DEFERRED_SIGNAL_EXITS {
        loop {
            let state = slot.state.load(Ordering::Acquire);
            if !matches!(state, SIGNAL_EXIT_PENDING | SIGNAL_EXIT_READY) {
                break;
            }
            if slot
                .state
                .compare_exchange(
                    state,
                    SIGNAL_EXIT_READING,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_err()
            {
                continue;
            }
            if slot.child_pid.load(Ordering::Relaxed) == child_pid.as_u64() {
                slot.state.store(SIGNAL_EXIT_EMPTY, Ordering::Release);
                return;
            }
            slot.state.store(state, Ordering::Release);
            break;
        }
    }
}

/// Make this CPU's pending signal-exit intents visible to its idle loop. The
/// syscall-return fatal-signal path calls `switch_to_idle()` after discarding
/// the notification; ordinary consumers cancel while the intent is pending.
#[cfg(target_arch = "x86_64")]
pub(crate) fn arm_deferred_signal_exits_for_current_cpu() {
    let current_cpu = x86_cpu_id();
    for slot in &DEFERRED_SIGNAL_EXITS {
        if slot.owner_cpu.load(Ordering::Relaxed) == current_cpu {
            let _ = slot.state.compare_exchange(
                SIGNAL_EXIT_PENDING,
                SIGNAL_EXIT_READY,
                Ordering::Release,
                Ordering::Relaxed,
            );
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn take_deferred_signal_exit() -> Option<crate::signal::delivery::ParentNotification> {
    let current_cpu = x86_cpu_id();
    for slot in &DEFERRED_SIGNAL_EXITS {
        if slot
            .state
            .compare_exchange(
                SIGNAL_EXIT_READY,
                SIGNAL_EXIT_READING,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            continue;
        }
        if slot.owner_cpu.load(Ordering::Relaxed) != current_cpu {
            slot.state.store(SIGNAL_EXIT_READY, Ordering::Release);
            continue;
        }
        let child_pid = ProcessId::new(slot.child_pid.load(Ordering::Relaxed));
        let exit_code = slot.exit_code.load(Ordering::Relaxed) as i64 as i32;
        let victim_tid = match slot.victim_tid.load(Ordering::Relaxed) {
            0 => None,
            tid => Some(tid),
        };
        slot.state.store(SIGNAL_EXIT_EMPTY, Ordering::Release);
        return Some(crate::signal::delivery::ParentNotification {
            child_pid,
            exit_code,
            victim_tid,
        });
    }
    None
}

/// Complete one deferred fatal-signal exit from off-stack, IRQ-enabled x86
/// idle context.
#[cfg(target_arch = "x86_64")]
pub fn complete_one_deferred_signal_exit() -> bool {
    let Some(notification) = take_deferred_signal_exit() else {
        return false;
    };
    crate::signal::delivery::notify_parent_of_termination_deferred(&notification);
    true
}

/// Visit queued AArch64 fault victims without consuming their process-exit
/// intents. The scheduler uses this while holding its own lock so a victim is
/// quarantined before selection; kreclaimd remains the sole consumer.
#[cfg(target_arch = "aarch64")]
pub(crate) fn for_each_deferred_fault_exit(mut visit: impl FnMut(u64)) {
    if DEFERRED_FAULT_EXIT_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    for buffer in &DEFERRED_FAULT_EXIT_BUFFERS {
        for slot in &buffer.slots {
            let tid = slot.load(Ordering::Acquire);
            if tid != DEFERRED_FAULT_EXIT_EMPTY {
                visit(tid);
            }
        }
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
