//! Lock-free teardown observability.
//!
//! Phase 0 established the provider and Phase 1 adds retirement-proof
//! producers. Counters owned by later phases remain registered and readable
//! here without an increment site yet.

use crate::tracing::counter::{register_counter, TraceCounter};
use crate::tracing::provider::{register_provider, TraceProvider};
use core::sync::atomic::{AtomicU64, Ordering};

pub const PROVIDER_ID: u8 = 0x0a;
pub const TEARDOWN_DEFER_EVENT: u16 = ((PROVIDER_ID as u16) << 8) | 0x00;
pub const TEARDOWN_RECLAIM_EVENT: u16 = ((PROVIDER_ID as u16) << 8) | 0x01;
pub const EXIT_SGI_SENT_EVENT: u16 = ((PROVIDER_ID as u16) << 8) | 0x02;
pub const EXIT_SGI_BATCH_EVENT: u16 = ((PROVIDER_ID as u16) << 8) | 0x03;
pub const EXIT_KICK_OBSERVED_EVENT: u16 = ((PROVIDER_ID as u16) << 8) | 0x04;

pub(crate) const EXIT_KICK_BUCKETS: usize = 64;
pub(crate) const EXIT_KICK_LOCK: u64 = 0b10;
pub(crate) const EXIT_KICK_OBSERVED_BIT: u64 = 0b01;
pub(crate) const KICK_RESERVE_ATTEMPTS: usize = 4;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static EXIT_KICK_TEST_HOOK_PID: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static EXIT_KICK_TEST_HOOK_RESERVED: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static EXIT_KICK_TEST_HOOK_RELEASE: AtomicU64 = AtomicU64::new(1);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static EXIT_KICK_TEST_HOOK_CPU: AtomicU64 = AtomicU64::new(u64::MAX);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static PROBE_DISPATCHED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static PROBE_ROOT_RELEASE_OBSERVATION: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const PROBE_ROOT_RELEASE_DONE: u64 = 1 << 0;
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const PROBE_ROOT_HARDWARE_CLEAR: u64 = 1 << 1;
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const PROBE_ROOT_SHADOW_CLEAR: u64 = 1 << 2;
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const PROBE_ROOT_CACHED_CLEAR: u64 = 1 << 3;

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
static X86_CREATING_DISPATCH_PROBE_DISPATCHED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
static X86_CREATING_DISPATCH_ROOT_OBSERVATION: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
const X86_CREATING_DISPATCH_ROOT_RELEASE_DONE: u64 = 1 << 0;
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
const X86_CREATING_DISPATCH_ROOT_HARDWARE_CLEAR: u64 = 1 << 1;
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
const X86_CREATING_DISPATCH_ROOT_SHADOW_CLEAR: u64 = 1 << 2;
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
const X86_CREATING_DISPATCH_ROOT_CACHED_CLEAR: u64 = 1 << 3;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
extern "C" fn creating_dispatch_probe_entry(thread_id: u64, root: u64) -> ! {
    crate::per_cpu_aarch64::preempt_disable();
    PROBE_DISPATCHED.store(true, Ordering::Release);
    let (hardware_clear, shadow_clear, cached_clear) =
        crate::arch_impl::aarch64::context_switch::quiesce_probe_ttbr0_for_test(thread_id, root);
    let observation = PROBE_ROOT_RELEASE_DONE
        | if hardware_clear {
            PROBE_ROOT_HARDWARE_CLEAR
        } else {
            0
        }
        | if shadow_clear {
            PROBE_ROOT_SHADOW_CLEAR
        } else {
            0
        }
        | if cached_clear {
            PROBE_ROOT_CACHED_CLEAR
        } else {
            0
        };
    PROBE_ROOT_RELEASE_OBSERVATION.store(observation, Ordering::Release);
    crate::arch_impl::aarch64::context_switch::schedule_terminated_from_exit(thread_id)
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
extern "C" fn creating_dispatch_probe_entry_x86(thread_id: u64, root: u64) -> ! {
    unsafe {
        crate::arch_disable_interrupts();
        crate::memory::process_memory::switch_to_kernel_page_table();
    }
    crate::per_cpu::set_next_cr3(0);
    crate::per_cpu::set_saved_process_cr3(0);

    let hardware_clear = x86_64::registers::control::Cr3::read()
        .0
        .start_address()
        .as_u64()
        != root;
    let shadow_clear = crate::per_cpu::get_next_cr3() != root
        && crate::per_cpu::get_saved_process_cr3() != root;
    let cached_clear = crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        thread.set_terminated();
        thread.cached_ttbr0 != root
    })
    .unwrap_or(false);
    let observation = X86_CREATING_DISPATCH_ROOT_RELEASE_DONE
        | if hardware_clear {
            X86_CREATING_DISPATCH_ROOT_HARDWARE_CLEAR
        } else {
            0
        }
        | if shadow_clear {
            X86_CREATING_DISPATCH_ROOT_SHADOW_CLEAR
        } else {
            0
        }
        | if cached_clear {
            X86_CREATING_DISPATCH_ROOT_CACHED_CLEAR
        } else {
            0
        };
    X86_CREATING_DISPATCH_ROOT_OBSERVATION.store(observation, Ordering::Release);
    X86_CREATING_DISPATCH_PROBE_DISPATCHED.store(true, Ordering::Release);
    crate::task::scheduler::set_need_resched();

    loop {
        crate::arch_halt_with_interrupts();
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const KTHREAD_EXIT_PROGRESS_SLOT_COUNT: usize = 64;
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const _: () = assert!(KTHREAD_EXIT_PROGRESS_SLOT_COUNT.is_power_of_two());

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
struct KthreadExitProgressSlot {
    tid: AtomicU64,
    steps: AtomicU64,
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl KthreadExitProgressSlot {
    const fn new() -> Self {
        Self {
            tid: AtomicU64::new(0),
            steps: AtomicU64::new(0),
        }
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static KTHREAD_EXIT_PROGRESS_ACTIVE: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static KTHREAD_EXIT_PROGRESS: [KthreadExitProgressSlot; KTHREAD_EXIT_PROGRESS_SLOT_COUNT] =
    [const { KthreadExitProgressSlot::new() }; KTHREAD_EXIT_PROGRESS_SLOT_COUNT];

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
struct KthreadExitProgressGuard;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl KthreadExitProgressGuard {
    fn arm() -> Self {
        // The gate runs once per boot and thread IDs are never reused. Keep the
        // table insertion-only instead of clearing it: a recorder that observed
        // ACTIVE=1 just before a prior guard dropped can never race a reset and
        // write a stale step into a newly assigned slot.
        KTHREAD_EXIT_PROGRESS_ACTIVE.store(1, Ordering::Release);
        Self
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl Drop for KthreadExitProgressGuard {
    fn drop(&mut self) {
        KTHREAD_EXIT_PROGRESS_ACTIVE.store(0, Ordering::Release);
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn kthread_exit_progress_slot(tid: u64, create: bool) -> Option<&'static KthreadExitProgressSlot> {
    if tid == 0 || KTHREAD_EXIT_PROGRESS_ACTIVE.load(Ordering::Acquire) == 0 {
        return None;
    }

    let start = tid as usize & (KTHREAD_EXIT_PROGRESS_SLOT_COUNT - 1);
    for offset in 0..KTHREAD_EXIT_PROGRESS_SLOT_COUNT {
        let slot = &KTHREAD_EXIT_PROGRESS
            [(start.wrapping_add(offset)) & (KTHREAD_EXIT_PROGRESS_SLOT_COUNT - 1)];
        let current = slot.tid.load(Ordering::Acquire);
        if current == tid {
            return Some(slot);
        }
        if current == 0 && !create {
            // Slots are insertion-only, so an empty slot terminates this probe.
            return None;
        }
        if current == 0 {
            match slot
                .tid
                .compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Some(slot),
                Err(existing) if existing == tid => return Some(slot),
                Err(_) => {}
            }
        }
    }
    None
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn watch_kthread_exit_progress_for_test(tid: u64) -> bool {
    kthread_exit_progress_slot(tid, true).is_some()
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn kthread_exit_progress_for_test(tid: u64) -> u64 {
    kthread_exit_progress_slot(tid, false)
        .map(|slot| slot.steps.load(Ordering::Acquire))
        .unwrap_or(0)
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub(crate) fn record_kthread_exit_stage_for_test(tid: u64) {
    if let Some(slot) = kthread_exit_progress_slot(tid, false) {
        slot.steps.fetch_add(1, Ordering::Release);
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
struct ExitKickTestHookGuard;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl ExitKickTestHookGuard {
    fn arm(pid: u64) -> Self {
        EXIT_KICK_TEST_HOOK_CPU.store(u64::MAX, Ordering::Relaxed);
        EXIT_KICK_TEST_HOOK_RESERVED.store(0, Ordering::Relaxed);
        EXIT_KICK_TEST_HOOK_RELEASE.store(0, Ordering::Relaxed);
        EXIT_KICK_TEST_HOOK_PID.store(pid, Ordering::Release);
        Self
    }

    fn release(&self) {
        EXIT_KICK_TEST_HOOK_RELEASE.store(1, Ordering::Release);
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl Drop for ExitKickTestHookGuard {
    fn drop(&mut self) {
        // A failed assertion or early return must never strand the publisher
        // in the synthetic reserve-to-commit hold point.
        EXIT_KICK_TEST_HOOK_RELEASE.store(1, Ordering::Release);
        EXIT_KICK_TEST_HOOK_PID.store(0, Ordering::Release);
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
#[inline(always)]
fn hold_exit_kick_test_publisher(pid: u64) {
    if EXIT_KICK_TEST_HOOK_PID.load(Ordering::Acquire) != pid {
        return;
    }
    EXIT_KICK_TEST_HOOK_CPU.store(
        crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as u64,
        Ordering::Relaxed,
    );
    EXIT_KICK_TEST_HOOK_RESERVED.store(1, Ordering::Release);
    while EXIT_KICK_TEST_HOOK_RELEASE.load(Ordering::Acquire) == 0 {
        core::hint::spin_loop();
    }
}

pub(crate) struct KickSlot {
    pub(crate) pid: AtomicU64,
    pub(crate) at: AtomicU64,
    pub(crate) state: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KickPublishResult {
    Published { generation: u64, displaced: bool },
    ReservationLost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KickObservation {
    pub(crate) generation: u64,
    pub(crate) pid: u64,
    pub(crate) at: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct KickReservation {
    displaced_state: u64,
    generation: u64,
}

impl KickSlot {
    const fn new() -> Self {
        Self {
            pid: AtomicU64::new(0),
            at: AtomicU64::new(0),
            state: AtomicU64::new(0),
        }
    }

    #[inline(always)]
    pub(crate) fn reserve(&self) -> Option<KickReservation> {
        let mut current = self.state.load(Ordering::Relaxed);
        for _ in 0..KICK_RESERVE_ATTEMPTS {
            if current & EXIT_KICK_LOCK != 0 {
                return None;
            }
            let generation = (current >> 2).wrapping_add(1);
            let reserved_state = (generation << 2) | EXIT_KICK_LOCK;
            match self.state.compare_exchange_weak(
                current,
                reserved_state,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(KickReservation {
                        displaced_state: current,
                        generation,
                    });
                }
                Err(actual) => current = actual,
            }
        }
        None
    }

    #[inline(always)]
    pub(crate) fn commit(
        &self,
        reservation: KickReservation,
        pid: u64,
        at: u64,
    ) -> KickPublishResult {
        let displaced = reservation.displaced_state >> 2 != 0
            && reservation.displaced_state & EXIT_KICK_OBSERVED_BIT == 0
            && self.pid.load(Ordering::Relaxed) != pid;
        self.pid.store(pid, Ordering::Relaxed);
        self.at.store(at, Ordering::Relaxed);
        self.state
            .store(reservation.generation << 2, Ordering::Release);
        KickPublishResult::Published {
            generation: reservation.generation,
            displaced,
        }
    }

    #[inline(always)]
    pub(crate) fn publish(&self, pid: u64, at: u64) -> KickPublishResult {
        match self.reserve() {
            Some(reservation) => {
                #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
                hold_exit_kick_test_publisher(pid);
                self.commit(reservation, pid, at)
            }
            None => KickPublishResult::ReservationLost,
        }
    }

    #[inline(always)]
    pub(crate) fn observe(&self, expected_pid: u64) -> Option<KickObservation> {
        let first_state = self.state.load(Ordering::Acquire);
        if first_state >> 2 == 0 || first_state & (EXIT_KICK_LOCK | EXIT_KICK_OBSERVED_BIT) != 0 {
            return None;
        }

        let pid = self.pid.load(Ordering::Relaxed);
        let at = self.at.load(Ordering::Relaxed);
        let second_state = self.state.load(Ordering::Acquire);
        if second_state != first_state || pid != expected_pid {
            return None;
        }

        self.state
            .compare_exchange(
                first_state,
                first_state | EXIT_KICK_OBSERVED_BIT,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .ok()
            .map(|_| KickObservation {
                generation: first_state >> 2,
                pid,
                at,
            })
    }

    #[inline(always)]
    pub(crate) fn is_observed_for(&self, expected_pid: u64) -> bool {
        let first_state = self.state.load(Ordering::Acquire);
        if first_state >> 2 == 0
            || first_state & EXIT_KICK_LOCK != 0
            || first_state & EXIT_KICK_OBSERVED_BIT == 0
        {
            return false;
        }
        let pid = self.pid.load(Ordering::Relaxed);
        let second_state = self.state.load(Ordering::Acquire);
        second_state == first_state && pid == expected_pid
    }
}

pub(crate) static EXIT_KICK_SLOTS: [KickSlot; EXIT_KICK_BUCKETS] =
    [const { KickSlot::new() }; EXIT_KICK_BUCKETS];

static EXIT_KICK_BUCKET_PUBLISH_COUNTS: [AtomicU64; EXIT_KICK_BUCKETS] =
    [const { AtomicU64::new(0) }; EXIT_KICK_BUCKETS];
static EXIT_KICK_BUCKET_OBSERVED_COUNTS: [AtomicU64; EXIT_KICK_BUCKETS] =
    [const { AtomicU64::new(0) }; EXIT_KICK_BUCKETS];
static EXIT_KICK_BUCKET_COLLISION_COUNTS: [AtomicU64; EXIT_KICK_BUCKETS] =
    [const { AtomicU64::new(0) }; EXIT_KICK_BUCKETS];

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
    RECLAIM_DRAIN_NESTED_REFUSED,
    "Reclaim calls refused while another drain owns the queues"
);
counter!(
    TEARDOWN_LOCK_ORDER_SUSPECT,
    "Suspect teardown lock ordering"
);
counter!(ROOT_PROOF_BLOCKED_EPOCH, "Retirements blocked by epoch");
counter!(
    ROOT_PROOF_BLOCKED_HW,
    "Retirements blocked by the local hardware root register"
);
counter!(
    ROOT_PROOF_BLOCKED_SHADOW,
    "Retirements blocked by TTBR0 shadows"
);
counter!(
    ROOT_PROOF_BLOCKED_CACHED,
    "Retirements blocked by scheduler roots"
);
counter!(
    ROOT_PROOF_BLOCKED_LIVE_ROW,
    "Retirements blocked by live process rows"
);
counter!(
    RETIRE_EMPTY_ONLINE_MASK,
    "Retirement fences refused for empty online masks"
);
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
counter!(EXIT_SGI_SENT, "Teardown-attributed expedite SGIs");
counter!(EXIT_KICK_PUBLISHED, "Published exit-kick buckets");
counter!(EXIT_KICK_OBSERVED, "Observed exit-kick victims");
counter!(EXIT_KICK_BUCKET_COLLISION, "Exit-kick bucket collisions");
counter!(RECEIPT_DROPPED_UNRETIRED, "Receipts recovered by Drop");
counter!(LEDGER_CLAIM_MISMATCH, "Exit-obligation claimer mismatches");
counter!(LEDGER_CLAIM_ORPHANED, "Recovered orphaned exit claims");
counter!(FRAME_RETURN_REFUSED_DOUBLE, "Double frame returns refused");
// Production deallocation synthesizes authority from the generation it just
// read. A stale mismatch therefore requires a double-free racing with reuse;
// the boot gate is the deterministic PR-1a exerciser.
counter!(FRAME_RETURN_REFUSED_STALE, "Stale frame returns refused");
counter!(
    FRAME_RETURN_REFUSED_NEVER_ALLOCATED,
    "Never-allocated frame returns refused"
);
counter!(
    FRAME_RETURN_REFUSED_UNTRACKED,
    "Untracked frame returns refused"
);
counter!(
    FRAME_DUPLICATE_ALLOC_REFUSED,
    "Duplicate frame allocations refused"
);
counter!(FRAME_LOST_CONTENDED, "Frame returns lost to contention");
counter!(
    FRAME_RETURN_REFUSED_LIVE_LEAF,
    "Returns of frames with live leaf mappings refused"
);
counter!(LEAF_MAPPINGS_RECORDED, "Process leaf mappings recorded");
counter!(LEAF_MAPPINGS_RELEASED, "Process leaf mappings released");
counter!(LEAF_FRAMES_RETURNED, "Process leaf frames returned");
counter!(
    LEAF_DECREF_UNREGISTERED,
    "Unregistered process leaf decrements refused"
);
counter!(
    LEAF_CUSTODY_REFUSED,
    "Process leaf custody classifications refused"
);
counter!(PT_TABLE_FRAMES_RECORDED, "Process table frames recorded");
counter!(
    PT_ROOT_ABANDONED_NO_PROOF,
    "Process roots abandoned without a retirement pipeline"
);
counter!(
    PT_ROOT_ABANDONED_NO_ARCH,
    "Process roots abandoned without an architecture pipeline"
);
counter!(
    PT_ROOT_ABANDONED_TERMINATED,
    "Process roots abandoned after prior termination"
);
counter!(
    PT_ROOT_DROPPED_UNDECIDED,
    "Process roots dropped without a disposition"
);
counter!(PT_ROOTS_RETIRED, "Process roots retired after proof");
counter!(
    PT_TABLE_FRAMES_RETURNED,
    "Recorded process table frames returned"
);
counter!(
    PT_RETIRE_FRAMES_LOST,
    "Process table retirement frames lost to contention"
);
counter!(
    PT_ROOT_DROPPED_MID_RETIRE,
    "Process roots dropped during bounded retirement"
);
counter!(
    PT_RETIRE_BUDGET_REQUEUED,
    "Process table retirements requeued at the frame budget"
);
counter!(
    PT_ROOT_SLOT_REFUSED,
    "Mappings refused into inherited root slots"
);
counter!(PT_SHADOW_ROOT_CLEARED, "Saved x86 process roots cleared");
counter!(CLONE_ADMISSION_ADMITTED, "Clone admissions accepted");
counter!(CLONE_ADMISSION_REFUSED, "Clone admissions refused");

// Declaration-only until the phase named in PLAN.md. These intentionally have
// no trace_count! producer yet.
counter!(TEARDOWN_ENTRY_GROUP, "Group teardown entries");
counter!(EXIT_REQUEST_OBSERVED, "Observed latched exit requests");
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

pub const COUNTER_COUNT: usize = 74;

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
    &RECLAIM_DRAIN_NESTED_REFUSED,
    &TEARDOWN_LOCK_ORDER_SUSPECT,
    &ROOT_PROOF_BLOCKED_EPOCH,
    &ROOT_PROOF_BLOCKED_HW,
    &ROOT_PROOF_BLOCKED_SHADOW,
    &ROOT_PROOF_BLOCKED_CACHED,
    &ROOT_PROOF_BLOCKED_LIVE_ROW,
    &RETIRE_EMPTY_ONLINE_MASK,
    &EXIT_SGI_SENT,
    &EXIT_REQUEST_OBSERVED,
    &EXIT_KICK_PUBLISHED,
    &EXIT_KICK_OBSERVED,
    &EXIT_KICK_BUCKET_COLLISION,
    &RECEIPT_DROPPED_UNRETIRED,
    &LEDGER_CLAIM_MISMATCH,
    &LEDGER_CLAIM_ORPHANED,
    &FRAME_RETURN_REFUSED_DOUBLE,
    &FRAME_RETURN_REFUSED_STALE,
    &FRAME_RETURN_REFUSED_NEVER_ALLOCATED,
    &FRAME_RETURN_REFUSED_UNTRACKED,
    &FRAME_DUPLICATE_ALLOC_REFUSED,
    &FRAME_LOST_CONTENDED,
    &FRAME_RETURN_REFUSED_LIVE_LEAF,
    &LEAF_MAPPINGS_RECORDED,
    &LEAF_MAPPINGS_RELEASED,
    &LEAF_FRAMES_RETURNED,
    &LEAF_DECREF_UNREGISTERED,
    &LEAF_CUSTODY_REFUSED,
    &PT_TABLE_FRAMES_RECORDED,
    &PT_ROOT_ABANDONED_NO_PROOF,
    &PT_ROOT_ABANDONED_NO_ARCH,
    &PT_ROOT_ABANDONED_TERMINATED,
    &PT_ROOT_DROPPED_UNDECIDED,
    &PT_ROOTS_RETIRED,
    &PT_TABLE_FRAMES_RETURNED,
    &PT_RETIRE_FRAMES_LOST,
    &PT_ROOT_DROPPED_MID_RETIRE,
    &PT_RETIRE_BUDGET_REQUEUED,
    &PT_ROOT_SLOT_REFUSED,
    &PT_SHADOW_ROOT_CLEARED,
    &CLONE_ADMISSION_ADMITTED,
    &CLONE_ADMISSION_REFUSED,
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

/// Emit the root-disposition counters from normal context.
///
/// The production userspace heartbeat reads `/proc/trace/counters`, so that
/// cold procfs path repeats this line as the hardware run progresses. Both
/// architecture boot paths also emit it once before launching userspace.
pub fn emit_root_custody_summary() {
    crate::serial_println!(
        "[PT_ROOT_CUSTODY:no_proof={}:no_arch={}:terminated={}:undecided={}:mid_retire={}:retired={}]",
        PT_ROOT_ABANDONED_NO_PROOF.aggregate(),
        PT_ROOT_ABANDONED_NO_ARCH.aggregate(),
        PT_ROOT_ABANDONED_TERMINATED.aggregate(),
        PT_ROOT_DROPPED_UNDECIDED.aggregate(),
        PT_ROOT_DROPPED_MID_RETIRE.aggregate(),
        PT_ROOTS_RETIRED.aggregate()
    );
}

#[cfg(feature = "boot_tests")]
const BOOT_TEST_PID_COUNT_SLOTS: usize = 256;

#[cfg(feature = "boot_tests")]
struct BootTestPidCountSlot {
    pid: AtomicU64,
    defer_count: AtomicU64,
    reclaim_count: AtomicU64,
    quarantine_count: AtomicU64,
    sgi_sent_count: AtomicU64,
    kick_observed_count: AtomicU64,
    kick_observed_interval: AtomicU64,
    masked_frames_walked: AtomicU64,
    report_count: AtomicU64,
    table_frames_recorded: AtomicU64,
    table_frames_returned: AtomicU64,
    table_frames_lost: AtomicU64,
    roots_retired: AtomicU64,
}

#[cfg(feature = "boot_tests")]
impl BootTestPidCountSlot {
    const fn new() -> Self {
        Self {
            pid: AtomicU64::new(0),
            defer_count: AtomicU64::new(0),
            reclaim_count: AtomicU64::new(0),
            quarantine_count: AtomicU64::new(0),
            sgi_sent_count: AtomicU64::new(0),
            kick_observed_count: AtomicU64::new(0),
            kick_observed_interval: AtomicU64::new(0),
            masked_frames_walked: AtomicU64::new(0),
            report_count: AtomicU64::new(0),
            table_frames_recorded: AtomicU64::new(0),
            table_frames_returned: AtomicU64::new(0),
            table_frames_lost: AtomicU64::new(0),
            roots_retired: AtomicU64::new(0),
        }
    }
}

#[cfg(feature = "boot_tests")]
static BOOT_TEST_PID_COUNTS: [BootTestPidCountSlot; BOOT_TEST_PID_COUNT_SLOTS] =
    [const { BootTestPidCountSlot::new() }; BOOT_TEST_PID_COUNT_SLOTS];

#[cfg(feature = "boot_tests")]
static BOOT_TEST_PID_COUNTS_ACTIVE: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "boot_tests")]
struct BootTestPidCountsGuard;

#[cfg(feature = "boot_tests")]
impl Drop for BootTestPidCountsGuard {
    fn drop(&mut self) {
        BOOT_TEST_PID_COUNTS_ACTIVE.store(0, Ordering::Release);
    }
}

#[cfg(feature = "boot_tests")]
enum BootTestPidCountKind {
    Defer,
    Reclaim,
    Quarantine,
    SgiSent,
    KickObserved(u64),
    MaskedFramesWalked,
    Report,
    TableFramesRecorded(u64),
    TableFrameReturned,
    TableFrameLost,
    RootRetired,
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
                BootTestPidCountKind::Quarantine => {
                    slot.quarantine_count.fetch_add(1, Ordering::Relaxed);
                }
                BootTestPidCountKind::SgiSent => {
                    slot.sgi_sent_count.fetch_add(1, Ordering::Relaxed);
                }
                BootTestPidCountKind::KickObserved(interval) => {
                    slot.kick_observed_interval
                        .store(interval, Ordering::Relaxed);
                    slot.kick_observed_count.fetch_add(1, Ordering::Release);
                }
                BootTestPidCountKind::MaskedFramesWalked => {
                    slot.masked_frames_walked.fetch_add(1, Ordering::Relaxed);
                }
                BootTestPidCountKind::Report => {
                    slot.report_count.fetch_add(1, Ordering::Relaxed);
                }
                BootTestPidCountKind::TableFramesRecorded(count) => {
                    slot.table_frames_recorded
                        .fetch_add(count, Ordering::Relaxed);
                }
                BootTestPidCountKind::TableFrameReturned => {
                    slot.table_frames_returned.fetch_add(1, Ordering::Relaxed);
                }
                BootTestPidCountKind::TableFrameLost => {
                    slot.table_frames_lost.fetch_add(1, Ordering::Relaxed);
                }
                BootTestPidCountKind::RootRetired => {
                    slot.roots_retired.fetch_add(1, Ordering::Release);
                }
            }
            return;
        }
        if slot_pid == 0 {
            return;
        }
    }
}

#[cfg(feature = "boot_tests")]
fn reset_boot_test_pid_counts() -> BootTestPidCountsGuard {
    BOOT_TEST_PID_COUNTS_ACTIVE.store(0, Ordering::Release);
    for slot in &BOOT_TEST_PID_COUNTS {
        slot.pid.store(0, Ordering::Relaxed);
        slot.defer_count.store(0, Ordering::Relaxed);
        slot.reclaim_count.store(0, Ordering::Relaxed);
        slot.quarantine_count.store(0, Ordering::Relaxed);
        slot.sgi_sent_count.store(0, Ordering::Relaxed);
        slot.kick_observed_count.store(0, Ordering::Relaxed);
        slot.kick_observed_interval.store(0, Ordering::Relaxed);
        slot.masked_frames_walked.store(0, Ordering::Relaxed);
        slot.report_count.store(0, Ordering::Relaxed);
        slot.table_frames_recorded.store(0, Ordering::Relaxed);
        slot.table_frames_returned.store(0, Ordering::Relaxed);
        slot.table_frames_lost.store(0, Ordering::Relaxed);
        slot.roots_retired.store(0, Ordering::Relaxed);
    }
    BOOT_TEST_PID_COUNTS_ACTIVE.store(1, Ordering::Release);
    BootTestPidCountsGuard
}

#[cfg(feature = "boot_tests")]
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

#[cfg(feature = "boot_tests")]
#[derive(Clone, Copy, Default)]
struct BootTestPidCounts {
    defer_count: u64,
    reclaim_count: u64,
    table_frames_recorded: u64,
    table_frames_returned: u64,
    table_frames_lost: u64,
    roots_retired: u64,
}

#[cfg(feature = "boot_tests")]
fn boot_test_pid_counts(pid: u64) -> BootTestPidCounts {
    let start = pid as usize & (BOOT_TEST_PID_COUNT_SLOTS - 1);
    for offset in 0..BOOT_TEST_PID_COUNT_SLOTS {
        let slot = &BOOT_TEST_PID_COUNTS[(start + offset) & (BOOT_TEST_PID_COUNT_SLOTS - 1)];
        let slot_pid = slot.pid.load(Ordering::Acquire);
        if slot_pid == pid {
            return BootTestPidCounts {
                defer_count: slot.defer_count.load(Ordering::Relaxed),
                reclaim_count: slot.reclaim_count.load(Ordering::Relaxed),
                table_frames_recorded: slot.table_frames_recorded.load(Ordering::Relaxed),
                table_frames_returned: slot.table_frames_returned.load(Ordering::Relaxed),
                table_frames_lost: slot.table_frames_lost.load(Ordering::Relaxed),
                roots_retired: slot.roots_retired.load(Ordering::Relaxed),
            };
        }
        if slot_pid == 0 {
            break;
        }
    }
    BootTestPidCounts::default()
}

#[cfg(feature = "boot_tests")]
fn boot_test_pid_counts_complete(pids: &[u64]) -> bool {
    pids.iter().all(|pid| {
        let counts = boot_test_pid_counts(*pid);
        counts.defer_count >= 1 && counts.defer_count == counts.reclaim_count
    })
}

#[inline(always)]
pub fn record_pt_retire_started(pid: u64, table_frames: u64) {
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::TableFramesRecorded(table_frames));
    #[cfg(not(feature = "boot_tests"))]
    let _ = (pid, table_frames);
}

#[inline(always)]
pub fn record_pt_frame_returned(pid: u64) {
    crate::trace_count!(PT_TABLE_FRAMES_RETURNED);
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::TableFrameReturned);
    #[cfg(not(feature = "boot_tests"))]
    let _ = pid;
}

#[inline(always)]
pub fn record_pt_frame_lost(pid: u64) {
    crate::trace_count!(PT_RETIRE_FRAMES_LOST);
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::TableFrameLost);
    #[cfg(not(feature = "boot_tests"))]
    let _ = pid;
}

#[inline(always)]
pub fn record_pt_root_retired(pid: u64) {
    crate::trace_count!(PT_ROOTS_RETIRED);
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::RootRetired);
    #[cfg(not(feature = "boot_tests"))]
    let _ = pid;
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
pub fn record_quarantine(pid: u64) {
    crate::trace_count!(TEARDOWN_QUARANTINE);
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::Quarantine);
    #[cfg(not(feature = "boot_tests"))]
    let _ = pid;
}

#[inline(always)]
pub fn record_exit_sgi_sent(pid: u64, batch: u64) {
    crate::trace_event!(TEARDOWN_PROVIDER, EXIT_SGI_SENT_EVENT, pid as u32);
    crate::trace_event!(TEARDOWN_PROVIDER, EXIT_SGI_BATCH_EVENT, batch as u32);
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::SgiSent);
}

#[inline(always)]
pub fn record_exit_kick_published(bucket: usize) {
    EXIT_KICK_BUCKET_PUBLISH_COUNTS[bucket].fetch_add(1, Ordering::Relaxed);
}

#[inline(always)]
pub fn record_exit_kick_collision(bucket: usize) {
    crate::trace_count!(EXIT_KICK_BUCKET_COLLISION);
    EXIT_KICK_BUCKET_COLLISION_COUNTS[bucket].fetch_add(1, Ordering::Relaxed);
}

#[inline(always)]
pub fn record_exit_kick_observed(pid: u64, interval: u64) {
    crate::trace_count!(EXIT_KICK_OBSERVED);
    EXIT_KICK_BUCKET_OBSERVED_COUNTS[pid as usize % EXIT_KICK_BUCKETS]
        .fetch_add(1, Ordering::Relaxed);
    crate::trace_event!(TEARDOWN_PROVIDER, EXIT_KICK_OBSERVED_EVENT, pid as u32);
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::KickObserved(interval));
    #[cfg(not(feature = "boot_tests"))]
    let _ = interval;
}

#[inline(always)]
pub fn record_masked_frames_walked(pid: u64) {
    crate::trace_count!(TEARDOWN_MASKED_FRAMES_WALKED);
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::MaskedFramesWalked);
    #[cfg(not(feature = "boot_tests"))]
    let _ = pid;
}

#[inline(always)]
pub fn record_report(pid: u64) {
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::Report);
    #[cfg(not(feature = "boot_tests"))]
    let _ = pid;
}

#[cfg(feature = "boot_tests")]
#[derive(Clone, Copy)]
pub struct TeardownPidEvidence {
    pub defer_count: u64,
    pub reclaim_count: u64,
    pub quarantine_count: u64,
    pub sgi_sent_count: u64,
    pub kick_observed_count: u64,
    pub kick_observed_interval: u64,
    pub masked_frames_walked: u64,
    pub report_count: u64,
    pub bucket_published_count: u64,
    pub bucket_observed_count: u64,
    pub bucket_collision_count: u64,
}

#[cfg(feature = "boot_tests")]
pub fn teardown_pid_evidence(pid: u64) -> Option<TeardownPidEvidence> {
    // Reading the per-pid procfs file is the test's explicit opt-in. It occurs
    // before the kill, so every hot-path recorder remains bounded and lock-free.
    BOOT_TEST_PID_COUNTS_ACTIVE.store(1, Ordering::Release);
    if !track_boot_test_pid(pid) {
        return None;
    }
    let slot = BOOT_TEST_PID_COUNTS
        .iter()
        .find(|slot| slot.pid.load(Ordering::Acquire) == pid)?;
    let bucket = pid as usize % EXIT_KICK_BUCKETS;
    Some(TeardownPidEvidence {
        defer_count: slot.defer_count.load(Ordering::Acquire),
        reclaim_count: slot.reclaim_count.load(Ordering::Acquire),
        quarantine_count: slot.quarantine_count.load(Ordering::Acquire),
        sgi_sent_count: slot.sgi_sent_count.load(Ordering::Acquire),
        kick_observed_count: slot.kick_observed_count.load(Ordering::Acquire),
        kick_observed_interval: slot.kick_observed_interval.load(Ordering::Acquire),
        masked_frames_walked: slot.masked_frames_walked.load(Ordering::Acquire),
        report_count: slot.report_count.load(Ordering::Acquire),
        bucket_published_count: EXIT_KICK_BUCKET_PUBLISH_COUNTS[bucket].load(Ordering::Acquire),
        bucket_observed_count: EXIT_KICK_BUCKET_OBSERVED_COUNTS[bucket].load(Ordering::Acquire),
        bucket_collision_count: EXIT_KICK_BUCKET_COLLISION_COUNTS[bucket].load(Ordering::Acquire),
    })
}

#[cfg(feature = "boot_tests")]
pub fn exit_kick_bucket_publish_count(bucket: usize) -> u64 {
    EXIT_KICK_BUCKET_PUBLISH_COUNTS[bucket % EXIT_KICK_BUCKETS].load(Ordering::Acquire)
}

#[inline(always)]
pub fn record_exit_request(already_terminated: bool) {
    if already_terminated {
        crate::trace_count!(EXIT_REPEAT_REQUESTS);
    } else {
        crate::trace_count!(EXIT_FIRST_REQUESTS);
    }
}

#[inline(always)]
pub fn record_clone_admission(admitted: bool) {
    if admitted {
        crate::trace_count!(CLONE_ADMISSION_ADMITTED);
    } else {
        crate::trace_count!(CLONE_ADMISSION_REFUSED);
    }
}

#[cfg(feature = "boot_tests")]
pub fn clone_admission_admitted() -> u64 {
    CLONE_ADMISSION_ADMITTED.aggregate()
}

#[cfg(feature = "boot_tests")]
pub fn clone_admission_refused() -> u64 {
    CLONE_ADMISSION_REFUSED.aggregate()
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

#[cfg(feature = "boot_tests")]
const RETIRE_SENTINEL_SUBTREES: usize = 3;

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
const EXEC_COHORT_CHILDREN: usize = 16;
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
const EXEC_COHORT_SUPERSEDED_PER_CHILD: usize = 3;
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
const EXEC_COHORT_DRAINED_AT_EXEC: usize = 2;

#[cfg(feature = "boot_tests")]
fn map_retire_sentinels(
    page_table: &mut crate::memory::process_memory::ProcessPageTable,
) -> Result<u64, &'static str> {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::{Page, PageTableFlags, Size4KiB, VirtAddr};
    use crate::memory::frame_allocator::{allocate_frame, deallocate_frame};
    #[cfg(target_arch = "x86_64")]
    use x86_64::{
        structures::paging::{Page, PageTableFlags, Size4KiB},
        VirtAddr,
    };

    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let mut expected = page_table.recorded_table_frames_for_gate() as u64;
    let sentinels = page_table
        .gate_sentinels(RETIRE_SENTINEL_SUBTREES)
        .ok_or("table offered too few unshared root slots")?;
    for sentinel in sentinels {
        expected += sentinel.table_frames as u64;
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(sentinel.address));
        if !page_table.gate_page_is_unmapped(page) {
            return Err("retire sentinel address was already mapped");
        }
        let frame = allocate_frame().ok_or("sentinel leaf allocation failed")?;
        if page_table.map_page(page, frame, flags).is_err() {
            deallocate_frame(frame);
            return Err("sentinel mapping failed");
        }
    }
    Ok(expected)
}

#[cfg(feature = "boot_tests")]
fn frame_allocator_used_frames() -> usize {
    let stats = crate::memory::frame_allocator::memory_stats();
    stats
        .allocated_frames
        .saturating_sub(crate::memory::frame_allocator::free_list_len_for_gate())
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn retirement_oracle_clock_now() -> u64 {
    crate::arch_impl::aarch64::timer::rdtsc()
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn retirement_oracle_clock_delta(milliseconds: u64) -> u64 {
    crate::arch_impl::aarch64::timer::frequency_hz()
        .saturating_mul(milliseconds)
        / 1000
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
fn retirement_oracle_clock_now() -> u64 {
    crate::time::timer::get_monotonic_time()
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
fn retirement_oracle_clock_delta(milliseconds: u64) -> u64 {
    milliseconds
}

#[cfg(feature = "boot_tests")]
pub fn fork_exit_defer_reclaim_pairing_test() -> crate::test_framework::registry::TestResult {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    use crate::test_framework::registry::TestResult;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    // Claim single-threaded ownership of the deferred-reclaim queues for the
    // whole measurement window. The queues are quiescent here (before any fork),
    // so this mirrors the sibling reclaim_progress_gate_test: peer CPUs early-
    // return from reclaim_deferred_process_resources() while the owner is set,
    // so every free and its reclaim-count increment happen on this CPU. Without
    // it, a peer could drain the shared PENDING queue concurrently and score an
    // entry freed-but-counter-not-yet-visible as unpaired against the deadline.
    let _reclaim_owner = match crate::task::process_task::BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(_) => return TestResult::Fail("reclaim queues not quiescent at pairing-test start"),
    };

    let teardown_entry_exit_before = TEARDOWN_ENTRY_EXIT.aggregate();
    let exit_first_requests_before = EXIT_FIRST_REQUESTS.aggregate();
    let exit_repeat_requests_before = EXIT_REPEAT_REQUESTS.aggregate();
    let masked_frames_walked_before = TEARDOWN_MASKED_FRAMES_WALKED.aggregate();
    let fd_closes_under_pm_before = FD_CLOSES_UNDER_PM.aggregate();
    let reclaim_enqueue_under_pm_before = RECLAIM_ENQUEUE_UNDER_PM.aggregate();
    let lock_order_suspect_before = TEARDOWN_LOCK_ORDER_SUSPECT.aggregate();
    let proof_under_queue_lock_before = PROOF_UNDER_QUEUE_LOCK.aggregate();
    let reclaim_context_violations_before = RECLAIM_CONTEXT_VIOLATIONS.aggregate();
    let receipt_dropped_before = RECEIPT_DROPPED_UNRETIRED.aggregate();

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
    let entry = VirtAddr::new(0x0040_0000);
    let stack_top = VirtAddr::new(0x0080_0000);
    let stack_bottom = VirtAddr::new(0x007f_0000);
    let tls = VirtAddr::new(0x0001_0000);
    #[cfg(target_arch = "aarch64")]
    let parent_privilege = crate::task::thread::ThreadPrivilege::User;
    #[cfg(target_arch = "x86_64")]
    // The x86 fork helper intentionally Box::leak's a kernel-stack allocation
    // for userspace children. This fixture never dispatches its synthetic
    // threads, so kernel privilege isolates O4 to the page-table lifecycle.
    let parent_privilege = crate::task::thread::ThreadPrivilege::Kernel;
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
        parent_privilege,
    );
    parent_thread.owner_pid = Some(parent_pid.as_u64());
    #[cfg(target_arch = "aarch64")]
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

    #[cfg(target_arch = "aarch64")]
    {
        // Exercise the known unproved immediate-release path before the leak
        // measurement. Its intentional abandonment is therefore present in
        // both allocator baselines and cannot disguise a deferred-root leak.
        let immediate_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
            Ok(page_table) => alloc::boxed::Box::new(page_table),
            Err(_) => return TestResult::Fail("baseline fork page-table allocation failed"),
        };
        let immediate = {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail("process manager unavailable during baseline fork");
            };
            let child_pid = match manager.fork_process_aarch64(
                parent_pid,
                parent_context.clone(),
                immediate_page_table,
            ) {
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
    }

    let mut pairing_child_pids = [0u64; 64];
    let mut pairing_child_count = 0;
    let pid_counts_guard = reset_boot_test_pid_counts();
    let mut expected_tables = 0u64;
    #[cfg(target_arch = "x86_64")]
    let mut expected_pending_old_tables = 0u64;
    #[cfg(target_arch = "aarch64")]
    let expected_pending_old_tables = 0u64;
    let allocator_used_before = frame_allocator_used_frames();
    let roots_retired_before = PT_ROOTS_RETIRED.aggregate();
    let table_frames_returned_before = PT_TABLE_FRAMES_RETURNED.aggregate();
    let table_frames_lost_before = PT_RETIRE_FRAMES_LOST.aggregate();
    let leaf_mappings_recorded_before = LEAF_MAPPINGS_RECORDED.aggregate();
    let leaf_mappings_released_before = LEAF_MAPPINGS_RELEASED.aggregate();
    let leaf_frames_returned_before = LEAF_FRAMES_RETURNED.aggregate();
    let dropped_undecided_before = PT_ROOT_DROPPED_UNDECIDED.aggregate();
    let dropped_mid_retire_before = PT_ROOT_DROPPED_MID_RETIRE.aggregate();
    let budget_requeued_before = PT_RETIRE_BUDGET_REQUEUED.aggregate();
    let no_arch_before = PT_ROOT_ABANDONED_NO_ARCH.aggregate();
    let refusal_counters_before = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];
    // The nine labels mirror PLAN P2's disjoint adapted-site table. The exact
    // source ratchets below prove which concrete callers use each custody
    // shape; this runtime matrix injects one independent process through every
    // labeled shape and then continues to 64 iterations for the P0 soak.
    const ADAPTED_SITE_CLASSES: [u8; 9] = [
        1, // aarch64 lower-EL data abort
        1, // aarch64 lower-EL instruction abort
        1, // aarch64 lower-EL SP alignment fault
        1, // aarch64 lower-EL PC alignment fault
        1, // x86_64 page fault
        1, // x86_64 general-protection fault
        1, // process::exit_current
        3, // handle_thread_exit phase-1 receipt
        2, // SIGKILL after quarantine/expedite
    ];
    for iteration in 0..64 {
        let mut child_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
            Ok(page_table) => alloc::boxed::Box::new(page_table),
            Err(_) => return TestResult::Fail("pairing fork page-table allocation failed"),
        };
        let child_expected_tables = match map_retire_sentinels(child_page_table.as_mut()) {
            Ok(expected) => expected,
            Err(_) => return TestResult::Fail("pairing retire sentinel mapping failed"),
        };
        if iteration == 0 {
            expected_tables = child_expected_tables;
        } else if child_expected_tables != expected_tables {
            return TestResult::Fail("pairing sentinel hierarchy cost changed between children");
        }
        #[cfg(target_arch = "x86_64")]
        let mut pending_old_page_table = if iteration == 0 {
            match crate::memory::process_memory::ProcessPageTable::new() {
                Ok(page_table) => {
                    expected_pending_old_tables =
                        page_table.recorded_table_frames_for_gate() as u64;
                    Some(alloc::boxed::Box::new(page_table))
                }
                Err(_) => return TestResult::Fail("pairing old page-table allocation failed"),
            }
        } else {
            None
        };
        let child = {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail("process manager unavailable during pairing fork");
            };
            #[cfg(target_arch = "aarch64")]
            let fork_result = manager.fork_process_aarch64(
                parent_pid,
                parent_context.clone(),
                child_page_table,
            );
            #[cfg(target_arch = "x86_64")]
            let fork_result = manager.fork_process_with_page_table(
                parent_pid,
                None,
                None,
                child_page_table,
            );
            let child_pid = match fork_result {
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
            #[cfg(target_arch = "x86_64")]
            if let Some(old_page_table) = pending_old_page_table.take() {
                let Some(child_process) = manager.get_process_mut(child_pid) else {
                    return TestResult::Fail("pairing child disappeared before old-root install");
                };
                child_process.pending_old_page_tables.push(old_page_table);
            }
            (child_pid, child_tid)
        };

        pairing_child_pids[pairing_child_count] = child.0.as_u64();
        if !track_boot_test_pid(pairing_child_pids[pairing_child_count]) {
            return TestResult::Fail("per-PID pairing tally table capacity exhausted");
        }
        pairing_child_count += 1;

        let site_class = ADAPTED_SITE_CLASSES[iteration % ADAPTED_SITE_CLASSES.len()];
        if site_class == 3 {
            {
                let _force_live =
                    crate::task::process_task::ForceLiveReclaimTestGuard::arm(child.0.as_u64());
                crate::task::process_task::ProcessScheduler::handle_thread_exit(child.1, 0);
            }
            // Preserve the gate's first/repeat accounting workload without
            // creating a second receipt or reclaim record.
            crate::task::process_task::ProcessScheduler::handle_thread_exit(child.1, 0);
        } else {
            crate::process::exit_process_for_teardown_test(child.0, 0);
            crate::task::process_task::ProcessScheduler::handle_thread_exit(child.1, 0);
        }
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

    let quiesce_deadline = retirement_oracle_clock_now()
        .saturating_add(retirement_oracle_clock_delta(5_000));
    loop {
        crate::task::scheduler::nudge_retirement_grace_for_test();
        let boundary_deadline = retirement_oracle_clock_now()
            .saturating_add(retirement_oracle_clock_delta(1));
        while retirement_oracle_clock_now() < boundary_deadline {
            core::hint::spin_loop();
        }
        crate::task::process_task::boot_reclaim_deferred_process_resources();
        if boot_test_pid_counts_complete(&pairing_child_pids) {
            break;
        }
        if retirement_oracle_clock_now() >= quiesce_deadline {
            break;
        }
        core::hint::spin_loop();
    }

    // The final owned drain has returned. It performed every free and its
    // reclaim-count store on this CPU; publish those stores with an Acquire
    // fence before scoring so a store that landed inside the last drain cannot
    // be read as unpaired below.
    core::sync::atomic::fence(Ordering::Acquire);

    let teardown_entry_exit_delta = TEARDOWN_ENTRY_EXIT
        .aggregate()
        .saturating_sub(teardown_entry_exit_before);
    let exit_first_requests_delta = EXIT_FIRST_REQUESTS
        .aggregate()
        .saturating_sub(exit_first_requests_before);
    let exit_repeat_requests_delta = EXIT_REPEAT_REQUESTS
        .aggregate()
        .saturating_sub(exit_repeat_requests_before);
    let allocator_used_after = frame_allocator_used_frames();
    let roots_retired_delta = PT_ROOTS_RETIRED
        .aggregate()
        .saturating_sub(roots_retired_before);
    let table_frames_returned_delta = PT_TABLE_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(table_frames_returned_before);
    let table_frames_lost_delta = PT_RETIRE_FRAMES_LOST
        .aggregate()
        .saturating_sub(table_frames_lost_before);
    let leaf_mappings_recorded_delta = LEAF_MAPPINGS_RECORDED
        .aggregate()
        .saturating_sub(leaf_mappings_recorded_before);
    let leaf_mappings_released_delta = LEAF_MAPPINGS_RELEASED
        .aggregate()
        .saturating_sub(leaf_mappings_released_before);
    let leaf_frames_returned_delta = LEAF_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(leaf_frames_returned_before);
    #[cfg(target_arch = "aarch64")]
    let live_leaf_refused_delta = FRAME_RETURN_REFUSED_LIVE_LEAF
        .aggregate()
        .saturating_sub(refusal_counters_before[5]);
    let expected_leaves = (RETIRE_SENTINEL_SUBTREES * pairing_child_pids.len()) as u64;
    #[cfg(target_arch = "x86_64")]
    let cohort_recorded = expected_tables * pairing_child_pids.len() as u64
        + expected_pending_old_tables;
    #[cfg(target_arch = "x86_64")]
    let allocator_balance = allocator_used_after as i64 - allocator_used_before as i64;
    #[cfg(target_arch = "aarch64")]
    crate::serial_println!(
        "[PT_RETIRE_ORACLE:aarch64:cycles=64:used_before={}:used_after={}:expected_tables={}:roots={}:returned={}:lost={}]",
        allocator_used_before,
        allocator_used_after,
        expected_tables,
        roots_retired_delta,
        table_frames_returned_delta,
        table_frames_lost_delta
    );
    #[cfg(target_arch = "aarch64")]
    crate::serial_println!(
        "[PT_LEAF_ORACLE:aarch64:cycles=64:expected={}:recorded={}:released={}:returned={}:live_refused={}:used_before={}:used_after={}]",
        expected_leaves,
        leaf_mappings_recorded_delta,
        leaf_mappings_released_delta,
        leaf_frames_returned_delta,
        live_leaf_refused_delta,
        allocator_used_before,
        allocator_used_after
    );
    if allocator_used_after != allocator_used_before {
        return TestResult::Fail("retire leak oracle did not return frame accounting to baseline");
    }
    if leaf_mappings_recorded_delta != expected_leaves
        || leaf_mappings_released_delta != expected_leaves
        || leaf_frames_returned_delta != expected_leaves
    {
        return TestResult::Fail("leaf leak oracle committed-effect accounting was not exact");
    }
    if teardown_entry_exit_delta < 64 {
        return TestResult::Fail("TEARDOWN_ENTRY_EXIT workload delta did not reach 64");
    }
    if exit_first_requests_delta < 64 || exit_repeat_requests_delta < 64 {
        return TestResult::Fail("first/repeat exit request workload deltas did not reach 64");
    }
    let pending_old_pid = pairing_child_pids[0];
    for pid in pairing_child_pids {
        let counts = boot_test_pid_counts(pid);
        let has_pending_old_root = pid == pending_old_pid && expected_pending_old_tables != 0;
        let pending_old_roots = u64::from(has_pending_old_root);
        let pending_old_tables = if has_pending_old_root {
            expected_pending_old_tables
        } else {
            0
        };
        if counts.defer_count == 0 {
            return TestResult::Fail("adapted-site per-PID defer proof was absent");
        }
        if counts.defer_count > 1 {
            return TestResult::Fail("adapted-site per-PID defer proof was duplicated");
        }
        if counts.reclaim_count == 0 {
            return TestResult::Fail("adapted-site per-PID reclaim proof was absent");
        }
        if counts.reclaim_count > 1 {
            return TestResult::Fail("adapted-site per-PID reclaim proof was duplicated");
        }
        if counts.roots_retired != 1 + pending_old_roots {
            return TestResult::Fail("retire cohort per-PID root completion was not exact");
        }
        if counts.table_frames_recorded != expected_tables + pending_old_tables {
            return TestResult::Fail(
                "retire cohort per-PID anti-vacuity table count was not exact",
            );
        }
        if counts.table_frames_returned != counts.table_frames_recorded + counts.roots_retired {
            return TestResult::Fail("retire cohort per-PID committed return equality failed");
        }
        if counts.table_frames_lost != 0 {
            return TestResult::Fail("retire cohort per-PID frame loss was nonzero");
        }
    }
    let dropped_undecided_delta = PT_ROOT_DROPPED_UNDECIDED
        .aggregate()
        .saturating_sub(dropped_undecided_before);
    let dropped_mid_retire_delta = PT_ROOT_DROPPED_MID_RETIRE
        .aggregate()
        .saturating_sub(dropped_mid_retire_before);
    let no_arch_delta = PT_ROOT_ABANDONED_NO_ARCH
        .aggregate()
        .saturating_sub(no_arch_before);
    let pending_old_roots = u64::from(expected_pending_old_tables != 0);
    if roots_retired_delta != pairing_child_pids.len() as u64 + pending_old_roots
        || table_frames_returned_delta
            != (expected_tables + 1) * pairing_child_pids.len() as u64
                + expected_pending_old_tables
                + pending_old_roots
        || table_frames_lost_delta != 0
        || dropped_undecided_delta != 0
        || dropped_mid_retire_delta != 0
        || PT_RETIRE_BUDGET_REQUEUED
            .aggregate()
            .saturating_sub(budget_requeued_before)
            != 0
        || no_arch_delta != 0
    {
        return TestResult::Fail("retire cohort global committed-effect accounting failed");
    }
    let refusal_counters_after = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];
    if refusal_counters_after != refusal_counters_before {
        return TestResult::Fail("retire cohort triggered an unexpected frame refusal");
    }
    #[cfg(target_arch = "aarch64")]
    if TEARDOWN_MASKED_FRAMES_WALKED
        .aggregate()
        .saturating_sub(masked_frames_walked_before)
        == 0
        || FD_CLOSES_UNDER_PM
            .aggregate()
            .saturating_sub(fd_closes_under_pm_before)
            == 0
    {
        return TestResult::Fail("retained pre-P7 under-PM baseline unexpectedly disappeared");
    }
    #[cfg(target_arch = "x86_64")]
    if TEARDOWN_MASKED_FRAMES_WALKED
        .aggregate()
        .saturating_sub(masked_frames_walked_before)
        != 0
        || FD_CLOSES_UNDER_PM
            .aggregate()
            .saturating_sub(fd_closes_under_pm_before)
            == 0
    {
        return TestResult::Fail("x86 deferred exits walked leaves or lost the FD workload");
    }
    if RECLAIM_ENQUEUE_UNDER_PM
        .aggregate()
        .saturating_sub(reclaim_enqueue_under_pm_before)
        != 0
    {
        return TestResult::Fail("P2 reclaim enqueue still occurred under PM");
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
    if RECEIPT_DROPPED_UNRETIRED
        .aggregate()
        .saturating_sub(receipt_dropped_before)
        != 0
        || LEDGER_CLAIM_ORPHANED.aggregate() != 0
    {
        return TestResult::Fail("receipt custody or pre-P6b orphan counter moved");
    }
    let _all_counter_readers = snapshot();

    #[cfg(target_arch = "aarch64")]
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

    #[cfg(target_arch = "x86_64")]
    {
        let parent_reclaim = {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail("process manager unavailable for parent cleanup");
            };
            let reclaim = {
                let Some(parent) = manager.get_process_mut(parent_pid) else {
                    return TestResult::Fail("pairing parent disappeared before cleanup");
                };
                let reclaim = crate::task::process_task::defer_process_resources(parent);
                crate::task::process_task::release_process_resources(parent);
                reclaim
            };
            manager.remove_process(parent_pid);
            reclaim
        };
        crate::task::process_task::enqueue_process_reclaim(parent_reclaim);
        let cleanup_deadline = retirement_oracle_clock_now()
            .saturating_add(retirement_oracle_clock_delta(5_000));
        while crate::task::process_task::boot_reclaim_locations(parent_pid.as_u64())
            != (false, false)
        {
            crate::task::scheduler::nudge_retirement_grace_for_test();
            let boundary_deadline = retirement_oracle_clock_now()
                .saturating_add(retirement_oracle_clock_delta(1));
            while retirement_oracle_clock_now() < boundary_deadline {
                core::hint::spin_loop();
            }
            crate::task::process_task::boot_reclaim_deferred_process_resources();
            if retirement_oracle_clock_now() >= cleanup_deadline {
                break;
            }
        }
        if crate::task::process_task::boot_reclaim_locations(parent_pid.as_u64())
            != (false, false)
        {
            return TestResult::Fail("pairing parent deferred cleanup did not quiesce");
        }
    }

    core::mem::drop(pid_counts_guard);
    #[cfg(target_arch = "x86_64")]
    {
        crate::serial_println!("[TEST:process:x86_retire_cohort:PASS]");
        crate::serial_println!(
            "[PT_RETIRE_COHORT:x86:children={}:retired={}:returned={}:recorded={}:lost={}:no_arch={}:undecided={}:mid_retire={}:balance={}]",
            pairing_child_pids.len(),
            roots_retired_delta,
            table_frames_returned_delta,
            cohort_recorded,
            table_frames_lost_delta,
            no_arch_delta,
            dropped_undecided_delta,
            dropped_mid_retire_delta,
            allocator_balance
        );
    }
    TestResult::Pass
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_retire_cohort_gate() {
    crate::serial_println!("[TEST:process:x86_retire_cohort:START]");
    let result = fork_exit_defer_reclaim_pairing_test();
    if !result.is_pass() {
        crate::serial_println!("[TEST:process:x86_retire_cohort:FAIL:{:?}]", result);
    }
    // Deliberate fail-loud boot policy, identical to the two sibling gates: never
    // continue past a failed custody oracle and emit misleading later boot markers.
    assert!(result.is_pass(), "x86 retire cohort gate failed");
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn exec_supersede_cohort_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;
    use x86_64::VirtAddr;

    let _reclaim_owner = match crate::task::process_task::BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(_) => return TestResult::Fail("reclaim queues not quiescent at exec cohort start"),
    };

    let parent_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
        Ok(page_table) => alloc::boxed::Box::new(page_table),
        Err(_) => return TestResult::Fail("exec cohort parent page-table allocation failed"),
    };
    let parent_pid = {
        let manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_ref() else {
            return TestResult::Fail("process manager unavailable for exec cohort parent PID");
        };
        manager.allocate_pid()
    };
    fn test_user_entry() {}
    let entry = VirtAddr::new(0x0040_0000);
    let stack_top = VirtAddr::new(0x0080_0000);
    let stack_bottom = VirtAddr::new(0x007f_0000);
    let tls = VirtAddr::new(0x0001_0000);
    let mut parent_process = crate::process::Process::new(
        parent_pid,
        alloc::string::String::from("exec_cohort_parent"),
        entry,
    );
    let mut parent_thread = crate::task::thread::Thread::new(
        alloc::string::String::from("exec_cohort_parent_main"),
        test_user_entry,
        stack_top,
        stack_bottom,
        tls,
        crate::task::thread::ThreadPrivilege::Kernel,
    );
    parent_thread.owner_pid = Some(parent_pid.as_u64());
    parent_process.page_table = Some(parent_page_table);
    parent_process.set_main_thread(parent_thread);
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for exec cohort parent insert");
        };
        manager.insert_process(parent_pid, parent_process);
    }

    // #573 production-path arm: the real x86 exec bodies must release a
    // never-published address space on failure and must not strip a live
    // process of the address space it is still running on.
    let corrupt = crate::memory::process_memory::x86_corrupt_executable_fixture();
    let used_before = frame_allocator_used_frames();
    let undecided_before = PT_ROOT_DROPPED_UNDECIDED.aggregate();
    let mid_retire_before = PT_ROOT_DROPPED_MID_RETIRE.aggregate();
    let lost_before = PT_RETIRE_FRAMES_LOST.aggregate();
    let custody_refused_before = LEAF_CUSTODY_REFUSED.aggregate();
    let decref_unregistered_before = LEAF_DECREF_UNREGISTERED.aggregate();
    let double_before = FRAME_RETURN_REFUSED_DOUBLE.aggregate();
    let stale_before = FRAME_RETURN_REFUSED_STALE.aggregate();
    let untracked_before = FRAME_RETURN_REFUSED_UNTRACKED.aggregate();
    let root_slot_refused_before = PT_ROOT_SLOT_REFUSED.aggregate();
    let (plain, plain_kept, with_argv, argv_kept, name_kept) = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail(
                "process manager unavailable for failed exec production-path arm",
            );
        };
        let plain = manager.exec_process(parent_pid, &corrupt, Some("corrupt_exec"));
        let plain_kept = manager
            .get_process(parent_pid)
            .map(|process| process.page_table.is_some())
            .unwrap_or(false);
        let argv: [&[u8]; 1] = [b"corrupt_exec\0"];
        let with_argv =
            manager.exec_process_with_argv(parent_pid, &corrupt, Some("corrupt_exec"), &argv);
        let argv_kept = manager
            .get_process(parent_pid)
            .map(|process| process.page_table.is_some())
            .unwrap_or(false);
        let name_kept = manager
            .get_process(parent_pid)
            .map(|process| process.name == "exec_cohort_parent")
            .unwrap_or(false);
        (plain, plain_kept, with_argv, argv_kept, name_kept)
    };
    let used_after = frame_allocator_used_frames();
    let balance = used_after as i64 - used_before as i64;
    let plain_err = matches!(plain, Err("Segment data out of bounds"));
    let argv_err = matches!(with_argv, Err("Segment data out of bounds"));
    let undecided_delta = PT_ROOT_DROPPED_UNDECIDED
        .aggregate()
        .saturating_sub(undecided_before);
    let mid_retire_delta = PT_ROOT_DROPPED_MID_RETIRE
        .aggregate()
        .saturating_sub(mid_retire_before);
    let lost_delta = PT_RETIRE_FRAMES_LOST
        .aggregate()
        .saturating_sub(lost_before);
    let custody_refused_delta = LEAF_CUSTODY_REFUSED
        .aggregate()
        .saturating_sub(custody_refused_before);
    let decref_unregistered_delta = LEAF_DECREF_UNREGISTERED
        .aggregate()
        .saturating_sub(decref_unregistered_before);
    let double_delta = FRAME_RETURN_REFUSED_DOUBLE
        .aggregate()
        .saturating_sub(double_before);
    let stale_delta = FRAME_RETURN_REFUSED_STALE
        .aggregate()
        .saturating_sub(stale_before);
    let untracked_delta = FRAME_RETURN_REFUSED_UNTRACKED
        .aggregate()
        .saturating_sub(untracked_before);
    let root_slot_refused_delta = PT_ROOT_SLOT_REFUSED
        .aggregate()
        .saturating_sub(root_slot_refused_before);
    crate::serial_println!(
        "[EXEC_FAILED_RELEASE_PROD:x86:plain_err={}:plain_kept={}:argv_err={}:argv_kept={}:name_kept={}:balance={}:undecided={}:mid_retire={}:lost={}:custody_refused={}:decref_unregistered={}:double={}:stale={}:untracked={}:root_slot_refused={}]",
        plain_err,
        plain_kept,
        argv_err,
        argv_kept,
        name_kept,
        balance,
        undecided_delta,
        mid_retire_delta,
        lost_delta,
        custody_refused_delta,
        decref_unregistered_delta,
        double_delta,
        stale_delta,
        untracked_delta,
        root_slot_refused_delta,
    );
    if !plain_err {
        return TestResult::Fail("exec_process did not fail on a corrupt executable");
    }
    if !plain_kept {
        return TestResult::Fail(
            "exec_process stripped a live process of its address space on failure",
        );
    }
    if !argv_err {
        return TestResult::Fail("exec_process_with_argv did not fail on a corrupt executable");
    }
    if !argv_kept {
        return TestResult::Fail(
            "exec_process_with_argv stripped a live process of its address space on failure",
        );
    }
    if !name_kept {
        return TestResult::Fail("a failed exec mutated the process identity");
    }
    if balance != 0 {
        return TestResult::Fail("a failed x86 exec did not return its half-built address space");
    }
    if undecided_delta != 0
        || mid_retire_delta != 0
        || lost_delta != 0
        || custody_refused_delta != 0
        || decref_unregistered_delta != 0
        || double_delta != 0
        || stale_delta != 0
        || untracked_delta != 0
    {
        return TestResult::Fail("a failed x86 exec left an unclassified or over-returned root");
    }

    let pid_counts_guard = reset_boot_test_pid_counts();
    let allocator_used_before = frame_allocator_used_frames();
    let roots_retired_before = PT_ROOTS_RETIRED.aggregate();
    let table_frames_returned_before = PT_TABLE_FRAMES_RETURNED.aggregate();
    let table_frames_lost_before = PT_RETIRE_FRAMES_LOST.aggregate();
    let leaf_mappings_recorded_before = LEAF_MAPPINGS_RECORDED.aggregate();
    let leaf_mappings_released_before = LEAF_MAPPINGS_RELEASED.aggregate();
    let leaf_frames_returned_before = LEAF_FRAMES_RETURNED.aggregate();
    let dropped_undecided_before = PT_ROOT_DROPPED_UNDECIDED.aggregate();
    let dropped_mid_retire_before = PT_ROOT_DROPPED_MID_RETIRE.aggregate();
    let no_arch_before = PT_ROOT_ABANDONED_NO_ARCH.aggregate();
    let refusal_counters_before = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];

    let mut child_pids = [0u64; EXEC_COHORT_CHILDREN];
    let mut expected_tables = 0u64;
    let mut first_failure: Option<&'static str> = None;
    for child_index in 0..EXEC_COHORT_CHILDREN {
        let mut live_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
            Ok(page_table) => alloc::boxed::Box::new(page_table),
            Err(_) => return TestResult::Fail("exec cohort live page-table allocation failed"),
        };
        let live_expected_tables = match map_retire_sentinels(live_page_table.as_mut()) {
            Ok(expected) => expected,
            Err(_) => return TestResult::Fail("exec cohort live sentinel mapping failed"),
        };
        if child_index == 0 {
            expected_tables = live_expected_tables;
        } else if live_expected_tables != expected_tables {
            return TestResult::Fail("exec cohort sentinel hierarchy cost changed");
        }

        let mut superseded_page_tables: alloc::vec::Vec<
            alloc::boxed::Box<crate::memory::process_memory::ProcessPageTable>,
        > = alloc::vec::Vec::new();
        for _ in 0..EXEC_COHORT_SUPERSEDED_PER_CHILD {
            let mut superseded_page_table =
                match crate::memory::process_memory::ProcessPageTable::new() {
                    Ok(page_table) => alloc::boxed::Box::new(page_table),
                    Err(_) => {
                        return TestResult::Fail(
                            "exec cohort superseded page-table allocation failed",
                        )
                    }
                };
            let superseded_expected_tables =
                match map_retire_sentinels(superseded_page_table.as_mut()) {
                    Ok(expected) => expected,
                    Err(_) => {
                        return TestResult::Fail("exec cohort superseded sentinel mapping failed")
                    }
                };
            if superseded_expected_tables != expected_tables {
                return TestResult::Fail("exec cohort sentinel hierarchy cost changed");
            }
            superseded_page_tables.push(superseded_page_table);
        }

        let child = {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail("process manager unavailable during exec cohort fork");
            };
            let child_pid =
                match manager.fork_process_with_page_table(parent_pid, None, None, live_page_table)
                {
                    Ok(pid) => pid,
                    Err(_) => return TestResult::Fail("exec cohort fork failed"),
                };
            let Some(child_tid) = manager
                .get_process(child_pid)
                .and_then(|process| process.main_thread.as_ref())
                .map(|thread| thread.id)
            else {
                return TestResult::Fail("exec cohort child has no main thread");
            };
            let Some(child_process) = manager.get_process_mut(child_pid) else {
                return TestResult::Fail("exec cohort child disappeared before old-root install");
            };
            child_process
                .pending_old_page_tables
                .extend(superseded_page_tables);
            (child_pid, child_tid)
        };

        child_pids[child_index] = child.0.as_u64();
        if !track_boot_test_pid(child_pids[child_index]) {
            return TestResult::Fail("exec cohort per-PID tally table capacity exhausted");
        }

        let child_leaf_recorded_before = LEAF_MAPPINGS_RECORDED.aggregate();
        let child_leaf_released_before = LEAF_MAPPINGS_RELEASED.aggregate();
        let child_leaf_returned_before = LEAF_FRAMES_RETURNED.aggregate();
        {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail(
                    "process manager unavailable during exec cohort exec-time drain",
                );
            };
            let Some(child_process) = manager.get_process_mut(child.0) else {
                return TestResult::Fail("exec cohort child disappeared before exec-time drain");
            };
            let mut pending = core::mem::take(&mut child_process.pending_old_page_tables);
            let survivor_count = EXEC_COHORT_SUPERSEDED_PER_CHILD - EXEC_COHORT_DRAINED_AT_EXEC;
            let survivors = pending.split_off(pending.len() - survivor_count);
            child_process.pending_old_page_tables = pending;
            // A populated superseded address space costs more than one frame, so
            // a one-frame drain must report incomplete and leave every pending table
            // in place for the next pass.
            let probe_pending_before = child_process.pending_old_page_tables.len();
            let mut probe_budget = 1u32;
            let probe_complete = child_process.drain_old_page_tables_bounded(&mut probe_budget);
            if (probe_complete
                || child_process.pending_old_page_tables.len() != probe_pending_before)
                && first_failure.is_none()
            {
                first_failure = Some(
                    "exec cohort bounded drain completed a populated address space within a one-frame budget",
                );
            }
            child_process.drain_old_page_tables();
            if !child_process.pending_old_page_tables.is_empty() && first_failure.is_none() {
                first_failure = Some("exec cohort exec-time drain did not complete");
            }
            child_process.pending_old_page_tables.extend(survivors);
        }

        let expected_exec_drained_leaves =
            (EXEC_COHORT_DRAINED_AT_EXEC * RETIRE_SENTINEL_SUBTREES) as u64;
        if LEAF_MAPPINGS_RELEASED
            .aggregate()
            .saturating_sub(child_leaf_released_before)
            != expected_exec_drained_leaves
            && first_failure.is_none()
        {
            first_failure = Some("exec cohort per-child leaf release equality failed");
        }
        if LEAF_FRAMES_RETURNED
            .aggregate()
            .saturating_sub(child_leaf_returned_before)
            != expected_exec_drained_leaves
            && first_failure.is_none()
        {
            first_failure = Some("exec cohort per-child leaf return equality failed");
        }
        if LEAF_MAPPINGS_RECORDED
            .aggregate()
            .saturating_sub(child_leaf_recorded_before)
            != 0
            && first_failure.is_none()
        {
            first_failure = Some("exec cohort drain recorded new leaf custody");
        }

        crate::process::exit_process_for_teardown_test(child.0, 0);
        crate::task::process_task::ProcessScheduler::handle_thread_exit(child.1, 0);
        {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail("process manager unavailable during exec cohort reap");
            };
            manager.remove_process(child.0);
            if let Some(parent) = manager.get_process_mut(parent_pid) {
                parent.children.retain(|pid| *pid != child.0);
            }
        }
    }

    let quiesce_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    loop {
        crate::task::scheduler::nudge_retirement_grace_for_test();
        let boundary_deadline =
            retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
        while retirement_oracle_clock_now() < boundary_deadline {
            core::hint::spin_loop();
        }
        crate::task::process_task::boot_reclaim_deferred_process_resources();
        if boot_test_pid_counts_complete(&child_pids) {
            break;
        }
        if retirement_oracle_clock_now() >= quiesce_deadline {
            break;
        }
        core::hint::spin_loop();
    }

    core::sync::atomic::fence(Ordering::Acquire);

    let allocator_used_after = frame_allocator_used_frames();
    let roots_retired_delta = PT_ROOTS_RETIRED
        .aggregate()
        .saturating_sub(roots_retired_before);
    let table_frames_returned_delta = PT_TABLE_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(table_frames_returned_before);
    let table_frames_lost_delta = PT_RETIRE_FRAMES_LOST
        .aggregate()
        .saturating_sub(table_frames_lost_before);
    let leaf_mappings_recorded_delta = LEAF_MAPPINGS_RECORDED
        .aggregate()
        .saturating_sub(leaf_mappings_recorded_before);
    let leaf_mappings_released_delta = LEAF_MAPPINGS_RELEASED
        .aggregate()
        .saturating_sub(leaf_mappings_released_before);
    let leaf_frames_returned_delta = LEAF_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(leaf_frames_returned_before);
    let dropped_undecided_delta = PT_ROOT_DROPPED_UNDECIDED
        .aggregate()
        .saturating_sub(dropped_undecided_before);
    let dropped_mid_retire_delta = PT_ROOT_DROPPED_MID_RETIRE
        .aggregate()
        .saturating_sub(dropped_mid_retire_before);
    let no_arch_delta = PT_ROOT_ABANDONED_NO_ARCH
        .aggregate()
        .saturating_sub(no_arch_before);
    let custody_refused_delta = LEAF_CUSTODY_REFUSED
        .aggregate()
        .saturating_sub(refusal_counters_before[7]);
    let decref_unregistered_delta = LEAF_DECREF_UNREGISTERED
        .aggregate()
        .saturating_sub(refusal_counters_before[6]);
    let mut cohort_recorded = 0u64;
    let mut cohort_returned = 0u64;
    let mut cohort_roots = 0u64;
    let mut cohort_lost = 0u64;
    for pid in child_pids {
        let counts = boot_test_pid_counts(pid);
        cohort_recorded = cohort_recorded.saturating_add(counts.table_frames_recorded);
        cohort_returned = cohort_returned.saturating_add(counts.table_frames_returned);
        cohort_roots = cohort_roots.saturating_add(counts.roots_retired);
        cohort_lost = cohort_lost.saturating_add(counts.table_frames_lost);
    }
    let allocator_balance = allocator_used_after as i64 - allocator_used_before as i64;

    crate::serial_println!(
        "[PT_EXEC_COHORT:x86:children={}:superseded={}:roots={}:returned={}:recorded={}:lost={}:leaf_recorded={}:leaf_released={}:leaf_returned={}:custody_refused={}:decref_unregistered={}:undecided={}:mid_retire={}:no_arch={}:balance={}]",
        EXEC_COHORT_CHILDREN,
        EXEC_COHORT_SUPERSEDED_PER_CHILD,
        cohort_roots,
        cohort_returned,
        cohort_recorded,
        cohort_lost,
        leaf_mappings_recorded_delta,
        leaf_mappings_released_delta,
        leaf_frames_returned_delta,
        custody_refused_delta,
        decref_unregistered_delta,
        dropped_undecided_delta,
        dropped_mid_retire_delta,
        no_arch_delta,
        allocator_balance
    );
    if let Some(reason) = first_failure {
        return TestResult::Fail(reason);
    }
    for pid in child_pids {
        let counts = boot_test_pid_counts(pid);
        if counts.roots_retired != EXEC_COHORT_SUPERSEDED_PER_CHILD as u64 + 1 {
            return TestResult::Fail("exec cohort per-PID root retirement was not exact");
        }
        if counts.table_frames_returned
            != counts.table_frames_recorded + EXEC_COHORT_SUPERSEDED_PER_CHILD as u64 + 1
        {
            return TestResult::Fail("exec cohort per-PID committed return equality failed");
        }
        if counts.table_frames_lost != 0 {
            return TestResult::Fail("exec cohort per-PID frame loss was nonzero");
        }
        if counts.table_frames_recorded
            != (EXEC_COHORT_SUPERSEDED_PER_CHILD as u64 + 1) * expected_tables
        {
            return TestResult::Fail(
                "exec cohort per-PID anti-vacuity table count was not exact",
            );
        }
    }
    if roots_retired_delta != cohort_roots {
        return TestResult::Fail("exec cohort global root retirement did not match the per-PID sum");
    }
    if table_frames_returned_delta != cohort_returned {
        return TestResult::Fail("exec cohort global table return did not match the per-PID sum");
    }
    if table_frames_lost_delta != cohort_lost {
        return TestResult::Fail("exec cohort global frame loss did not match the per-PID sum");
    }
    let expected_cohort_leaves = (EXEC_COHORT_CHILDREN
        * (EXEC_COHORT_SUPERSEDED_PER_CHILD + 1)
        * RETIRE_SENTINEL_SUBTREES) as u64;
    if leaf_mappings_recorded_delta != expected_cohort_leaves
        || leaf_mappings_released_delta != expected_cohort_leaves
        || leaf_frames_returned_delta != expected_cohort_leaves
    {
        return TestResult::Fail("exec cohort leaf committed-effect accounting was not exact");
    }
    if dropped_undecided_delta != 0 || dropped_mid_retire_delta != 0 || no_arch_delta != 0 {
        return TestResult::Fail("exec cohort dropped a root without a disposition");
    }
    let refusal_counters_after = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];
    if refusal_counters_after != refusal_counters_before {
        return TestResult::Fail("exec cohort triggered an unexpected frame refusal");
    }
    if allocator_used_after != allocator_used_before {
        return TestResult::Fail("exec cohort did not return frame accounting to baseline");
    }

    let parent_reclaim = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for exec cohort parent cleanup");
        };
        let reclaim = {
            let Some(parent) = manager.get_process_mut(parent_pid) else {
                return TestResult::Fail("exec cohort parent disappeared before cleanup");
            };
            let reclaim = crate::task::process_task::defer_process_resources(parent);
            crate::task::process_task::release_process_resources(parent);
            reclaim
        };
        manager.remove_process(parent_pid);
        reclaim
    };
    crate::task::process_task::enqueue_process_reclaim(parent_reclaim);
    let cleanup_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    while crate::task::process_task::boot_reclaim_locations(parent_pid.as_u64()) != (false, false) {
        crate::task::scheduler::nudge_retirement_grace_for_test();
        let boundary_deadline =
            retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
        while retirement_oracle_clock_now() < boundary_deadline {
            core::hint::spin_loop();
        }
        crate::task::process_task::boot_reclaim_deferred_process_resources();
        if retirement_oracle_clock_now() >= cleanup_deadline {
            break;
        }
    }
    if crate::task::process_task::boot_reclaim_locations(parent_pid.as_u64()) != (false, false) {
        return TestResult::Fail("exec cohort parent deferred cleanup did not quiesce");
    }

    core::mem::drop(pid_counts_guard);
    crate::serial_println!("[TEST:process:x86_exec_cohort:PASS]");
    TestResult::Pass
}

/// Deliver `sig` to every row a group-scoped kill aimed at `group` would select.
/// Membership is the production predicate: thread_group_id.unwrap_or(pid) == group.
/// Returns the number of live rows the signal actually reached.
#[cfg(feature = "boot_tests")]
fn signal_thread_group_for_test(group: u64, sig: u32) -> usize {
    let mut manager_guard = crate::process::manager();
    let Some(manager) = manager_guard.as_mut() else {
        return 0;
    };
    let target_pids: alloc::vec::Vec<_> = manager
        .all_processes()
        .into_iter()
        .filter(|process| {
            !process.is_terminated()
                && process.thread_group_id.unwrap_or(process.id.as_u64()) == group
        })
        .map(|process| process.id)
        .collect();
    let mut reached = 0;
    for pid in target_pids {
        if let Some(process) = manager.get_process_mut(pid) {
            process.signals.set_pending(sig);
            reached += usize::from(process.signals.is_pending(sig));
        }
    }
    reached
}

#[cfg(feature = "boot_tests")]
fn take_pending_signal_for_test(pid: crate::process::ProcessId, sig: u32) -> bool {
    let mut manager_guard = crate::process::manager();
    let Some(manager) = manager_guard.as_mut() else {
        return false;
    };
    let Some(process) = manager.get_process_mut(pid) else {
        return false;
    };
    if process.is_terminated() || !process.signals.is_pending(sig) {
        return false;
    }
    process.signals.clear_pending(sig);
    true
}

#[cfg(feature = "boot_tests")]
pub fn exec_detach_oracle_test() -> crate::test_framework::registry::TestResult {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    use crate::test_framework::registry::TestResult;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    // All per-architecture residuals below are manifestations of issue #583:
    // `GuardedStack::drop` does not reclaim user stack frames. They are counted
    // rather than freed because a counted leak beats an over-free; closing #583
    // should drive all of them to zero.
    #[cfg(target_arch = "aarch64")]
    const EXPECTED_LEAF_RESIDUAL: u64 = 16;
    #[cfg(target_arch = "x86_64")]
    const EXPECTED_LEAF_RESIDUAL: u64 = 16;
    #[cfg(target_arch = "aarch64")]
    const EXPECTED_STACK_RESIDUAL: i64 = 18;
    #[cfg(target_arch = "x86_64")]
    const EXPECTED_STACK_RESIDUAL: i64 = 149;
    const EXPECTED_ROOTS_CREATED: u64 = 5;

    fn test_user_entry() {}

    fn process_row(pid: crate::process::ProcessId, name: &'static str) -> crate::process::Process {
        let entry = VirtAddr::new(0x0040_0000);
        let stack_top = VirtAddr::new(0x0080_0000);
        let stack_bottom = VirtAddr::new(0x007f_0000);
        let tls = VirtAddr::new(0x0001_0000);
        let mut process =
            crate::process::Process::new(pid, alloc::string::String::from(name), entry);
        let mut thread = crate::task::thread::Thread::new(
            alloc::string::String::from(name),
            test_user_entry,
            stack_top,
            stack_bottom,
            tls,
            crate::task::thread::ThreadPrivilege::Kernel,
        );
        thread.owner_pid = Some(pid.as_u64());
        process.set_main_thread(thread);
        process
    }

    fn mark_clone_vm_member(
        process: &mut crate::process::Process,
        leader_root: u64,
        leader_pid: crate::process::ProcessId,
    ) {
        assert!(process.inherited_cr3.replace(leader_root).is_none());
        assert!(process
            .thread_group_id
            .replace(leader_pid.as_u64())
            .is_none());
    }

    fn invoke_exec(
        manager: &mut crate::process::ProcessManager,
        pid: crate::process::ProcessId,
        elf: &[u8],
        with_argv: bool,
    ) -> Result<(), &'static str> {
        if with_argv {
            let argv: [&[u8]; 1] = [b"exec_detach_oracle\0"];
            manager
                .exec_process_with_argv(pid, elf, Some("exec_detach_oracle"), &argv)
                .map(|_| ())
        } else {
            manager
                .exec_process(pid, elf, Some("exec_detach_oracle"))
                .map(|_| ())
        }
    }

    fn exit_and_remove_unowned_row(
        pid: crate::process::ProcessId,
        unavailable_reason: &'static str,
        missing_reason: &'static str,
    ) -> Result<(), &'static str> {
        let thread_id = {
            let manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_ref() else {
                return Err(unavailable_reason);
            };
            manager
                .get_process(pid)
                .and_then(|process| process.main_thread.as_ref())
                .map(|thread| thread.id)
                .ok_or(missing_reason)?
        };
        crate::process::exit_process_for_teardown_test(pid, 0);
        crate::task::process_task::ProcessScheduler::handle_thread_exit(thread_id, 0);
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return Err(unavailable_reason);
        };
        if !manager
            .get_process(pid)
            .map(|process| process.is_terminated())
            .unwrap_or(false)
        {
            return Err(missing_reason);
        }
        manager.remove_from_ready_queue(pid);
        manager.remove_process(pid);
        Ok(())
    }

    fn retire_and_remove_owned_row(
        pid: crate::process::ProcessId,
        unavailable_reason: &'static str,
        missing_reason: &'static str,
    ) -> Result<(), &'static str> {
        let reclaim = {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return Err(unavailable_reason);
            };
            let reclaim = {
                let Some(process) = manager.get_process_mut(pid) else {
                    return Err(missing_reason);
                };
                let reclaim = crate::task::process_task::defer_process_resources(process);
                crate::task::process_task::release_process_resources(process);
                reclaim
            };
            manager.remove_from_ready_queue(pid);
            manager.remove_process(pid);
            reclaim
        };
        crate::task::process_task::enqueue_process_reclaim(reclaim);
        let cleanup_deadline =
            retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
        while crate::task::process_task::boot_reclaim_locations(pid.as_u64()) != (false, false) {
            crate::task::scheduler::nudge_retirement_grace_for_test();
            let boundary_deadline =
                retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
            while retirement_oracle_clock_now() < boundary_deadline {
                core::hint::spin_loop();
            }
            crate::task::process_task::boot_reclaim_deferred_process_resources();
            if retirement_oracle_clock_now() >= cleanup_deadline {
                return Err("exec detach owned-row deferred cleanup did not quiesce");
            }
        }
        Ok(())
    }

    let reclaim_owner = match crate::task::process_task::BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(_) => {
            return TestResult::Fail("reclaim queues not quiescent at exec detach oracle start")
        }
    };
    let allocator_used_before = frame_allocator_used_frames();
    let table_frames_recorded_before = PT_TABLE_FRAMES_RECORDED.aggregate();
    let table_frames_returned_before = PT_TABLE_FRAMES_RETURNED.aggregate();
    let roots_retired_before = PT_ROOTS_RETIRED.aggregate();
    let leaf_mappings_recorded_before = LEAF_MAPPINGS_RECORDED.aggregate();
    let leaf_mappings_released_before = LEAF_MAPPINGS_RELEASED.aggregate();
    let leaf_frames_returned_before = LEAF_FRAMES_RETURNED.aggregate();
    let table_frames_lost_before = PT_RETIRE_FRAMES_LOST.aggregate();
    let dropped_undecided_before = PT_ROOT_DROPPED_UNDECIDED.aggregate();
    let dropped_mid_retire_before = PT_ROOT_DROPPED_MID_RETIRE.aggregate();
    let no_arch_before = PT_ROOT_ABANDONED_NO_ARCH.aggregate();
    let refusal_counters_before = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];

    #[cfg(target_arch = "aarch64")]
    let corrupt = crate::memory::process_memory::corrupt_executable_fixture();
    #[cfg(target_arch = "x86_64")]
    let corrupt = crate::memory::process_memory::x86_corrupt_executable_fixture();
    #[cfg(target_arch = "aarch64")]
    let valid = crate::memory::process_memory::valid_executable_fixture();
    #[cfg(target_arch = "x86_64")]
    let valid = crate::memory::process_memory::x86_valid_executable_fixture();

    let leader_pid = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for exec detach leader insert");
        };
        match manager.create_process(alloc::string::String::from("exec_detach_leader"), &valid) {
            Ok(pid) => pid,
            Err(_) => return TestResult::Fail("exec detach leader page-table allocation failed"),
        }
    };
    let mut roots_created = 1u64;
    let leader_root = {
        let manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_ref() else {
            return TestResult::Fail("process manager unavailable for exec detach leader root");
        };
        let Some(page_table) = manager
            .get_process(leader_pid)
            .and_then(|process| process.page_table.as_ref())
        else {
            return TestResult::Fail("exec detach leader row has no page table");
        };
        page_table.level_4_frame().start_address().as_u64()
    };

    let mut bodies = 0usize;
    let mut fail_preserved = 0usize;
    #[cfg(target_arch = "aarch64")]
    let mut sibling_refused = 0usize;
    #[cfg(target_arch = "x86_64")]
    let sibling_refused = 0usize;
    let mut success_detached = 0usize;
    let mut fresh_root = 0usize;
    let mut tgid_self = 0usize;
    let mut old_group_reached_pre = 0usize;
    let mut old_group_missed_post = 0usize;
    let mut self_group_reached_post = 0usize;
    let mut first_failure: Option<&'static str> = None;
    let mut reclaim_pids = [0u64; 5];
    let mut reclaim_pid_count = 0usize;

    for body in 0..2 {
        let with_argv = body == 1;
        let member_pid = {
            let manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_ref() else {
                return TestResult::Fail("process manager unavailable for exec detach member PID");
            };
            manager.allocate_pid()
        };
        let mut member = process_row(member_pid, "exec_detach_member");
        mark_clone_vm_member(&mut member, leader_root, leader_pid);
        {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail(
                    "process manager unavailable for exec detach member insert",
                );
            };
            manager.insert_process(member_pid, member);
        }
        bodies += 1;

        let old_group_pre_count =
            signal_thread_group_for_test(leader_pid.as_u64(), crate::signal::constants::SIGUSR1);
        let member_reached_pre =
            take_pending_signal_for_test(member_pid, crate::signal::constants::SIGUSR1);
        let leader_reached_pre =
            take_pending_signal_for_test(leader_pid, crate::signal::constants::SIGUSR1);
        if old_group_pre_count == 2 && member_reached_pre && leader_reached_pre {
            old_group_reached_pre += 1;
        } else if first_failure.is_none() {
            first_failure =
                Some("a kill aimed at the pre-exec group failed to reach a still-member row");
        }

        let failed_exec = {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail("process manager unavailable for exec detach failure arm");
            };
            invoke_exec(manager, member_pid, &corrupt, with_argv)
        };
        let (failed_inherited_cr3, failed_thread_group_id) = {
            let manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_ref() else {
                return TestResult::Fail(
                    "process manager unavailable for exec detach failure observation",
                );
            };
            match manager.get_process(member_pid) {
                Some(process) => (process.inherited_cr3, process.thread_group_id),
                None => {
                    if first_failure.is_none() {
                        first_failure = Some("exec detach member disappeared after failed exec");
                    }
                    (None, None)
                }
            }
        };
        let failure_error_exact = matches!(failed_exec, Err("Segment data out of bounds"));
        if failure_error_exact {
            roots_created = roots_created.saturating_add(1);
        }
        let failure_inherited_preserved = failed_inherited_cr3 == Some(leader_root);
        let failure_tgid_preserved = failed_thread_group_id == Some(leader_pid.as_u64());
        if !failure_error_exact && first_failure.is_none() {
            first_failure = Some("exec detach corrupt fixture did not fail at segment bounds");
        }
        if !failure_inherited_preserved && first_failure.is_none() {
            first_failure = Some("failed exec mutated inherited_cr3");
        }
        if !failure_tgid_preserved && first_failure.is_none() {
            first_failure = Some("failed exec mutated thread_group_id");
        }
        if failure_error_exact && failure_inherited_preserved && failure_tgid_preserved {
            fail_preserved += 1;
        }

        #[cfg(target_arch = "aarch64")]
        {
            let sibling_pid = {
                let manager_guard = crate::process::manager();
                let Some(manager) = manager_guard.as_ref() else {
                    return TestResult::Fail(
                        "process manager unavailable for exec detach sibling PID",
                    );
                };
                manager.allocate_pid()
            };
            let mut sibling = process_row(sibling_pid, "exec_detach_sibling");
            mark_clone_vm_member(&mut sibling, leader_root, leader_pid);
            {
                let mut manager_guard = crate::process::manager();
                let Some(manager) = manager_guard.as_mut() else {
                    return TestResult::Fail(
                        "process manager unavailable for exec detach sibling insert",
                    );
                };
                manager.insert_process(sibling_pid, sibling);
            }

            let refused_exec = {
                let mut manager_guard = crate::process::manager();
                let Some(manager) = manager_guard.as_mut() else {
                    return TestResult::Fail(
                        "process manager unavailable for exec detach sibling arm",
                    );
                };
                invoke_exec(manager, member_pid, &valid, with_argv)
            };
            let (refused_inherited_cr3, refused_thread_group_id) = {
                let manager_guard = crate::process::manager();
                let Some(manager) = manager_guard.as_ref() else {
                    return TestResult::Fail(
                        "process manager unavailable for exec detach sibling observation",
                    );
                };
                match manager.get_process(member_pid) {
                    Some(process) => (process.inherited_cr3, process.thread_group_id),
                    None => {
                        if first_failure.is_none() {
                            first_failure =
                                Some("exec detach member disappeared after sibling refusal");
                        }
                        (None, None)
                    }
                }
            };
            let sibling_error_exact = matches!(
                refused_exec,
                Err("exec blocked while CLONE_VM sibling shares old address space")
            );
            let sibling_inherited_preserved = refused_inherited_cr3 == Some(leader_root);
            let sibling_tgid_preserved = refused_thread_group_id == Some(leader_pid.as_u64());
            if !sibling_error_exact && first_failure.is_none() {
                first_failure = Some("live CLONE_VM sibling did not block exec");
            }
            if !sibling_inherited_preserved && first_failure.is_none() {
                first_failure = Some("sibling-refused exec mutated inherited_cr3");
            }
            if !sibling_tgid_preserved && first_failure.is_none() {
                first_failure = Some("sibling-refused exec mutated thread_group_id");
            }
            if sibling_error_exact && sibling_inherited_preserved && sibling_tgid_preserved {
                sibling_refused += 1;
            }

            match exit_and_remove_unowned_row(
                sibling_pid,
                "process manager unavailable for exec detach sibling cleanup",
                "exec detach sibling disappeared before cleanup",
            ) {
                Ok(()) => {
                    reclaim_pids[reclaim_pid_count] = sibling_pid.as_u64();
                    reclaim_pid_count += 1;
                }
                Err(reason) if first_failure.is_none() => first_failure = Some(reason),
                Err(_) => {}
            }
        }

        let successful_exec = {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return TestResult::Fail("process manager unavailable for exec detach success arm");
            };
            invoke_exec(manager, member_pid, &valid, with_argv)
        };
        let mut detached = false;
        let mut root_is_fresh = false;
        let mut effective_tgid_is_self = false;
        {
            let manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_ref() else {
                return TestResult::Fail(
                    "process manager unavailable for exec detach success observation",
                );
            };
            if let Some(process) = manager.get_process(member_pid) {
                detached = process.inherited_cr3.is_none() && process.thread_group_id.is_none();
                root_is_fresh = process
                    .page_table
                    .as_ref()
                    .map(|page_table| {
                        page_table.level_4_frame().start_address().as_u64() != leader_root
                    })
                    .unwrap_or(false);
                effective_tgid_is_self =
                    process.thread_group_id.unwrap_or(member_pid.as_u64()) == member_pid.as_u64();
            } else if first_failure.is_none() {
                first_failure = Some("exec detach member disappeared after successful exec");
            }
        }
        if successful_exec.is_err() && first_failure.is_none() {
            first_failure = Some("valid exec fixture did not commit");
        }
        if successful_exec.is_ok() {
            roots_created = roots_created.saturating_add(1);
        }
        if !detached && first_failure.is_none() {
            first_failure = Some("successful exec did not clear both CLONE_VM fields");
        }
        if !root_is_fresh && first_failure.is_none() {
            first_failure = Some("successful exec did not publish an observed fresh root");
        }
        if !effective_tgid_is_self && first_failure.is_none() {
            first_failure = Some("successful exec effective thread-group ID was not self");
        }
        if successful_exec.is_ok() && detached {
            success_detached += 1;
        }
        if successful_exec.is_ok() && root_is_fresh {
            fresh_root += 1;
        }
        if successful_exec.is_ok() && effective_tgid_is_self {
            tgid_self += 1;
        }

        let old_group_post_count =
            signal_thread_group_for_test(leader_pid.as_u64(), crate::signal::constants::SIGUSR1);
        let member_reached_from_old_group =
            take_pending_signal_for_test(member_pid, crate::signal::constants::SIGUSR1);
        let leader_reached_post =
            take_pending_signal_for_test(leader_pid, crate::signal::constants::SIGUSR1);
        if old_group_post_count == 1 && !member_reached_from_old_group && leader_reached_post {
            old_group_missed_post += 1;
        } else if first_failure.is_none() {
            first_failure = Some("a kill aimed at the pre-exec group still reached the exec'd row");
        }

        let self_group_post_count =
            signal_thread_group_for_test(member_pid.as_u64(), crate::signal::constants::SIGUSR1);
        let member_reached_from_self_group =
            take_pending_signal_for_test(member_pid, crate::signal::constants::SIGUSR1);
        if self_group_post_count == 1 && member_reached_from_self_group {
            self_group_reached_post += 1;
        } else if first_failure.is_none() {
            first_failure =
                Some("a kill aimed at the post-exec group failed to reach the exec'd row");
        }

        match retire_and_remove_owned_row(
            member_pid,
            "process manager unavailable for exec detach member cleanup",
            "exec detach member disappeared before cleanup",
        ) {
            Ok(()) => {
                reclaim_pids[reclaim_pid_count] = member_pid.as_u64();
                reclaim_pid_count += 1;
            }
            Err(reason) if first_failure.is_none() => first_failure = Some(reason),
            Err(_) => {}
        }
    }

    match retire_and_remove_owned_row(
        leader_pid,
        "process manager unavailable for exec detach leader cleanup",
        "exec detach leader disappeared before cleanup",
    ) {
        Ok(()) => {
            reclaim_pids[reclaim_pid_count] = leader_pid.as_u64();
            reclaim_pid_count += 1;
        }
        Err(reason) if first_failure.is_none() => first_failure = Some(reason),
        Err(_) => {}
    }

    let quiesce_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    loop {
        crate::task::scheduler::nudge_retirement_grace_for_test();
        let boundary_deadline =
            retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
        while retirement_oracle_clock_now() < boundary_deadline {
            core::hint::spin_loop();
        }
        crate::task::process_task::boot_reclaim_deferred_process_resources();
        if reclaim_pids[..reclaim_pid_count]
            .iter()
            .all(|pid| crate::task::process_task::boot_reclaim_locations(*pid) == (false, false))
        {
            break;
        }
        if retirement_oracle_clock_now() >= quiesce_deadline {
            if first_failure.is_none() {
                first_failure = Some("exec detach deferred cleanup did not quiesce");
            }
            break;
        }
        core::hint::spin_loop();
    }
    core::sync::atomic::fence(Ordering::Acquire);

    let allocator_used_after = frame_allocator_used_frames();
    let stack_residual = allocator_used_after as i64 - allocator_used_before as i64;
    let table_frames_recorded_delta = PT_TABLE_FRAMES_RECORDED
        .aggregate()
        .saturating_sub(table_frames_recorded_before);
    let table_frames_returned_delta = PT_TABLE_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(table_frames_returned_before);
    let roots_retired_delta = PT_ROOTS_RETIRED
        .aggregate()
        .saturating_sub(roots_retired_before);
    let leaf_mappings_recorded_delta = LEAF_MAPPINGS_RECORDED
        .aggregate()
        .saturating_sub(leaf_mappings_recorded_before);
    let leaf_mappings_released_delta = LEAF_MAPPINGS_RELEASED
        .aggregate()
        .saturating_sub(leaf_mappings_released_before);
    let leaf_frames_returned_delta = LEAF_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(leaf_frames_returned_before);
    // The page-table mapping records are released, but external GuardedStack
    // frames are not returned until issue #583 is closed.
    let leaf_residual = leaf_mappings_recorded_delta.saturating_sub(leaf_frames_returned_delta);
    let table_frames_lost_delta = PT_RETIRE_FRAMES_LOST
        .aggregate()
        .saturating_sub(table_frames_lost_before);
    let dropped_undecided_delta = PT_ROOT_DROPPED_UNDECIDED
        .aggregate()
        .saturating_sub(dropped_undecided_before);
    let dropped_mid_retire_delta = PT_ROOT_DROPPED_MID_RETIRE
        .aggregate()
        .saturating_sub(dropped_mid_retire_before);
    let no_arch_delta = PT_ROOT_ABANDONED_NO_ARCH
        .aggregate()
        .saturating_sub(no_arch_before);
    let refusal_counters_after = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];
    let refusal_balance = refusal_counters_after
        .iter()
        .zip(refusal_counters_before.iter())
        .fold(0u64, |balance, (after, before)| {
            balance.saturating_add((*after).abs_diff(*before))
        });
    // PT_TABLE_FRAMES_RECORDED counts intermediate tables; roots have their own
    // custody count and are included in PT_TABLE_FRAMES_RETURNED.
    let table_frames_recorded = table_frames_recorded_delta.saturating_add(roots_created);
    let custody_balance = table_frames_returned_delta
        .abs_diff(table_frames_recorded)
        .saturating_add(roots_retired_delta.abs_diff(roots_created))
        .saturating_add(table_frames_lost_delta)
        .saturating_add(dropped_undecided_delta)
        .saturating_add(dropped_mid_retire_delta)
        .saturating_add(no_arch_delta)
        .saturating_add(refusal_balance);
    core::mem::drop(reclaim_owner);
    if bodies != 2 && first_failure.is_none() {
        first_failure = Some("exec detach oracle did not exercise both exec bodies");
    }
    if fail_preserved != 2 && first_failure.is_none() {
        first_failure = Some("failed exec preservation count was not exact");
    }
    #[cfg(target_arch = "aarch64")]
    if sibling_refused != 2 && first_failure.is_none() {
        first_failure = Some("live-sibling refusal count was not exact");
    }
    #[cfg(target_arch = "x86_64")]
    if sibling_refused != 0 && first_failure.is_none() {
        first_failure = Some("x86 unexpectedly exercised the aarch64 sibling guard");
    }
    if success_detached != 2 && first_failure.is_none() {
        first_failure = Some("successful exec detach count was not exact");
    }
    if fresh_root != 2 && first_failure.is_none() {
        first_failure = Some("fresh exec root count was not exact");
    }
    if tgid_self != 2 && first_failure.is_none() {
        first_failure = Some("self thread-group ID count was not exact");
    }
    if old_group_reached_pre != 2 && first_failure.is_none() {
        first_failure =
            Some("a kill aimed at the pre-exec group failed to reach a still-member row");
    }
    if old_group_missed_post != 2 && first_failure.is_none() {
        first_failure = Some("a kill aimed at the pre-exec group still reached the exec'd row");
    }
    if self_group_reached_post != 2 && first_failure.is_none() {
        first_failure = Some("a kill aimed at the post-exec group failed to reach the exec'd row");
    }
    if roots_created != EXPECTED_ROOTS_CREATED && first_failure.is_none() {
        first_failure = Some("exec detach oracle did not create exactly five roots");
    }
    if table_frames_returned_delta != table_frames_recorded && first_failure.is_none() {
        first_failure = Some("exec detach table-frame custody equality failed");
    }
    if roots_retired_delta != roots_created && first_failure.is_none() {
        first_failure = Some("exec detach root custody equality failed");
    }
    if leaf_mappings_recorded_delta != leaf_mappings_released_delta && first_failure.is_none() {
        first_failure = Some("exec detach leaf mapping release equality failed");
    }
    if (table_frames_lost_delta != 0
        || dropped_undecided_delta != 0
        || dropped_mid_retire_delta != 0
        || no_arch_delta != 0)
        && first_failure.is_none()
    {
        first_failure = Some("exec detach left an unclassified or lost root");
    }
    if refusal_counters_after != refusal_counters_before && first_failure.is_none() {
        first_failure = Some("exec detach triggered an unexpected frame refusal");
    }
    if custody_balance != 0 && first_failure.is_none() {
        first_failure = Some("exec detach custody balance was nonzero");
    }
    if leaf_residual != EXPECTED_LEAF_RESIDUAL && first_failure.is_none() {
        first_failure = Some("exec detach user-stack leaf residual changed");
    }
    if stack_residual != EXPECTED_STACK_RESIDUAL && first_failure.is_none() {
        first_failure = Some("exec detach user-stack residual changed");
    }

    #[cfg(target_arch = "aarch64")]
    let arch = "aarch64";
    #[cfg(target_arch = "x86_64")]
    let arch = "x86";
    crate::serial_println!(
        "[EXEC_DETACH_ORACLE:{}:bodies={}:fail_preserved={}:sibling_refused={}:success_detached={}:fresh_root={}:tgid_self={}:custody_balance={}:leaf_residual={}:stack_residual={}:old_group_reached_pre={}:old_group_missed_post={}:self_group_reached_post={}]",
        arch,
        bodies,
        fail_preserved,
        sibling_refused,
        success_detached,
        fresh_root,
        tgid_self,
        custody_balance,
        leaf_residual,
        stack_residual,
        old_group_reached_pre,
        old_group_missed_post,
        self_group_reached_post
    );
    if let Some(reason) = first_failure {
        return TestResult::Fail(reason);
    }

    crate::serial_println!("[TEST:process:exec_detach_oracle:PASS]");
    TestResult::Pass
}

#[cfg(feature = "boot_tests")]
fn creating_dispatch_reclaim_progress_sample() -> [u64; 4] {
    [
        PT_RETIRE_BUDGET_REQUEUED.aggregate(),
        PT_TABLE_FRAMES_RETURNED.aggregate(),
        PT_ROOTS_RETIRED.aggregate(),
        TEARDOWN_RECLAIM.aggregate(),
    ]
}

#[cfg(feature = "boot_tests")]
fn creating_dispatch_reclaim_once() {
    crate::task::process_task::boot_reclaim_deferred_process_resources();
    #[cfg(target_arch = "aarch64")]
    crate::task::scheduler::reclaim_terminated_threads();
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
/// Block the test kthread for one timer tick so the single-CPU x86 gate must
/// enter the real interrupt-return scheduler before it can sample again.
fn creating_dispatch_x86_scheduler_opportunity() {
    use crate::task::thread::ThreadState;

    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return;
    };
    let (seconds, nanoseconds) = crate::time::get_monotonic_time_ns();
    let wake_time_ns = seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(nanoseconds)
        .saturating_add(1_000_000);
    crate::task::scheduler::with_scheduler(|scheduler| {
        scheduler.block_current_for_timer(wake_time_ns);
    });
    crate::task::scheduler::yield_current();

    loop {
        crate::arch_halt_with_interrupts();
        let still_blocked = crate::task::scheduler::with_scheduler(|scheduler| {
            scheduler
                .get_thread(thread_id)
                .is_some_and(|thread| thread.state == ThreadState::BlockedOnTimer)
        })
        .unwrap_or(false);
        if !still_blocked {
            break;
        }
    }
}

#[cfg(feature = "boot_tests")]
fn creating_dispatch_settle_reclaim() -> (u64, bool) {
    let settle_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    let mut settle_rounds = 0u64;
    let mut stable_rounds = 0u64;
    let mut settle_sample = creating_dispatch_reclaim_progress_sample();
    loop {
        #[cfg(target_arch = "aarch64")]
        let grace_target = crate::task::scheduler::retirement_grace_target();
        crate::task::scheduler::nudge_retirement_grace_for_test();
        #[cfg(target_arch = "aarch64")]
        let grace_elapsed = loop {
            if crate::task::scheduler::retirement_grace_elapsed(&grace_target) {
                break true;
            }
            if retirement_oracle_clock_now() >= settle_deadline {
                break false;
            }
            core::hint::spin_loop();
        };
        #[cfg(target_arch = "x86_64")]
        let grace_elapsed = {
            let boundary_deadline =
                retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
            while retirement_oracle_clock_now() < boundary_deadline {
                core::hint::spin_loop();
            }
            true
        };
        if !grace_elapsed {
            return (settle_rounds, true);
        }
        creating_dispatch_reclaim_once();
        core::sync::atomic::fence(Ordering::Acquire);
        let next_sample = creating_dispatch_reclaim_progress_sample();
        settle_rounds = settle_rounds.saturating_add(1);
        if crate::task::process_task::boot_reclaim_queue_census() == (0, 0)
            && next_sample == settle_sample
        {
            stable_rounds = stable_rounds.saturating_add(1);
            if stable_rounds >= 3 {
                return (settle_rounds, false);
            }
        } else {
            stable_rounds = 0;
        }
        settle_sample = next_sample;
        if retirement_oracle_clock_now() >= settle_deadline {
            return (settle_rounds, true);
        }
        core::hint::spin_loop();
    }
}

#[cfg(feature = "boot_tests")]
fn creating_dispatch_retire_and_remove_owned_row(
    pid: crate::process::ProcessId,
    probe_reference_clear: [bool; 3],
    retirement_blockers: &mut [bool; 3],
) -> Result<(), &'static str> {
    let reclaim = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return Err("process manager unavailable for creating-dispatch cleanup");
        };
        let Some(process) = manager.get_process_mut(pid) else {
            return Err("creating-dispatch row disappeared before cleanup");
        };
        crate::task::process_task::defer_process_resources(process)
    };

    let observed = crate::task::process_task::boot_root_reference_blockers(&reclaim);
    *retirement_blockers = [
        !probe_reference_clear[0] || observed.0,
        !probe_reference_clear[1] || observed.1,
        !probe_reference_clear[2] || observed.2,
    ];
    if retirement_blockers
        .iter()
        .copied()
        .any(core::convert::identity)
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return Err("process manager unavailable for creating-dispatch root restoration");
        };
        let Some(process) = manager.get_process_mut(pid) else {
            return Err("creating-dispatch row disappeared before root restoration");
        };
        crate::task::process_task::boot_restore_process_resources(process, reclaim)?;
        return Err(
            "creating-dispatch root retained a hardware/shadow/cached reference at retirement",
        );
    }

    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return Err("process manager unavailable for creating-dispatch row removal");
        };
        let Some(process) = manager.get_process_mut(pid) else {
            return Err("creating-dispatch row disappeared before removal");
        };
        crate::task::process_task::release_process_resources(process);
        manager.remove_from_ready_queue(pid);
        manager.remove_process(pid);
    }
    crate::task::process_task::enqueue_process_reclaim(reclaim);
    Ok(())
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn creating_dispatch_refusal_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;
    use alloc::boxed::Box;

    /// This oracle creates exactly one real process row through
    /// `manager.create_process(...)`; its 64 KiB user stack is 16 x 4 KiB
    /// frames. `GuardedStack::drop` does not reclaim them (issue #583), so all
    /// 16 mappings remain recorded instead of returned. Counted rather than
    /// freed is deliberate: a counted leak beats an over-free. Closing #583
    /// should drive this and the exec-detach residuals to zero together.
    const EXPECTED_LEAF_RESIDUAL: u64 = 16;
    /// The same unreclaimed 16 x 4 KiB user-stack frames leave the allocator
    /// exactly 16 frames heavier. This is the same issue #583 residual class
    /// pinned by `exec_detach_oracle_test`, and should reach zero with it.
    const EXPECTED_USER_STACK_RESIDUAL: i64 = 16;

    let reclaim_owner = match crate::task::process_task::BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(_) => {
            return TestResult::Fail(
                "reclaim queues not quiescent at creating-dispatch oracle start",
            )
        }
    };
    let allocator_used_before = frame_allocator_used_frames();
    let table_frames_recorded_before = PT_TABLE_FRAMES_RECORDED.aggregate();
    let table_frames_returned_before = PT_TABLE_FRAMES_RETURNED.aggregate();
    let roots_retired_before = PT_ROOTS_RETIRED.aggregate();
    let leaf_mappings_recorded_before = LEAF_MAPPINGS_RECORDED.aggregate();
    let leaf_mappings_released_before = LEAF_MAPPINGS_RELEASED.aggregate();
    let leaf_frames_returned_before = LEAF_FRAMES_RETURNED.aggregate();
    let table_frames_lost_before = PT_RETIRE_FRAMES_LOST.aggregate();
    let dropped_undecided_before = PT_ROOT_DROPPED_UNDECIDED.aggregate();
    let dropped_mid_retire_before = PT_ROOT_DROPPED_MID_RETIRE.aggregate();
    let no_arch_before = PT_ROOT_ABANDONED_NO_ARCH.aggregate();
    let refusal_counters_before = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];

    let valid = crate::memory::process_memory::valid_executable_fixture();
    let pid = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for creating-dispatch process");
        };
        match manager.create_process(
            alloc::string::String::from("creating_dispatch_probe"),
            &valid,
        ) {
            Ok(pid) => pid,
            Err(_) => {
                return TestResult::Fail("creating-dispatch process creation failed");
            }
        }
    };

    let (thread_id, root, thread_box) = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for creating-dispatch attach");
        };
        let Some(process) = manager.get_process_mut(pid) else {
            return TestResult::Fail("creating-dispatch row disappeared before probe attach");
        };
        let Some(root) = process
            .page_table
            .as_ref()
            .map(|page_table| page_table.level_4_frame().start_address().as_u64())
        else {
            return TestResult::Fail("creating-dispatch probe row had no page-table root");
        };
        let Some(process_thread) = process.main_thread.as_mut() else {
            return TestResult::Fail("creating-dispatch probe attach did not persist");
        };
        let thread_id = process_thread.id;
        let Some(kernel_stack_top) = process_thread.kernel_stack_top else {
            return TestResult::Fail("creating-dispatch main thread had no kernel stack");
        };
        process_thread.context = crate::task::thread::CpuContext::new_kernel_thread(
            creating_dispatch_probe_entry as u64,
            kernel_stack_top.as_u64(),
        );
        process_thread.context.x0 = thread_id;
        process_thread.context.x1 = root;
        process_thread.blocked_in_syscall = true;
        let scheduler_thread = process_thread.clone();
        process.force_unpublished_for_test();
        (thread_id, root, Box::new(scheduler_thread))
    };

    PROBE_DISPATCHED.store(false, Ordering::Release);
    PROBE_ROOT_RELEASE_OBSERVATION.store(0, Ordering::Release);
    let refused_before =
        crate::arch_impl::aarch64::context_switch::userspace_dispatch_creating_refused();
    crate::task::scheduler::spawn_on_cpu_for_test(thread_box, 1);

    let refusal_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    let refusal_delta = loop {
        let observed =
            crate::arch_impl::aarch64::context_switch::userspace_dispatch_creating_refused()
                .saturating_sub(refused_before);
        if observed >= 2 || retirement_oracle_clock_now() >= refusal_deadline {
            break observed;
        }
        core::hint::spin_loop();
    };
    let dispatched_before_publish = PROBE_DISPATCHED.load(Ordering::Acquire);

    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for creating-dispatch publish");
        };
        let Some(process) = manager.get_process_mut(pid) else {
            return TestResult::Fail("creating-dispatch row disappeared before publication");
        };
        process.set_ready();
    }

    let dispatch_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    while (!PROBE_DISPATCHED.load(Ordering::Acquire)
        || PROBE_ROOT_RELEASE_OBSERVATION.load(Ordering::Acquire) & PROBE_ROOT_RELEASE_DONE == 0)
        && retirement_oracle_clock_now() < dispatch_deadline
    {
        core::hint::spin_loop();
    }
    let dispatched_after_publish =
        !dispatched_before_publish && PROBE_DISPATCHED.load(Ordering::Acquire);
    let probe_root_observation = PROBE_ROOT_RELEASE_OBSERVATION.load(Ordering::Acquire);
    let probe_reference_clear = [
        probe_root_observation & PROBE_ROOT_HARDWARE_CLEAR != 0,
        probe_root_observation & PROBE_ROOT_SHADOW_CLEAR != 0,
        probe_root_observation & PROBE_ROOT_CACHED_CLEAR != 0,
    ];

    let mut first_failure = if refusal_delta == 0 {
        Some("creating row was not refused through scheduler dispatch")
    } else if refusal_delta < 2 {
        Some("creating-dispatch refusal did not requeue for a real retry")
    } else if dispatched_before_publish {
        Some("creating-dispatch probe ran before process publication")
    } else if !dispatched_after_publish {
        Some("creating-dispatch probe did not run after process publication")
    } else {
        None
    };

    let mut retirement_blockers = [true; 3];
    if let Err(reason) = creating_dispatch_retire_and_remove_owned_row(
        pid,
        probe_reference_clear,
        &mut retirement_blockers,
    )
    {
        if first_failure.is_none() {
            first_failure = Some(reason);
        }
    }

    let cleanup_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    loop {
        crate::task::scheduler::nudge_retirement_grace_for_test();
        let boundary_deadline =
            retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
        while retirement_oracle_clock_now() < boundary_deadline {
            core::hint::spin_loop();
        }
        crate::task::process_task::boot_reclaim_deferred_process_resources();
        crate::task::scheduler::reclaim_terminated_threads();
        let process_reclaimed =
            crate::task::process_task::boot_reclaim_locations(pid.as_u64()) == (false, false);
        let thread_reclaimed = crate::task::scheduler::with_scheduler(|scheduler| {
            scheduler.get_thread(thread_id).is_none()
        })
        .unwrap_or(false);
        if process_reclaimed && thread_reclaimed {
            break;
        }
        if retirement_oracle_clock_now() >= cleanup_deadline {
            if first_failure.is_none() {
                first_failure = Some("creating-dispatch deferred cleanup did not quiesce");
            }
            break;
        }
        core::hint::spin_loop();
    }
    crate::task::scheduler::clear_cpu_affinity_for_test(thread_id);

    let (settle_rounds, settle_timed_out) = creating_dispatch_settle_reclaim();
    core::sync::atomic::fence(Ordering::Acquire);

    let allocator_used_after = frame_allocator_used_frames();
    let user_stack_residual = allocator_used_after as i64 - allocator_used_before as i64;
    let table_frames_recorded_delta = PT_TABLE_FRAMES_RECORDED
        .aggregate()
        .saturating_sub(table_frames_recorded_before);
    let table_frames_returned_delta = PT_TABLE_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(table_frames_returned_before);
    let roots_retired_delta = PT_ROOTS_RETIRED
        .aggregate()
        .saturating_sub(roots_retired_before);
    let leaf_mappings_recorded_delta = LEAF_MAPPINGS_RECORDED
        .aggregate()
        .saturating_sub(leaf_mappings_recorded_before);
    let leaf_mappings_released_delta = LEAF_MAPPINGS_RELEASED
        .aggregate()
        .saturating_sub(leaf_mappings_released_before);
    let leaf_frames_returned_delta = LEAF_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(leaf_frames_returned_before);
    let leaf_residual = leaf_mappings_recorded_delta.saturating_sub(leaf_frames_returned_delta);
    let table_frames_lost_delta = PT_RETIRE_FRAMES_LOST
        .aggregate()
        .saturating_sub(table_frames_lost_before);
    let dropped_undecided_delta = PT_ROOT_DROPPED_UNDECIDED
        .aggregate()
        .saturating_sub(dropped_undecided_before);
    let dropped_mid_retire_delta = PT_ROOT_DROPPED_MID_RETIRE
        .aggregate()
        .saturating_sub(dropped_mid_retire_before);
    let no_arch_delta = PT_ROOT_ABANDONED_NO_ARCH
        .aggregate()
        .saturating_sub(no_arch_before);
    let refusal_counters_after = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];
    let refusal_balance = refusal_counters_after
        .iter()
        .zip(refusal_counters_before.iter())
        .fold(0u64, |balance, (after, before)| {
            balance.saturating_add((*after).abs_diff(*before))
        });
    let table_frames_recorded = table_frames_recorded_delta.saturating_add(1);
    let custody_balance = table_frames_returned_delta
        .abs_diff(table_frames_recorded)
        .saturating_add(roots_retired_delta.abs_diff(1))
        .saturating_add(table_frames_lost_delta)
        .saturating_add(dropped_undecided_delta)
        .saturating_add(dropped_mid_retire_delta)
        .saturating_add(no_arch_delta)
        .saturating_add(refusal_balance);
    core::mem::drop(reclaim_owner);

    if table_frames_returned_delta != table_frames_recorded && first_failure.is_none() {
        first_failure = Some("creating-dispatch table-frame custody equality failed");
    }
    if roots_retired_delta != 1 && first_failure.is_none() {
        first_failure = Some("creating-dispatch root custody equality failed");
    }
    if leaf_mappings_recorded_delta != leaf_mappings_released_delta && first_failure.is_none() {
        first_failure = Some("creating-dispatch leaf mapping release equality failed");
    }
    if (table_frames_lost_delta != 0
        || dropped_undecided_delta != 0
        || dropped_mid_retire_delta != 0
        || no_arch_delta != 0)
        && first_failure.is_none()
    {
        first_failure = Some("creating-dispatch left an unclassified or lost root");
    }
    if refusal_counters_after != refusal_counters_before && first_failure.is_none() {
        first_failure = Some("creating-dispatch triggered an unexpected frame refusal");
    }
    if custody_balance != 0 && first_failure.is_none() {
        first_failure = Some("creating-dispatch custody balance was nonzero");
    }
    if leaf_residual != EXPECTED_LEAF_RESIDUAL && first_failure.is_none() {
        first_failure = Some("creating-dispatch user-stack leaf residual changed");
    }
    if user_stack_residual != EXPECTED_USER_STACK_RESIDUAL && first_failure.is_none() {
        first_failure = Some("creating-dispatch user-stack residual changed");
    }

    crate::serial_println!(
        "[CREATING_DISPATCH_ORACLE_DIAG:aarch64:refusal_delta={}:leaf_residual={}:user_stack_residual={}:balance={}:settle_rounds={}:root={:#x}:root_release_done={}:probe_hw_clear={}:probe_shadow_clear={}:probe_cached_clear={}:retire_hw_blocked={}:retire_shadow_blocked={}:retire_cached_blocked={}]",
        refusal_delta,
        leaf_residual,
        user_stack_residual,
        custody_balance,
        settle_rounds,
        root,
        usize::from(probe_root_observation & PROBE_ROOT_RELEASE_DONE != 0),
        usize::from(probe_reference_clear[0]),
        usize::from(probe_reference_clear[1]),
        usize::from(probe_reference_clear[2]),
        usize::from(retirement_blockers[0]),
        usize::from(retirement_blockers[1]),
        usize::from(retirement_blockers[2])
    );
    if settle_timed_out {
        return TestResult::Fail(
            "creating-dispatch settle timed out before queues and counters stabilized",
        );
    }
    if let Some(reason) = first_failure {
        return TestResult::Fail(reason);
    }

    crate::serial_println!(
        "[CREATING_DISPATCH_ORACLE:aarch64:injected=1:refused_via_dispatch=1:requeue_retried=1:dispatched_after_publish=1:balance=0:leaf_residual={}:user_stack_residual={}]",
        leaf_residual,
        user_stack_residual
    );
    crate::serial_println!("[TEST:process:creating_dispatch_refusal:PASS]");
    TestResult::Pass
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn creating_dispatch_refusal_x86_test() -> crate::test_framework::registry::TestResult {
    use crate::task::thread::{CpuContext, ThreadPrivilege, ThreadState};
    use crate::test_framework::registry::TestResult;
    use alloc::boxed::Box;
    use x86_64::VirtAddr;

    // REMOTE-BOOT PLACEHOLDERS: these deliberately impossible sentinels keep
    // the gate red until a real x86 boot reports both measured values on the
    // unconditional CREATING_DISPATCH_ORACLE_DIAG line. Replace the constants,
    // the structural pins, and the gate literal together from that observation.
    const EXPECTED_X86_LEAF_RESIDUAL_REPLACE_AFTER_REMOTE_BOOT: u64 = u64::MAX;
    const EXPECTED_X86_USER_STACK_RESIDUAL_REPLACE_AFTER_REMOTE_BOOT: i64 = i64::MIN;

    let reclaim_owner = match crate::task::process_task::BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(_) => {
            return TestResult::Fail(
                "reclaim queues not quiescent at x86 creating-dispatch oracle start",
            )
        }
    };
    let allocator_used_before = frame_allocator_used_frames();
    let table_frames_recorded_before = PT_TABLE_FRAMES_RECORDED.aggregate();
    let table_frames_returned_before = PT_TABLE_FRAMES_RETURNED.aggregate();
    let roots_retired_before = PT_ROOTS_RETIRED.aggregate();
    let leaf_mappings_recorded_before = LEAF_MAPPINGS_RECORDED.aggregate();
    let leaf_mappings_released_before = LEAF_MAPPINGS_RELEASED.aggregate();
    let leaf_frames_returned_before = LEAF_FRAMES_RETURNED.aggregate();
    let table_frames_lost_before = PT_RETIRE_FRAMES_LOST.aggregate();
    let dropped_undecided_before = PT_ROOT_DROPPED_UNDECIDED.aggregate();
    let dropped_mid_retire_before = PT_ROOT_DROPPED_MID_RETIRE.aggregate();
    let no_arch_before = PT_ROOT_ABANDONED_NO_ARCH.aggregate();
    let refusal_counters_before = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];

    let valid = crate::memory::process_memory::x86_valid_executable_fixture();
    let pid = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail(
                "process manager unavailable for x86 creating-dispatch process",
            );
        };
        match manager.create_process(
            alloc::string::String::from("creating_dispatch_probe_x86"),
            &valid,
        ) {
            Ok(pid) => pid,
            Err(_) => {
                return TestResult::Fail("x86 creating-dispatch process creation failed");
            }
        }
    };

    let (thread_id, root, tls_block, injected, thread_box) = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail(
                "process manager unavailable for x86 creating-dispatch attach",
            );
        };
        let Some(process) = manager.get_process_mut(pid) else {
            return TestResult::Fail("x86 creating-dispatch row disappeared before probe attach");
        };
        let Some(root) = process
            .page_table
            .as_ref()
            .map(|page_table| page_table.level_4_frame().start_address().as_u64())
        else {
            return TestResult::Fail("x86 creating-dispatch probe row had no page-table root");
        };
        let Some(process_thread) = process.main_thread.as_mut() else {
            return TestResult::Fail("x86 creating-dispatch probe attach did not persist");
        };
        let thread_id = process_thread.id;
        let tls_block = process_thread.tls_block;
        let Some(kernel_stack_top) = process_thread.kernel_stack_top else {
            return TestResult::Fail("x86 creating-dispatch main thread had no kernel stack");
        };
        // Keep the scheduler-visible privilege User so switch_to_thread reaches
        // the blocked-in-syscall admission arm. Only the saved return frame is
        // kernel-mode, which that arm restores after the process is published.
        process_thread.context = CpuContext::new(
            VirtAddr::new(creating_dispatch_probe_entry_x86 as usize as u64),
            kernel_stack_top,
            ThreadPrivilege::Kernel,
        );
        process_thread.context.rdi = thread_id;
        process_thread.context.rsi = root;
        process_thread.context.rflags = 0x202;
        process_thread.blocked_in_syscall = true;
        let thread_is_runnable_user_fixture = process_thread.privilege == ThreadPrivilege::User
            && process_thread.state == ThreadState::Ready
            && process_thread.saved_userspace_context.is_none();
        let scheduler_thread = process_thread.clone();
        process.force_unpublished_for_test();
        let injected = process.is_unpublished()
            && thread_is_runnable_user_fixture
            && scheduler_thread.saved_userspace_context.is_none();
        (thread_id, root, tls_block, injected, Box::new(scheduler_thread))
    };
    let tls_registered = !tls_block.is_null()
        && crate::tls::get_thread_tls_block(thread_id).is_some_and(|registered| {
            registered == tls_block
        });

    X86_CREATING_DISPATCH_PROBE_DISPATCHED.store(false, Ordering::Release);
    X86_CREATING_DISPATCH_ROOT_OBSERVATION.store(0, Ordering::Release);
    let refused_before =
        crate::interrupts::context_switch::userspace_dispatch_creating_refused();
    crate::task::scheduler::spawn(thread_box);

    let refusal_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    let refusal_delta = loop {
        let observed =
            crate::interrupts::context_switch::userspace_dispatch_creating_refused()
                .saturating_sub(refused_before);
        if observed >= 2 || retirement_oracle_clock_now() >= refusal_deadline {
            break observed;
        }
        creating_dispatch_x86_scheduler_opportunity();
    };
    let dispatched_before_publish =
        X86_CREATING_DISPATCH_PROBE_DISPATCHED.load(Ordering::Acquire);

    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail(
                "process manager unavailable for x86 creating-dispatch publish",
            );
        };
        let Some(process) = manager.get_process_mut(pid) else {
            return TestResult::Fail("x86 creating-dispatch row disappeared before publication");
        };
        process.set_ready();
    }

    let dispatch_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    while (!X86_CREATING_DISPATCH_PROBE_DISPATCHED.load(Ordering::Acquire)
        || X86_CREATING_DISPATCH_ROOT_OBSERVATION.load(Ordering::Acquire)
            & X86_CREATING_DISPATCH_ROOT_RELEASE_DONE
            == 0)
        && retirement_oracle_clock_now() < dispatch_deadline
    {
        creating_dispatch_x86_scheduler_opportunity();
    }
    let dispatched_after_publish = !dispatched_before_publish
        && X86_CREATING_DISPATCH_PROBE_DISPATCHED.load(Ordering::Acquire);
    let probe_root_observation =
        X86_CREATING_DISPATCH_ROOT_OBSERVATION.load(Ordering::Acquire);
    let probe_reference_clear = [
        probe_root_observation & X86_CREATING_DISPATCH_ROOT_HARDWARE_CLEAR != 0,
        probe_root_observation & X86_CREATING_DISPATCH_ROOT_SHADOW_CLEAR != 0,
        probe_root_observation & X86_CREATING_DISPATCH_ROOT_CACHED_CLEAR != 0,
    ];

    let mut first_failure = if !injected {
        Some("x86 creating-dispatch scheduler fixture was not a runnable unpublished user thread")
    } else if !tls_registered {
        Some("x86 creating-dispatch main-thread TLS registration was missing")
    } else if refusal_delta == 0 {
        Some("x86 creating row was not refused through scheduler dispatch")
    } else if refusal_delta < 2 {
        Some("x86 creating-dispatch refusal did not requeue for a real retry")
    } else if dispatched_before_publish {
        Some("x86 creating-dispatch probe ran before process publication")
    } else if !dispatched_after_publish {
        Some("x86 creating-dispatch probe did not run after process publication")
    } else {
        None
    };

    let mut retirement_blockers = [true; 3];
    if let Err(reason) = creating_dispatch_retire_and_remove_owned_row(
        pid,
        probe_reference_clear,
        &mut retirement_blockers,
    ) {
        if first_failure.is_none() {
            first_failure = Some(reason);
        }
    }

    let cleanup_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    loop {
        crate::task::scheduler::nudge_retirement_grace_for_test();
        let boundary_deadline =
            retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
        while retirement_oracle_clock_now() < boundary_deadline {
            core::hint::spin_loop();
        }
        creating_dispatch_reclaim_once();
        let process_reclaimed =
            crate::task::process_task::boot_reclaim_locations(pid.as_u64()) == (false, false);
        let thread_quiesced = crate::task::scheduler::with_scheduler(|scheduler| {
            scheduler
                .get_thread(thread_id)
                .is_none_or(|thread| thread.state == ThreadState::Terminated)
        })
        .unwrap_or(false);
        if process_reclaimed && thread_quiesced {
            break;
        }
        if retirement_oracle_clock_now() >= cleanup_deadline {
            if first_failure.is_none() {
                first_failure = Some("x86 creating-dispatch deferred cleanup did not quiesce");
            }
            break;
        }
        core::hint::spin_loop();
    }

    let (settle_rounds, settle_timed_out) = creating_dispatch_settle_reclaim();
    core::sync::atomic::fence(Ordering::Acquire);

    let allocator_used_after = frame_allocator_used_frames();
    let user_stack_residual = allocator_used_after as i64 - allocator_used_before as i64;
    let table_frames_recorded_delta = PT_TABLE_FRAMES_RECORDED
        .aggregate()
        .saturating_sub(table_frames_recorded_before);
    let table_frames_returned_delta = PT_TABLE_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(table_frames_returned_before);
    let roots_retired_delta = PT_ROOTS_RETIRED
        .aggregate()
        .saturating_sub(roots_retired_before);
    let leaf_mappings_recorded_delta = LEAF_MAPPINGS_RECORDED
        .aggregate()
        .saturating_sub(leaf_mappings_recorded_before);
    let leaf_mappings_released_delta = LEAF_MAPPINGS_RELEASED
        .aggregate()
        .saturating_sub(leaf_mappings_released_before);
    let leaf_frames_returned_delta = LEAF_FRAMES_RETURNED
        .aggregate()
        .saturating_sub(leaf_frames_returned_before);
    let leaf_residual = leaf_mappings_recorded_delta.saturating_sub(leaf_frames_returned_delta);
    let table_frames_lost_delta = PT_RETIRE_FRAMES_LOST
        .aggregate()
        .saturating_sub(table_frames_lost_before);
    let dropped_undecided_delta = PT_ROOT_DROPPED_UNDECIDED
        .aggregate()
        .saturating_sub(dropped_undecided_before);
    let dropped_mid_retire_delta = PT_ROOT_DROPPED_MID_RETIRE
        .aggregate()
        .saturating_sub(dropped_mid_retire_before);
    let no_arch_delta = PT_ROOT_ABANDONED_NO_ARCH
        .aggregate()
        .saturating_sub(no_arch_before);
    let refusal_counters_after = [
        FRAME_RETURN_REFUSED_DOUBLE.aggregate(),
        FRAME_RETURN_REFUSED_STALE.aggregate(),
        FRAME_RETURN_REFUSED_NEVER_ALLOCATED.aggregate(),
        FRAME_RETURN_REFUSED_UNTRACKED.aggregate(),
        FRAME_DUPLICATE_ALLOC_REFUSED.aggregate(),
        FRAME_RETURN_REFUSED_LIVE_LEAF.aggregate(),
        LEAF_DECREF_UNREGISTERED.aggregate(),
        LEAF_CUSTODY_REFUSED.aggregate(),
    ];
    let refusal_balance = refusal_counters_after
        .iter()
        .zip(refusal_counters_before.iter())
        .fold(0u64, |balance, (after, before)| {
            balance.saturating_add((*after).abs_diff(*before))
        });
    let table_frames_recorded = table_frames_recorded_delta.saturating_add(1);
    let custody_balance = table_frames_returned_delta
        .abs_diff(table_frames_recorded)
        .saturating_add(roots_retired_delta.abs_diff(1))
        .saturating_add(table_frames_lost_delta)
        .saturating_add(dropped_undecided_delta)
        .saturating_add(dropped_mid_retire_delta)
        .saturating_add(no_arch_delta)
        .saturating_add(refusal_balance);
    core::mem::drop(reclaim_owner);

    if table_frames_returned_delta != table_frames_recorded && first_failure.is_none() {
        first_failure = Some("x86 creating-dispatch table-frame custody equality failed");
    }
    if roots_retired_delta != 1 && first_failure.is_none() {
        first_failure = Some("x86 creating-dispatch root custody equality failed");
    }
    if leaf_mappings_recorded_delta != leaf_mappings_released_delta && first_failure.is_none() {
        first_failure = Some("x86 creating-dispatch leaf mapping release equality failed");
    }
    if (table_frames_lost_delta != 0
        || dropped_undecided_delta != 0
        || dropped_mid_retire_delta != 0
        || no_arch_delta != 0)
        && first_failure.is_none()
    {
        first_failure = Some("x86 creating-dispatch left an unclassified or lost root");
    }
    if refusal_counters_after != refusal_counters_before && first_failure.is_none() {
        first_failure = Some("x86 creating-dispatch triggered an unexpected frame refusal");
    }
    if custody_balance != 0 && first_failure.is_none() {
        first_failure = Some("x86 creating-dispatch custody balance was nonzero");
    }
    if leaf_residual != EXPECTED_X86_LEAF_RESIDUAL_REPLACE_AFTER_REMOTE_BOOT
        && first_failure.is_none()
    {
        first_failure = Some("x86 creating-dispatch user-stack leaf residual changed");
    }
    if user_stack_residual != EXPECTED_X86_USER_STACK_RESIDUAL_REPLACE_AFTER_REMOTE_BOOT
        && first_failure.is_none()
    {
        first_failure = Some("x86 creating-dispatch user-stack residual changed");
    }

    crate::serial_println!(
        "[CREATING_DISPATCH_ORACLE_DIAG:x86:refusal_delta={}:leaf_residual={}:user_stack_residual={}:balance={}:settle_rounds={}:root={:#x}:root_release_done={}:probe_hw_clear={}:probe_shadow_clear={}:probe_cached_clear={}:retire_hw_blocked={}:retire_shadow_blocked={}:retire_cached_blocked={}:tls_registered={}]",
        refusal_delta,
        leaf_residual,
        user_stack_residual,
        custody_balance,
        settle_rounds,
        root,
        usize::from(
            probe_root_observation & X86_CREATING_DISPATCH_ROOT_RELEASE_DONE != 0
        ),
        usize::from(probe_reference_clear[0]),
        usize::from(probe_reference_clear[1]),
        usize::from(probe_reference_clear[2]),
        usize::from(retirement_blockers[0]),
        usize::from(retirement_blockers[1]),
        usize::from(retirement_blockers[2]),
        usize::from(tls_registered)
    );
    if settle_timed_out {
        return TestResult::Fail(
            "x86 creating-dispatch settle timed out before queues and counters stabilized",
        );
    }
    if let Some(reason) = first_failure {
        return TestResult::Fail(reason);
    }

    crate::serial_println!(
        "[CREATING_DISPATCH_ORACLE:x86:injected=1:refused_via_dispatch=1:requeue_retried=1:dispatched_after_publish=1:balance=0:leaf_residual={}:user_stack_residual={}]",
        leaf_residual,
        user_stack_residual
    );
    TestResult::Pass
}

#[cfg(feature = "boot_tests")]
pub fn clone_admission_oracle_test() -> crate::test_framework::registry::TestResult {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    use crate::test_framework::registry::TestResult;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    fn test_user_entry() {}

    fn test_thread(
        pid: crate::process::ProcessId,
        name: &'static str,
        state: crate::task::thread::ThreadState,
    ) -> crate::task::thread::Thread {
        let mut thread = crate::task::thread::Thread::new(
            alloc::string::String::from(name),
            test_user_entry,
            VirtAddr::new(0x0080_0000),
            VirtAddr::new(0x007f_0000),
            VirtAddr::new(0x0001_0000),
            crate::task::thread::ThreadPrivilege::Kernel,
        );
        thread.owner_pid = Some(pid.as_u64());
        thread.state = state;
        thread
    }

    fn published_dispatch_refused(process: &crate::process::Process) -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            let thread_id = process
                .main_thread
                .as_ref()
                .map(|thread| thread.id)
                .unwrap_or(0);
            crate::interrupts::context_switch::refuse_unpublished_dispatch(
                process,
                thread_id,
                process.id.as_u64(),
            )
        }
        #[cfg(target_arch = "aarch64")]
        {
            // The aarch64 predicate is the exact check used by
            // set_next_ttbr0_for_thread before it reads either TTBR0 source.
            crate::arch_impl::aarch64::context_switch::refuse_unpublished_dispatch(process)
        }
    }

    fn creating_refused_count() -> u64 {
        #[cfg(target_arch = "x86_64")]
        {
            crate::interrupts::context_switch::userspace_dispatch_creating_refused()
        }
        #[cfg(target_arch = "aarch64")]
        {
            crate::arch_impl::aarch64::context_switch::userspace_dispatch_creating_refused()
        }
    }

    fn exit_and_remove_row(
        pid: crate::process::ProcessId,
        unavailable_reason: &'static str,
        missing_reason: &'static str,
    ) -> Result<(), &'static str> {
        let thread_id = {
            let manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_ref() else {
                return Err(unavailable_reason);
            };
            manager
                .get_process(pid)
                .and_then(|process| process.main_thread.as_ref())
                .map(|thread| thread.id)
                .ok_or(missing_reason)?
        };
        crate::process::exit_process_for_teardown_test(pid, 0);
        crate::task::process_task::ProcessScheduler::handle_thread_exit(thread_id, 0);
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return Err(unavailable_reason);
        };
        manager.remove_from_ready_queue(pid);
        manager.remove_process(pid);
        Ok(())
    }

    let reclaim_owner = match crate::task::process_task::BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(_) => {
            return TestResult::Fail("reclaim queues not quiescent at clone admission oracle start")
        }
    };
    let allocator_used_before = frame_allocator_used_frames();
    let admitted_before = clone_admission_admitted();
    let refused_before = clone_admission_refused();
    let creating_before = creating_refused_count();
    let mut first_failure: Option<&'static str> = None;

    let (row_a_pid, live_admitted, terminated_refused, missing_refused) = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for clone admission rows");
        };
        let row_a_pid = manager.allocate_pid();
        let mut row_a = crate::process::Process::new(
            row_a_pid,
            alloc::string::String::from("clone_admission_a"),
            VirtAddr::new(0x0040_0000),
        );
        row_a.set_main_thread(test_thread(
            row_a_pid,
            "clone_admission_a_main",
            crate::task::thread::ThreadState::Ready,
        ));
        manager.insert_process(row_a_pid, row_a);
        let live_admitted = manager.admit_clone_into(row_a_pid);
        if let Some(row_a) = manager.get_process_mut(row_a_pid) {
            row_a.terminate_minimal(0);
        }
        let terminated_refused = !manager.admit_clone_into(row_a_pid);
        let missing_pid = manager.allocate_pid();
        let missing_refused = !manager.admit_clone_into(missing_pid);
        (
            row_a_pid,
            live_admitted,
            terminated_refused,
            missing_refused,
        )
    };

    if !live_admitted {
        first_failure = Some("live row refused clone admission");
    }
    if !terminated_refused && first_failure.is_none() {
        first_failure = Some("terminated row admitted clone");
    }
    if !missing_refused && first_failure.is_none() {
        first_failure = Some("missing row admitted clone");
    }

    let row_b_pid = {
        let manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_ref() else {
            return TestResult::Fail("process manager unavailable for clone admission row B PID");
        };
        manager.allocate_pid()
    };
    let mut row_b = crate::process::Process::new(
        row_b_pid,
        alloc::string::String::from("clone_admission_b"),
        VirtAddr::new(0x0040_0000),
    );
    row_b.attach_main_thread_unpublished(test_thread(
        row_b_pid,
        "clone_admission_b_unpublished",
        crate::task::thread::ThreadState::Blocked,
    ));

    let creating_refused = published_dispatch_refused(&row_b);
    let creating_after = creating_refused_count();
    let mut published_admitted = 0u64;

    row_b.set_ready();
    let ready_before = creating_refused_count();
    if !published_dispatch_refused(&row_b) && creating_refused_count() == ready_before {
        published_admitted += 1;
    } else if first_failure.is_none() {
        first_failure = Some("set_ready row was refused as unpublished");
    }

    row_b.state = crate::process::ProcessState::Creating;
    row_b.set_main_thread(test_thread(
        row_b_pid,
        "clone_admission_b_published",
        crate::task::thread::ThreadState::Ready,
    ));
    let main_thread_before = creating_refused_count();
    if !published_dispatch_refused(&row_b) && creating_refused_count() == main_thread_before {
        published_admitted += 1;
    } else if first_failure.is_none() {
        first_failure = Some("set_main_thread row was refused as unpublished");
    }

    if (!creating_refused || creating_after.saturating_sub(creating_before) != 1)
        && first_failure.is_none()
    {
        first_failure = Some("creating row refusal was not exact");
    }

    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail(
                "process manager unavailable for clone admission row B insert",
            );
        };
        manager.insert_process(row_b_pid, row_b);
    }

    for (pid, unavailable_reason, missing_reason) in [
        (
            row_a_pid,
            "process manager unavailable for clone admission row A cleanup",
            "clone admission row A disappeared before cleanup",
        ),
        (
            row_b_pid,
            "process manager unavailable for clone admission row B cleanup",
            "clone admission row B disappeared before cleanup",
        ),
    ] {
        match exit_and_remove_row(pid, unavailable_reason, missing_reason) {
            Ok(()) => {}
            Err(reason) if first_failure.is_none() => first_failure = Some(reason),
            Err(_) => {}
        }
    }

    let quiesce_deadline =
        retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(5_000));
    while [row_a_pid, row_b_pid].iter().any(|pid| {
        crate::task::process_task::boot_reclaim_locations(pid.as_u64()) != (false, false)
    }) {
        crate::task::scheduler::nudge_retirement_grace_for_test();
        let boundary_deadline =
            retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
        while retirement_oracle_clock_now() < boundary_deadline {
            core::hint::spin_loop();
        }
        crate::task::process_task::boot_reclaim_deferred_process_resources();
        if retirement_oracle_clock_now() >= quiesce_deadline {
            if first_failure.is_none() {
                first_failure = Some("clone admission deferred cleanup did not quiesce");
            }
            break;
        }
    }
    core::sync::atomic::fence(Ordering::Acquire);

    let admitted = clone_admission_admitted().saturating_sub(admitted_before);
    let refused = clone_admission_refused().saturating_sub(refused_before);
    let creating_refused = creating_refused_count().saturating_sub(creating_before);
    let allocator_used_after = frame_allocator_used_frames();
    let balance = allocator_used_after as i64 - allocator_used_before as i64;
    core::mem::drop(reclaim_owner);

    if admitted != 1 && first_failure.is_none() {
        first_failure = Some("clone admitted counter delta was not exact");
    }
    if refused != 2 && first_failure.is_none() {
        first_failure = Some("clone refused counter delta was not exact");
    }
    if creating_refused != 1 && first_failure.is_none() {
        first_failure = Some("creating-refused counter delta was not exact");
    }
    if published_admitted != 2 && first_failure.is_none() {
        first_failure = Some("published dispatch admission count was not exact");
    }
    if balance != 0 && first_failure.is_none() {
        first_failure = Some("clone admission oracle did not return frame accounting to baseline");
    }

    #[cfg(target_arch = "aarch64")]
    let arch = "aarch64";
    #[cfg(target_arch = "x86_64")]
    let arch = "x86";
    crate::serial_println!(
        "[CLONE_ADMISSION_ORACLE:{}:admitted={}:refused={}:creating_refused={}:published_admitted={}:balance={}]",
        arch,
        admitted,
        refused,
        creating_refused,
        published_admitted,
        balance
    );
    if let Some(reason) = first_failure {
        return TestResult::Fail(reason);
    }

    crate::serial_println!("[TEST:process:clone_admission_oracle:PASS]");
    TestResult::Pass
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_exec_cohort_gate() {
    crate::serial_println!("[TEST:process:x86_exec_cohort:START]");
    let result = exec_supersede_cohort_test();
    if !result.is_pass() {
        crate::serial_println!("[TEST:process:x86_exec_cohort:FAIL:{:?}]", result);
    }
    assert!(result.is_pass(), "x86 exec cohort gate failed");
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_exec_detach_gate() {
    crate::serial_println!("[TEST:process:exec_detach_oracle:START]");
    let result = exec_detach_oracle_test();
    if !result.is_pass() {
        crate::serial_println!("[TEST:process:exec_detach_oracle:FAIL:{:?}]", result);
    }
    assert!(result.is_pass(), "x86 exec detach oracle gate failed");
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_clone_admission_gate() {
    crate::serial_println!("[TEST:process:clone_admission_oracle:START]");
    let result = clone_admission_oracle_test();
    if !result.is_pass() {
        crate::serial_println!("[TEST:process:clone_admission_oracle:FAIL:{:?}]", result);
    }
    assert!(result.is_pass(), "x86 clone admission oracle gate failed");
}

// P20 retains its calibrated 45s local ceiling, but both it and P17 consume
// one 65s budget anchored near kernel entry. Thus P17 + P20 <= 65s and the
// 90s Phase-1 harness keeps 90s - 65s = 25s for other tests and overhead.
// This does not constrain the realistic starvation evidence: <=10s total,
// with the longest observed individual wait about 8s.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const EXIT_KICK_GATE_CEILING_MILLISECONDS: u64 = 45_000;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn exit_kick_protocol_gate_test() -> crate::test_framework::registry::TestResult {
    use crate::arch_impl::aarch64::timer_interrupt::record_exit_kick_gate_watchdog_heartbeat;
    use crate::test_framework::registry::TestResult;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use core::sync::atomic::AtomicBool;

    // ---- Cross-CPU handshake watchdog (test harness only) --------------------
    //
    // Each coordinator-owned wait has its own CNTVCT deadline. Every wait keeps an
    // unconditional eight-second floor from wait entry. After that floor, it may
    // fail only when worker-owned work or per-TID exit-stage counters have also
    // made no advance for three seconds. The effective no-progress deadline is
    // therefore max(wait_start + 8s, last_advance + 3s). This matters when progress
    // is a union: one sibling's early advance cannot shorten another sibling's
    // first-schedule allowance. CPU timer ticks never count as progress. Each storm
    // join observes only its awaited worker's work counter plus that worker's exit
    // stages, plus genuine dependency progress when a worker is deliberately
    // blocked on another worker. A separate
    // 15-second per-wait ceiling and one 45-second gate ceiling remain hard
    // backstops across late-true recoveries. The gate is additionally capped by
    // the shared 65-second Phase-1 liveness clock that also bounds SMP bring-up,
    // so the two watchdogs cannot compose past the external harness. Resched SGIs
    // are re-sent every 50ms. Each re-kick also records a boot-test-only CPU-0
    // watchdog heartbeat, preventing the generic five-second soft-lockup detector
    // from preempting this gate's worker-specific verdict. This observes the
    // EXIT_KICK test without changing its protocol. See P20 in
    // docs/polling-allowlist.md.
    //
    // Eight seconds remains the proven starvation-tolerant first-progress floor.
    // It cannot lose the race against the five-second soft-lockup detector:
    // gate entry, wait entry, and every 50ms re-kick advance the heartbeat that
    // detector treats as progress, while a genuinely wedged worker still reaches
    // this gate's own actionable no-progress FAIL after the full eight seconds.
    const FIRST_PROGRESS_WINDOW_MILLISECONDS: u64 = 8_000;
    const NO_PROGRESS_WINDOW_MILLISECONDS: u64 = 3_000;
    const ABSOLUTE_WAIT_CEILING_MILLISECONDS: u64 = 15_000;
    const GATE_CEILING_MILLISECONDS: u64 = EXIT_KICK_GATE_CEILING_MILLISECONDS;
    const RESCHED_REKICK_INTERVAL_MILLISECONDS: u64 = 50;
    const BREADCRUMB_INTERVAL_MILLISECONDS: u64 = 1_000;
    const CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS: u64 = 100_000;

    #[derive(Clone, Copy)]
    struct WaitProgress {
        work: u64,
        exit: u64,
        workers: [u64; 3],
    }

    impl WaitProgress {
        const fn work(work: u64) -> Self {
            Self {
                work,
                exit: 0,
                workers: [0; 3],
            }
        }

        const fn workers(workers: [u64; 3]) -> Self {
            Self {
                work: workers[0]
                    .saturating_add(workers[1])
                    .saturating_add(workers[2]),
                exit: 0,
                workers,
            }
        }

        fn advanced_from(self, previous: Self) -> bool {
            self.work > previous.work || self.exit > previous.exit
        }
    }

    #[derive(Clone, Copy)]
    enum WaitFailureKind {
        NoProgress,
        AbsoluteCeiling,
        GateCeiling,
        PhaseOneCeiling,
        CounterStall,
        CounterUnavailable,
        ProgressUnavailable,
        JoinFailed,
    }

    impl WaitFailureKind {
        fn cause(self) -> &'static str {
            match self {
                Self::NoProgress => "no_progress",
                Self::AbsoluteCeiling => "absolute_ceiling",
                Self::GateCeiling => "gate_ceiling",
                Self::PhaseOneCeiling => "phase_one_ceiling",
                Self::CounterStall => "cntvct_stall",
                Self::CounterUnavailable => "counter_frequency_unavailable",
                Self::ProgressUnavailable => "exit_progress_unavailable",
                Self::JoinFailed => "join_failed",
            }
        }

        fn message(self, site_message: &'static str) -> &'static str {
            match self {
                Self::NoProgress | Self::AbsoluteCeiling => site_message,
                // These aggregate ceilings can be consumed by earlier waits or
                // boot stages. Their permanent messages must neither blame this
                // site's CPU nor match the per-CPU liveness-failure classifier.
                Self::GateCeiling => {
                    "exit_kick_gate: gate liveness ceiling exhausted before this wait's condition was observed (not a per-CPU stall)"
                }
                Self::PhaseOneCeiling => {
                    "exit_kick_gate: shared Phase-1 liveness budget exhausted before this wait's condition was observed (not a per-CPU stall)"
                }
                Self::CounterStall => {
                    "exit_kick_gate: CNTVCT stalled while enforcing wait deadline"
                }
                Self::CounterUnavailable => {
                    "exit_kick_gate: CNTFRQ unavailable; cannot enforce wait deadline"
                }
                Self::ProgressUnavailable => "exit_kick_gate: exit-progress tracking unavailable",
                Self::JoinFailed => "exit_kick_gate: kthread join failed after exit observation",
            }
        }
    }

    fn ticks_to_milliseconds(ticks: u64, counter_frequency_hz: u64) -> u64 {
        ticks.saturating_mul(1_000) / counter_frequency_hz.max(1)
    }

    struct WaitEvidence<'a> {
        wait_name: &'a str,
        failure: WaitFailureKind,
        elapsed_ms: u64,
        window_budget_ms: u64,
        re_kick_sgis: u64,
        condition_current: u64,
        condition_expected: u64,
        progress_start: WaitProgress,
        progress_final: WaitProgress,
        last_advance_ms_ago: u64,
        late_true: bool,
    }

    fn print_wait_evidence(evidence: WaitEvidence<'_>) {
        // Keep evidence on the bracketed prefix. Only permanent FAIL messages
        // use `exit_kick_gate:` so late_true=1 recovery cannot match the
        // canonical `exit_kick_gate:.*unresponsive` failure grep.
        crate::serial_println!(
            "[exit_kick_gate] wait={} cause={} elapsed_ms={} window_budget_ms={} re_kick_sgis={} cpus_online={} condition_current={} condition_expected={} progress_work_start={} progress_work_final={} progress_exit_start={} progress_exit_final={} worker_1_progress_start={} worker_1_progress_final={} worker_2_progress_start={} worker_2_progress_final={} worker_3_progress_start={} worker_3_progress_final={} last_advance_ms_ago={} late_true={}",
            evidence.wait_name,
            evidence.failure.cause(),
            evidence.elapsed_ms,
            evidence.window_budget_ms,
            evidence.re_kick_sgis,
            crate::arch_impl::aarch64::smp::cpus_online(),
            evidence.condition_current,
            evidence.condition_expected,
            evidence.progress_start.work,
            evidence.progress_final.work,
            evidence.progress_start.exit,
            evidence.progress_final.exit,
            evidence.progress_start.workers[0],
            evidence.progress_final.workers[0],
            evidence.progress_start.workers[1],
            evidence.progress_final.workers[1],
            evidence.progress_start.workers[2],
            evidence.progress_final.workers[2],
            evidence.last_advance_ms_ago,
            evidence.late_true as u8,
        );
    }

    fn spin_with_resched<C, M, P>(
        wait_name: &'static str,
        condition_value: C,
        condition_met: M,
        condition_expected: u64,
        progress: P,
        kick_cpus: &[usize],
        phase_one_started_at: u64,
        gate_started_at: u64,
    ) -> Result<(), WaitFailureKind>
    where
        C: Fn() -> u64,
        M: Fn(u64) -> bool,
        P: Fn() -> WaitProgress,
    {
        let initial_condition = condition_value();
        if condition_met(initial_condition) {
            return Ok(());
        }

        let progress_start = progress();
        let counter_frequency_hz = crate::arch_impl::aarch64::timer::frequency_hz();
        if counter_frequency_hz == 0 {
            // A guessed frequency here could stretch every watchdog deadline by
            // the guess ratio and silently defeat the bound. Boot's 1 MHz guess
            // instead deliberately errs toward a short CPU bring-up wait.
            let late_condition = condition_value();
            let progress_final = progress();
            let late_true = condition_met(late_condition);
            print_wait_evidence(WaitEvidence {
                wait_name,
                failure: WaitFailureKind::CounterUnavailable,
                elapsed_ms: 0,
                window_budget_ms: 0,
                re_kick_sgis: 0,
                condition_current: late_condition,
                condition_expected,
                progress_start,
                progress_final,
                last_advance_ms_ago: 0,
                late_true,
            });
            return Err(WaitFailureKind::CounterUnavailable);
        }

        let first_progress_ticks =
            crate::arch_impl::aarch64::timer::milliseconds_to_ticks(
                counter_frequency_hz,
                FIRST_PROGRESS_WINDOW_MILLISECONDS,
            );
        let no_progress_ticks =
            crate::arch_impl::aarch64::timer::milliseconds_to_ticks(
                counter_frequency_hz,
                NO_PROGRESS_WINDOW_MILLISECONDS,
            );
        let absolute_ceiling_ticks =
            crate::arch_impl::aarch64::timer::milliseconds_to_ticks(
                counter_frequency_hz,
                ABSOLUTE_WAIT_CEILING_MILLISECONDS,
            );
        let gate_ceiling_ticks =
            crate::arch_impl::aarch64::timer::milliseconds_to_ticks(
                counter_frequency_hz,
                GATE_CEILING_MILLISECONDS,
            );
        let phase_one_ceiling_ticks =
            crate::arch_impl::aarch64::timer::milliseconds_to_ticks(
                counter_frequency_hz,
                crate::test_framework::PHASE_ONE_LIVENESS_BUDGET_MILLISECONDS,
            );
        let re_kick_ticks =
            crate::arch_impl::aarch64::timer::milliseconds_to_ticks(
                counter_frequency_hz,
                RESCHED_REKICK_INTERVAL_MILLISECONDS,
            );
        let breadcrumb_ticks =
            crate::arch_impl::aarch64::timer::milliseconds_to_ticks(
                counter_frequency_hz,
                BREADCRUMB_INTERVAL_MILLISECONDS,
            );
        let wait_start = crate::arch_impl::aarch64::timer::rdtsc_serialized();
        record_exit_kick_gate_watchdog_heartbeat();
        let mut last_advance = wait_start;
        let mut last_progress = progress_start;
        let mut last_counter_sample = wait_start;
        let mut last_re_kick = wait_start;
        let mut last_breadcrumb = wait_start;
        let mut iterations = 0u64;
        let mut re_kick_sgis = 0u64;

        loop {
            let condition_current = condition_value();
            if condition_met(condition_current) {
                return Ok(());
            }

            let now = crate::arch_impl::aarch64::timer::rdtsc_serialized();
            let progress_current = progress();
            if progress_current.advanced_from(last_progress) {
                last_progress = progress_current;
                last_advance = now;
            }
            let no_progress_deadline_elapsed =
                crate::arch_impl::aarch64::timer::elapsed_ticks(last_advance, wait_start)
                .saturating_add(no_progress_ticks);
            let progress_deadline_elapsed =
                core::cmp::max(first_progress_ticks, no_progress_deadline_elapsed);

            iterations = iterations.wrapping_add(1);
            let elapsed = crate::arch_impl::aarch64::timer::elapsed_ticks(now, wait_start);
            let mut failure = None;
            if crate::arch_impl::aarch64::timer::elapsed_ticks(now, phase_one_started_at)
                >= phase_one_ceiling_ticks
            {
                failure = Some(WaitFailureKind::PhaseOneCeiling);
            }
            if failure.is_none()
                && crate::arch_impl::aarch64::timer::elapsed_ticks(now, gate_started_at)
                    >= gate_ceiling_ticks
            {
                failure = Some(WaitFailureKind::GateCeiling);
            }
            if failure.is_none() && elapsed >= absolute_ceiling_ticks {
                failure = Some(WaitFailureKind::AbsoluteCeiling);
            }
            if failure.is_none() && elapsed >= progress_deadline_elapsed {
                failure = Some(WaitFailureKind::NoProgress);
            }
            if failure.is_none() && iterations % CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS == 0 {
                let counter_delta = crate::arch_impl::aarch64::timer::elapsed_ticks(
                    now,
                    last_counter_sample,
                );
                if counter_delta == 0 {
                    failure = Some(WaitFailureKind::CounterStall);
                }
                last_counter_sample = now;
            }

            if let Some(failure) = failure {
                let verdict_at = crate::arch_impl::aarch64::timer::rdtsc_serialized();
                let late_condition = condition_value();
                let progress_final = progress();
                let late_true = condition_met(late_condition);
                let window_budget_ms = match failure {
                    WaitFailureKind::NoProgress => {
                        ticks_to_milliseconds(progress_deadline_elapsed, counter_frequency_hz)
                    }
                    WaitFailureKind::AbsoluteCeiling => ABSOLUTE_WAIT_CEILING_MILLISECONDS,
                    WaitFailureKind::GateCeiling => GATE_CEILING_MILLISECONDS,
                    WaitFailureKind::PhaseOneCeiling => {
                        crate::test_framework::PHASE_ONE_LIVENESS_BUDGET_MILLISECONDS
                    }
                    WaitFailureKind::CounterStall
                    | WaitFailureKind::CounterUnavailable
                    | WaitFailureKind::ProgressUnavailable
                    | WaitFailureKind::JoinFailed => 0,
                };
                print_wait_evidence(WaitEvidence {
                    wait_name,
                    failure,
                    elapsed_ms: ticks_to_milliseconds(
                        crate::arch_impl::aarch64::timer::elapsed_ticks(verdict_at, wait_start),
                        counter_frequency_hz,
                    ),
                    window_budget_ms,
                    re_kick_sgis,
                    condition_current: late_condition,
                    condition_expected,
                    progress_start,
                    progress_final,
                    last_advance_ms_ago: ticks_to_milliseconds(
                        crate::arch_impl::aarch64::timer::elapsed_ticks(verdict_at, last_advance),
                        counter_frequency_hz,
                    ),
                    late_true,
                });
                // A no-progress threshold can race the awaited store, so its
                // final reread may recover. Hard ceilings and counter failures
                // remain failures even if the condition changes concurrently.
                let recoverable_late_true =
                    matches!(failure, WaitFailureKind::NoProgress) && late_true;
                return if recoverable_late_true {
                    Ok(())
                } else {
                    Err(failure)
                };
            }

            if crate::arch_impl::aarch64::timer::elapsed_ticks(now, last_re_kick)
                >= re_kick_ticks
            {
                for &cpu in kick_cpus {
                    crate::arch_impl::aarch64::gic::send_sgi(
                        crate::arch_impl::aarch64::constants::SGI_RESCHEDULE as u8,
                        cpu as u8,
                    );
                    re_kick_sgis = re_kick_sgis.saturating_add(1);
                }
                record_exit_kick_gate_watchdog_heartbeat();
                last_re_kick = now;
            }

            if elapsed >= no_progress_ticks
                && crate::arch_impl::aarch64::timer::elapsed_ticks(now, last_breadcrumb)
                    >= breadcrumb_ticks
            {
                crate::serial_println!(
                    "[exit_kick_gate] wait={} breadcrumb=1 elapsed_ms={} progress_work={} progress_exit={} re_kick_sgis={}",
                    wait_name,
                    ticks_to_milliseconds(elapsed, counter_frequency_hz),
                    progress_current.work,
                    progress_current.exit,
                    re_kick_sgis,
                );
                last_breadcrumb = now;
            }

            crate::task::scheduler::yield_current();
            core::hint::spin_loop();
        }
    }

    fn join_with_resched<P>(
        wait_name: &'static str,
        handle: &crate::task::kthread::KthreadHandle,
        work_progress: P,
        kick_cpus: &[usize],
        phase_one_started_at: u64,
        gate_started_at: u64,
    ) -> Result<(), WaitFailureKind>
    where
        P: Fn() -> u64,
    {
        let tid = handle.tid();
        if !watch_kthread_exit_progress_for_test(tid) {
            let progress = WaitProgress::work(work_progress());
            print_wait_evidence(WaitEvidence {
                wait_name,
                failure: WaitFailureKind::ProgressUnavailable,
                elapsed_ms: 0,
                window_budget_ms: 0,
                re_kick_sgis: 0,
                condition_current: crate::task::kthread::kthread_has_exited_for_test(handle) as u64,
                condition_expected: 1,
                progress_start: progress,
                progress_final: progress,
                last_advance_ms_ago: 0,
                late_true: false,
            });
            return Err(WaitFailureKind::ProgressUnavailable);
        }
        let progress_start = WaitProgress {
            work: work_progress(),
            exit: kthread_exit_progress_for_test(tid),
            workers: [0; 3],
        };
        spin_with_resched(
            wait_name,
            || crate::task::kthread::kthread_has_exited_for_test(handle) as u64,
            |value| value != 0,
            1,
            || WaitProgress {
                work: work_progress(),
                exit: kthread_exit_progress_for_test(tid),
                workers: [0; 3],
            },
            kick_cpus,
            phase_one_started_at,
            gate_started_at,
        )?;
        if crate::task::kthread::kthread_join(handle).is_err() {
            let progress_final = WaitProgress {
                work: work_progress(),
                exit: kthread_exit_progress_for_test(tid),
                workers: [0; 3],
            };
            print_wait_evidence(WaitEvidence {
                wait_name,
                failure: WaitFailureKind::JoinFailed,
                elapsed_ms: 0,
                window_budget_ms: 0,
                re_kick_sgis: 0,
                condition_current: 1,
                condition_expected: 1,
                progress_start,
                progress_final,
                last_advance_ms_ago: 0,
                late_true: false,
            });
            return Err(WaitFailureKind::JoinFailed);
        }
        Ok(())
    }

    record_exit_kick_gate_watchdog_heartbeat();
    let phase_one_started_at = match crate::test_framework::phase_one_liveness_started_at() {
        Some(started_at) => started_at,
        None => {
            return TestResult::Fail(
                "exit_kick_gate: shared Phase-1 liveness budget anchor unavailable",
            )
        }
    };
    let gate_started_at = crate::arch_impl::aarch64::timer::rdtsc_serialized();
    let _exit_progress_guard = KthreadExitProgressGuard::arm();

    struct BrokenV3Slot {
        pid: AtomicU64,
        at: AtomicU64,
        seq: AtomicU64,
    }

    impl BrokenV3Slot {
        const fn new() -> Self {
            Self {
                pid: AtomicU64::new(0),
                at: AtomicU64::new(0),
                seq: AtomicU64::new(0),
            }
        }

        fn publish(&self, pid: u64, at: u64) {
            self.pid.store(pid, Ordering::Relaxed);
            self.at.store(at, Ordering::Relaxed);
            self.seq.fetch_add(2, Ordering::Release);
        }

        fn observe(&self, expected_pid: u64) -> Option<(u64, u64)> {
            let first = self.seq.load(Ordering::Acquire);
            if first == 0 || first & 1 != 0 {
                return None;
            }
            let pid = self.pid.load(Ordering::Relaxed);
            let at = self.at.load(Ordering::Relaxed);
            if self.seq.load(Ordering::Acquire) != first || pid != expected_pid {
                return None;
            }
            self.seq
                .compare_exchange(first, first | 1, Ordering::AcqRel, Ordering::Relaxed)
                .ok()
                .map(|_| (pid, at))
        }
    }

    // Reproduce the historical v3 sticky-observed defect exactly: fetch_add(2)
    // preserves bit zero, so generation two is absent after generation one was
    // observed. The real protocol must pass the same sequential reuse cycle.
    let broken = BrokenV3Slot::new();
    broken.publish(7, 11);
    if broken.observe(7) != Some((7, 11)) {
        return TestResult::Fail("broken-v3 first observation did not establish the fixture");
    }
    broken.publish(71, 12);
    if broken.observe(71).is_some() {
        return TestResult::Fail("broken-v3 fixture unexpectedly allowed bucket reuse");
    }

    let reusable = KickSlot::new();
    let first = reusable.publish(7, 21);
    let first_observation = reusable.observe(7);
    let second = reusable.publish(71, 22);
    let second_observation = reusable.observe(71);
    if !matches!(
        first,
        KickPublishResult::Published {
            generation: 1,
            displaced: false
        }
    ) || first_observation
        != Some(KickObservation {
            generation: 1,
            pid: 7,
            at: 21,
        })
        || !matches!(
            second,
            KickPublishResult::Published {
                generation: 2,
                displaced: false
            }
        )
        || second_observation
            != Some(KickObservation {
                generation: 2,
                pid: 71,
                at: 22,
            })
    {
        return TestResult::Fail("real exit-kick protocol failed sequential bucket reuse");
    }

    // Reservation-lost arm: the RAII-disarmed hook holds publisher A on one
    // CPU after reserve. Publisher B must run on a different CPU, lose that
    // reservation, leave the payload untouched, account the collision, and
    // still reach the SGI broadcast tail before A is released.
    const RESERVATION_PID_A: u64 = u64::MAX - 7;
    const RESERVATION_PID_B: u64 = RESERVATION_PID_A - EXIT_KICK_BUCKETS as u64;
    const PUBLISHER_A_CPU: usize = 1;
    const PUBLISHER_B_CPU: usize = 2;
    let reservation_bucket = RESERVATION_PID_A as usize % EXIT_KICK_BUCKETS;
    let reservation_slot = &EXIT_KICK_SLOTS[reservation_bucket];
    let online_cpus = crate::arch_impl::aarch64::smp::cpus_online() as usize;
    if PUBLISHER_A_CPU >= online_cpus || PUBLISHER_B_CPU >= online_cpus {
        return TestResult::Fail("exit-kick reservation-loss publisher CPU is not online");
    }
    let payload_before = (
        reservation_slot.pid.load(Ordering::Relaxed),
        reservation_slot.at.load(Ordering::Relaxed),
    );
    let published_before = EXIT_KICK_PUBLISHED.aggregate();
    let collision_before = EXIT_KICK_BUCKET_COLLISION.aggregate();
    let sgi_before = EXIT_SGI_SENT.aggregate();
    let publisher_a_done = Arc::new(AtomicU64::new(0));
    let publisher_a_progress = Arc::new(AtomicU64::new(0));
    let publisher_b_done = Arc::new(AtomicU64::new(0));
    let publisher_b_progress = Arc::new(AtomicU64::new(0));
    let publisher_b_cpu = Arc::new(AtomicU64::new(u64::MAX));
    let hook = ExitKickTestHookGuard::arm(RESERVATION_PID_A);

    let a_done = Arc::clone(&publisher_a_done);
    let a_progress = Arc::clone(&publisher_a_progress);
    let publisher_a = match crate::task::kthread::kthread_run_on_cpu_for_test(
        move || {
            a_progress.fetch_add(1, Ordering::Release);
            crate::task::scheduler::Scheduler::send_exit_expedite_sgi(
                RESERVATION_PID_A,
                crate::task::scheduler::GroupBatchId::for_single_victim(RESERVATION_PID_A),
            );
            a_progress.fetch_add(1, Ordering::Release);
            a_done.store(1, Ordering::Release);
            a_progress.fetch_add(1, Ordering::Release);
        },
        "exit_kick_reserve_a",
        PUBLISHER_A_CPU,
    ) {
        Ok(handle) => {
            if !watch_kthread_exit_progress_for_test(handle.tid()) {
                return TestResult::Fail(
                    "exit_kick_gate: publisher A exit-progress registration failed",
                );
            }
            handle
        }
        Err(_) => return TestResult::Fail("failed to spawn held exit-kick publisher A"),
    };

    if let Err(failure) = spin_with_resched(
        "publisher_a_reservation",
        || EXIT_KICK_TEST_HOOK_RESERVED.load(Ordering::Acquire),
        |value| value != 0,
        1,
        || WaitProgress::work(publisher_a_progress.load(Ordering::Acquire)),
        &[PUBLISHER_A_CPU],
        phase_one_started_at,
        gate_started_at,
    ) {
        // `hook` is dropped on return, releasing publisher A from its hold.
        return TestResult::Fail(failure.message(
            "exit_kick_gate: publisher A reservation handshake stuck, CPU 1 unresponsive",
        ));
    }

    let b_done = Arc::clone(&publisher_b_done);
    let b_progress = Arc::clone(&publisher_b_progress);
    let b_cpu = Arc::clone(&publisher_b_cpu);
    let publisher_b = match crate::task::kthread::kthread_run_on_cpu_for_test(
        move || {
            b_progress.fetch_add(1, Ordering::Release);
            b_cpu.store(
                crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as u64,
                Ordering::Relaxed,
            );
            b_progress.fetch_add(1, Ordering::Release);
            crate::task::scheduler::Scheduler::send_exit_expedite_sgi(
                RESERVATION_PID_B,
                crate::task::scheduler::GroupBatchId::for_single_victim(RESERVATION_PID_B),
            );
            b_progress.fetch_add(1, Ordering::Release);
            b_done.store(1, Ordering::Release);
            b_progress.fetch_add(1, Ordering::Release);
        },
        "exit_kick_reserve_b",
        PUBLISHER_B_CPU,
    ) {
        Ok(handle) => {
            if !watch_kthread_exit_progress_for_test(handle.tid()) {
                return TestResult::Fail(
                    "exit_kick_gate: publisher B exit-progress registration failed",
                );
            }
            handle
        }
        Err(_) => {
            hook.release();
            let _ = crate::task::kthread::kthread_join(&publisher_a);
            return TestResult::Fail("failed to spawn colliding exit-kick publisher B");
        }
    };

    if let Err(failure) = spin_with_resched(
        "publisher_b_completion",
        || publisher_b_done.load(Ordering::Acquire),
        |value| value != 0,
        1,
        || WaitProgress::work(publisher_b_progress.load(Ordering::Acquire)),
        &[PUBLISHER_B_CPU],
        phase_one_started_at,
        gate_started_at,
    ) {
        // `hook` is dropped on return, releasing publisher A from its hold.
        return TestResult::Fail(failure.message(
            "exit_kick_gate: publisher B completion handshake stuck, CPU 2 unresponsive",
        ));
    }

    let publisher_a_cpu = EXIT_KICK_TEST_HOOK_CPU.load(Ordering::Acquire);
    let publisher_b_cpu = publisher_b_cpu.load(Ordering::Acquire);
    let loser_payload = (
        reservation_slot.pid.load(Ordering::Relaxed),
        reservation_slot.at.load(Ordering::Relaxed),
    );
    let loser_accounting_exact = EXIT_KICK_PUBLISHED.aggregate() == published_before
        && EXIT_KICK_BUCKET_COLLISION.aggregate() == collision_before + 1
        && EXIT_SGI_SENT.aggregate() == sgi_before + 1;

    hook.release();
    if let Err(failure) = join_with_resched(
        "publisher_b_join",
        &publisher_b,
        || publisher_b_progress.load(Ordering::Acquire),
        &[PUBLISHER_B_CPU],
        phase_one_started_at,
        gate_started_at,
    ) {
        return TestResult::Fail(
            failure.message("exit_kick_gate: publisher B join stuck, CPU 2 unresponsive"),
        );
    }
    if let Err(failure) = join_with_resched(
        "publisher_a_join",
        &publisher_a,
        || publisher_a_progress.load(Ordering::Acquire),
        &[PUBLISHER_A_CPU],
        phase_one_started_at,
        gate_started_at,
    ) {
        return TestResult::Fail(
            failure.message("exit_kick_gate: publisher A join stuck, CPU 1 unresponsive"),
        );
    }
    core::mem::drop(hook);

    if publisher_a_done.load(Ordering::Acquire) != 1
        || publisher_a_cpu == u64::MAX
        || publisher_b_cpu == u64::MAX
        || publisher_a_cpu == publisher_b_cpu
        || publisher_a_cpu != PUBLISHER_A_CPU as u64
        || publisher_b_cpu != PUBLISHER_B_CPU as u64
        || loser_payload != payload_before
        || !loser_accounting_exact
        || EXIT_KICK_PUBLISHED.aggregate() != published_before + 1
        || EXIT_KICK_BUCKET_COLLISION.aggregate() != collision_before + 1
        || EXIT_SGI_SENT.aggregate() != sgi_before + 2
    {
        return TestResult::Fail("deterministic two-CPU reservation-loss arm failed");
    }

    let Some(reservation_observation) = reservation_slot.observe(RESERVATION_PID_A) else {
        return TestResult::Fail("reservation winner's publication was not observable");
    };
    if reservation_observation.pid != RESERVATION_PID_A || reservation_observation.at == 0 {
        return TestResult::Fail("reservation winner's tuple was contaminated");
    }

    // Displacement arm: use the production helper twice, deliberately taking
    // no observation between A and B. B's successful replacement must count
    // as both a publication and exactly one displaced collision, and only B's
    // coherent tuple may subsequently be claimed.
    const DISPLACEMENT_PID_A: u64 = RESERVATION_PID_A - (2 * EXIT_KICK_BUCKETS as u64);
    const DISPLACEMENT_PID_B: u64 = DISPLACEMENT_PID_A - EXIT_KICK_BUCKETS as u64;
    let displacement_published_before = EXIT_KICK_PUBLISHED.aggregate();
    let displacement_collision_before = EXIT_KICK_BUCKET_COLLISION.aggregate();
    let displacement_sgi_before = EXIT_SGI_SENT.aggregate();
    crate::task::scheduler::Scheduler::send_exit_expedite_sgi(
        DISPLACEMENT_PID_A,
        crate::task::scheduler::GroupBatchId::for_single_victim(DISPLACEMENT_PID_A),
    );
    crate::task::scheduler::Scheduler::send_exit_expedite_sgi(
        DISPLACEMENT_PID_B,
        crate::task::scheduler::GroupBatchId::for_single_victim(DISPLACEMENT_PID_B),
    );
    let Some(displacement_observation) = reservation_slot.observe(DISPLACEMENT_PID_B) else {
        return TestResult::Fail("displaced production publication was not observable");
    };
    if displacement_observation.pid != DISPLACEMENT_PID_B
        || displacement_observation.at == 0
        || EXIT_KICK_PUBLISHED.aggregate() != displacement_published_before + 2
        || EXIT_KICK_BUCKET_COLLISION.aggregate() != displacement_collision_before + 1
        || EXIT_SGI_SENT.aggregate() != displacement_sgi_before + 2
    {
        return TestResult::Fail("unobserved generation displacement was not exact");
    }

    const ATTEMPTS_PER_PUBLISHER: u64 = 5_000;
    const TOTAL_ATTEMPTS: u64 = ATTEMPTS_PER_PUBLISHER * 2;
    const PID_A: u64 = 10;
    const PID_B: u64 = PID_A + EXIT_KICK_BUCKETS as u64;

    struct OracleRow {
        pid: AtomicU64,
        token: AtomicU64,
    }

    struct Accounting {
        workers_ready: AtomicU64,
        start: AtomicBool,
        abort: AtomicBool,
        publisher_a_cpu_mask: AtomicU64,
        publisher_b_cpu_mask: AtomicU64,
        observer_cpu_mask: AtomicU64,
        publisher_a_progress: AtomicU64,
        publisher_b_progress: AtomicU64,
        observer_progress: AtomicU64,
        next_token: AtomicU64,
        published: AtomicU64,
        collisions: AtomicU64,
        collisions_reservation_lost: AtomicU64,
        collisions_displaced: AtomicU64,
        observations: AtomicU64,
        mismatches: AtomicU64,
        publishers_done: AtomicU64,
        observer_running: AtomicBool,
        observer_done: AtomicBool,
    }

    let storm_slot = Arc::new(KickSlot::new());
    let oracle = Arc::new(
        (0..=TOTAL_ATTEMPTS)
            .map(|_| OracleRow {
                pid: AtomicU64::new(0),
                token: AtomicU64::new(0),
            })
            .collect::<Vec<_>>(),
    );
    let accounting = Arc::new(Accounting {
        workers_ready: AtomicU64::new(0),
        start: AtomicBool::new(false),
        abort: AtomicBool::new(false),
        publisher_a_cpu_mask: AtomicU64::new(0),
        publisher_b_cpu_mask: AtomicU64::new(0),
        observer_cpu_mask: AtomicU64::new(0),
        publisher_a_progress: AtomicU64::new(0),
        publisher_b_progress: AtomicU64::new(0),
        observer_progress: AtomicU64::new(0),
        next_token: AtomicU64::new(0),
        published: AtomicU64::new(0),
        collisions: AtomicU64::new(0),
        collisions_reservation_lost: AtomicU64::new(0),
        collisions_displaced: AtomicU64::new(0),
        observations: AtomicU64::new(0),
        mismatches: AtomicU64::new(0),
        publishers_done: AtomicU64::new(0),
        observer_running: AtomicBool::new(false),
        observer_done: AtomicBool::new(false),
    });

    struct StormAbortGuard {
        accounting: Arc<Accounting>,
        armed: bool,
    }

    impl StormAbortGuard {
        fn arm(accounting: &Arc<Accounting>) -> Self {
            Self {
                accounting: Arc::clone(accounting),
                armed: true,
            }
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for StormAbortGuard {
        fn drop(&mut self) {
            if self.armed {
                // Release every worker-side rendezvous before the coordinator
                // returns a permanent FAIL. Live workers then leave their
                // closures instead of spinning for the rest of the boot.
                self.accounting.abort.store(true, Ordering::Release);
                self.accounting.start.store(true, Ordering::Release);
            }
        }
    }

    let mut storm_abort_guard = StormAbortGuard::arm(&accounting);

    let spawn_publisher = |pid: u64, name: &'static str, cpu: usize| {
        let slot = Arc::clone(&storm_slot);
        let oracle = Arc::clone(&oracle);
        let accounting = Arc::clone(&accounting);
        crate::task::kthread::kthread_run_on_cpu_for_test(
            move || {
                let cpu_bit = 1u64 << crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
                if pid == PID_A {
                    accounting
                        .publisher_a_cpu_mask
                        .fetch_or(cpu_bit, Ordering::Relaxed);
                } else {
                    accounting
                        .publisher_b_cpu_mask
                        .fetch_or(cpu_bit, Ordering::Relaxed);
                }
                if pid == PID_A {
                    accounting
                        .publisher_a_progress
                        .fetch_add(1, Ordering::Release);
                } else {
                    accounting
                        .publisher_b_progress
                        .fetch_add(1, Ordering::Release);
                }
                accounting.workers_ready.fetch_add(1, Ordering::Release);
                while !accounting.start.load(Ordering::Acquire) {
                    if accounting.abort.load(Ordering::Acquire) {
                        return;
                    }
                    crate::task::scheduler::yield_current();
                    core::hint::spin_loop();
                }
                if accounting.abort.load(Ordering::Acquire) {
                    return;
                }
                while !accounting.observer_running.load(Ordering::Acquire) {
                    if accounting.abort.load(Ordering::Acquire) {
                        return;
                    }
                    crate::task::scheduler::yield_current();
                    core::hint::spin_loop();
                }
                if pid == PID_B {
                    while accounting.observations.load(Ordering::Acquire) == 0 {
                        if accounting.abort.load(Ordering::Acquire) {
                            return;
                        }
                        crate::task::scheduler::yield_current();
                        core::hint::spin_loop();
                    }
                }
                for attempt in 0..ATTEMPTS_PER_PUBLISHER {
                    if accounting.abort.load(Ordering::Acquire) {
                        return;
                    }
                    if pid == PID_A {
                        accounting
                            .publisher_a_progress
                            .fetch_add(1, Ordering::Release);
                    } else {
                        accounting
                            .publisher_b_progress
                            .fetch_add(1, Ordering::Release);
                    }
                    let cpu_bit =
                        1u64 << crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
                    if pid == PID_A {
                        accounting
                            .publisher_a_cpu_mask
                            .fetch_or(cpu_bit, Ordering::Relaxed);
                    } else {
                        accounting
                            .publisher_b_cpu_mask
                            .fetch_or(cpu_bit, Ordering::Relaxed);
                    }
                    let token = accounting.next_token.fetch_add(1, Ordering::Relaxed) + 1;
                    let Some(reservation) = slot.reserve() else {
                        accounting
                            .collisions_reservation_lost
                            .fetch_add(1, Ordering::Relaxed);
                        accounting.collisions.fetch_add(1, Ordering::Relaxed);
                        continue;
                    };
                    let generation = reservation.generation as usize;
                    oracle[generation].pid.store(pid, Ordering::Relaxed);
                    oracle[generation].token.store(token, Ordering::Release);
                    match slot.commit(reservation, pid, token) {
                        KickPublishResult::Published { displaced, .. } => {
                            accounting.published.fetch_add(1, Ordering::Relaxed);
                            if displaced {
                                accounting
                                    .collisions_displaced
                                    .fetch_add(1, Ordering::Relaxed);
                                accounting.collisions.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        KickPublishResult::ReservationLost => {
                            accounting.mismatches.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    if pid == PID_A && attempt == 0 {
                        while accounting.observations.load(Ordering::Acquire) == 0 {
                            if accounting.abort.load(Ordering::Acquire) {
                                return;
                            }
                            crate::task::scheduler::yield_current();
                            core::hint::spin_loop();
                        }
                    }
                    if attempt & 63 == 63 {
                        crate::task::scheduler::yield_current();
                    }
                }
                accounting.publishers_done.fetch_add(1, Ordering::Release);
                if pid == PID_A {
                    accounting
                        .publisher_a_progress
                        .fetch_add(1, Ordering::Release);
                } else {
                    accounting
                        .publisher_b_progress
                        .fetch_add(1, Ordering::Release);
                }
            },
            name,
            cpu,
        )
    };

    struct StormSpawnPreemptGuard;

    impl StormSpawnPreemptGuard {
        fn enter() -> Self {
            crate::per_cpu_aarch64::preempt_disable();
            Self
        }
    }

    impl Drop for StormSpawnPreemptGuard {
        fn drop(&mut self) {
            crate::per_cpu_aarch64::preempt_enable();
        }
    }

    // Keep the creating thread fixed while the test-only spawn primitive queues
    // the three workers on scheduler-managed CPUs. CPU 0 runs the boot-test
    // executor outside the scheduler and cannot service a pinned kthread. The
    // RAII guard restores preemption on every error return.
    let spawn_guard = StormSpawnPreemptGuard::enter();

    let online_cpus = crate::arch_impl::aarch64::smp::cpus_online() as usize;
    let worker_cpus = [1, 2, 3];
    if worker_cpus.iter().any(|cpu| *cpu >= online_cpus) {
        return TestResult::Fail("exit-kick storm requires four online CPUs");
    }

    let publisher_a = match spawn_publisher(PID_A, "exit_kick_pub_a", worker_cpus[0]) {
        Ok(handle) => {
            if !watch_kthread_exit_progress_for_test(handle.tid()) {
                return TestResult::Fail(
                    "exit_kick_gate: storm publisher A exit-progress registration failed",
                );
            }
            handle
        }
        Err(_) => return TestResult::Fail("failed to spawn exit-kick publisher A"),
    };
    let publisher_b = match spawn_publisher(PID_B, "exit_kick_pub_b", worker_cpus[1]) {
        Ok(handle) => {
            if !watch_kthread_exit_progress_for_test(handle.tid()) {
                return TestResult::Fail(
                    "exit_kick_gate: storm publisher B exit-progress registration failed",
                );
            }
            handle
        }
        Err(_) => return TestResult::Fail("failed to spawn exit-kick publisher B"),
    };

    let observer_slot = Arc::clone(&storm_slot);
    let observer_oracle = Arc::clone(&oracle);
    let observer_accounting = Arc::clone(&accounting);
    let observer = match crate::task::kthread::kthread_run_on_cpu_for_test(
        move || {
            let cpu_bit = 1u64 << crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
            observer_accounting
                .observer_cpu_mask
                .fetch_or(cpu_bit, Ordering::Relaxed);
            observer_accounting
                .observer_progress
                .fetch_add(1, Ordering::Release);
            observer_accounting
                .workers_ready
                .fetch_add(1, Ordering::Release);
            while !observer_accounting.start.load(Ordering::Acquire) {
                if observer_accounting.abort.load(Ordering::Acquire) {
                    return;
                }
                crate::task::scheduler::yield_current();
                core::hint::spin_loop();
            }
            if observer_accounting.abort.load(Ordering::Acquire) {
                return;
            }
            let mut publishers_done_seen = 0u64;
            observer_accounting
                .observer_running
                .store(true, Ordering::Release);
            observer_accounting
                .observer_progress
                .fetch_add(1, Ordering::Release);
            loop {
                if observer_accounting.abort.load(Ordering::Acquire) {
                    return;
                }
                let publishers_done = observer_accounting
                    .publishers_done
                    .load(Ordering::Acquire);
                if publishers_done > publishers_done_seen {
                    observer_accounting.observer_progress.fetch_add(
                        publishers_done - publishers_done_seen,
                        Ordering::Release,
                    );
                    publishers_done_seen = publishers_done;
                }
                if publishers_done_seen == 2 {
                    break;
                }
                let cpu_bit = 1u64 << crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id();
                observer_accounting
                    .observer_cpu_mask
                    .fetch_or(cpu_bit, Ordering::Relaxed);
                if let Some(observation) = observer_slot.observe(PID_A) {
                    let row = &observer_oracle[observation.generation as usize];
                    let token = row.token.load(Ordering::Acquire);
                    let expected_pid = row.pid.load(Ordering::Relaxed);
                    if expected_pid != observation.pid || token != observation.at {
                        observer_accounting
                            .mismatches
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    observer_accounting
                        .observations
                        .fetch_add(1, Ordering::Relaxed);
                    observer_accounting
                        .observer_progress
                        .fetch_add(1, Ordering::Release);
                }
                if let Some(observation) = observer_slot.observe(PID_B) {
                    let row = &observer_oracle[observation.generation as usize];
                    let token = row.token.load(Ordering::Acquire);
                    let expected_pid = row.pid.load(Ordering::Relaxed);
                    if expected_pid != observation.pid || token != observation.at {
                        observer_accounting
                            .mismatches
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    observer_accounting
                        .observations
                        .fetch_add(1, Ordering::Relaxed);
                    observer_accounting
                        .observer_progress
                        .fetch_add(1, Ordering::Release);
                }
                core::hint::spin_loop();
            }
            observer_accounting
                .observer_done
                .store(true, Ordering::Release);
            observer_accounting
                .observer_progress
                .fetch_add(1, Ordering::Release);
        },
        "exit_kick_observer",
        worker_cpus[2],
    ) {
        Ok(handle) => {
            if !watch_kthread_exit_progress_for_test(handle.tid()) {
                return TestResult::Fail(
                    "exit_kick_gate: storm observer exit-progress registration failed",
                );
            }
            handle
        }
        Err(_) => return TestResult::Fail("failed to spawn exit-kick observer"),
    };
    core::mem::drop(spawn_guard);

    if let Err(failure) = spin_with_resched(
        "workers_ready",
        || accounting.workers_ready.load(Ordering::Acquire),
        |value| value == 3,
        3,
        || {
            WaitProgress::workers([
                accounting.publisher_a_progress.load(Ordering::Acquire),
                accounting.publisher_b_progress.load(Ordering::Acquire),
                accounting.observer_progress.load(Ordering::Acquire),
            ])
        },
        &worker_cpus,
        phase_one_started_at,
        gate_started_at,
    ) {
        return TestResult::Fail(failure.message(
            "exit_kick_gate: workers_ready never reached 3, a worker CPU (1/2/3) is unresponsive",
        ));
    }
    accounting.start.store(true, Ordering::Release);

    let storm_publisher_a_progress = || {
        accounting
            .publisher_a_progress
            .load(Ordering::Acquire)
            .saturating_add(accounting.observer_progress.load(Ordering::Acquire))
    };
    let storm_publisher_b_progress = || accounting.publisher_b_progress.load(Ordering::Acquire);
    let storm_observer_progress = || accounting.observer_progress.load(Ordering::Acquire);

    if let Err(failure) = join_with_resched(
        "storm_publisher_a_join",
        &publisher_a,
        &storm_publisher_a_progress,
        &worker_cpus,
        phase_one_started_at,
        gate_started_at,
    ) {
        return TestResult::Fail(
            failure.message(
                "exit_kick_gate: storm publisher A progress/exit stalled; a worker CPU (1/2/3) is unresponsive",
            ),
        );
    }
    if let Err(failure) = join_with_resched(
        "storm_publisher_b_join",
        &publisher_b,
        &storm_publisher_b_progress,
        &worker_cpus,
        phase_one_started_at,
        gate_started_at,
    ) {
        return TestResult::Fail(
            failure.message(
                "exit_kick_gate: storm publisher B progress/exit stalled; a worker CPU (1/2/3) is unresponsive",
            ),
        );
    }
    if let Err(failure) = join_with_resched(
        "storm_observer_join",
        &observer,
        &storm_observer_progress,
        &worker_cpus,
        phase_one_started_at,
        gate_started_at,
    ) {
        return TestResult::Fail(
            failure.message(
                "exit_kick_gate: storm observer progress/exit stalled; a worker CPU (1/2/3) is unresponsive",
            ),
        );
    }
    storm_abort_guard.disarm();

    let attempts = accounting.next_token.load(Ordering::Acquire);
    let published = accounting.published.load(Ordering::Acquire);
    let collisions_reservation_lost = accounting
        .collisions_reservation_lost
        .load(Ordering::Acquire);
    let collisions_displaced = accounting.collisions_displaced.load(Ordering::Acquire);
    let collisions = accounting.collisions.load(Ordering::Acquire);
    let publisher_a_cpu_mask = accounting.publisher_a_cpu_mask.load(Ordering::Acquire);
    let publisher_b_cpu_mask = accounting.publisher_b_cpu_mask.load(Ordering::Acquire);
    let observer_cpu_mask = accounting.observer_cpu_mask.load(Ordering::Acquire);
    let occupied_three_distinct_cpus = (0..64).any(|a| {
        publisher_a_cpu_mask & (1 << a) != 0
            && (0..64).any(|b| {
                b != a
                    && publisher_b_cpu_mask & (1 << b) != 0
                    && (0..64).any(|observer| {
                        observer != a && observer != b && observer_cpu_mask & (1 << observer) != 0
                    })
            })
    });
    if !occupied_three_distinct_cpus {
        return TestResult::Fail("exit-kick storm did not occupy three distinct CPUs");
    }
    if attempts != TOTAL_ATTEMPTS {
        return TestResult::Fail("exit-kick storm did not execute all publisher attempts");
    }
    if attempts != published + collisions_reservation_lost {
        return TestResult::Fail("exit-kick publish/lost accounting identity failed");
    }
    if collisions != collisions_reservation_lost + collisions_displaced {
        return TestResult::Fail("exit-kick collision sub-arm accounting identity failed");
    }
    if accounting.mismatches.load(Ordering::Acquire) != 0 {
        return TestResult::Fail("exit-kick storm observed a torn or cross-generation tuple");
    }
    if accounting.observations.load(Ordering::Acquire) == 0 {
        return TestResult::Fail("exit-kick storm observer never claimed a publication");
    }
    if !accounting.observer_done.load(Ordering::Acquire) {
        return TestResult::Fail("exit-kick storm observer did not complete");
    }
    if storm_slot.state.load(Ordering::Acquire) & EXIT_KICK_LOCK != 0 {
        return TestResult::Fail("exit-kick storm left the bucket reserved");
    }

    if !matches!(
        storm_slot.publish(PID_A, TOTAL_ATTEMPTS + 1),
        KickPublishResult::Published { .. }
    ) || storm_slot.observe(PID_A).is_none()
    {
        return TestResult::Fail("exit-kick slot remained wedged after storm");
    }

    TestResult::Pass
}
