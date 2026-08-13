//! Process-Task Integration
//!
//! This module bridges the gap between the Process Manager and the Task Scheduler,
//! allowing processes to be scheduled as tasks.

use crate::ipc::fd::FileDescriptor;
use crate::memory::process_memory::AbandonReason;
use crate::process::ProcessId;
use crate::task::scheduler;
use crate::task::thread::{Thread, ThreadPrivilege};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

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

#[derive(Clone, Copy)]
struct ParkRecord {
    fence_at_park: scheduler::RetirementFence,
    row_epoch_at_park: u64,
    age_epoch_sum_at_park: u64,
}

#[derive(Clone, Copy, Default)]
struct RootProof {
    blocked_epoch: bool,
    blocked_hw: bool,
    blocked_shadow: bool,
    blocked_cached: bool,
    blocked_live_row: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RootBlocker {
    Epoch,
    Hardware,
    Shadow,
    Cached,
    LiveRow,
}

#[derive(Clone, Copy)]
enum UnparkReason {
    Epoch,
    Row,
    Age,
}

#[inline(always)]
fn roots_match(left: u64, right: u64) -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch_impl::aarch64::ttbr0::roots_match(left, right)
    }
    #[cfg(target_arch = "x86_64")]
    {
        left != 0 && right != 0 && (left & !0xfff) == (right & !0xfff)
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn local_hardware_root() -> u64 {
    crate::arch_impl::aarch64::ttbr0::local_ttbr0_root()
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn local_hardware_root() -> u64 {
    x86_64::registers::control::Cr3::read()
        .0
        .start_address()
        .as_u64()
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn shadow_root_is_live(reclaim: &PendingProcessReclaim, online_mask: u64) -> bool {
    reclaim
        .page_table
        .iter()
        .chain(reclaim.old_page_tables.iter())
        .any(|page_table| {
            crate::arch_impl::aarch64::ttbr0::is_ttbr0_root_live_in_mask(
                page_table.level_4_frame().start_address().as_u64(),
                online_mask,
            )
        })
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn shadow_root_is_live(reclaim: &PendingProcessReclaim, online_mask: u64) -> bool {
    online_mask & 1 != 0
        && (reclaim.any_root_matches(crate::per_cpu::get_next_cr3())
            || reclaim.any_root_matches(crate::per_cpu::get_saved_process_cr3()))
}

/// Retire the per-CPU CR3 shadow that would otherwise name a deferred root
/// forever. `saved_process_cr3` is stamped on every userspace entry and is only
/// consumed by a return to userspace that did not context-switch — a return the
/// exiting thread never takes — so nothing else clears it. Left set, the Shadow
/// proof leg blocks this receipt on every pass with no park and no progress.
#[cfg(target_arch = "x86_64")]
fn clear_shadow_root(root: u64) {
    if roots_match(crate::per_cpu::get_saved_process_cr3(), root) {
        crate::per_cpu::set_saved_process_cr3(0);
        crate::trace_count!(crate::tracing::providers::teardown::PT_SHADOW_ROOT_CLEARED);
    }
}

impl PendingProcessReclaim {
    fn any_root_matches(&self, candidate: u64) -> bool {
        self.page_table
            .iter()
            .chain(self.old_page_tables.iter())
            .any(|page_table| {
                roots_match(
                    candidate,
                    page_table.level_4_frame().start_address().as_u64(),
                )
            })
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
        if self.any_root_matches(local_hardware_root())
            || boot_forces_blocker(self.pid, RootBlocker::Hardware, allow_boot_injection)
        {
            return RootProof::blocked(RootBlocker::Hardware);
        }
        if shadow_root_is_live(self, self.after_epoch.online_mask)
            || boot_forces_blocker(self.pid, RootBlocker::Shadow, allow_boot_injection)
        {
            return RootProof::blocked(RootBlocker::Shadow);
        }
        RootProof::default()
    }

    fn cached_root_is_live(&self) -> bool {
        #[cfg(target_arch = "aarch64")]
        {
            scheduler::with_scheduler(|scheduler| {
                scheduler.any_cached_ttbr0_matches(|cached| self.any_root_matches(cached))
            })
            .unwrap_or(false)
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            false
        }
    }

    fn live_row_names_root(&self) -> bool {
        crate::process::manager().as_ref().is_some_and(|manager| {
            manager.any_live_root_matches(|row_root| self.any_root_matches(row_root))
        })
    }

    fn reclaim_bounded(&mut self) -> crate::memory::process_memory::RetireProgress {
        use crate::memory::process_memory::{RetireProgress, RETIRE_FRAME_BUDGET};

        let mut budget = RETIRE_FRAME_BUDGET;
        #[cfg(target_arch = "aarch64")]
        while budget > 0 {
            let Some(old_page_table) = self.old_page_tables.last_mut() else {
                break;
            };
            if old_page_table.cleanup_for_exec(self.pid, &mut budget) != RetireProgress::Complete {
                return RetireProgress::Budgeted;
            }
            self.old_page_tables.pop();
        }
        #[cfg(target_arch = "x86_64")]
        if !drain_old_page_tables_counted(self.pid, &mut self.old_page_tables, &mut budget) {
            return RetireProgress::Budgeted;
        }
        if !self.old_page_tables.is_empty() {
            return RetireProgress::Budgeted;
        }

        let Some(page_table) = self.page_table.as_mut() else {
            return RetireProgress::Complete;
        };
        page_table.release_mapped_leaves();
        let progress = page_table.retire_bounded(self.pid, &mut budget);
        if progress == RetireProgress::Complete {
            self.page_table = None;
        }
        progress
    }
}

static PENDING_PROCESS_RECLAIMS: spin::Mutex<alloc::vec::Vec<PendingProcessReclaim>> =
    spin::Mutex::new(alloc::vec::Vec::new());
static PARKED_PROCESS_RECLAIMS: spin::Mutex<alloc::vec::Vec<PendingProcessReclaim>> =
    spin::Mutex::new(alloc::vec::Vec::new());
static RECLAIM_PASS_ID: AtomicU32 = AtomicU32::new(0);
static ROW_REMOVAL_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Set while a production drain owns the deferred-reclaim queues.
///
/// A kernel fault that abandons a drain mid-pass IRETs into `idle_loop`, whose
/// first statement drains again. Without this claim that entry would spin
/// forever on a queue mutex whose owner no longer exists. An abandoned drain
/// leaves the flag set on purpose: every later drain then refuses, so the
/// failure mode is a bounded leak instead of a hard hang.
static RECLAIM_DRAIN_ACTIVE: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_TEST_OWNER: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_FORCED_PID: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_FORCED_BLOCKER: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_PASS_START: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_PASS_SELECTIONS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_LAST_PARK_EPOCHS: [AtomicU64; scheduler::MAX_CPUS] =
    [const { AtomicU64::new(0) }; scheduler::MAX_CPUS];
#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_LAST_PARK_MASK: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_LAST_PARK_ROW_EPOCH: AtomicU64 = AtomicU64::new(0);

const PROOF_FAILURES_BEFORE_PARK: u8 = 3;
const PARK_AGE_BACKSTOP_EPOCHS: u64 = 64;

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
    ROW_REMOVAL_EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// Consume x86 exec-superseded roots at exit, counting the walk when it runs
/// under the process-manager lock. This is the x86 producer for
/// TEARDOWN_MASKED_FRAMES_WALKED: a deferred exit that walks nothing keeps the
/// counter flat, which is exactly what the leaf-timing oracle asserts.
#[cfg(target_arch = "x86_64")]
fn drain_old_page_tables_counted(
    pid: u64,
    old_page_tables: &mut alloc::vec::Vec<
        alloc::boxed::Box<crate::memory::process_memory::ProcessPageTable>,
    >,
    budget: &mut u32,
) -> bool {
    if old_page_tables.is_empty() {
        return true;
    }
    if crate::process::process_manager_held_on_current_cpu() {
        crate::tracing::providers::teardown::record_masked_frames_walked(pid);
    }
    while *budget > 0 {
        let Some(old_page_table) = old_page_tables.pop() else {
            return true;
        };
        *budget -= 1;
        old_page_table.cleanup_for_exec();
    }
    old_page_tables.is_empty()
}

#[cfg(any(target_arch = "aarch64", feature = "boot_tests"))]
pub(crate) fn release_process_resources(process: &mut crate::process::Process) {
    #[cfg(target_arch = "aarch64")]
    if crate::process::process_manager_held_on_current_cpu() {
        crate::tracing::providers::teardown::record_masked_frames_walked(process.id.as_u64());
    }
    #[cfg(target_arch = "aarch64")]
    process.cleanup_cow_frames();
    #[cfg(target_arch = "aarch64")]
    process.drain_old_page_tables();
    #[cfg(target_arch = "x86_64")]
    {
        let mut budget = u32::MAX;
        let pid = process.id.as_u64();
        let _ = drain_old_page_tables_counted(
            pid,
            &mut process.pending_old_page_tables,
            &mut budget,
        );
    }
    #[cfg(target_arch = "aarch64")]
    if let Some(page_table) = process.page_table.take() {
        page_table.abandon(AbandonReason::NoProofPipeline);
    }
    #[cfg(target_arch = "x86_64")]
    debug_assert!(process.page_table.is_none());
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
    #[cfg(feature = "boot_tests")]
    let root_is_live = root_is_live
        || FORCE_LIVE_RECLAIM_TEST_PID.load(Ordering::Acquire) == process.id.as_u64();
    if !root_is_live {
        return None;
    }

    Some(defer_process_resources(process))
}

#[cfg(feature = "boot_tests")]
static FORCE_LIVE_RECLAIM_TEST_PID: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "boot_tests")]
pub(crate) struct ForceLiveReclaimTestGuard;

#[cfg(feature = "boot_tests")]
impl ForceLiveReclaimTestGuard {
    pub(crate) fn arm(pid: u64) -> Self {
        #[cfg(target_arch = "aarch64")]
        FORCE_LIVE_RECLAIM_TEST_PID.store(pid, Ordering::Release);
        #[cfg(target_arch = "x86_64")]
        {
            let _ = pid;
            let _ = FORCE_LIVE_RECLAIM_TEST_PID.load(Ordering::Relaxed);
        }
        Self
    }
}

#[cfg(feature = "boot_tests")]
impl Drop for ForceLiveReclaimTestGuard {
    fn drop(&mut self) {
        #[cfg(target_arch = "aarch64")]
        FORCE_LIVE_RECLAIM_TEST_PID.store(0, Ordering::Release);
    }
}

pub(crate) fn defer_process_resources(
    process: &mut crate::process::Process,
) -> PendingProcessReclaim {
    // Carry superseded exec roots into the same proof-gated receipt as the
    // current root. The owning drain consumes them after PM is out of scope.
    crate::tracing::providers::teardown::record_defer(process.id.as_u64());
    let page_table = process.page_table.take();
    #[cfg(target_arch = "x86_64")]
    if let Some(page_table) = page_table.as_ref() {
        clear_shadow_root(page_table.level_4_frame().start_address().as_u64());
    }
    PendingProcessReclaim {
        pid: process.id.as_u64(),
        page_table,
        old_page_tables: core::mem::take(&mut process.pending_old_page_tables),
        after_epoch: scheduler::retirement_grace_target(),
        last_pass: 0,
        proof_failures: 0,
        parked: None,
    }
}

fn abandon_unqueued_reclaim(mut reclaim: PendingProcessReclaim) {
    #[cfg(target_arch = "aarch64")]
    let reason = AbandonReason::NoProofPipeline;
    #[cfg(target_arch = "x86_64")]
    let reason = AbandonReason::NoArchPipeline;

    // A leak is the acceptable OOM residual; freeing before proof could
    // over-free a root or leaf that is still live in hardware.
    if let Some(page_table) = reclaim.page_table.take() {
        page_table.abandon(reason);
    }
    for old_page_table in reclaim.old_page_tables.drain(..) {
        old_page_table.abandon(reason);
    }
}

#[cfg(feature = "boot_tests")]
static BOOT_RECLAIM_FORCE_RESERVE_FAILURE: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "boot_tests")]
#[inline]
fn boot_forces_reclaim_reserve_failure() -> bool {
    BOOT_RECLAIM_FORCE_RESERVE_FAILURE.load(Ordering::Acquire) != 0
}

fn push_pending_or_abandon(reclaim: PendingProcessReclaim) {
    let mut reclaim = Some(reclaim);
    let queued = crate::arch_without_interrupts(|| {
        let Some(mut pending) = PENDING_PROCESS_RECLAIMS.try_lock() else {
            return false;
        };
        #[cfg(feature = "boot_tests")]
        let reservation = if boot_forces_reclaim_reserve_failure() {
            pending.try_reserve(usize::MAX)
        } else {
            pending.try_reserve(1)
        };
        #[cfg(not(feature = "boot_tests"))]
        let reservation = pending.try_reserve(1);
        if reservation.is_err() {
            false
        } else {
            pending.push(reclaim.take().expect("reclaim queued once"));
            true
        }
    });
    if !queued {
        abandon_unqueued_reclaim(reclaim.take().expect("unqueued reclaim retained"));
    }
}

pub(crate) fn enqueue_process_reclaim(reclaim: PendingProcessReclaim) {
    if crate::process::process_manager_held_on_current_cpu() {
        crate::trace_count!(
            crate::tracing::providers::teardown::RECLAIM_ENQUEUE_UNDER_PM
        );
    }
    push_pending_or_abandon(reclaim);
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
                            #[cfg(target_arch = "x86_64")]
                            let receipt = {
                                let reclaim = defer_process_resources(process);
                                drop(process.stack.take());
                                Some(crate::process::RetirementReceipt::from_reclaim(reclaim))
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
                        already_terminated,
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
            already_terminated,
        )) = phase1_result
        {
            if let Some(mut receipt) = retirement_receipt {
                if let Some(reclaim) = receipt.take_contents() {
                    enqueue_process_reclaim(reclaim);
                }
            }

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

            if !already_terminated {
                crate::task::exit_tally::record_exit(&process_name, exit_code);
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

fn park_reclaim(mut reclaim: PendingProcessReclaim) {
    let snapshot_at_park = scheduler::RetirementSnapshot::capture();
    let fence_at_park = snapshot_at_park.as_fence();
    let row_epoch_at_park = ROW_REMOVAL_EPOCH.load(Ordering::Relaxed);
    #[cfg(feature = "boot_tests")]
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
    let immediate_unpark = park_record
        .unpark_reason(&snapshot_at_park, row_epoch_at_park)
        .is_some();
    reclaim.parked = Some(park_record);
    let mut reclaim = Some(reclaim);
    let parked = crate::arch_without_interrupts(|| {
        let Some(mut parked) = PARKED_PROCESS_RECLAIMS.try_lock() else {
            return false;
        };
        if parked.try_reserve(1).is_err() {
            false
        } else {
            parked.push(reclaim.take().expect("reclaim parked once"));
            true
        }
    });
    if !parked {
        let mut reclaim = reclaim.take().expect("unparked reclaim retained");
        reclaim.parked = None;
        push_pending_or_abandon(reclaim);
        return;
    }
    if immediate_unpark {
        crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARK_IMMEDIATE_UNPARK);
    }
    crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARKED);
    crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARK_RESIDENT);
}

fn unpark_sweep_with_snapshot(snapshot: scheduler::RetirementSnapshot, row_epoch: u64) {
    let mut ready = alloc::vec::Vec::new();
    let swept = crate::arch_without_interrupts(|| {
        let Some(mut parked) = PARKED_PROCESS_RECLAIMS.try_lock() else {
            return false;
        };
        let mut index = 0;
        while index < parked.len() {
            let reason = parked[index]
                .parked
                .as_ref()
                .and_then(|record| record.unpark_reason(&snapshot, row_epoch));
            if let Some(reason) = reason {
                if ready.try_reserve(1).is_err() {
                    index += 1;
                    continue;
                }
                let mut reclaim = parked.swap_remove(index);
                reclaim.parked = None;
                reclaim.proof_failures = 0;
                record_unpark(reason);
                ready.push(reclaim);
            } else {
                index += 1;
            }
        }
        true
    });
    if !swept {
        return;
    }
    if !ready.is_empty() {
        let mut ready = Some(ready);
        let queued = crate::arch_without_interrupts(|| {
            let Some(mut pending) = PENDING_PROCESS_RECLAIMS.try_lock() else {
                return false;
            };
            let ready_len = ready.as_ref().expect("ready reclaims retained").len();
            if pending.try_reserve(ready_len).is_err() {
                false
            } else {
                pending.extend(ready.take().expect("ready reclaims queued once"));
                true
            }
        });
        if !queued {
            for reclaim in ready.take().expect("unqueued ready reclaims retained") {
                abandon_unqueued_reclaim(reclaim);
            }
        }
    }
}

fn unpark_sweep() {
    let snapshot = scheduler::RetirementSnapshot::capture();
    let row_epoch = ROW_REMOVAL_EPOCH.load(Ordering::Relaxed);
    unpark_sweep_with_snapshot(snapshot, row_epoch);
}

fn boot_forces_blocker(pid: u64, blocker: RootBlocker, allow_boot_injection: bool) -> bool {
    #[cfg(feature = "boot_tests")]
    {
        allow_boot_injection
            && BOOT_RECLAIM_FORCED_PID.load(Ordering::Acquire) == pid
            && BOOT_RECLAIM_FORCED_BLOCKER.load(Ordering::Acquire) == blocker as u64 + 1
    }
    #[cfg(not(feature = "boot_tests"))]
    {
        let _ = (pid, blocker, allow_boot_injection);
        false
    }
}

fn boot_after_step_two(fence: &scheduler::RetirementFence) {
    #[cfg(feature = "boot_tests")]
    if BOOT_RECLAIM_ADVANCE_AFTER_STEP_TWO.swap(0, Ordering::AcqRel) != 0 {
        for cpu_id in 0..scheduler::MAX_CPUS {
            if fence.online_mask & (1 << cpu_id) != 0 {
                scheduler::note_scheduling_epoch(cpu_id);
            }
        }
    }
    #[cfg(not(feature = "boot_tests"))]
    let _ = fence;
}

fn boot_begin_reclaim_pass(boot_test_owned: bool) {
    #[cfg(feature = "boot_tests")]
    if boot_test_owned {
        let queue_len = crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().len());
        BOOT_RECLAIM_PASS_START.store(queue_len as u64, Ordering::Relaxed);
        BOOT_RECLAIM_PASS_SELECTIONS.store(0, Ordering::Relaxed);
    }
    #[cfg(not(feature = "boot_tests"))]
    let _ = boot_test_owned;
}

fn boot_note_reclaim_selection(boot_test_owned: bool) {
    #[cfg(feature = "boot_tests")]
    if boot_test_owned {
        BOOT_RECLAIM_PASS_SELECTIONS.fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(not(feature = "boot_tests"))]
    let _ = boot_test_owned;
}

fn boot_finish_reclaim_pass(boot_test_owned: bool) {
    #[cfg(feature = "boot_tests")]
    if boot_test_owned {
        debug_assert!(
            BOOT_RECLAIM_PASS_SELECTIONS.load(Ordering::Relaxed)
                <= BOOT_RECLAIM_PASS_START.load(Ordering::Relaxed)
        );
    }
    #[cfg(not(feature = "boot_tests"))]
    let _ = boot_test_owned;
}

/// Reclaim process frames whose cross-CPU TTBR0 retention has quiesced.
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
    #[cfg(feature = "boot_tests")]
    if BOOT_RECLAIM_TEST_OWNER.load(Ordering::Acquire) != 0 {
        return;
    }
    if RECLAIM_DRAIN_ACTIVE
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        // Refusing benign nesting is correct: the receipt is already queued; the owning drain will take it.
        crate::trace_count!(
            crate::tracing::providers::teardown::RECLAIM_DRAIN_NESTED_REFUSED
        );
        return;
    }

    reclaim_deferred_process_resources_for_pass(my_pass, false);
    RECLAIM_DRAIN_ACTIVE.store(false, Ordering::Release);
}

fn reclaim_deferred_process_resources_for_pass(my_pass: u32, boot_test_owned: bool) {
    #[cfg(not(feature = "boot_tests"))]
    let _ = boot_test_owned;
    #[cfg(feature = "boot_tests")]
    if !boot_test_owned && BOOT_RECLAIM_TEST_OWNER.load(Ordering::Acquire) != 0 {
        return;
    }
    unpark_sweep();
    boot_begin_reclaim_pass(boot_test_owned);

    loop {
        #[cfg(feature = "boot_tests")]
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
                        push_pending_or_abandon(reclaim);
                    }
                } else if reclaim.reclaim_bounded()
                    == crate::memory::process_memory::RetireProgress::Complete
                {
                    crate::tracing::providers::teardown::record_reclaim(reclaim.pid);
                } else {
                    crate::trace_count!(
                        crate::tracing::providers::teardown::PT_RETIRE_BUDGET_REQUEUED
                    );
                    push_pending_or_abandon(reclaim);
                }
            }
            None => break,
        }
    }
    boot_finish_reclaim_pass(boot_test_owned);
}

#[cfg(feature = "boot_tests")]
pub(crate) fn boot_reclaim_deferred_process_resources() {
    let my_pass = next_reclaim_pass_id(
        RECLAIM_PASS_ID
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1),
    );
    reclaim_deferred_process_resources_for_pass(my_pass, true);
}

#[cfg(feature = "boot_tests")]
const BOOT_RECLAIM_PID_BASE: u64 = u64::MAX - 0x1000;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn current_cpu_id() -> u64 {
    crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id()
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
fn current_cpu_id() -> u64 {
    use crate::arch_impl::current::X86PerCpu;
    use crate::arch_impl::PerCpuOps;

    X86PerCpu::cpu_id()
}

#[cfg(feature = "boot_tests")]
pub(crate) struct BootReclaimTestGuard;

#[cfg(feature = "boot_tests")]
impl BootReclaimTestGuard {
    pub(crate) fn enter() -> Result<Self, &'static str> {
        let owner = current_cpu_id().wrapping_add(1);
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

#[cfg(feature = "boot_tests")]
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

#[cfg(feature = "boot_tests")]
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

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
struct BootReclaimReserveFailureGuard;

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
impl BootReclaimReserveFailureGuard {
    fn arm() -> Self {
        BOOT_RECLAIM_FORCE_RESERVE_FAILURE.store(1, Ordering::Release);
        Self
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
impl Drop for BootReclaimReserveFailureGuard {
    fn drop(&mut self) {
        BOOT_RECLAIM_FORCE_RESERVE_FAILURE.store(0, Ordering::Release);
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
fn boot_page_table_reclaim(
    pid: u64,
) -> Result<(PendingProcessReclaim, usize, u64), &'static str> {
    let page_table = alloc::boxed::Box::new(
        crate::memory::process_memory::ProcessPageTable::new()
            .map_err(|_| "proof root allocation failed")?,
    );
    let recorded = page_table.recorded_table_frames_for_gate();
    let root = page_table.level_4_frame().start_address().as_u64();
    let mut reclaim = boot_test_reclaim(pid);
    reclaim.page_table = Some(page_table);
    Ok((reclaim, recorded, root))
}

#[cfg(feature = "boot_tests")]
fn boot_oversized_page_table(
) -> Result<
    (
        alloc::boxed::Box<crate::memory::process_memory::ProcessPageTable>,
        usize,
    ),
    &'static str,
> {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::{Page, PageTableFlags, Size4KiB, VirtAddr};
    use crate::memory::frame_allocator::{allocate_frame, deallocate_frame};
    use crate::memory::process_memory::RETIRE_FRAME_BUDGET;
    #[cfg(target_arch = "x86_64")]
    use x86_64::{
        structures::paging::{Page, PageTableFlags, Size4KiB},
        VirtAddr,
    };

    let mut page_table = alloc::boxed::Box::new(
        crate::memory::process_memory::ProcessPageTable::new()
            .map_err(|_| "oversized root allocation failed")?,
    );
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let subtree_count = RETIRE_FRAME_BUDGET as usize / 3 + 1;
    let sentinels = page_table
        .gate_sentinels(subtree_count)
        .ok_or("oversized fixture found too few unshared root slots")?;
    let mut expected_recorded = page_table.recorded_table_frames_for_gate();
    for sentinel in sentinels {
        expected_recorded += sentinel.table_frames;
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(sentinel.address));
        if !page_table.gate_page_is_unmapped(page) {
            return Err("oversized sentinel address was already mapped");
        }
        let frame = allocate_frame().ok_or("oversized leaf allocation failed")?;
        if page_table.map_page(page, frame, flags).is_err() {
            deallocate_frame(frame);
            return Err("oversized sentinel mapping failed");
        }
        page_table
            .unmap_page(page)
            .map_err(|_| "oversized sentinel unmap failed")?;
    }
    let recorded = page_table.recorded_table_frames_for_gate();
    if recorded != expected_recorded || recorded <= RETIRE_FRAME_BUDGET as usize {
        return Err("oversized hierarchy did not record the derived lease count");
    }
    Ok((page_table, recorded))
}

#[cfg(feature = "boot_tests")]
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

#[cfg(feature = "boot_tests")]
fn boot_push_live(pid: u64) {
    crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().push(boot_test_reclaim(pid)));
}

#[cfg(feature = "boot_tests")]
fn boot_push_parked(pid: u64, record: ParkRecord) {
    let mut reclaim = boot_test_reclaim(pid);
    reclaim.proof_failures = PROOF_FAILURES_BEFORE_PARK;
    reclaim.parked = Some(record);
    crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARKED);
    crate::trace_count!(crate::tracing::providers::teardown::RECLAIM_PARK_RESIDENT);
    crate::arch_without_interrupts(|| PARKED_PROCESS_RECLAIMS.lock().push(reclaim));
}

#[cfg(feature = "boot_tests")]
pub(crate) fn boot_reclaim_locations(pid: u64) -> (bool, bool) {
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

#[cfg(feature = "boot_tests")]
fn boot_force_blocker(pid: u64, blocker: Option<RootBlocker>) {
    BOOT_RECLAIM_FORCED_PID.store(pid, Ordering::Release);
    BOOT_RECLAIM_FORCED_BLOCKER.store(
        blocker.map_or(0, |blocker| blocker as u64 + 1),
        Ordering::Release,
    );
}

#[cfg(feature = "boot_tests")]
fn boot_last_park_snapshot() -> (scheduler::RetirementSnapshot, u64) {
    let mut epochs = [0; scheduler::MAX_CPUS];
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

#[cfg(feature = "boot_tests")]
fn boot_synthetic_park(mask: u64, first_epoch: u64) -> (ParkRecord, scheduler::RetirementSnapshot) {
    let mut epochs = [0; scheduler::MAX_CPUS];
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

#[cfg(feature = "boot_tests")]
pub fn retirement_fence_gate_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;

    let empty_before = crate::tracing::providers::teardown::RETIRE_EMPTY_ONLINE_MASK.aggregate();
    let empty = scheduler::RetirementFence {
        epochs: [0; scheduler::MAX_CPUS],
        online_mask: 0,
    };
    let zero = scheduler::RetirementSnapshot {
        epochs: [0; scheduler::MAX_CPUS],
        online_mask: 0,
    };
    if zero.fence_elapsed(&empty)
        || crate::tracing::providers::teardown::RETIRE_EMPTY_ONLINE_MASK.aggregate() <= empty_before
    {
        return TestResult::Fail("empty retirement mask elapsed or was not counted");
    }

    let mut target_epochs = [0; scheduler::MAX_CPUS];
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

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
fn x86_root_blocker_counters() -> [u64; 5] {
    use crate::tracing::providers::teardown as trace;

    [
        trace::ROOT_PROOF_BLOCKED_EPOCH.aggregate(),
        trace::ROOT_PROOF_BLOCKED_HW.aggregate(),
        trace::ROOT_PROOF_BLOCKED_SHADOW.aggregate(),
        trace::ROOT_PROOF_BLOCKED_CACHED.aggregate(),
        trace::ROOT_PROOF_BLOCKED_LIVE_ROW.aggregate(),
    ]
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
fn x86_forced_root_proof_case(pid: u64, blocker: RootBlocker) -> Result<(), &'static str> {
    use crate::tracing::providers::teardown as trace;

    let blocker_index = match blocker {
        RootBlocker::Epoch => 0,
        RootBlocker::Hardware => 1,
        RootBlocker::LiveRow => 4,
        RootBlocker::Shadow | RootBlocker::Cached => {
            return Err("unsupported forced x86 proof blocker")
        }
    };
    let (reclaim, recorded, _) = boot_page_table_reclaim(pid)?;
    let blocker_before = x86_root_blocker_counters();
    let returned_before = trace::PT_TABLE_FRAMES_RETURNED.aggregate();
    let retired_before = trace::PT_ROOTS_RETIRED.aggregate();
    let lost_before = trace::PT_RETIRE_FRAMES_LOST.aggregate();
    let reclaimed_before = trace::TEARDOWN_RECLAIM.aggregate();

    boot_force_blocker(pid, Some(blocker));
    crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().push(reclaim));
    boot_reclaim_deferred_process_resources();
    boot_force_blocker(pid, None);

    let mut blocked_expected = blocker_before;
    blocked_expected[blocker_index] += 1;
    if x86_root_blocker_counters() != blocked_expected
        || trace::PT_TABLE_FRAMES_RETURNED.aggregate() != returned_before
        || trace::PT_ROOTS_RETIRED.aggregate() != retired_before
        || trace::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before
        || trace::TEARDOWN_RECLAIM.aggregate() != reclaimed_before
        || boot_reclaim_locations(pid) != (true, false)
    {
        return Err("forced x86 proof blocker freed or lost its receipt");
    }

    boot_reclaim_deferred_process_resources();
    if x86_root_blocker_counters() != blocked_expected
        || trace::PT_TABLE_FRAMES_RETURNED.aggregate() != returned_before + recorded as u64 + 1
        || trace::PT_ROOTS_RETIRED.aggregate() != retired_before + 1
        || trace::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before
        || trace::TEARDOWN_RECLAIM.aggregate() != reclaimed_before + 1
        || boot_reclaim_locations(pid) != (false, false)
    {
        return Err("cleared x86 proof blocker did not retire cleanly");
    }
    Ok(())
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
fn x86_shadow_proof_case(pid: u64) -> Result<(), &'static str> {
    use crate::tracing::providers::teardown as trace;

    let (reclaim, recorded, root) = boot_page_table_reclaim(pid)?;
    let blocker_before = x86_root_blocker_counters();
    let returned_before = trace::PT_TABLE_FRAMES_RETURNED.aggregate();
    let retired_before = trace::PT_ROOTS_RETIRED.aggregate();
    let lost_before = trace::PT_RETIRE_FRAMES_LOST.aggregate();
    let reclaimed_before = trace::TEARDOWN_RECLAIM.aggregate();
    crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().push(reclaim));

    let previous_next = crate::per_cpu::get_next_cr3();
    crate::per_cpu::set_next_cr3(root);
    boot_reclaim_deferred_process_resources();
    crate::per_cpu::set_next_cr3(previous_next);
    let mut next_expected = blocker_before;
    next_expected[2] += 1;
    if x86_root_blocker_counters() != next_expected
        || trace::PT_TABLE_FRAMES_RETURNED.aggregate() != returned_before
        || trace::PT_ROOTS_RETIRED.aggregate() != retired_before
        || trace::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before
        || trace::TEARDOWN_RECLAIM.aggregate() != reclaimed_before
        || boot_reclaim_locations(pid) != (true, false)
    {
        return Err("next_cr3 shadow blocker freed or lost its receipt");
    }

    let previous_saved = crate::per_cpu::get_saved_process_cr3();
    unsafe {
        crate::arch_impl::x86_64::percpu::X86PerCpu::set_saved_process_cr3(root);
    }
    boot_reclaim_deferred_process_resources();
    unsafe {
        crate::arch_impl::x86_64::percpu::X86PerCpu::set_saved_process_cr3(previous_saved);
    }
    let mut saved_expected = blocker_before;
    saved_expected[2] += 2;
    if x86_root_blocker_counters() != saved_expected
        || trace::PT_TABLE_FRAMES_RETURNED.aggregate() != returned_before
        || trace::PT_ROOTS_RETIRED.aggregate() != retired_before
        || trace::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before
        || trace::TEARDOWN_RECLAIM.aggregate() != reclaimed_before
        || boot_reclaim_locations(pid) != (true, false)
    {
        return Err("saved_process_cr3 shadow blocker freed or lost its receipt");
    }

    boot_reclaim_deferred_process_resources();
    if x86_root_blocker_counters() != saved_expected
        || trace::PT_TABLE_FRAMES_RETURNED.aggregate() != returned_before + recorded as u64 + 1
        || trace::PT_ROOTS_RETIRED.aggregate() != retired_before + 1
        || trace::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before
        || trace::TEARDOWN_RECLAIM.aggregate() != reclaimed_before + 1
        || boot_reclaim_locations(pid) != (false, false)
    {
        return Err("cleared x86 shadow blockers did not retire cleanly");
    }
    Ok(())
}

#[cfg(feature = "boot_tests")]
pub fn reclaim_progress_gate_test() -> crate::test_framework::registry::TestResult {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    use crate::test_framework::registry::TestResult;
    use crate::tracing::providers::teardown as trace;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

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

    #[cfg(target_arch = "x86_64")]
    {
        let mut contended = match crate::memory::process_memory::ProcessPageTable::new() {
            Ok(page_table) => page_table,
            Err(_) => return TestResult::Fail("E: x86 page-table construction failed"),
        };
        let custody_frames = contended.custody_frames_for_gate();
        let recorded = contended.recorded_table_frames_for_gate();
        let free_before = crate::memory::frame_allocator::free_list_len_for_gate();
        let allocator_lost_before = trace::FRAME_LOST_CONTENDED.aggregate();
        let retire_lost_before = trace::PT_RETIRE_FRAMES_LOST.aggregate();
        let returned_before = trace::PT_TABLE_FRAMES_RETURNED.aggregate();
        let retired_before = trace::PT_ROOTS_RETIRED.aggregate();
        let no_arch_before = trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate();
        let undecided_before = trace::PT_ROOT_DROPPED_UNDECIDED.aggregate();
        let refusals_before = [
            trace::FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
            trace::FRAME_RETURN_REFUSED_STALE.aggregate(),
            trace::FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
            trace::FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
            trace::FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        ];
        let mut budget = crate::memory::process_memory::RETIRE_FRAME_BUDGET;
        let progress = crate::memory::frame_allocator::retire_with_free_list_contended(
            &mut contended,
            BOOT_RECLAIM_PID_BASE + 90,
            &mut budget,
        );
        let mut repaired = true;
        for frame in custody_frames.iter().copied() {
            repaired &= crate::memory::frame_allocator::republish_frame_for_gate(frame);
        }
        let refusals_after = [
            trace::FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
            trace::FRAME_RETURN_REFUSED_STALE.aggregate(),
            trace::FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
            trace::FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
            trace::FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        ];
        if progress != crate::memory::process_memory::RetireProgress::Complete
            || custody_frames.len() != recorded + 1
            || trace::FRAME_LOST_CONTENDED
                .aggregate()
                .saturating_sub(allocator_lost_before)
                < custody_frames.len() as u64
            || trace::PT_RETIRE_FRAMES_LOST.aggregate()
                != retire_lost_before + custody_frames.len() as u64
            || trace::PT_TABLE_FRAMES_RETURNED.aggregate() != returned_before
            || trace::PT_ROOTS_RETIRED.aggregate() != retired_before + 1
            || trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate() != no_arch_before
            || trace::PT_ROOT_DROPPED_UNDECIDED.aggregate() != undecided_before
            || refusals_after != refusals_before
            || !repaired
            || crate::memory::frame_allocator::free_list_len_for_gate()
                != free_before + custody_frames.len()
        {
            return TestResult::Fail("E: x86 retirement contention was not isolated and repaired");
        }

        let mut healthy = match crate::memory::process_memory::ProcessPageTable::new() {
            Ok(page_table) => page_table,
            Err(_) => {
                return TestResult::Fail("E: healthy recovery page-table construction failed")
            }
        };
        let healthy_recorded = healthy.recorded_table_frames_for_gate();
        let healthy_returned_before = trace::PT_TABLE_FRAMES_RETURNED.aggregate();
        let healthy_retired_before = trace::PT_ROOTS_RETIRED.aggregate();
        let mut healthy_budget = crate::memory::process_memory::RETIRE_FRAME_BUDGET;
        if healthy.retire_bounded(BOOT_RECLAIM_PID_BASE + 91, &mut healthy_budget)
            != crate::memory::process_memory::RetireProgress::Complete
            || trace::PT_TABLE_FRAMES_RETURNED.aggregate()
                != healthy_returned_before + healthy_recorded as u64 + 1
            || trace::PT_ROOTS_RETIRED.aggregate() != healthy_retired_before + 1
            || trace::PT_RETIRE_FRAMES_LOST.aggregate()
                != retire_lost_before + custody_frames.len() as u64
            || trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate() != no_arch_before
            || trace::PT_ROOT_DROPPED_UNDECIDED.aggregate() != undecided_before
        {
            return TestResult::Fail("E: healthy retirement did not recover after contention");
        }
    }

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
            VirtAddr::new(0x400000),
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

    #[cfg(target_arch = "aarch64")]
    {
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
    }

    if scheduler::MAX_CPUS >= 2 {
        let age_pid = BOOT_RECLAIM_PID_BASE + 3;
        let age_advance_cpu = scheduler::MAX_CPUS.saturating_sub(2);
        let age_last_cpu = scheduler::MAX_CPUS.saturating_sub(1);
        let age_mask = (1 << age_advance_cpu) | (1 << age_last_cpu);
        let (age_record, age_snapshot) = boot_synthetic_park(age_mask, 200);
        boot_push_parked(age_pid, age_record);
        if trace::RECLAIM_PARK_RESIDENT.aggregate() != resident_before.wrapping_add(1) {
            return TestResult::Fail("age-arm receipt was not observably resident");
        }
        let mut age_63 = age_snapshot;
        age_63.epochs[age_advance_cpu] = age_63.epochs[age_advance_cpu].wrapping_add(63);
        unpark_sweep_with_snapshot(age_63, age_record.row_epoch_at_park);
        if boot_reclaim_locations(age_pid) != (false, true) {
            return TestResult::Fail("age unpark arm fired before 64 scheduling epochs");
        }
        boot_force_blocker(age_pid, Some(RootBlocker::LiveRow));
        let mut age_64 = age_63;
        age_64.epochs[age_advance_cpu] = age_64.epochs[age_advance_cpu].wrapping_add(1);
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
    } else if scheduler::MAX_CPUS == 1 {
        let epoch_pid = BOOT_RECLAIM_PID_BASE + 3;
        let epoch_cpu = 0;
        let (epoch_record, epoch_snapshot) = boot_synthetic_park(1 << epoch_cpu, 200);
        boot_push_parked(epoch_pid, epoch_record);
        if trace::RECLAIM_PARK_RESIDENT.aggregate() != resident_before.wrapping_add(1) {
            return TestResult::Fail("single-CPU epoch-arm receipt was not observably resident");
        }
        boot_force_blocker(epoch_pid, Some(RootBlocker::LiveRow));
        let epoch_before = trace::RECLAIM_UNPARKED_EPOCH.aggregate();
        let age_before = trace::RECLAIM_UNPARKED_AGE.aggregate();
        let mut epoch_advanced = epoch_snapshot;
        epoch_advanced.epochs[epoch_cpu] = epoch_advanced.epochs[epoch_cpu].wrapping_add(1);
        unpark_sweep_with_snapshot(epoch_advanced, epoch_record.row_epoch_at_park);
        boot_reclaim_deferred_process_resources();
        if trace::RECLAIM_UNPARKED_EPOCH
            .aggregate()
            .saturating_sub(epoch_before)
            != 1
            || trace::RECLAIM_UNPARKED_AGE.aggregate() != age_before
            || boot_reclaim_locations(epoch_pid) != (true, false)
        {
            return TestResult::Fail("single-CPU advance did not unpark through the epoch arm alone");
        }
        boot_force_blocker(epoch_pid, None);
        boot_reclaim_deferred_process_resources();
    } else {
        return TestResult::Fail("reclaim progress gate requires at least one CPU");
    }

    #[cfg(target_arch = "aarch64")]
    {
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
    }

    #[cfg(target_arch = "x86_64")]
    {
        if x86_forced_root_proof_case(
            BOOT_RECLAIM_PID_BASE + 10,
            RootBlocker::Hardware,
        )
        .is_err()
        {
            return TestResult::Fail("P1: x86 hardware proof injection failed");
        }
        if x86_shadow_proof_case(BOOT_RECLAIM_PID_BASE + 11).is_err() {
            return TestResult::Fail("P2: x86 CR3 shadow injections failed");
        }
        if x86_forced_root_proof_case(
            BOOT_RECLAIM_PID_BASE + 12,
            RootBlocker::LiveRow,
        )
        .is_err()
        {
            return TestResult::Fail("P3: x86 live-row proof injection failed");
        }
        if x86_forced_root_proof_case(BOOT_RECLAIM_PID_BASE + 13, RootBlocker::Epoch).is_err() {
            return TestResult::Fail("P4: x86 epoch proof injection failed");
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

    // O2/F: the fixture derives a hierarchy larger than the production budget.
    // The first pass returns exactly the budget, then re-proves and completes.
    let oversized_pid = BOOT_RECLAIM_PID_BASE + 100;
    let mut oversized = boot_test_reclaim(oversized_pid);
    let oversized_recorded;
    match boot_oversized_page_table() {
        Ok((page_table, recorded)) => {
            oversized.page_table = Some(page_table);
            oversized_recorded = recorded;
        }
        Err(message) => return TestResult::Fail(message),
    }
    let budget_before = trace::PT_RETIRE_BUDGET_REQUEUED.aggregate();
    let returned_before = trace::PT_TABLE_FRAMES_RETURNED.aggregate();
    let roots_before = trace::PT_ROOTS_RETIRED.aggregate();
    let lost_before = trace::PT_RETIRE_FRAMES_LOST.aggregate();
    let no_arch_before = trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate();
    let undecided_before = trace::PT_ROOT_DROPPED_UNDECIDED.aggregate();
    #[cfg(target_arch = "x86_64")]
    let oversized_refusals_before = [
        trace::FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        trace::FRAME_RETURN_REFUSED_STALE.aggregate(),
        trace::FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        trace::FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        trace::FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
    ];
    let reclaimed_before = trace::TEARDOWN_RECLAIM.aggregate();
    crate::arch_without_interrupts(|| PENDING_PROCESS_RECLAIMS.lock().push(oversized));
    boot_reclaim_deferred_process_resources();
    if trace::PT_RETIRE_BUDGET_REQUEUED
        .aggregate()
        .saturating_sub(budget_before)
        != 1
        || trace::PT_TABLE_FRAMES_RETURNED
            .aggregate()
            .saturating_sub(returned_before)
            != crate::memory::process_memory::RETIRE_FRAME_BUDGET as u64
        || trace::PT_ROOTS_RETIRED.aggregate() != roots_before
        || trace::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before
        || trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate() != no_arch_before
        || trace::PT_ROOT_DROPPED_UNDECIDED.aggregate() != undecided_before
        || trace::TEARDOWN_RECLAIM.aggregate() != reclaimed_before
        || boot_reclaim_locations(oversized_pid) != (true, false)
    {
        return TestResult::Fail("F: oversized retirement did not requeue at its exact budget");
    }
    boot_reclaim_deferred_process_resources();
    if trace::PT_RETIRE_BUDGET_REQUEUED.aggregate() != budget_before + 1
        || trace::PT_TABLE_FRAMES_RETURNED
            .aggregate()
            .saturating_sub(returned_before)
            != oversized_recorded as u64 + 1
        || trace::PT_ROOTS_RETIRED.aggregate() != roots_before + 1
        || trace::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before
        || trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate() != no_arch_before
        || trace::PT_ROOT_DROPPED_UNDECIDED.aggregate() != undecided_before
        || trace::TEARDOWN_RECLAIM.aggregate() != reclaimed_before + 1
        || boot_reclaim_locations(oversized_pid) != (false, false)
    {
        return TestResult::Fail("F: oversized retirement did not complete after re-proof");
    }
    #[cfg(target_arch = "x86_64")]
    if [
        trace::FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        trace::FRAME_RETURN_REFUSED_STALE.aggregate(),
        trace::FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        trace::FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        trace::FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
    ] != oversized_refusals_before
    {
        return TestResult::Fail("F: oversized retirement triggered a frame refusal");
    }

    // O2/I: dropping an intentionally interrupted retirement counts the root
    // still in custody and never retries an already-popped lease.
    let (mut interrupted, interrupted_recorded) = match boot_oversized_page_table() {
        Ok(fixture) => fixture,
        Err(message) => return TestResult::Fail(message),
    };
    let mid_before = trace::PT_ROOT_DROPPED_MID_RETIRE.aggregate();
    let double_before = trace::FRAME_RETURN_REFUSED_DOUBLE.aggregate();
    let interrupted_returned_before = trace::PT_TABLE_FRAMES_RETURNED.aggregate();
    let interrupted_roots_before = trace::PT_ROOTS_RETIRED.aggregate();
    let interrupted_lost_before = trace::PT_RETIRE_FRAMES_LOST.aggregate();
    let interrupted_no_arch_before = trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate();
    let interrupted_undecided_before = trace::PT_ROOT_DROPPED_UNDECIDED.aggregate();
    let mut budget = crate::memory::process_memory::RETIRE_FRAME_BUDGET;
    if interrupted.retire_bounded(BOOT_RECLAIM_PID_BASE + 101, &mut budget)
        != crate::memory::process_memory::RetireProgress::Budgeted
        || interrupted_recorded <= crate::memory::process_memory::RETIRE_FRAME_BUDGET as usize
    {
        return TestResult::Fail("I: oversized retirement did not stop at its budget");
    }
    drop(interrupted);
    if trace::PT_ROOT_DROPPED_MID_RETIRE.aggregate() != mid_before + 1
        || trace::FRAME_RETURN_REFUSED_DOUBLE.aggregate() != double_before
        || trace::PT_TABLE_FRAMES_RETURNED.aggregate()
            != interrupted_returned_before
                + crate::memory::process_memory::RETIRE_FRAME_BUDGET as u64
        || trace::PT_ROOTS_RETIRED.aggregate() != interrupted_roots_before
        || trace::PT_RETIRE_FRAMES_LOST.aggregate() != interrupted_lost_before
        || trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate() != interrupted_no_arch_before
        || trace::PT_ROOT_DROPPED_UNDECIDED.aggregate() != interrupted_undecided_before
    {
        return TestResult::Fail("I: interrupted retirement did not fail closed");
    }

    // O2/J: completion is terminal; a second call has no accounting effect.
    let mut completed = match crate::memory::process_memory::ProcessPageTable::new() {
        Ok(page_table) => page_table,
        Err(_) => return TestResult::Fail("J: page-table construction failed"),
    };
    let mut budget = crate::memory::process_memory::RETIRE_FRAME_BUDGET;
    if completed.retire_bounded(BOOT_RECLAIM_PID_BASE + 102, &mut budget)
        != crate::memory::process_memory::RetireProgress::Complete
    {
        return TestResult::Fail("J: initial root retirement did not complete");
    }
    let terminal_before = trace::snapshot();
    if completed.retire_bounded(BOOT_RECLAIM_PID_BASE + 102, &mut budget)
        != crate::memory::process_memory::RetireProgress::Complete
        || trace::snapshot() != terminal_before
    {
        return TestResult::Fail("J: completed retirement was not idempotent");
    }

    #[cfg(target_arch = "x86_64")]
    {
        use x86_64::structures::paging::{Page, PageTableFlags, Size4KiB};

        let refused_pid = BOOT_RECLAIM_PID_BASE + 103;
        let (mut refused, _, _) = match boot_page_table_reclaim(refused_pid) {
            Ok(fixture) => fixture,
            Err(message) => return TestResult::Fail(message),
        };
        let refused_page =
            Page::<Size4KiB>::containing_address(VirtAddr::new(0x0123_4000));
        let refused_frame = match crate::memory::frame_allocator::allocate_frame() {
            Some(frame) => frame,
            None => return TestResult::Fail("Q: refused leaf allocation failed"),
        };
        let refused_flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
        let Some(refused_page_table) = refused.page_table.as_mut() else {
            return TestResult::Fail("Q: refused page table disappeared before mapping");
        };
        if refused_page_table
            .map_page(refused_page, refused_frame, refused_flags)
            .is_err()
        {
            crate::memory::frame_allocator::deallocate_frame(refused_frame);
            return TestResult::Fail("Q: refused sentinel mapping failed");
        }
        let refused_used_before = crate::memory::frame_allocator::memory_stats()
            .allocated_frames
            .saturating_sub(crate::memory::frame_allocator::free_list_len_for_gate());
        let no_arch_before = trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate();
        let returned_before = trace::PT_TABLE_FRAMES_RETURNED.aggregate();
        let retired_before = trace::PT_ROOTS_RETIRED.aggregate();
        let lost_before = trace::PT_RETIRE_FRAMES_LOST.aggregate();
        let undecided_before = trace::PT_ROOT_DROPPED_UNDECIDED.aggregate();
        {
            let reserve_failure = BootReclaimReserveFailureGuard::arm();
            enqueue_process_reclaim(refused);
            drop(reserve_failure);
        }
        if trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate() != no_arch_before + 1
            || trace::PT_TABLE_FRAMES_RETURNED.aggregate() != returned_before
            || trace::PT_ROOTS_RETIRED.aggregate() != retired_before
            || trace::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before
            || trace::PT_ROOT_DROPPED_UNDECIDED.aggregate() != undecided_before
            || crate::memory::frame_allocator::memory_stats()
                .allocated_frames
                .saturating_sub(crate::memory::frame_allocator::free_list_len_for_gate())
                != refused_used_before
            || boot_reclaim_locations(refused_pid) != (false, false)
        {
            return TestResult::Fail("Q: x86 enqueue reservation failure did not fail closed");
        }

        let healthy_pid = BOOT_RECLAIM_PID_BASE + 104;
        let (healthy, healthy_recorded, _) = match boot_page_table_reclaim(healthy_pid) {
            Ok(fixture) => fixture,
            Err(message) => return TestResult::Fail(message),
        };
        enqueue_process_reclaim(healthy);
        boot_reclaim_deferred_process_resources();
        if trace::PT_ROOT_ABANDONED_NO_ARCH.aggregate() != no_arch_before + 1
            || trace::PT_TABLE_FRAMES_RETURNED.aggregate()
                != returned_before + healthy_recorded as u64 + 1
            || trace::PT_ROOTS_RETIRED.aggregate() != retired_before + 1
            || trace::PT_RETIRE_FRAMES_LOST.aggregate() != lost_before
            || trace::PT_ROOT_DROPPED_UNDECIDED.aggregate() != undecided_before
            || boot_reclaim_locations(healthy_pid) != (false, false)
        {
            return TestResult::Fail("Q: healthy enqueue remained impaired after reservation failure");
        }
    }

    if trace::RECLAIM_PARK_RESIDENT.aggregate() != resident_before
        || trace::PROOF_UNDER_QUEUE_LOCK.aggregate() != proof_lock_before
    {
        return TestResult::Fail("P1 reclaim gate left residents or nested proof locks");
    }
    TestResult::Pass
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_retirement_fence_gate() {
    crate::serial_println!("[TEST:process:retirement_fence_gate:START]");
    let result = retirement_fence_gate_test();
    if result.is_pass() {
        crate::serial_println!("[TEST:process:retirement_fence_gate:PASS]");
    } else {
        crate::serial_println!(
            "[TEST:process:retirement_fence_gate:FAIL:{:?}]",
            result
        );
    }
    assert!(result.is_pass(), "x86 retirement fence gate failed");
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_reclaim_progress_gate() {
    crate::serial_println!("[TEST:process:reclaim_progress_gate:START]");
    let result = reclaim_progress_gate_test();
    if result.is_pass() {
        crate::serial_println!("[TEST:process:reclaim_progress_gate:PASS]");
    } else {
        crate::serial_println!("[TEST:process:reclaim_progress_gate:FAIL:{:?}]", result);
    }
    assert!(result.is_pass(), "x86 reclaim progress gate failed");
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
