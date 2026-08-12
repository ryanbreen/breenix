//! Process-Task Integration
//!
//! This module bridges the gap between the Process Manager and the Task Scheduler,
//! allowing processes to be scheduled as tasks.

use crate::ipc::fd::FileDescriptor;
use crate::memory::process_memory::AbandonReason;
use crate::process::ProcessId;
use crate::task::scheduler;
use crate::task::thread::{Thread, ThreadPrivilege};
#[cfg(target_arch = "aarch64")]
use core::sync::atomic::AtomicU32;
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
        crate::trace_count!(
            crate::tracing::providers::teardown::DEFERRED_FAULT_RING_DROPPED
        );
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

#[cfg(target_arch = "aarch64")]
pub(crate) struct PendingProcessReclaim {
    pid: u64,
    page_table: Option<alloc::boxed::Box<crate::memory::process_memory::ProcessPageTable>>,
    old_page_tables: alloc::vec::Vec<
        alloc::boxed::Box<crate::memory::process_memory::ProcessPageTable>,
    >,
    after_epoch: scheduler::RetirementFence,
    last_pass: u32,
    proof_failures: u8,
    parked: Option<ParkRecord>,
}

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
struct ParkRecord {
    fence_at_park: scheduler::RetirementFence,
    row_epoch_at_park: u64,
    age_epoch_sum_at_park: u64,
}

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy, Default)]
struct RootProof {
    blocked_epoch: bool,
    blocked_hw: bool,
    blocked_shadow: bool,
    blocked_cached: bool,
    blocked_live_row: bool,
}

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum RootBlocker {
    Epoch,
    Hardware,
    Shadow,
    Cached,
    LiveRow,
}

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
enum UnparkReason {
    Epoch,
    Row,
    Age,
}

#[cfg(target_arch = "aarch64")]
impl PendingProcessReclaim {
    fn any_root_matches<F>(&self, mut root_matches: F) -> bool
    where
        F: FnMut(u64) -> bool,
    {
        self.page_table
            .iter()
            .chain(self.old_page_tables.iter())
            .any(|page_table| root_matches(page_table.level_4_frame().start_address().as_u64()))
    }

    fn lock_free_root_proof(
        &self,
        snapshot: &scheduler::RetirementSnapshot,
        allow_boot_injection: bool,
    ) -> RootProof {
        if !snapshot.fence_elapsed(&self.after_epoch)
            || boot_forces_blocker(self.pid, RootBlocker::Epoch, allow_boot_injection)
        {
            return RootProof::blocked(RootBlocker::Epoch);
        }
        let local_ttbr0 = crate::arch_impl::aarch64::ttbr0::local_ttbr0_root();
        if self.any_root_matches(|root| {
            crate::arch_impl::aarch64::ttbr0::roots_match(local_ttbr0, root)
        }) || boot_forces_blocker(self.pid, RootBlocker::Hardware, allow_boot_injection)
        {
            return RootProof::blocked(RootBlocker::Hardware);
        }
        if self.any_root_matches(|root| {
            crate::arch_impl::aarch64::ttbr0::is_ttbr0_root_live_in_mask(
                root,
                self.after_epoch.online_mask,
            )
        }) || boot_forces_blocker(self.pid, RootBlocker::Shadow, allow_boot_injection)
        {
            return RootProof::blocked(RootBlocker::Shadow);
        }
        RootProof::default()
    }

    fn cached_root_is_live(&self) -> bool {
        scheduler::with_scheduler(|scheduler| {
            scheduler.any_cached_ttbr0_matches(|cached| {
                self.any_root_matches(|root| {
                    crate::arch_impl::aarch64::ttbr0::roots_match(cached, root)
                })
            })
        })
        .unwrap_or(false)
    }

    fn live_row_names_root(&self) -> bool {
        crate::process::manager().as_ref().is_some_and(|manager| {
            manager.any_live_root_matches(|row_root| {
                self.any_root_matches(|root| {
                    crate::arch_impl::aarch64::ttbr0::roots_match(row_root, root)
                })
            })
        })
    }

    fn reclaim(mut self) {
        if let Some(page_table) = self.page_table.as_ref() {
            crate::process::process::cleanup_cow_page_table(page_table);
        }
        for old_page_table in self.old_page_tables.drain(..) {
            old_page_table.cleanup_for_exec();
        }
        if let Some(page_table) = self.page_table.take() {
            page_table.abandon(AbandonReason::NoProofPipeline);
        }
    }
}

#[cfg(target_arch = "aarch64")]
static PENDING_PROCESS_RECLAIMS: spin::Mutex<alloc::vec::Vec<PendingProcessReclaim>> =
    spin::Mutex::new(alloc::vec::Vec::new());
#[cfg(target_arch = "aarch64")]
static PARKED_PROCESS_RECLAIMS: spin::Mutex<alloc::vec::Vec<PendingProcessReclaim>> =
    spin::Mutex::new(alloc::vec::Vec::new());
#[cfg(target_arch = "aarch64")]
static RECLAIM_PASS_ID: AtomicU32 = AtomicU32::new(0);
#[cfg(target_arch = "aarch64")]
static ROW_REMOVAL_EPOCH: AtomicU64 = AtomicU64::new(0);

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_RECLAIM_TEST_OWNER: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_RECLAIM_FORCED_PID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_RECLAIM_FORCED_BLOCKER: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_RECLAIM_PASS_START: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_RECLAIM_PASS_SELECTIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_RECLAIM_LAST_PARK_EPOCHS: [AtomicU64; crate::arch_impl::aarch64::constants::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch_impl::aarch64::constants::MAX_CPUS];
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_RECLAIM_LAST_PARK_MASK: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_RECLAIM_LAST_PARK_ROW_EPOCH: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
const PROOF_FAILURES_BEFORE_PARK: u8 = 3;
#[cfg(target_arch = "aarch64")]
const PARK_AGE_BACKSTOP_EPOCHS: u64 = 64;

#[cfg(target_arch = "aarch64")]
impl RootProof {
    fn blocked(blocker: RootBlocker) -> Self {
        let mut proof = Self::default();
        match blocker {
            RootBlocker::Epoch => proof.blocked_epoch = true,
            RootBlocker::Hardware => proof.blocked_hw = true,
            RootBlocker::Shadow => proof.blocked_shadow = true,
            RootBlocker::Cached => proof.blocked_cached = true,
            RootBlocker::LiveRow => proof.blocked_live_row = true,
        }
        proof
    }

    fn blocker(self) -> Option<RootBlocker> {
        if self.blocked_epoch {
            Some(RootBlocker::Epoch)
        } else if self.blocked_hw {
            Some(RootBlocker::Hardware)
        } else if self.blocked_shadow {
            Some(RootBlocker::Shadow)
        } else if self.blocked_cached {
            Some(RootBlocker::Cached)
        } else if self.blocked_live_row {
            Some(RootBlocker::LiveRow)
        } else {
            None
        }
    }
}

#[cfg(target_arch = "aarch64")]
impl ParkRecord {
    fn unpark_reason(
        &self,
        snapshot: &scheduler::RetirementSnapshot,
        row_epoch: u64,
    ) -> Option<UnparkReason> {
        if snapshot.all_advanced_since(&self.fence_at_park) {
            Some(UnparkReason::Epoch)
        } else if row_epoch != self.row_epoch_at_park {
            Some(UnparkReason::Row)
        } else if snapshot
            .epoch_sum(self.fence_at_park.online_mask)
            .wrapping_sub(self.age_epoch_sum_at_park)
            >= PARK_AGE_BACKSTOP_EPOCHS
        {
            Some(UnparkReason::Age)
        } else {
            None
        }
    }
}

pub(crate) fn note_process_row_removed() {
    #[cfg(target_arch = "aarch64")]
    ROW_REMOVAL_EPOCH.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn release_process_resources(process: &mut crate::process::Process) {
    if crate::process::process_manager_held_on_current_cpu() {
        crate::tracing::providers::teardown::record_masked_frames_walked(process.id.as_u64());
    }
    process.cleanup_cow_frames();
    process.drain_old_page_tables();
    if let Some(page_table) = process.page_table.take() {
        #[cfg(target_arch = "aarch64")]
        page_table.abandon(AbandonReason::NoProofPipeline);
        #[cfg(not(target_arch = "aarch64"))]
        page_table.abandon(AbandonReason::NoArchPipeline);
    }
    drop(process.stack.take());
    process.pending_old_page_tables.clear();
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn defer_live_process_resources(
    process: &mut crate::process::Process,
) -> Option<PendingProcessReclaim> {
    let snapshot = scheduler::RetirementSnapshot::capture();
    let root_is_live = process
        .page_table
        .iter()
        .chain(process.pending_old_page_tables.iter())
        .any(|page_table| {
            crate::arch_impl::aarch64::ttbr0::is_ttbr0_root_live_in_mask(
                page_table.level_4_frame().start_address().as_u64(),
                snapshot.online_mask,
            )
        });
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    let root_is_live = root_is_live
        || FORCE_LIVE_RECLAIM_TEST_PID.load(Ordering::Acquire) == process.id.as_u64();
    if !root_is_live {
        return None;
    }

    Some(defer_process_resources(process))
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static FORCE_LIVE_RECLAIM_TEST_PID: AtomicU64 = AtomicU64::new(0);

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub(crate) struct ForceLiveReclaimTestGuard;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl ForceLiveReclaimTestGuard {
    pub(crate) fn arm(pid: u64) -> Self {
        FORCE_LIVE_RECLAIM_TEST_PID.store(pid, Ordering::Release);
        Self
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl Drop for ForceLiveReclaimTestGuard {
    fn drop(&mut self) {
        FORCE_LIVE_RECLAIM_TEST_PID.store(0, Ordering::Release);
    }
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn defer_process_resources(
    process: &mut crate::process::Process,
) -> PendingProcessReclaim {
    crate::tracing::providers::teardown::record_defer(process.id.as_u64());
    PendingProcessReclaim {
        pid: process.id.as_u64(),
        page_table: process.page_table.take(),
        old_page_tables: core::mem::take(&mut process.pending_old_page_tables),
        after_epoch: scheduler::retirement_grace_target(),
        last_pass: 0,
        proof_failures: 0,
        parked: None,
    }
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn enqueue_process_reclaim(reclaim: PendingProcessReclaim) {
    if crate::process::process_manager_held_on_current_cpu() {
        crate::trace_count!(
            crate::tracing::providers::teardown::RECLAIM_ENQUEUE_UNDER_PM
        );
    }
    crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().push(reclaim));
}

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
        crate::trace_count!(crate::tracing::providers::teardown::TEARDOWN_ENTRY_EXIT);
        // Capture the claimer before taking PM. This is a separate scheduler-only
        // acquisition; no scheduler state is consulted while PM is live.
        let report_claimer = scheduler::current_thread_id().unwrap_or(thread_id);
        // Phase 1: Under PM lock — minimal work only
        let phase1_result = {
            if let Some(ref mut manager) = *crate::process::manager() {
                if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                    let already_terminated = process.is_terminated();
                    crate::tracing::providers::teardown::record_exit_request(already_terminated);
                    if !already_terminated {
                        process.exit_notifications.seed();
                    }
                    let parent_pid = process.parent;
                    let process_name = process.name.clone();
                    let children = if pid == ProcessId::new(1) {
                        alloc::vec::Vec::new()
                    } else {
                        core::mem::take(&mut process.children)
                    };

                    // Extract FDs without closing them under the PM lock.
                    let fd_entries = process.take_fd_entries();
                    let retirement_receipt: Option<crate::process::RetirementReceipt> =
                        if already_terminated {
                            // Preserve the single-CoW-decref invariant: external
                            // terminate() already walked these mappings, so raw-drop
                            // them without another reclaim/decref path.
                            if let Some(page_table) = process.page_table.take() {
                                page_table.abandon(AbandonReason::AlreadyTerminated);
                            }
                            drop(process.stack.take());
                            process.pending_old_page_tables.clear();
                            None
                        } else {
                            #[cfg(target_arch = "aarch64")]
                            let receipt =
                                if let Some(reclaim) = defer_live_process_resources(process) {
                                    drop(process.stack.take());
                                    Some(crate::process::RetirementReceipt::from_reclaim(reclaim))
                                } else {
                                    release_process_resources(process);
                                    None
                                };
                            #[cfg(not(target_arch = "aarch64"))]
                            let receipt = {
                                release_process_resources(process);
                                None
                            };
                            receipt
                        };

                    // Keep termination after release/deferral. Once page_table is
                    // None, cleanup_cow_frames() cannot repeat the CoW walk; moving
                    // terminate()/terminate_minimal() earlier breaks that invariant.
                    process.terminate_minimal(exit_code);
                    // terminate_minimal() is a no-op on a repeat teardown pass, so
                    // preserve and report the first-recorded status.
                    let reported_exit_code = process.exit_code.unwrap_or(exit_code);
                    let report_claimed = process.exit_notifications.claim_report(report_claimer);
                    let sigchld_pending = matches!(
                        process.exit_notifications.sigchld,
                        crate::process::process::ExitObligationState::Pending
                    );

                    // Set SIGCHLD on parent and get parent thread ID for wakeup
                    let parent_tid = if let Some(parent_pid) = parent_pid {
                        if let Some(parent_process) = manager.get_process_mut(parent_pid) {
                            if sigchld_pending {
                                use crate::signal::constants::SIGCHLD;
                                parent_process.signals.set_pending(SIGCHLD);
                            }
                            parent_process.main_thread.as_ref().map(|t| t.id)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if sigchld_pending {
                        if let Some(process) = manager.get_process_mut(pid) {
                            process.exit_notifications.complete_sigchld();
                        }
                    }

                    // Reparent children to init (PID 1)
                    if !children.is_empty() {
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

                    Some((
                        pid,
                        process_name,
                        fd_entries,
                        parent_tid,
                        retirement_receipt,
                        report_claimed,
                        reported_exit_code,
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        }; // PM lock dropped here

        // Phase 2: No PM lock — safe to do pipe wakeups, scheduler calls, logging
        if let Some((
            pid,
            process_name,
            fd_entries,
            parent_tid,
            retirement_receipt,
            report_claimed,
            reported_exit_code,
        )) = phase1_result
        {
            #[cfg(target_arch = "aarch64")]
            if let Some(mut receipt) = retirement_receipt {
                if let Some(reclaim) = receipt.take_contents() {
                    enqueue_process_reclaim(reclaim);
                }
            }
            #[cfg(not(target_arch = "aarch64"))]
            let _retirement_receipt = retirement_receipt;

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

            if report_claimed {
                #[cfg(feature = "btrt")]
                crate::test_framework::btrt::on_process_exit(pid.as_u64(), reported_exit_code);
                #[cfg(not(feature = "btrt"))]
                let _ = reported_exit_code;
                crate::tracing::providers::teardown::record_report(pid.as_u64());

                let completing_claimer = scheduler::current_thread_id().unwrap_or(report_claimer);
                let _ = crate::process::with_process_manager(|manager| {
                    if let Some(process) = manager.get_process_mut(pid) {
                        process
                            .exit_notifications
                            .complete_report(completing_claimer);
                    }
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

#[cfg(target_arch = "aarch64")]
fn next_reclaim_pass_id(mut pass: u32) -> u32 {
    while pass == 0 {
        pass = RECLAIM_PASS_ID
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
    }
    pass
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

#[cfg(target_arch = "aarch64")]
fn record_root_blocker(blocker: RootBlocker) {
    match blocker {
        RootBlocker::Epoch => {
            crate::trace_count!(crate::tracing::providers::teardown::ROOT_PROOF_BLOCKED_EPOCH);
        }
        RootBlocker::Hardware => {
            crate::trace_count!(crate::tracing::providers::teardown::ROOT_PROOF_BLOCKED_HW);
        }
        RootBlocker::Shadow => {
            crate::trace_count!(crate::tracing::providers::teardown::ROOT_PROOF_BLOCKED_SHADOW);
        }
        RootBlocker::Cached => {
            crate::trace_count!(crate::tracing::providers::teardown::ROOT_PROOF_BLOCKED_CACHED);
        }
        RootBlocker::LiveRow => {
            crate::trace_count!(crate::tracing::providers::teardown::ROOT_PROOF_BLOCKED_LIVE_ROW);
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn record_unpark(reason: UnparkReason) {
    match reason {
        UnparkReason::Epoch => {
            crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_UNPARKED_EPOCH);
        }
        UnparkReason::Row => {
            crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_UNPARKED_ROW);
        }
        UnparkReason::Age => {
            crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_UNPARKED_AGE);
        }
    }
    crate::trace_count_add!(
        crate::tracing::providers::teardown::RECLAIM_PARK_RESIDENT,
        u64::MAX
    );
}

#[cfg(target_arch = "aarch64")]
fn park_reclaim(mut reclaim: PendingProcessReclaim) {
    let snapshot_at_park = scheduler::RetirementSnapshot::capture();
    let fence_at_park = snapshot_at_park.as_fence();
    let row_epoch_at_park = ROW_REMOVAL_EPOCH.load(Ordering::Relaxed);
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    if BOOT_RECLAIM_TEST_OWNER.load(Ordering::Acquire) != 0 {
        for (slot, epoch) in BOOT_RECLAIM_LAST_PARK_EPOCHS
            .iter()
            .zip(snapshot_at_park.epochs)
        {
            slot.store(epoch, Ordering::Relaxed);
        }
        BOOT_RECLAIM_LAST_PARK_MASK.store(snapshot_at_park.online_mask, Ordering::Relaxed);
        BOOT_RECLAIM_LAST_PARK_ROW_EPOCH.store(row_epoch_at_park, Ordering::Relaxed);
    }
    let park_record = ParkRecord {
        fence_at_park,
        row_epoch_at_park,
        age_epoch_sum_at_park: snapshot_at_park.epoch_sum(fence_at_park.online_mask),
    };
    if park_record
        .unpark_reason(&snapshot_at_park, row_epoch_at_park)
        .is_some()
    {
        crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARK_IMMEDIATE_UNPARK);
    }
    reclaim.parked = Some(park_record);
    crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARKED);
    crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARK_RESIDENT);
    crate::arch_without_interrupts(|| PARKED_PROCESS_RECLAIMS.lock().push(reclaim));
}

#[cfg(target_arch = "aarch64")]
fn unpark_sweep_with_snapshot(snapshot: scheduler::RetirementSnapshot, row_epoch: u64) {
    let mut ready = alloc::vec::Vec::new();
    crate::arch_without_interrupts(|| {
        let mut parked = PARKED_PROCESS_RECLAIMS.lock();
        let mut index = 0;
        while index < parked.len() {
            let reason = parked[index]
                .parked
                .as_ref()
                .and_then(|record| record.unpark_reason(&snapshot, row_epoch));
            if let Some(reason) = reason {
                let mut reclaim = parked.swap_remove(index);
                reclaim.parked = None;
                reclaim.proof_failures = 0;
                record_unpark(reason);
                ready.push(reclaim);
            } else {
                index += 1;
            }
        }
    });
    if !ready.is_empty() {
        crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().extend(ready));
    }
}

#[cfg(target_arch = "aarch64")]
fn unpark_sweep() {
    let snapshot = scheduler::RetirementSnapshot::capture();
    let row_epoch = ROW_REMOVAL_EPOCH.load(Ordering::Relaxed);
    unpark_sweep_with_snapshot(snapshot, row_epoch);
}

#[cfg(target_arch = "aarch64")]
fn boot_forces_blocker(pid: u64, blocker: RootBlocker, allow_boot_injection: bool) -> bool {
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    {
        allow_boot_injection
            && BOOT_RECLAIM_FORCED_PID.load(Ordering::Acquire) == pid
            && BOOT_RECLAIM_FORCED_BLOCKER.load(Ordering::Acquire) == blocker as u64 + 1
    }
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    {
        let _ = (pid, blocker, allow_boot_injection);
        false
    }
}

#[cfg(target_arch = "aarch64")]
fn boot_after_step_two(fence: &scheduler::RetirementFence) {
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    if BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO.swap(0, Ordering::AcqRel) != 0 {
        for cpu_id in 0..crate::arch_impl::aarch64::constants::MAX_CPUS {
            if fence.online_mask & (1 << cpu_id) != 0 {
                scheduler::note_scheduling_epoch(cpu_id);
            }
        }
    }
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = fence;
}

#[cfg(target_arch = "aarch64")]
fn boot_begin_reclaim_pass(boot_test_owned: bool) {
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    if boot_test_owned {
        let queue_len = crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().len());
        BOOT_RECLAIM_PASS_START.store(queue_len as u64, Ordering::Relaxed);
        BOOT_RECLAIM_PASS_SELECTIONS.store(0, Ordering::Relaxed);
    }
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = boot_test_owned;
}

#[cfg(target_arch = "aarch64")]
fn boot_note_reclaim_selection(boot_test_owned: bool) {
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    if boot_test_owned {
        BOOT_RECLAIM_PASS_SELECTIONS.fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = boot_test_owned;
}

#[cfg(target_arch = "aarch64")]
fn boot_finish_reclaim_pass(boot_test_owned: bool) {
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    if boot_test_owned {
        debug_assert!(
            BOOT_RECLAIM_PASS_SELECTIONS.load(Ordering::Relaxed)
                <= BOOT_RECLAIM_PASS_START.load(Ordering::Relaxed)
        );
    }
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = boot_test_owned;
}

/// Reclaim process frames whose cross-CPU TTBR0 retention has quiesced.
#[cfg(target_arch = "aarch64")]
pub fn reclaim_deferred_process_resources() {
    let my_pass = next_reclaim_pass_id(
        RECLAIM_PASS_ID
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1),
    );
    if crate::process::process_manager_held_on_current_cpu()
        || crate::tracing::providers::teardown::scheduler_scope_active()
    {
        crate::trace_count!(
            crate::tracing::providers::teardown::RECLAIM_CONTEXT_VIOLATIONS
        );
        return;
    }
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    if BOOT_RECLAIM_TEST_OWNER.load(Ordering::Acquire) != 0 {
        return;
    }

    reclaim_deferred_process_resources_for_pass(my_pass, false);
}

#[cfg(target_arch = "aarch64")]
fn reclaim_deferred_process_resources_for_pass(my_pass: u32, boot_test_owned: bool) {
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = boot_test_owned;
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    if !boot_test_owned && BOOT_RECLAIM_TEST_OWNER.load(Ordering::Acquire) != 0 {
        return;
    }
    unpark_sweep();
    boot_begin_reclaim_pass(boot_test_owned);

    loop {
        #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
        if !boot_test_owned && BOOT_RECLAIM_TEST_OWNER.load(Ordering::Acquire) != 0 {
            break;
        }
        let reclaim = crate::arch_without_interrupts(|| {
            let mut pending = PENDING_PROCESS_RECLAIMS.lock();
            let _proof_scope =
                crate::tracing::providers::teardown::ReclaimProofScope::enter();
            let snapshot = scheduler::RetirementSnapshot::capture();
            let ready = pending.iter_mut().position(|reclaim| {
                if reclaim.last_pass == my_pass {
                    crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PASS_SKIPPED);
                    return false;
                }
                let proof = reclaim.lock_free_root_proof(&snapshot, false);
                if let Some(blocker) = proof.blocker() {
                    record_root_blocker(blocker);
                    return false;
                }
                reclaim.last_pass = my_pass;
                true
            });
            ready.map(|index| pending.swap_remove(index))
        });

        match reclaim {
            Some(mut reclaim) => {
                boot_note_reclaim_selection(boot_test_owned);
                let snapshot = scheduler::RetirementSnapshot::capture();
                let mut proof = reclaim.lock_free_root_proof(&snapshot, true);
                boot_after_step_two(&reclaim.after_epoch);
                if proof.blocker().is_none()
                    && (reclaim.cached_root_is_live()
                        || boot_forces_blocker(reclaim.pid, RootBlocker::Cached, true))
                {
                    proof = RootProof::blocked(RootBlocker::Cached);
                }
                if proof.blocker().is_none()
                    && (reclaim.live_row_names_root()
                        || boot_forces_blocker(reclaim.pid, RootBlocker::LiveRow, true))
                {
                    proof = RootProof::blocked(RootBlocker::LiveRow);
                }

                if let Some(blocker) = proof.blocker() {
                    record_root_blocker(blocker);
                    if matches!(blocker, RootBlocker::Cached | RootBlocker::LiveRow) {
                        reclaim.proof_failures = reclaim.proof_failures.saturating_add(1);
                    }
                    if reclaim.proof_failures == PROOF_FAILURES_BEFORE_PARK {
                        park_reclaim(reclaim);
                    } else {
                        crate::arch_without_interrupts(|| {
                            PENDING_PROCESS_RECLAIMS.lock().push(reclaim)
                        });
                    }
                } else {
                    crate::tracing::providers::teardown::record_reclaim(reclaim.pid);
                    reclaim.reclaim();
                }
            }
            None => break,
        }
    }
    boot_finish_reclaim_pass(boot_test_owned);
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub(crate) fn boot_reclaim_deferred_process_resources() {
    let my_pass = next_reclaim_pass_id(
        RECLAIM_PASS_ID
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1),
    );
    reclaim_deferred_process_resources_for_pass(my_pass, true);
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const BOOT_RECLAIM_PID_BASE: u64 = u64::MAX - 0x1000;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub(crate) struct BootReclaimTestGuard;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl BootReclaimTestGuard {
    pub(crate) fn enter() -> Result<Self, &'static str> {
        let owner = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id().wrapping_add(1);
        BOOT_RECLAIM_TEST_OWNER
            .compare_exchange(0, owner, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "another reclaim injection is active")?;
        let live_empty =
            crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().is_empty());
        let parked_empty =
            crate::arch_without_interrupts(|| PARKED_PROCESS_RECLAIMS.lock().is_empty());
        let queues_empty = live_empty && parked_empty;
        if !queues_empty {
            BOOT_RECLAIM_TEST_OWNER.store(0, Ordering::Release);
            return Err("reclaim queues were not quiescent before P1 gate");
        }
        BOOT_RECLAIM_FORCED_PID.store(0, Ordering::Relaxed);
        BOOT_RECLAIM_FORCED_BLOCKER.store(0, Ordering::Relaxed);
        BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO.store(0, Ordering::Relaxed);
        Ok(Self)
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl Drop for BootReclaimTestGuard {
    fn drop(&mut self) {
        BOOT_RECLAIM_FORCED_PID.store(0, Ordering::Release);
        BOOT_RECLAIM_FORCED_BLOCKER.store(0, Ordering::Release);
        BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO.store(0, Ordering::Release);
        crate::arch_without_interrupts(|| {
            PENDING_PROCESS_RECLAIMS
                .lock()
                .retain(|reclaim| reclaim.pid < BOOT_RECLAIM_PID_BASE);
            let mut parked = PARKED_PROCESS_RECLAIMS.lock();
            let before = parked.len();
            parked.retain(|reclaim| reclaim.pid < BOOT_RECLAIM_PID_BASE);
            for _ in parked.len()..before {
                crate::trace_count_add!(
                    crate::tracing::providers::teardown::RECLAIM_PARK_RESIDENT,
                    u64::MAX
                );
            }
        });
        BOOT_RECLAIM_TEST_OWNER.store(0, Ordering::Release);
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_test_reclaim(pid: u64) -> PendingProcessReclaim {
    PendingProcessReclaim {
        pid,
        page_table: None,
        old_page_tables: alloc::vec::Vec::new(),
        after_epoch: scheduler::RetirementSnapshot::capture().as_fence(),
        last_pass: 0,
        proof_failures: 0,
        parked: None,
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn retirement_receipt_drop_gate_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;
    use crate::tracing::providers::teardown::{RECEIPT_DROPPED_UNRETIRED, TEARDOWN_RECLAIM};

    let _guard = match BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(message) => return TestResult::Fail(message),
    };
    let pid = BOOT_RECLAIM_PID_BASE + 1;
    let queue_before = crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().len());
    let dropped_before = RECEIPT_DROPPED_UNRETIRED.aggregate();
    let reclaimed_before = TEARDOWN_RECLAIM.aggregate();

    let receipt = crate::process::RetirementReceipt::from_reclaim(boot_test_reclaim(pid));
    core::mem::drop(receipt);

    let queue_after = crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().len());
    if queue_after != queue_before + 1
        || RECEIPT_DROPPED_UNRETIRED
            .aggregate()
            .saturating_sub(dropped_before)
            != 1
        || TEARDOWN_RECLAIM
            .aggregate()
            .saturating_sub(reclaimed_before)
            != 0
        || !boot_reclaim_locations(pid).0
    {
        return TestResult::Fail("dropped retirement receipt did not re-enqueue without reclaim");
    }

    TestResult::Pass
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_push_live(pid: u64) {
    crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().push(boot_test_reclaim(pid)));
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_push_parked(pid: u64, record: ParkRecord) {
    let mut reclaim = boot_test_reclaim(pid);
    reclaim.proof_failures = PROOF_FAILURES_BEFORE_PARK;
    reclaim.parked = Some(record);
    crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARKED);
    crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARK_RESIDENT);
    crate::arch_without_interrupts(|| PARKED_PROCESS_RECLAIMS.lock().push(reclaim));
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_reclaim_locations(pid: u64) -> (bool, bool) {
    let live = crate::arch_without_interrupts(|| {
        PENDING_PROCESS_RECLAIMS
            .lock()
            .iter()
            .any(|reclaim| reclaim.pid == pid)
    });
    let parked = crate::arch_without_interrupts(|| {
        PARKED_PROCESS_RECLAIMS
            .lock()
            .iter()
            .any(|reclaim| reclaim.pid == pid)
    });
    (live, parked)
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_force_blocker(pid: u64, blocker: Option<RootBlocker>) {
    BOOT_RECLAIM_FORCED_PID.store(pid, Ordering::Release);
    BOOT_RECLAIM_FORCED_BLOCKER.store(
        blocker.map_or(0, |blocker| blocker as u64 + 1),
        Ordering::Release,
    );
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_last_park_snapshot() -> (scheduler::RetirementSnapshot, u64) {
    let mut epochs = [0; crate::arch_impl::aarch64::constants::MAX_CPUS];
    for (epoch, slot) in epochs.iter_mut().zip(&BOOT_RECLAIM_LAST_PARK_EPOCHS) {
        *epoch = slot.load(Ordering::Relaxed);
    }
    (
        scheduler::RetirementSnapshot {
            epochs,
            online_mask: BOOT_RECLAIM_LAST_PARK_MASK.load(Ordering::Relaxed),
        },
        BOOT_RECLAIM_LAST_PARK_ROW_EPOCH.load(Ordering::Relaxed),
    )
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_synthetic_park(mask: u64, first_epoch: u64) -> (ParkRecord, scheduler::RetirementSnapshot) {
    let mut epochs = [0; crate::arch_impl::aarch64::constants::MAX_CPUS];
    for (cpu_id, epoch) in epochs.iter_mut().enumerate() {
        if mask & (1 << cpu_id) != 0 {
            *epoch = first_epoch;
        }
    }
    let snapshot = scheduler::RetirementSnapshot {
        epochs,
        online_mask: mask,
    };
    let fence_at_park = snapshot.as_fence();
    (
        ParkRecord {
            fence_at_park,
            row_epoch_at_park: ROW_REMOVAL_EPOCH.load(Ordering::Relaxed),
            age_epoch_sum_at_park: snapshot.epoch_sum(mask),
        },
        snapshot,
    )
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn retirement_fence_gate_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;

    let empty_before = crate::tracing::providers::teardown::RETIRE_EMPTY_ONLINE_MASK.aggregate();
    let empty = scheduler::RetirementFence {
        epochs: [0; crate::arch_impl::aarch64::constants::MAX_CPUS],
        online_mask: 0,
    };
    let zero = scheduler::RetirementSnapshot {
        epochs: [0; crate::arch_impl::aarch64::constants::MAX_CPUS],
        online_mask: 0,
    };
    if zero.fence_elapsed(&empty)
        || crate::tracing::providers::teardown::RETIRE_EMPTY_ONLINE_MASK.aggregate() <= empty_before
    {
        return TestResult::Fail("empty retirement mask elapsed or was not counted");
    }

    let mut target_epochs = [0; crate::arch_impl::aarch64::constants::MAX_CPUS];
    target_epochs[0] = u64::MAX;
    let wrapped_target = scheduler::RetirementFence {
        epochs: target_epochs,
        online_mask: 1,
    };
    let mut now_epochs = target_epochs;
    now_epochs[0] = 1;
    let wrapped_now = scheduler::RetirementSnapshot {
        epochs: now_epochs,
        online_mask: 1,
    };
    if !wrapped_now.fence_elapsed(&wrapped_target) {
        return TestResult::Fail("wrap-safe retirement comparison rejected elapsed fence");
    }
    target_epochs[0] = 1;
    now_epochs[0] = u64::MAX;
    if (scheduler::RetirementSnapshot {
        epochs: now_epochs,
        online_mask: 1,
    })
    .fence_elapsed(&scheduler::RetirementFence {
        epochs: target_epochs,
        online_mask: 1,
    }) {
        return TestResult::Fail("wrap-safe retirement comparison accepted future fence");
    }
    TestResult::Pass
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn reclaim_progress_gate_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;
    use crate::tracing::providers::teardown as trace;

    let _guard = match BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(message) => return TestResult::Fail(message),
    };
    let proof_lock_before = trace::PROOF_UNDER_QUEUE_LOCK.aggregate();
    let resident_before = trace::RECLAIM_PARK_RESIDENT.aggregate();
    let parked_before = trace::RECLAIM_PARKED.aggregate();
    let skipped_before = trace::RECLAIM_PASS_SKIPPED.aggregate();
    let immediate_before = trace::RECLAIM_PARK_IMMEDIATE_UNPARK.aggregate();
    let reclaim_before = trace::TEARDOWN_RECLAIM.aggregate();

    let blocked_pid = BOOT_RECLAIM_PID_BASE;
    let ready_pid = BOOT_RECLAIM_PID_BASE + 1;
    boot_force_blocker(blocked_pid, Some(RootBlocker::LiveRow));
    boot_push_live(blocked_pid);
    boot_push_live(ready_pid);
    boot_reclaim_deferred_process_resources();
    let selections = BOOT_RECLAIM_PASS_SELECTIONS.load(Ordering::Relaxed);
    let pass_start = BOOT_RECLAIM_PASS_START.load(Ordering::Relaxed);
    if selections != 2 || selections > pass_start || pass_start != 2 {
        return TestResult::Fail("bounded reclaim pass selected a candidate twice");
    }
    if boot_reclaim_locations(blocked_pid) != (true, false)
        || boot_reclaim_locations(ready_pid) != (false, false)
        || trace::TEARDOWN_RECLAIM
            .aggregate()
            .saturating_sub(reclaim_before)
            != 1
        || trace::RECLAIM_PASS_SKIPPED.aggregate() <= skipped_before
    {
        return TestResult::Fail("live-row refusal spun or starved a ready receipt");
    }
    boot_reclaim_deferred_process_resources();
    BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO.store(1, Ordering::Release);
    boot_reclaim_deferred_process_resources();
    if trace::RECLAIM_PARKED
        .aggregate()
        .saturating_sub(parked_before)
        != 1
        || boot_reclaim_locations(blocked_pid) != (false, true)
        || trace::RECLAIM_PARK_RESIDENT.aggregate() != resident_before.wrapping_add(1)
    {
        return TestResult::Fail("persistent live-row refusal did not park exactly at K=3");
    }

    let unpark_before = trace::RECLAIM_UNPARKED_EPOCH
        .aggregate()
        .wrapping_add(trace::RECLAIM_UNPARKED_ROW.aggregate())
        .wrapping_add(trace::RECLAIM_UNPARKED_AGE.aggregate());
    let (park_snapshot, park_row_epoch) = boot_last_park_snapshot();
    unpark_sweep_with_snapshot(park_snapshot, park_row_epoch);
    let unpark_after = trace::RECLAIM_UNPARKED_EPOCH
        .aggregate()
        .wrapping_add(trace::RECLAIM_UNPARKED_ROW.aggregate())
        .wrapping_add(trace::RECLAIM_UNPARKED_AGE.aggregate());
    if boot_reclaim_locations(blocked_pid) != (false, true) {
        return TestResult::Fail("fresh park entry moved without an unpark trigger");
    }
    if unpark_after != unpark_before {
        return TestResult::Fail("fresh park snapshot selected an unpark arm");
    }
    if trace::RECLAIM_PARK_IMMEDIATE_UNPARK.aggregate() != immediate_before {
        return TestResult::Fail("fresh park was immediately eligible when recorded");
    }
    boot_force_blocker(blocked_pid, None);
    let row_marker = {
        let mut manager = crate::process::manager();
        let Some(manager) = manager.as_mut() else {
            return TestResult::Fail("process manager unavailable for row-arm gate");
        };
        let pid = manager.allocate_pid();
        let process = crate::process::Process::new(
            pid,
            alloc::string::String::from("p1_row_epoch_gate"),
            crate::memory::arch_stub::VirtAddr::new(0x400000),
        );
        manager.insert_process(pid, process);
        manager.remove_process(pid);
        ROW_REMOVAL_EPOCH.load(Ordering::Relaxed)
    };
    let row_before = trace::RECLAIM_UNPARKED_ROW.aggregate();
    unpark_sweep_with_snapshot(park_snapshot, row_marker);
    if trace::RECLAIM_UNPARKED_ROW
        .aggregate()
        .saturating_sub(row_before)
        != 1
        || boot_reclaim_locations(blocked_pid) != (true, false)
    {
        return TestResult::Fail("row-removal unpark arm did not fire");
    }
    boot_reclaim_deferred_process_resources();

    let cached_pid = BOOT_RECLAIM_PID_BASE + 2;
    boot_force_blocker(cached_pid, Some(RootBlocker::Cached));
    boot_push_live(cached_pid);
    for _ in 0..PROOF_FAILURES_BEFORE_PARK {
        boot_reclaim_deferred_process_resources();
    }
    if boot_reclaim_locations(cached_pid) != (false, true)
        || trace::RECLAIM_PARK_RESIDENT.aggregate() != resident_before.wrapping_add(1)
    {
        return TestResult::Fail("cached-root refusal did not become epoch-parked");
    }
    boot_force_blocker(cached_pid, None);
    let (cached_snapshot, cached_row_epoch) = boot_last_park_snapshot();
    let mut epoch_advanced = cached_snapshot;
    for (cpu_id, epoch) in epoch_advanced.epochs.iter_mut().enumerate() {
        if epoch_advanced.online_mask & (1 << cpu_id) != 0 {
            *epoch = epoch.wrapping_add(1);
        }
    }
    let epoch_before = trace::RECLAIM_UNPARKED_EPOCH.aggregate();
    unpark_sweep_with_snapshot(epoch_advanced, cached_row_epoch);
    if trace::RECLAIM_UNPARKED_EPOCH
        .aggregate()
        .saturating_sub(epoch_before)
        != 1
        || boot_reclaim_locations(cached_pid) != (true, false)
    {
        return TestResult::Fail("epoch unpark arm did not return the cached-root receipt");
    }
    boot_reclaim_deferred_process_resources();

    let age_pid = BOOT_RECLAIM_PID_BASE + 3;
    let age_mask = (1 << 6) | (1 << 7);
    let (age_record, age_snapshot) = boot_synthetic_park(age_mask, 200);
    boot_push_parked(age_pid, age_record);
    if trace::RECLAIM_PARK_RESIDENT.aggregate() != resident_before.wrapping_add(1) {
        return TestResult::Fail("age-arm receipt was not observably resident");
    }
    let mut age_63 = age_snapshot;
    age_63.epochs[6] = age_63.epochs[6].wrapping_add(63);
    unpark_sweep_with_snapshot(age_63, age_record.row_epoch_at_park);
    if boot_reclaim_locations(age_pid) != (false, true) {
        return TestResult::Fail("age unpark arm fired before 64 scheduling epochs");
    }
    boot_force_blocker(age_pid, Some(RootBlocker::LiveRow));
    let mut age_64 = age_63;
    age_64.epochs[6] = age_64.epochs[6].wrapping_add(1);
    let age_before = trace::RECLAIM_UNPARKED_AGE.aggregate();
    unpark_sweep_with_snapshot(age_64, age_record.row_epoch_at_park);
    boot_reclaim_deferred_process_resources();
    if trace::RECLAIM_UNPARKED_AGE
        .aggregate()
        .saturating_sub(age_before)
        != 1
        || boot_reclaim_locations(age_pid) != (true, false)
    {
        return TestResult::Fail("age arm did not re-prove the receipt at epoch sum 64");
    }
    boot_force_blocker(age_pid, None);
    boot_reclaim_deferred_process_resources();

    let blocker_cases = [
        (RootBlocker::Hardware, &trace::ROOT_PROOF_BLOCKED_HW),
        (RootBlocker::Shadow, &trace::ROOT_PROOF_BLOCKED_SHADOW),
        (RootBlocker::Cached, &trace::ROOT_PROOF_BLOCKED_CACHED),
        (RootBlocker::LiveRow, &trace::ROOT_PROOF_BLOCKED_LIVE_ROW),
    ];
    for (offset, (blocker, counter)) in blocker_cases.into_iter().enumerate() {
        let pid = BOOT_RECLAIM_PID_BASE + 10 + offset as u64;
        let before = counter.aggregate();
        boot_force_blocker(pid, Some(blocker));
        boot_push_live(pid);
        boot_reclaim_deferred_process_resources();
        if counter.aggregate().saturating_sub(before) != 1
            || boot_reclaim_locations(pid) != (true, false)
        {
            return TestResult::Fail("forced RootProof refusal lost its detached receipt");
        }
        boot_force_blocker(pid, None);
        boot_reclaim_deferred_process_resources();
        if boot_reclaim_locations(pid) != (false, false) {
            return TestResult::Fail("reinserted RootProof receipt did not eventually retire");
        }
    }

    let full_before = trace::TEARDOWN_RECLAIM.aggregate();
    for offset in 0..8 {
        boot_push_live(BOOT_RECLAIM_PID_BASE + 32 + offset);
    }
    boot_reclaim_deferred_process_resources();
    if BOOT_RECLAIM_PASS_START.load(Ordering::Relaxed) != 8
        || BOOT_RECLAIM_PASS_SELECTIONS.load(Ordering::Relaxed) != 8
        || trace::TEARDOWN_RECLAIM
            .aggregate()
            .saturating_sub(full_before)
            != 8
    {
        return TestResult::Fail("pass cursor acted as a reclaim cap");
    }
    if trace::RECLAIM_PARK_RESIDENT.aggregate() != resident_before
        || trace::PROOF_UNDER_QUEUE_LOCK.aggregate() != proof_lock_before
    {
        return TestResult::Fail("P1 reclaim gate left residents or nested proof locks");
    }
    TestResult::Pass
}

#[cfg(feature = "boot_tests")]
pub fn deferred_fault_ring_overflow_injection() -> bool {
    // Exercise the production push/drain implementation without touching any
    // per-CPU ring that live fault handling can concurrently use.
    let buffer = DeferredFaultExitBuffer::new();
    let mut drained = alloc::vec::Vec::new();
    let mut accepted = 0usize;
    for tid in 1..=17 {
        if buffer.push(u64::MAX - tid) {
            accepted += 1;
        }
    }

    drained.clear();
    buffer.drain(&mut drained);
    let quiescent_count = drained.len();
    drained.clear();
    buffer.drain(&mut drained);

    accepted == DEFERRED_FAULT_EXIT_SLOTS
        && quiescent_count == DEFERRED_FAULT_EXIT_SLOTS
        && drained.is_empty()
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
