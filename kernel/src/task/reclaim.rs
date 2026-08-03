//! Deferred process resource reclamation.

use alloc::alloc::{alloc, Layout};
use alloc::boxed::Box;
use alloc::vec::Vec;
#[cfg(target_arch = "aarch64")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use crate::memory::process_memory::ProcessPageTable;
use crate::memory::stack::GuardedStack;
use crate::process::ProcessId;

/// Resources reserved at process birth and filled by the one-shot exit commit.
pub struct ProcessGrave {
    pub pid: ProcessId,
    pub exit_code: i32,
    pub page_table: Option<Box<ProcessPageTable>>,
    pub old_page_tables: Vec<Box<ProcessPageTable>>,
    pub stack: Option<Box<GuardedStack>>,
    #[cfg(target_arch = "aarch64")]
    pub fence: super::scheduler::RetirementFence,
    pub queued_at_ns: u64,
    pub warned: bool,
    pub(crate) next: *mut ProcessGrave,
}

// The intrusive next pointer is owned exclusively by the graveyard stack or by
// the reclaimer that detached it. No producer dereferences another node.
unsafe impl Send for ProcessGrave {}

static GRAVEYARD: AtomicPtr<ProcessGrave> = AtomicPtr::new(core::ptr::null_mut());
#[cfg(target_arch = "aarch64")]
static GRAVE_DETACH_LOCK: spin::Mutex<()> = spin::Mutex::new(());
static RECLAIM_WORK_GEN: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static RECLAIM_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static RECLAIM_WAKE_PENDING: AtomicBool = AtomicBool::new(false);
#[cfg(target_arch = "aarch64")]
static RECLAIM_KTHREAD: spin::Once<crate::task::KthreadHandle> = spin::Once::new();
static GRAVES_QUEUED: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static GRAVES_RECLAIMED: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static GRAVES_BLOCKED: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
const RECLAIM_STALL_WARN_NS: u64 = 30_000_000_000;

/// Capability proving that destructive reclaim runs in a preemptible context.
pub struct ReclaimContext(());

impl ReclaimContext {
    pub(crate) fn assert_preemptible() -> Self {
        debug_assert!(crate::arch_interrupts_enabled());
        #[cfg(target_arch = "aarch64")]
        debug_assert_eq!(
            crate::arch_impl::aarch64::percpu::Aarch64PerCpu::preempt_count()
                & crate::arch_impl::aarch64::constants::PREEMPT_ACTIVE,
            0
        );
        debug_assert!(!crate::process::process_manager_lock_held_by_current_cpu());
        Self(())
    }
}

/// Dispose of page tables retired by the preceding exec before beginning the
/// next address-space replacement. The PM transaction only moves the existing
/// Vec buffer; all frame walks, logging, and destruction happen after the lock
/// is released in a preemptible context.
pub fn drain_old_page_tables_for_exec(pid: ProcessId) {
    let old_page_tables = crate::process::manager()
        .as_mut()
        .and_then(|manager| manager.take_old_page_tables_for_exec(pid));
    let Some(old_page_tables) = old_page_tables else {
        return;
    };
    if old_page_tables.is_empty() {
        return;
    }
    let context = ReclaimContext::assert_preemptible();
    for page_table in old_page_tables {
        page_table.cleanup_for_exec(&context);
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ReclaimStats {
    pub reclaimed: u64,
    pub blocked: u64,
}

impl ProcessGrave {
    /// Allocate the grave fallibly so process creation can report ENOMEM.
    pub fn try_new(pid: ProcessId) -> Option<Box<Self>> {
        let layout = Layout::new::<Self>();
        let ptr = unsafe { alloc(layout).cast::<Self>() };
        if ptr.is_null() {
            return None;
        }

        unsafe {
            ptr.write(Self {
                pid,
                exit_code: 0,
                page_table: None,
                old_page_tables: Vec::new(),
                stack: None,
                #[cfg(target_arch = "aarch64")]
                fence: super::scheduler::RetirementFence::invalid(),
                queued_at_ns: 0,
                warned: false,
                next: core::ptr::null_mut(),
            });
            Some(Box::from_raw(ptr))
        }
    }

    fn release_stack(&mut self, _context: &ReclaimContext) {
        drop(self.stack.take());
    }
}

/// Publish one preallocated grave on the intrusive Treiber stack.
pub fn push_grave(grave: Box<ProcessGrave>) {
    GRAVES_QUEUED.fetch_add(1, Ordering::Relaxed);
    push_grave_inner(grave);
}

fn push_grave_inner(grave: Box<ProcessGrave>) {
    let grave = Box::into_raw(grave);
    let mut head = GRAVEYARD.load(Ordering::Relaxed);
    loop {
        unsafe {
            (*grave).next = head;
        }
        match GRAVEYARD.compare_exchange_weak(head, grave, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => return,
            Err(observed) => head = observed,
        }
    }
}

/// Detach the entire graveyard for one reclaimer pass.
///
/// `GRAVE_DETACH_LOCK` serializes every consumer. Producers and blocked-grave
/// requeues only push, so consumers use one atomic swap and never perform an
/// ABA-sensitive single-node pop.
#[cfg(target_arch = "aarch64")]
fn take_all_graves() -> *mut ProcessGrave {
    GRAVEYARD.swap(core::ptr::null_mut(), Ordering::Acquire)
}

pub fn kreclaim_wake() {
    RECLAIM_WORK_GEN.fetch_add(1, Ordering::Release);
    #[cfg(target_arch = "aarch64")]
    {
        RECLAIM_WAKE_PENDING.store(true, Ordering::Release);
        crate::task::scheduler::set_need_resched();
    }
}

/// Complete a durable reclaimer wake while already holding the scheduler lock.
/// Producers publish only atomics; this consumer performs the actual kthread
/// unpark protocol without recursively acquiring the scheduler lock.
#[cfg(target_arch = "aarch64")]
pub(crate) fn drain_reclaim_wake(scheduler: &mut super::scheduler::Scheduler) {
    if !RECLAIM_WAKE_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    let Some(handle) = RECLAIM_KTHREAD.get() else {
        RECLAIM_WAKE_PENDING.store(true, Ordering::Release);
        return;
    };
    crate::task::kthread::kthread_unpark_with_scheduler(handle, scheduler);
}

#[cfg(target_arch = "aarch64")]
pub fn dump_reclaim_state() {
    use crate::arch_impl::aarch64::context_switch::{raw_uart_dec, raw_uart_str};

    raw_uart_str("  grave_queued=");
    raw_uart_dec(GRAVES_QUEUED.load(Ordering::Relaxed));
    raw_uart_str(" reclaimed=");
    raw_uart_dec(GRAVES_RECLAIMED.load(Ordering::Relaxed));
    raw_uart_str(" blocked_observations=");
    raw_uart_dec(GRAVES_BLOCKED.load(Ordering::Relaxed));
    raw_uart_str(" work_gen=");
    raw_uart_dec(RECLAIM_WORK_GEN.load(Ordering::Acquire));
    raw_uart_str(" fault_exit_dropped=");
    raw_uart_dec(
        crate::task::process_task::FAULT_EXIT_INTENT_DROPPED.load(Ordering::Relaxed),
    );
    raw_uart_str(" frame_decref_underflow=");
    raw_uart_dec(
        crate::memory::frame_metadata::FRAME_DECREF_UNDERFLOW.load(Ordering::Relaxed),
    );
    raw_uart_str(" frame_decref_untracked=");
    raw_uart_dec(
        crate::memory::frame_metadata::FRAME_DECREF_UNTRACKED.load(Ordering::Relaxed),
    );
    raw_uart_str("\n");
}

#[cfg(target_arch = "aarch64")]
fn grave_reclaimable(
    grave: &ProcessGrave,
    snapshot: &super::scheduler::RetirementSnapshot,
) -> (bool, u64, u32, Option<u64>) {
    let root_mask = !0xFFFF_0000_0000_0FFF;
    let contains_root = |candidate: u64| {
        grave
            .page_table
            .iter()
            .chain(grave.old_page_tables.iter())
            .any(|page_table| {
                page_table.level_4_frame().start_address().as_u64() & root_mask
                    == candidate & root_mask
            })
    };

    let live_sharer_root = {
        let manager = crate::process::manager();
        manager.as_ref().and_then(|manager| {
            grave
                .page_table
                .iter()
                .chain(grave.old_page_tables.iter())
                .map(|page_table| page_table.level_4_frame().start_address().as_u64())
                .find(|root| manager.root_has_live_sharer(*root, None))
        })
    };
    if let Some(root) = live_sharer_root {
        return (false, root, 0, None);
    }

    let cached_holder =
        crate::task::scheduler::cached_ttbr0_holder_matching(snapshot, contains_root);
    for page_table in grave
        .page_table
        .iter()
        .chain(grave.old_page_tables.iter())
    {
        let root = page_table.level_4_frame().start_address().as_u64();
        let cached_thread = cached_holder
            .filter(|(_, cached_root)| *cached_root == root & root_mask)
            .map(|(thread_id, _)| thread_id);
        let liveness =
            crate::arch_impl::aarch64::root_liveness(snapshot, root, cached_thread);
        if liveness.is_live() {
            return (
                false,
                root,
                liveness.blocker_mask(),
                liveness.cached_thread,
            );
        }
    }
    (true, 0, 0, None)
}

#[cfg(target_arch = "aarch64")]
fn reclaim_detached(
    context: &ReclaimContext,
    mut cursor: *mut ProcessGrave,
    max_reclaimed: Option<u64>,
) -> ReclaimStats {
    let mut stats = ReclaimStats::default();
    let mut ready = core::ptr::null_mut();
    let mut ready_count = 0u64;

    while !cursor.is_null() {
        let mut grave = unsafe { Box::from_raw(cursor) };
        cursor = grave.next;
        if max_reclaimed.is_some_and(|limit| ready_count >= limit) {
            push_grave_inner(grave);
            continue;
        }
        let reclaimability = super::scheduler::RetirementSnapshot::acquire(&grave.fence)
            .map(|snapshot| grave_reclaimable(&grave, &snapshot));
        let (reclaimable, blocked_root, blocker_mask, cached_thread) =
            reclaimability.unwrap_or((false, 0, grave.fence.online_mask(), None));
        if !reclaimable {
            stats.blocked += 1;
            GRAVES_BLOCKED.fetch_add(1, Ordering::Relaxed);
            let (secs, nanos) = crate::time::get_monotonic_time_ns();
            let now = secs.saturating_mul(1_000_000_000) + nanos;
            let is_worker = crate::task::scheduler::current_thread_id()
                == Some(RECLAIM_TID.load(Ordering::Acquire));
            if is_worker
                && !grave.warned
                && now.saturating_sub(grave.queued_at_ns) >= RECLAIM_STALL_WARN_NS
            {
                grave.warned = true;
                log::warn!(
                    "kreclaimd stalled pid={} root={:#x} blocker_mask={:#x} cached_tid={} age_ns={}",
                    grave.pid.as_u64(),
                    blocked_root,
                    blocker_mask,
                    cached_thread.unwrap_or(0),
                    now.saturating_sub(grave.queued_at_ns)
                );
            }
            push_grave_inner(grave);
            continue;
        }

        let grave = Box::into_raw(grave);
        unsafe {
            (*grave).next = ready;
        }
        ready = grave;
        ready_count += 1;
    }

    if !ready.is_null() {
        crate::arch_impl::aarch64::invalidate_user_tlb_broadcast();
    }
    while !ready.is_null() {
        let mut grave = unsafe { Box::from_raw(ready) };
        ready = grave.next;
        grave.release_stack(context);
        if let Some(page_table) = grave.page_table.take() {
            page_table.cleanup_for_exec(context);
        }
        for old_page_table in grave.old_page_tables.drain(..) {
            old_page_table.cleanup_for_exec(context);
        }
        if let Some(manager) = crate::process::manager().as_mut() {
            manager.mark_address_space_reclaimed(grave.pid);
        }
        stats.reclaimed += 1;
        GRAVES_RECLAIMED.fetch_add(1, Ordering::Relaxed);
        GRAVES_QUEUED.fetch_sub(1, Ordering::Relaxed);
    }
    stats
}

#[cfg(target_arch = "aarch64")]
pub fn reclaim_pass(context: &ReclaimContext) -> ReclaimStats {
    let cursor = {
        let _detach = GRAVE_DETACH_LOCK.lock();
        take_all_graves()
    };
    reclaim_detached(context, cursor, None)
}

/// Reclaim at most one grave from latency-sensitive allocation paths.
#[cfg(target_arch = "aarch64")]
pub fn reclaim_one(context: &ReclaimContext) -> ReclaimStats {
    let cursor = {
        let _detach = GRAVE_DETACH_LOCK.lock();
        take_all_graves()
    };
    reclaim_detached(context, cursor, Some(1))
}

#[cfg(target_arch = "aarch64")]
enum ReclaimWork {
    DidWork,
    Blocked,
    Empty,
}

#[cfg(target_arch = "aarch64")]
fn service_one(context: &ReclaimContext) -> ReclaimWork {
    if let Some(thread_id) = crate::task::process_task::take_deferred_fault_sigsegv_exit() {
        crate::task::process_task::ProcessScheduler::handle_thread_exit(thread_id, -11);
        crate::task::scheduler::request_exit_pending(thread_id);
        return ReclaimWork::DidWork;
    }

    let mut blocked = false;
    if crate::task::scheduler::finalize_one_exit_pending() {
        return ReclaimWork::DidWork;
    }
    blocked |= crate::task::scheduler::has_pending_thread_reclaim();

    let reparented = crate::process::manager()
        .as_mut()
        .is_some_and(crate::process::ProcessManager::service_one_reparent);
    if reparented {
        return ReclaimWork::DidWork;
    }

    let parent_wake = crate::process::manager()
        .as_mut()
        .and_then(crate::process::ProcessManager::claim_one_parent_wake);
    if let Some((pid, parent_tid)) = parent_wake {
        if let Some(parent_tid) = parent_tid {
            crate::task::scheduler::with_scheduler(|scheduler| {
                scheduler.unblock_for_child_exit(parent_tid);
                scheduler.unblock_for_signal(parent_tid);
            });
            crate::tracing::providers::process::trace_waitpid_wake(
                parent_tid as u16,
                pid.as_u64() as u16,
            );
        }
        return ReclaimWork::DidWork;
    }

    let graphics_pid = crate::process::manager()
        .as_mut()
        .and_then(crate::process::ProcessManager::claim_one_graphics_cleanup);
    if let Some(pid) = graphics_pid {
        crate::syscall::graphics::cleanup_windows_for_pid(pid.as_u64());
        return ReclaimWork::DidWork;
    }

    let fd_action = crate::process::manager()
        .as_mut()
        .and_then(crate::process::ProcessManager::claim_one_exit_fd);
    if let Some(action) = fd_action {
        if let crate::process::manager::ExitFdAction::Close(fd) = action {
            crate::task::process_task::close_owned_fd(fd);
        }
        return ReclaimWork::DidWork;
    }

    let stats = reclaim_pass(context);
    if stats.reclaimed != 0 {
        return ReclaimWork::DidWork;
    }
    blocked |= stats.blocked != 0;

    if let Some(thread) = crate::task::scheduler::detach_reclaimable_thread() {
        drop(thread);
        return ReclaimWork::DidWork;
    }
    blocked |= crate::task::scheduler::has_pending_thread_reclaim();

    let row_candidate = crate::process::manager()
        .as_ref()
        .and_then(crate::process::ProcessManager::removable_row_candidate);
    if let Some(candidate) = row_candidate {
        let stack_permit = candidate.owned_stack.and_then(|(thread_id, stack_top)| {
            crate::task::scheduler::process_row_stack_reclaim_permit(thread_id, stack_top)
        });
        if candidate.owned_stack.is_none() || stack_permit.is_some() {
            let row = crate::process::manager()
                .as_mut()
                .and_then(|manager| manager.detach_removable_row(candidate, stack_permit.as_ref()));
            if let Some(row) = row {
                drop(row);
                return ReclaimWork::DidWork;
            }
        }
    }

    if blocked {
        ReclaimWork::Blocked
    } else {
        ReclaimWork::Empty
    }
}

#[cfg(target_arch = "aarch64")]
fn block_for_liveness_retry() {
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    let now = secs.saturating_mul(1_000_000_000) + nanos;
    crate::task::scheduler::with_scheduler(|scheduler| {
        scheduler.block_current_for_timer(now.saturating_add(10_000_000));
    });
    crate::task::scheduler::set_need_resched();
    crate::task::scheduler::schedule();
}

#[cfg(target_arch = "aarch64")]
pub fn kreclaimd_main() {
    loop {
        let observed_generation = RECLAIM_WORK_GEN.load(Ordering::Acquire);
        let context = ReclaimContext::assert_preemptible();
        match service_one(&context) {
            ReclaimWork::DidWork => continue,
            ReclaimWork::Blocked => block_for_liveness_retry(),
            ReclaimWork::Empty => crate::task::kthread_park_if(|| {
                RECLAIM_WORK_GEN.load(Ordering::Acquire) == observed_generation
            }),
        }
    }
}

#[cfg(target_arch = "aarch64")]
pub fn init_reclaim_thread() -> Result<crate::task::KthreadHandle, crate::task::KthreadError> {
    let handle = crate::task::kthread_run(kreclaimd_main, "kreclaimd")?;
    RECLAIM_TID.store(handle.tid(), Ordering::Release);
    RECLAIM_KTHREAD.call_once(|| handle.clone());
    if RECLAIM_WAKE_PENDING.load(Ordering::Acquire) {
        crate::task::scheduler::set_need_resched();
    }
    Ok(handle)
}
