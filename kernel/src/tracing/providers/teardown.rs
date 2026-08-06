//! Lock-free teardown observability.
//!
//! Phase 0 records existing teardown behavior only. Counters whose producer is
//! introduced by a later phase are deliberately registered and readable here,
//! but have no increment site yet.

use crate::tracing::counter::{register_counter, TraceCounter};
use crate::tracing::provider::{register_provider, TraceProvider};
use core::sync::atomic::{AtomicU64, Ordering};

pub const PROVIDER_ID: u8 = 0x0a;
pub const TEARDOWN_DEFER_EVENT: u16 = ((PROVIDER_ID as u16) << 8) | 0x00;
pub const TEARDOWN_RECLAIM_EVENT: u16 = ((PROVIDER_ID as u16) << 8) | 0x01;

#[no_mangle]
pub static TEARDOWN_PROVIDER: TraceProvider = TraceProvider {
    name: "teardown",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

macro_rules! counter {
    ($name:ident, $description:literal) => {
        crate::define_trace_counter!($name, $description);
    };
}

counter!(TEARDOWN_ENTRY_EXIT, "Exit teardown entries");
counter!(TEARDOWN_ENTRY_FAULT, "Fault teardown entries");
counter!(TEARDOWN_ENTRY_SIGNAL, "Signal teardown entries");
counter!(EXIT_FIRST_REQUESTS, "First exit requests");
counter!(EXIT_REPEAT_REQUESTS, "Repeated exit requests");
counter!(TEARDOWN_QUARANTINE, "Scheduler quarantine operations");
counter!(TEARDOWN_DEFER, "Deferred process-resource retirements");
counter!(TEARDOWN_RECLAIM, "Reclaimed process-resource retirements");
counter!(
    TEARDOWN_MASKED_FRAMES_WALKED,
    "Frame walks under process manager"
);
counter!(
    FD_CLOSES_UNDER_PM,
    "File descriptors closed or extracted under process manager"
);
counter!(TEARDOWN_VICTIM_DIVERGENCE, "TID and CR3 victim divergence");
counter!(TEARDOWN_CR3_MISS, "Fault-victim CR3 misses");
counter!(
    EXIT_ATTRIBUTION_UNCERTAIN,
    "Neither CR3 nor dispatched TID resolved a fault victim"
);
counter!(DEFERRED_FAULT_RING_DROPPED, "Dropped deferred fault exits");
counter!(
    RECLAIM_ENQUEUE_UNDER_PM,
    "Reclaim enqueues under process manager"
);
counter!(
    PROOF_UNDER_QUEUE_LOCK,
    "PM or scheduler acquisitions during queue proof"
);
counter!(
    RECLAIM_CONTEXT_VIOLATIONS,
    "Reclaim calls from forbidden contexts"
);
counter!(
    TEARDOWN_LOCK_ORDER_SUSPECT,
    "Suspect teardown lock ordering"
);

// Declaration-only until the phase named in PLAN.md. These intentionally have
// no trace_count! producer in Phase 0.
counter!(TEARDOWN_ENTRY_GROUP, "Group teardown entries");
counter!(EXIT_SGI_SENT, "Teardown-attributed expedite SGIs");
counter!(EXIT_REQUEST_OBSERVED, "Observed latched exit requests");
counter!(EXIT_KICK_PUBLISHED, "Published exit-kick buckets");
counter!(EXIT_KICK_OBSERVED, "Observed exit-kick victims");
counter!(EXIT_KICK_BUCKET_COLLISION, "Exit-kick bucket collisions");
counter!(RECEIPT_DROPPED_UNRETIRED, "Receipts recovered by Drop");
counter!(
    RECLAIM_PASS_SKIPPED,
    "Reclaim candidates skipped in one pass"
);
counter!(RECLAIM_PARKED, "Parked reclaim candidates");
counter!(
    RECLAIM_UNPARKED_EPOCH,
    "Reclaims unparked by scheduling epoch"
);
counter!(RECLAIM_UNPARKED_ROW, "Reclaims unparked by row removal");
counter!(RECLAIM_UNPARKED_AGE, "Reclaims unparked by age backstop");
counter!(
    RECLAIM_PARK_IMMEDIATE_UNPARK,
    "Immediately unparked reclaims"
);
counter!(RECLAIM_PARK_RESIDENT, "Resident parked reclaims");
counter!(LEDGER_EFFECT_AMBIGUOUS_REPORT, "Ambiguous report effects");
counter!(
    TOMBSTONE_JOIN_REAP_SECOND,
    "Tombstone joins completed by reap"
);
counter!(
    TOMBSTONE_JOIN_RETIRE_SECOND,
    "Tombstone joins completed by retire"
);
counter!(
    EXIT_BLOCK_REFUSED_FAMILY,
    "Latched exits refused by blocking families"
);
counter!(
    EXIT_WAIT_CANCELLED_FAMILY,
    "Blocked exits cancelled by wait families"
);
counter!(INIT_FATAL_SIGNAL_DROPPED, "Fatal signals dropped for init");
counter!(
    INIT_FATAL_SIGNAL_DROPPED_GROUP,
    "Fatal group signals dropped for init"
);

pub const COUNTER_COUNT: usize = 39;

/// The registration and normal-context reader inventory. Keeping one inventory
/// makes a write-only counter structurally impossible without changing the P0
/// source ratchet.
pub static COUNTERS: [&TraceCounter; COUNTER_COUNT] = [
    &TEARDOWN_ENTRY_EXIT,
    &TEARDOWN_ENTRY_FAULT,
    &TEARDOWN_ENTRY_SIGNAL,
    &TEARDOWN_ENTRY_GROUP,
    &EXIT_FIRST_REQUESTS,
    &EXIT_REPEAT_REQUESTS,
    &TEARDOWN_QUARANTINE,
    &TEARDOWN_DEFER,
    &TEARDOWN_RECLAIM,
    &TEARDOWN_MASKED_FRAMES_WALKED,
    &FD_CLOSES_UNDER_PM,
    &TEARDOWN_VICTIM_DIVERGENCE,
    &TEARDOWN_CR3_MISS,
    &EXIT_ATTRIBUTION_UNCERTAIN,
    &DEFERRED_FAULT_RING_DROPPED,
    &RECLAIM_ENQUEUE_UNDER_PM,
    &PROOF_UNDER_QUEUE_LOCK,
    &RECLAIM_CONTEXT_VIOLATIONS,
    &TEARDOWN_LOCK_ORDER_SUSPECT,
    &EXIT_SGI_SENT,
    &EXIT_REQUEST_OBSERVED,
    &EXIT_KICK_PUBLISHED,
    &EXIT_KICK_OBSERVED,
    &EXIT_KICK_BUCKET_COLLISION,
    &RECEIPT_DROPPED_UNRETIRED,
    &RECLAIM_PASS_SKIPPED,
    &RECLAIM_PARKED,
    &RECLAIM_UNPARKED_EPOCH,
    &RECLAIM_UNPARKED_ROW,
    &RECLAIM_UNPARKED_AGE,
    &RECLAIM_PARK_IMMEDIATE_UNPARK,
    &RECLAIM_PARK_RESIDENT,
    &LEDGER_EFFECT_AMBIGUOUS_REPORT,
    &TOMBSTONE_JOIN_REAP_SECOND,
    &TOMBSTONE_JOIN_RETIRE_SECOND,
    &EXIT_BLOCK_REFUSED_FAMILY,
    &EXIT_WAIT_CANCELLED_FAMILY,
    &INIT_FATAL_SIGNAL_DROPPED,
    &INIT_FATAL_SIGNAL_DROPPED_GROUP,
];

pub fn init() {
    register_provider(&TEARDOWN_PROVIDER);
    for counter in COUNTERS {
        assert!(
            register_counter(counter).is_some(),
            "trace counter registry overflow while registering teardown counters"
        );
    }
}

/// Read every Phase-0 counter from normal context.
pub fn snapshot() -> [u64; COUNTER_COUNT] {
    core::array::from_fn(|index| COUNTERS[index].aggregate())
}

#[cfg(feature = "boot_tests")]
const BOOT_TEST_PID_COUNT_SLOTS: usize = 128;

#[cfg(feature = "boot_tests")]
struct BootTestPidCountSlot {
    pid: AtomicU64,
    defer_count: AtomicU64,
    reclaim_count: AtomicU64,
}

#[cfg(feature = "boot_tests")]
impl BootTestPidCountSlot {
    const fn new() -> Self {
        Self {
            pid: AtomicU64::new(0),
            defer_count: AtomicU64::new(0),
            reclaim_count: AtomicU64::new(0),
        }
    }
}

#[cfg(feature = "boot_tests")]
static BOOT_TEST_PID_COUNTS: [BootTestPidCountSlot; BOOT_TEST_PID_COUNT_SLOTS] =
    [const { BootTestPidCountSlot::new() }; BOOT_TEST_PID_COUNT_SLOTS];

#[cfg(feature = "boot_tests")]
static BOOT_TEST_PID_COUNTS_ACTIVE: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "boot_tests")]
enum BootTestPidCountKind {
    Defer,
    Reclaim,
}

#[cfg(feature = "boot_tests")]
#[inline(always)]
fn record_boot_test_pid_count(pid: u64, kind: BootTestPidCountKind) {
    if BOOT_TEST_PID_COUNTS_ACTIVE.load(Ordering::Acquire) == 0 || pid == 0 {
        return;
    }

    let start = pid as usize & (BOOT_TEST_PID_COUNT_SLOTS - 1);
    for offset in 0..BOOT_TEST_PID_COUNT_SLOTS {
        let slot = &BOOT_TEST_PID_COUNTS[(start + offset) & (BOOT_TEST_PID_COUNT_SLOTS - 1)];
        let slot_pid = slot.pid.load(Ordering::Acquire);
        if slot_pid == pid {
            match kind {
                BootTestPidCountKind::Defer => {
                    slot.defer_count.fetch_add(1, Ordering::Relaxed);
                }
                BootTestPidCountKind::Reclaim => {
                    slot.reclaim_count.fetch_add(1, Ordering::Relaxed);
                }
            }
            return;
        }
        if slot_pid == 0 {
            return;
        }
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn reset_boot_test_pid_counts() {
    BOOT_TEST_PID_COUNTS_ACTIVE.store(0, Ordering::Release);
    for slot in &BOOT_TEST_PID_COUNTS {
        slot.pid.store(0, Ordering::Relaxed);
        slot.defer_count.store(0, Ordering::Relaxed);
        slot.reclaim_count.store(0, Ordering::Relaxed);
    }
    BOOT_TEST_PID_COUNTS_ACTIVE.store(1, Ordering::Release);
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn track_boot_test_pid(pid: u64) -> bool {
    if pid == 0 {
        return false;
    }

    let start = pid as usize & (BOOT_TEST_PID_COUNT_SLOTS - 1);
    for offset in 0..BOOT_TEST_PID_COUNT_SLOTS {
        let slot = &BOOT_TEST_PID_COUNTS[(start + offset) & (BOOT_TEST_PID_COUNT_SLOTS - 1)];
        match slot
            .pid
            .compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return true,
            Err(slot_pid) if slot_pid == pid => return true,
            Err(_) => {}
        }
    }
    false
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_test_pid_counts(pid: u64) -> (u64, u64) {
    let start = pid as usize & (BOOT_TEST_PID_COUNT_SLOTS - 1);
    for offset in 0..BOOT_TEST_PID_COUNT_SLOTS {
        let slot = &BOOT_TEST_PID_COUNTS[(start + offset) & (BOOT_TEST_PID_COUNT_SLOTS - 1)];
        let slot_pid = slot.pid.load(Ordering::Acquire);
        if slot_pid == pid {
            return (
                slot.defer_count.load(Ordering::Relaxed),
                slot.reclaim_count.load(Ordering::Relaxed),
            );
        }
        if slot_pid == 0 {
            break;
        }
    }
    (0, 0)
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_test_pid_counts_complete(pids: &[u64]) -> bool {
    pids.iter().all(|pid| {
        let (defer_count, reclaim_count) = boot_test_pid_counts(*pid);
        defer_count >= 1 && defer_count == reclaim_count
    })
}

#[inline(always)]
pub fn record_defer(pid: u64) {
    crate::trace_count!(TEARDOWN_DEFER);
    crate::trace_event!(TEARDOWN_PROVIDER, TEARDOWN_DEFER_EVENT, pid as u32);
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::Defer);
}

#[inline(always)]
pub fn record_reclaim(pid: u64) {
    crate::trace_count!(TEARDOWN_RECLAIM);
    crate::trace_event!(TEARDOWN_PROVIDER, TEARDOWN_RECLAIM_EVENT, pid as u32);
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::Reclaim);
}

#[inline(always)]
pub fn record_exit_request(already_terminated: bool) {
    if already_terminated {
        crate::trace_count!(EXIT_REPEAT_REQUESTS);
    } else {
        crate::trace_count!(EXIT_FIRST_REQUESTS);
    }
}

static RECLAIM_PROOF_DEPTH: [AtomicU64; crate::tracing::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::tracing::MAX_CPUS];
static SCHEDULER_SCOPE_DEPTH: [AtomicU64; crate::tracing::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::tracing::MAX_CPUS];

#[inline(always)]
fn current_cpu() -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::arch_impl::current::percpu::X86PerCpu;
        use crate::arch_impl::PerCpuOps;
        X86PerCpu::cpu_id() as usize
    }
    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::current::percpu::Aarch64PerCpu;
        use crate::arch_impl::PerCpuOps;
        Aarch64PerCpu::cpu_id() as usize
    }
}

pub struct ReclaimProofScope {
    cpu: usize,
}

impl ReclaimProofScope {
    #[inline(always)]
    pub fn enter() -> Self {
        let cpu = current_cpu().min(crate::tracing::MAX_CPUS - 1);
        RECLAIM_PROOF_DEPTH[cpu].fetch_add(1, Ordering::Relaxed);
        Self { cpu }
    }
}

impl Drop for ReclaimProofScope {
    #[inline(always)]
    fn drop(&mut self) {
        RECLAIM_PROOF_DEPTH[self.cpu].fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct SchedulerScope {
    cpu: usize,
}

impl SchedulerScope {
    #[inline(always)]
    pub fn enter() -> Self {
        let cpu = current_cpu().min(crate::tracing::MAX_CPUS - 1);
        SCHEDULER_SCOPE_DEPTH[cpu].fetch_add(1, Ordering::Relaxed);
        Self { cpu }
    }
}

impl Drop for SchedulerScope {
    #[inline(always)]
    fn drop(&mut self) {
        SCHEDULER_SCOPE_DEPTH[self.cpu].fetch_sub(1, Ordering::Relaxed);
    }
}

#[inline(always)]
pub fn note_process_manager_acquire() {
    let cpu = current_cpu().min(crate::tracing::MAX_CPUS - 1);
    if RECLAIM_PROOF_DEPTH[cpu].load(Ordering::Relaxed) != 0 {
        crate::trace_count!(PROOF_UNDER_QUEUE_LOCK);
    }
}

#[inline(always)]
pub fn note_scheduler_acquire() {
    let cpu = current_cpu().min(crate::tracing::MAX_CPUS - 1);
    if RECLAIM_PROOF_DEPTH[cpu].load(Ordering::Relaxed) != 0 {
        crate::trace_count!(PROOF_UNDER_QUEUE_LOCK);
    }
}

#[inline(always)]
pub fn scheduler_scope_active() -> bool {
    SCHEDULER_SCOPE_DEPTH[current_cpu().min(crate::tracing::MAX_CPUS - 1)].load(Ordering::Relaxed)
        != 0
}

#[cfg(feature = "boot_tests")]
pub fn deferred_fault_ring_overflow_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;

    let dropped_before = DEFERRED_FAULT_RING_DROPPED.aggregate();
    let ring_behaved_as_expected =
        crate::task::process_task::deferred_fault_ring_overflow_injection();
    let dropped_delta = DEFERRED_FAULT_RING_DROPPED
        .aggregate()
        .saturating_sub(dropped_before);

    if ring_behaved_as_expected && dropped_delta >= 1 {
        TestResult::Pass
    } else {
        TestResult::Fail("deferred fault ring overflow was not counted and drained")
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn fork_exit_defer_reclaim_pairing_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;

    let teardown_entry_exit_before = TEARDOWN_ENTRY_EXIT.aggregate();
    let exit_first_requests_before = EXIT_FIRST_REQUESTS.aggregate();
    let exit_repeat_requests_before = EXIT_REPEAT_REQUESTS.aggregate();
    let masked_frames_walked_before = TEARDOWN_MASKED_FRAMES_WALKED.aggregate();
    let fd_closes_under_pm_before = FD_CLOSES_UNDER_PM.aggregate();
    let reclaim_enqueue_under_pm_before = RECLAIM_ENQUEUE_UNDER_PM.aggregate();
    let lock_order_suspect_before = TEARDOWN_LOCK_ORDER_SUSPECT.aggregate();
    let proof_under_queue_lock_before = PROOF_UNDER_QUEUE_LOCK.aggregate();
    let reclaim_context_violations_before = RECLAIM_CONTEXT_VIOLATIONS.aggregate();

    let parent_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
        Ok(page_table) => alloc::boxed::Box::new(page_table),
        Err(_) => return TestResult::Fail("parent page-table allocation failed"),
    };
    let parent_pid = {
        let manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_ref() else {
            return TestResult::Fail("process manager unavailable for parent PID");
        };
        manager.allocate_pid()
    };
    fn test_user_entry() {}
    let entry = crate::memory::arch_stub::VirtAddr::new(0x0040_0000);
    let stack_top = crate::memory::arch_stub::VirtAddr::new(0x0080_0000);
    let stack_bottom = crate::memory::arch_stub::VirtAddr::new(0x007f_0000);
    let tls = crate::memory::arch_stub::VirtAddr::new(0x0001_0000);
    let mut parent_process = crate::process::Process::new(
        parent_pid,
        alloc::string::String::from("teardown_pairing_parent"),
        entry,
    );
    let mut parent_thread = crate::task::thread::Thread::new(
        alloc::string::String::from("teardown_pairing_parent_main"),
        test_user_entry,
        stack_top,
        stack_bottom,
        tls,
        crate::task::thread::ThreadPrivilege::User,
    );
    parent_thread.owner_pid = Some(parent_pid.as_u64());
    let parent_context = parent_thread.context.clone();
    parent_process.page_table = Some(parent_page_table);
    parent_process.set_main_thread(parent_thread);
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for parent insert");
        };
        manager.insert_process(parent_pid, parent_process);
    };

    let mut pairing_child_pids = [0u64; 64];
    let mut pairing_child_count = 0;
    reset_boot_test_pid_counts();
    for _ in 0..64 {
        let child_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
            Ok(page_table) => alloc::boxed::Box::new(page_table),
            Err(_) => return TestResult::Fail("pairing fork page-table allocation failed"),
        };
        let child = {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail("process manager unavailable during pairing fork");
            };
            let child_pid = match manager.fork_process_aarch64(
                parent_pid,
                parent_context.clone(),
                child_page_table,
            ) {
                Ok(pid) => pid,
                Err(_) => return TestResult::Fail("pairing fork failed"),
            };
            let Some(child_tid) = manager
                .get_process(child_pid)
                .and_then(|process| process.main_thread.as_ref())
                .map(|thread| thread.id)
            else {
                return TestResult::Fail("pairing child has no main thread");
            };
            (child_pid, child_tid)
        };

        pairing_child_pids[pairing_child_count] = child.0.as_u64();
        if !track_boot_test_pid(pairing_child_pids[pairing_child_count]) {
            return TestResult::Fail("per-PID pairing tally table capacity exhausted");
        }
        pairing_child_count += 1;

        crate::process::exit_process_for_teardown_test(child.0, 0);
        crate::task::process_task::ProcessScheduler::handle_thread_exit(child.1, 0);
        {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail("process manager unavailable during pairing reap");
            };
            manager.remove_process(child.0);
            if let Some(parent) = manager.get_process_mut(parent_pid) {
                parent.children.retain(|pid| *pid != child.0);
            }
        }
    }

    // Exercise today's immediate-release path as part of the same explained
    // workload so the three known-under-PM baseline defects cannot pass at zero.
    let immediate_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
        Ok(page_table) => alloc::boxed::Box::new(page_table),
        Err(_) => return TestResult::Fail("baseline fork page-table allocation failed"),
    };
    let immediate = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable during baseline fork");
        };
        let child_pid =
            match manager.fork_process_aarch64(parent_pid, parent_context, immediate_page_table) {
                Ok(pid) => pid,
                Err(_) => return TestResult::Fail("baseline fork failed"),
            };
        let Some(child_tid) = manager
            .get_process(child_pid)
            .and_then(|process| process.main_thread.as_ref())
            .map(|thread| thread.id)
        else {
            return TestResult::Fail("baseline child has no main thread");
        };
        (child_pid, child_tid)
    };
    crate::task::process_task::ProcessScheduler::handle_thread_exit(immediate.1, 0);
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable during baseline reap");
        };
        manager.remove_process(immediate.0);
        if let Some(parent) = manager.get_process_mut(parent_pid) {
            parent.children.retain(|pid| *pid != immediate.0);
        }
    }

    let timer_frequency = crate::arch_impl::aarch64::timer::frequency_hz();
    let quiesce_deadline =
        crate::arch_impl::aarch64::timer::rdtsc().saturating_add(timer_frequency.saturating_mul(5));
    loop {
        crate::task::scheduler::nudge_retirement_grace_for_test();
        let boundary_deadline =
            crate::arch_impl::aarch64::timer::rdtsc().saturating_add(timer_frequency / 1000);
        while crate::arch_impl::aarch64::timer::rdtsc() < boundary_deadline {
            core::hint::spin_loop();
        }
        crate::task::process_task::reclaim_deferred_process_resources();
        if boot_test_pid_counts_complete(&pairing_child_pids) {
            break;
        }
        if crate::arch_impl::aarch64::timer::rdtsc() >= quiesce_deadline {
            break;
        }
        core::hint::spin_loop();
    }

    let teardown_entry_exit_delta = TEARDOWN_ENTRY_EXIT
        .aggregate()
        .saturating_sub(teardown_entry_exit_before);
    let exit_first_requests_delta = EXIT_FIRST_REQUESTS
        .aggregate()
        .saturating_sub(exit_first_requests_before);
    let exit_repeat_requests_delta = EXIT_REPEAT_REQUESTS
        .aggregate()
        .saturating_sub(exit_repeat_requests_before);
    if teardown_entry_exit_delta < 64 {
        return TestResult::Fail("TEARDOWN_ENTRY_EXIT workload delta did not reach 64");
    }
    if exit_first_requests_delta < 64 || exit_repeat_requests_delta < 64 {
        return TestResult::Fail("first/repeat exit request workload deltas did not reach 64");
    }
    for pid in pairing_child_pids {
        let (defer_count, reclaim_count) = boot_test_pid_counts(pid);
        if defer_count == 0 || defer_count != reclaim_count {
            return TestResult::Fail(
                "per-PID defer/reclaim count pairing failed for a pairing-test child",
            );
        }
    }
    BOOT_TEST_PID_COUNTS_ACTIVE.store(0, Ordering::Release);
    if TEARDOWN_MASKED_FRAMES_WALKED
        .aggregate()
        .saturating_sub(masked_frames_walked_before)
        == 0
        || FD_CLOSES_UNDER_PM
            .aggregate()
            .saturating_sub(fd_closes_under_pm_before)
            == 0
        || RECLAIM_ENQUEUE_UNDER_PM
            .aggregate()
            .saturating_sub(reclaim_enqueue_under_pm_before)
            == 0
    {
        return TestResult::Fail("expected Phase-0 under-PM baseline remained zero");
    }
    if TEARDOWN_LOCK_ORDER_SUSPECT
        .aggregate()
        .saturating_sub(lock_order_suspect_before)
        != 0
        || PROOF_UNDER_QUEUE_LOCK
            .aggregate()
            .saturating_sub(proof_under_queue_lock_before)
            != 0
        || RECLAIM_CONTEXT_VIOLATIONS
            .aggregate()
            .saturating_sub(reclaim_context_violations_before)
            != 0
    {
        return TestResult::Fail("healthy teardown lock-context counter moved");
    }
    let _all_counter_readers = snapshot();

    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for parent cleanup");
        };
        if let Some(parent) = manager.get_process_mut(parent_pid) {
            crate::task::process_task::release_process_resources(parent);
        }
        manager.remove_process(parent_pid);
    }

    TestResult::Pass
}
