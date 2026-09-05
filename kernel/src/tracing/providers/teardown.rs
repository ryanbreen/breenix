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
    RECLAIM_PASS_SELECTION_CAPPED,
    "Production drain passes that ended at the selection cap"
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
counter!(
    RECLAIM_ABANDONED_UNQUEUED,
    "Reclaims abandoned because the pending queue refused them"
);
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
counter!(CLONE_INIT_GROUP_REFUSED, "CLONE_VM joins refused into the designated init group");
counter!(
    INIT_ORDINARY_PID_ALLOCATIONS,
    "Ordinary process IDs allocated from next_pid"
);
counter!(
    INIT_RESERVED_PID_COLLISIONS,
    "Ordinary allocations that landed on the reserved init PID"
);
counter!(INIT_DESIGNATION_ACCEPTED, "Init designations accepted");
counter!(INIT_DESIGNATION_REFUSED, "Init designations refused");
counter!(
    INIT_DESIGNATION_RETIRED,
    "Init designations retired with their row"
);
counter!(INIT_PUBLICATIONS, "Init rows published after designation");
counter!(
    INIT_REPARENT_CHILDREN,
    "Children reparented onto the designated init"
);
counter!(
    INIT_REPARENT_SKIPPED_NO_INIT,
    "Reparent requests skipped because no init is designated"
);

// The tombstone family. P6a moves the two TOMBSTONE_JOIN counters out of the
// declaration-only region below and gives them their producers: the join's reap
// arm and its retire arm each increment the one named for being second.
// TOMBSTONE_RESIDENT is a gauge and follows RECLAIM_PARK_RESIDENT's idiom
// exactly — `trace_count!` to increment, `trace_count_add!(_, u64::MAX)` to
// decrement — so a resident tombstone at quiesce is a visible leak.
counter!(TOMBSTONE_RESIDENT, "Resident reaped-but-unremoved rows");
counter!(TOMBSTONE_REMOVED, "Rows removed by the two-event join");
counter!(
    TOMBSTONE_JOIN_REAP_SECOND,
    "Tombstone joins completed by reap"
);
counter!(
    TOMBSTONE_JOIN_RETIRE_SECOND,
    "Tombstone joins completed by retire"
);

// Declaration-only until the phase named in PLAN.md. These intentionally have
// no trace_count! producer yet.
counter!(TEARDOWN_ENTRY_GROUP, "Group teardown entries");
counter!(EXIT_REQUEST_OBSERVED, "Observed latched exit requests");
counter!(LEDGER_EFFECT_AMBIGUOUS_REPORT, "Ambiguous report effects");
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

pub const COUNTER_COUNT: usize = 87;

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
    &RECLAIM_PASS_SELECTION_CAPPED,
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
    &CLONE_INIT_GROUP_REFUSED,
    &INIT_ORDINARY_PID_ALLOCATIONS,
    &INIT_RESERVED_PID_COLLISIONS,
    &INIT_DESIGNATION_ACCEPTED,
    &INIT_DESIGNATION_REFUSED,
    &INIT_DESIGNATION_RETIRED,
    &INIT_PUBLICATIONS,
    &INIT_REPARENT_CHILDREN,
    &INIT_REPARENT_SKIPPED_NO_INIT,
    &RECLAIM_PASS_SKIPPED,
    &RECLAIM_PARKED,
    &RECLAIM_UNPARKED_EPOCH,
    &RECLAIM_UNPARKED_ROW,
    &RECLAIM_UNPARKED_AGE,
    &RECLAIM_PARK_IMMEDIATE_UNPARK,
    &RECLAIM_PARK_RESIDENT,
    &RECLAIM_ABANDONED_UNQUEUED,
    &LEDGER_EFFECT_AMBIGUOUS_REPORT,
    &TOMBSTONE_RESIDENT,
    &TOMBSTONE_REMOVED,
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

/// Emit the P6a tombstone census from normal context.
///
/// Gate extras (b) and (f) are read off this line by a real workload rather
/// than by a fixture: it rides the same three call sites as the root-custody
/// summary — both architectures' boot paths once before userspace, and the
/// production heartbeat's cold `/proc/trace/counters` read as the run
/// progresses — so `resident` is observably nonzero while children are being
/// reaped and back to zero once the drain has retired them. A `resident` that
/// never returns to zero is a visible leak with a counter naming it.
pub fn emit_tombstone_census() {
    crate::serial_println!(
        "[TOMBSTONE_CENSUS:resident={}:removed={}:reap_second={}:retire_second={}:abandoned_unqueued={}]",
        TOMBSTONE_RESIDENT.aggregate(),
        TOMBSTONE_REMOVED.aggregate(),
        TOMBSTONE_JOIN_REAP_SECOND.aggregate(),
        TOMBSTONE_JOIN_RETIRE_SECOND.aggregate(),
        RECLAIM_ABANDONED_UNQUEUED.aggregate()
    );
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
static KSTACK_QUIESCE_LEAK_EMITTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// #661 F2: restore whole-boot kernel-stack slot-leak coverage on x86.
///
/// The old ownership oracle's window-scoped slot equality would have caught a
/// foreign kernel stack allocated inside its stress window and never freed. The
/// #646 cohort assertions deliberately cannot see that case: foreign allocation
/// motion is emitted as census data, while the oracle's assertions are scoped to
/// the slots it enrolled by identity.
///
/// This does not recover window-scoped *detection*, because that would reopen the
/// #646 race: the stimulus is still bounded by the oracle's window, but the
/// verdict is read at the point where the race structurally cannot occur — x86
/// quiescence, after userspace is complete and the deferred reclaim queues are
/// drained or the existing tombstone backstop has elapsed. The allocator's
/// boot-test-only watch is opened by the ownership gate immediately before its
/// stress window and closed immediately after, so a watched slot is exactly a
/// slot allocated inside that window; cohort enrolment and slot return clear the
/// watch, leaving only still-live foreign slots for this census to count.
///
/// Disclosed residual: a kernel-stack slot leaked *outside* the oracle's window —
/// before it or during the userspace phase after it — is not covered here, and
/// deliberately so, because such a slot is indistinguishable at quiescence from a
/// thread that is legitimately still alive. The scope is x86 only; aarch64's
/// oracle gate still has no equivalent quiesce-time kernel-stack leak check, and
/// this follow-up does not claim to close that gap.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn x86_settled_kstack_leak_census() {
    // x86 only: there is no aarch64 counterpart for this settled census.
    if KSTACK_QUIESCE_LEAK_EMITTED.load(Ordering::Relaxed) {
        return;
    }
    if KSTACK_QUIESCE_LEAK_EMITTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let baseline = crate::memory::kernel_stack::kernel_stack_quiesce_baseline_outstanding();
    let now = crate::memory::kernel_stack::kernel_stack_pool_counters();
    let outstanding = now.slots_allocated.saturating_sub(now.slots_freed);
    let leaked = crate::memory::kernel_stack::kernel_stack_quiesce_leaked_slots();
    crate::serial_println!(
        "[KSTACK_QUIESCE_LEAK:baseline_outstanding={}:outstanding={}:leaked={}]",
        baseline,
        outstanding,
        leaked
    );
}

/// P6a PR-2, review finding B2 — the **settled** half of the x86 retention
/// sample, and the only place in the matrix that looks at a real workload's
/// tombstones once the workload is over.
///
/// The sample `sys_exit` takes at the end of the userspace phase is taken at the
/// instant the last userspace thread leaves, which is *before* any drain could
/// retire the rows the four live reaps just claimed: it measures the gauge
/// nonzero on four real production rows. Nothing on x86 samples again — the
/// strand reporter fires once, before userspace, and the heartbeat's procfs
/// reader is an aarch64 userspace program — so retention at quiesce was
/// unmeasured on this arch, which is exactly the gap the review named.
///
/// This runs from the idle loop: the first context that exists after every
/// userspace thread is gone, and the context that drives the production drain
/// itself. It fires when that drain has nothing left to do (both deferred
/// reclaim queues empty) or at a `SETTLE_MS` backstop, and it emits whatever it
/// reads either way. It never consults the gauge it reports: a census that
/// prints only when it likes the answer feeds a self-fulfilling gate literal,
/// and a stranded tombstone has to reach the gate as a red rather than as
/// silence.
///
/// It carries the two queue depths because retention at quiesce is only half an
/// answer without them — a nonzero `resident` with a nonempty `pending` names a
/// drain that never completed, which is a different fault from a join that
/// failed to remove a retired-and-reaped row, and the gate should be able to
/// tell them apart from the serial alone.
///
/// Its own marker, not `[TOMBSTONE_CENSUS:`, so the gate can pin this sample and
/// the userspace-end sample independently: the two can legitimately carry
/// identical retention fields, and a shared marker would make both unpinnable by
/// exact count.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn x86_settled_tombstone_census() {
    // #767 (ruling R176): written as `2_000` against a clock that returned raw
    // 200 Hz PIT ticks, so this backstop has been 10 s of wall clock for its
    // whole life (introduced at ca8e9f8f, 2026-08-25, long after PIT_HZ became
    // 200 at c16faca1). Scaling by MS_PER_TICK keeps it at that 10 s instead of
    // cutting it to 2 s as a side effect of the producer repair. It is the one
    // shortened budget with no aarch64 twin to transfer evidence from -- this
    // function is x86-only -- and an early expiry is directly gate-failing,
    // because run-x86-boot-tests.sh pins parked=0 and resident=0 on the line it
    // emits.
    const SETTLE_MS: u64 = 2_000 * crate::time::timer::MS_PER_TICK;
    static BACKSTOP_AT_MS: AtomicU64 = AtomicU64::new(0);
    static EMITTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

    if EMITTED.load(Ordering::Relaxed) {
        return;
    }
    if !crate::syscall::handlers::USERSPACE_TEST_COMPLETE.load(Ordering::Relaxed) {
        return;
    }
    let now = crate::time::timer::get_monotonic_time();
    let backstop_at = BACKSTOP_AT_MS.load(Ordering::Relaxed);
    if backstop_at == 0 {
        BACKSTOP_AT_MS.store(now.saturating_add(SETTLE_MS).max(1), Ordering::Relaxed);
        return;
    }
    let (pending, parked) = crate::task::process_task::boot_reclaim_queue_census();
    if (pending != 0 || parked != 0) && now < backstop_at {
        return;
    }
    if EMITTED.swap(true, Ordering::Relaxed) {
        return;
    }
    x86_settled_kstack_leak_census();
    crate::serial_println!(
        "[TOMBSTONE_QUIESCE:resident={}:removed={}:reap_second={}:retire_second={}:abandoned_unqueued={}:pending={}:parked={}]",
        TOMBSTONE_RESIDENT.aggregate(),
        TOMBSTONE_REMOVED.aggregate(),
        TOMBSTONE_JOIN_REAP_SECOND.aggregate(),
        TOMBSTONE_JOIN_RETIRE_SECOND.aggregate(),
        RECLAIM_ABANDONED_UNQUEUED.aggregate(),
        pending,
        parked,
    );
    emit_reclaim_drain_census();
}

/// #653 delta (3). The production drain's refusal counters, printed.
///
/// `RECLAIM_DRAIN_NESTED_REFUSED` and `RECLAIM_CONTEXT_VIOLATIONS` existed
/// before this and were emitted by nothing: a whole-boot loss of production
/// reclamation was inferable only three phases later, from a tombstone census
/// that had to be read alongside a queue depth. Naming them at the source makes
/// the failure visible where it happens.
///
/// Emitted from the settled census — once per boot, from the idle loop after the
/// userspace phase, in the same normal-context reporter that already prints
/// `[TOMBSTONE_QUIESCE:...]`. Nothing here runs on an interrupt, syscall or
/// context-switch path.
///
/// `injected` is the count of refusals staged on purpose by
/// `boot_prove_nested_drain_refusal`. Without it a `nested=0` line would be
/// indistinguishable from a refusal arm that no longer executes at all, and the
/// pin on this line would be vacuous.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
fn emit_reclaim_drain_census() {
    let (epoch, hardware, shadow, selectable) =
        crate::task::process_task::boot_pending_blocker_census();
    crate::serial_println!(
        "[RECLAIM_DRAIN:nested={}:context_violations={}:selection_capped={}:injected={}:pend_epoch={}:pend_hw={}:pend_shadow={}:pend_selectable={}]",
        RECLAIM_DRAIN_NESTED_REFUSED.aggregate(),
        RECLAIM_CONTEXT_VIOLATIONS.aggregate(),
        RECLAIM_PASS_SELECTION_CAPPED.aggregate(),
        crate::task::process_task::boot_injected_nested_refusals(),
        epoch,
        hardware,
        shadow,
        selectable,
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
    kernel_stack_slot_returns: AtomicU64,
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
            kernel_stack_slot_returns: AtomicU64::new(0),
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
    KernelStackSlotReturn,
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
                BootTestPidCountKind::KernelStackSlotReturn => {
                    slot.kernel_stack_slot_returns
                        .fetch_add(1, Ordering::Release);
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
        slot.kernel_stack_slot_returns.store(0, Ordering::Relaxed);
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
    kernel_stack_slot_returns: u64,
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
                kernel_stack_slot_returns: slot.kernel_stack_slot_returns.load(Ordering::Relaxed),
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
pub fn record_kernel_stack_slot_return(pid: u64) {
    // Called from `KernelStack::drop`; keep this allocation-free and lock-light.
    #[cfg(feature = "boot_tests")]
    record_boot_test_pid_count(pid, BootTestPidCountKind::KernelStackSlotReturn);
    #[cfg(not(feature = "boot_tests"))]
    let _ = pid;
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
    pub kernel_stack_slot_returns: u64,
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
        kernel_stack_slot_returns: slot.kernel_stack_slot_returns.load(Ordering::Acquire),
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

/// `CLONE_ADMISSION_ADMITTED` counts lifecycle admissions of the parent row, not published children: `admit_clone_into` runs upstream of the P5b init-group refusal, so `published_clone_children = CLONE_ADMISSION_ADMITTED - CLONE_INIT_GROUP_REFUSED - later resource failures`.
#[inline(always)]
pub fn record_clone_admission(admitted: bool) {
    if admitted {
        crate::trace_count!(CLONE_ADMISSION_ADMITTED);
    } else {
        crate::trace_count!(CLONE_ADMISSION_REFUSED);
    }
}

#[inline(always)]
pub fn record_init_group_refusal() {
    crate::trace_count!(CLONE_INIT_GROUP_REFUSED);
}

#[inline(always)]
pub fn record_ordinary_pid_allocation(raw_pid: u64, reserved_init_pid: u64) {
    crate::trace_count!(INIT_ORDINARY_PID_ALLOCATIONS);
    if raw_pid == reserved_init_pid {
        crate::trace_count!(INIT_RESERVED_PID_COLLISIONS);
    }
}

#[inline(always)]
pub fn record_init_designation(accepted: bool) {
    if accepted {
        crate::trace_count!(INIT_DESIGNATION_ACCEPTED);
    } else {
        crate::trace_count!(INIT_DESIGNATION_REFUSED);
    }
}

#[inline(always)]
pub fn record_init_designation_retired() {
    crate::trace_count!(INIT_DESIGNATION_RETIRED);
}

#[inline(always)]
pub fn record_init_publication() {
    crate::trace_count!(INIT_PUBLICATIONS);
}

#[inline(always)]
pub fn record_init_reparent(children: usize, designated: bool) {
    if designated {
        crate::trace_count_add!(INIT_REPARENT_CHILDREN, children as u64);
    } else {
        crate::trace_count!(INIT_REPARENT_SKIPPED_NO_INIT);
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

#[cfg(feature = "boot_tests")]
pub fn init_group_refusals_total() -> u64 {
    CLONE_INIT_GROUP_REFUSED.aggregate()
}

#[cfg(feature = "boot_tests")]
pub struct InitGroupWalk {
    rows: usize,
    init_tgid_rows: usize,
    foreign_tgid_rows: usize,
    init_row_present: bool,
    refused: u64,
}

#[cfg(feature = "boot_tests")]
pub fn init_group_walk_census(manager: &crate::process::ProcessManager) -> InitGroupWalk {
    let Some(init) = manager.designated_init() else {
        return InitGroupWalk {
            rows: 0,
            init_tgid_rows: 0,
            foreign_tgid_rows: 0,
            init_row_present: false,
            refused: 0,
        };
    };
    let init_tg_id = manager
        .get_process(init)
        .and_then(|process| process.thread_group_id)
        .unwrap_or(init.as_u64());
    let init_row_present = manager.get_process(init).is_some();
    let mut rows = 0;
    let mut init_tgid_rows = 0;
    let mut foreign_tgid_rows = 0;

    for (pid, process) in manager.iter_processes() {
        rows += 1;
        let effective_tg_id = process.thread_group_id.unwrap_or(pid.as_u64());
        if effective_tg_id == init_tg_id {
            init_tgid_rows += 1;
            if pid != init {
                foreign_tgid_rows += 1;
            }
        }
    }

    InitGroupWalk {
        rows,
        init_tgid_rows,
        foreign_tgid_rows,
        init_row_present,
        refused: CLONE_INIT_GROUP_REFUSED.aggregate(),
    }
}

#[cfg(feature = "boot_tests")]
pub fn emit_init_group_walk(walk: InitGroupWalk) {
    let verdict = if walk.init_row_present
        && walk.rows >= 1
        && walk.init_tgid_rows == 1
        && walk.foreign_tgid_rows == 0
    {
        "PASS"
    } else {
        "FAIL"
    };
    #[cfg(target_arch = "aarch64")]
    let arch = "aarch64";
    #[cfg(target_arch = "x86_64")]
    let arch = "x86";
    crate::serial_println!(
        "[INIT_GROUP_WALK:{}:rows={}:init_tgid_rows={}:foreign_tgid_rows={}:refused={}:verdict={}]",
        arch,
        walk.rows,
        walk.init_tgid_rows,
        walk.foreign_tgid_rows,
        walk.refused,
        verdict
    );
}

#[cfg(feature = "boot_tests")]
pub fn init_ordinary_pid_allocations() -> u64 {
    INIT_ORDINARY_PID_ALLOCATIONS.aggregate()
}

pub fn init_reserved_pid_collisions_total() -> u64 {
    INIT_RESERVED_PID_COLLISIONS.aggregate()
}

#[cfg(feature = "boot_tests")]
pub fn init_designation_accepted() -> u64 {
    INIT_DESIGNATION_ACCEPTED.aggregate()
}

#[cfg(feature = "boot_tests")]
pub fn init_designation_refused() -> u64 {
    INIT_DESIGNATION_REFUSED.aggregate()
}

#[cfg(feature = "boot_tests")]
pub fn init_designation_retired() -> u64 {
    INIT_DESIGNATION_RETIRED.aggregate()
}

#[cfg(feature = "boot_tests")]
pub fn init_publications() -> u64 {
    INIT_PUBLICATIONS.aggregate()
}

#[cfg(feature = "boot_tests")]
pub fn init_reparent_children() -> u64 {
    INIT_REPARENT_CHILDREN.aggregate()
}

#[cfg(feature = "boot_tests")]
pub fn init_reparent_skipped_no_init() -> u64 {
    INIT_REPARENT_SKIPPED_NO_INIT.aggregate()
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

/// The quiesce/cleanup budget the 13 retirement-oracle loops below wait out, in
/// the milliseconds `retirement_oracle_clock_delta` takes.
///
/// #767 (ruling R176). Each of those 13 loops was written as
/// `retirement_oracle_clock_delta(5_000)`, and each was measured green at the
/// wall-clock budget that expression actually bought at the time, which is not
/// the same number on the two arches:
///
/// * aarch64: `retirement_oracle_clock_delta` is derived from the generic
///   timer frequency, so 5_000 has been 5 s of wall clock since that arm was
///   written, and #767 does not touch that clock.
/// * x86_64: the clock underneath is `get_monotonic_time()`, which returned raw
///   200 Hz PIT ticks until #767 scaled it. `delta(5_000)` therefore bought
///   5000 ticks -- 25 s of wall clock, and 5000 rounds of the inner one-tick
///   wait. Leaving the literal at 5_000 once the producer is correct would cut
///   both to a fifth as a silent side effect of a units repair.
///
/// Scaling by `MS_PER_TICK` keeps each arch at the budget its green runs were
/// measured at: 25 s / 5000 rounds on x86, 5 s on aarch64. Tightening either is
/// a separate change that owes its own evidence.
///
/// The inner `retirement_oracle_clock_delta(1)` pacing waits are deliberately
/// not scaled: on x86 the clock advances in `MS_PER_TICK` steps, so `now + 1`
/// resolves to the next tick both before and after #767 -- one tick, 5 ms,
/// unchanged.
#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
const RETIREMENT_ORACLE_QUIESCE_MS: u64 = 5_000 * crate::time::timer::MS_PER_TICK;

/// aarch64's half of `RETIREMENT_ORACLE_QUIESCE_MS` above: 5 s, the value this
/// arch's frequency-derived oracle clock gives, unchanged by #767.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const RETIREMENT_ORACLE_QUIESCE_MS: u64 = 5_000;

/// `retirement_oracle_clock_now() + RETIREMENT_ORACLE_QUIESCE_MS`, in one place.
///
/// Thirteen loops in this module arm exactly this deadline; naming it keeps the
/// budget above from having to be restated thirteen times, which is how the
/// x86/aarch64 divergence it corrects went unnoticed.
#[cfg(feature = "boot_tests")]
fn retirement_oracle_quiesce_deadline() -> u64 {
    retirement_oracle_clock_now()
        .saturating_add(retirement_oracle_clock_delta(RETIREMENT_ORACLE_QUIESCE_MS))
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
    // These synthetic threads are never dispatched, so no CPU ever names their
    // kernel-stack slots and `KernelStack::drop`'s liveness guard cannot refuse.
    let parent_privilege = crate::task::thread::ThreadPrivilege::User;
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
            let Some(child_thread) = manager
                .get_process_mut(child_pid)
                .and_then(|process| process.main_thread.as_mut())
            else {
                return TestResult::Fail("pairing child has no main thread");
            };
            let child_tid = child_thread.id;
            let published = child_thread.publish_to_scheduler();
            #[cfg(target_arch = "x86_64")]
            if let Some(old_page_table) = pending_old_page_table.take() {
                let Some(child_process) = manager.get_process_mut(child_pid) else {
                    return TestResult::Fail("pairing child disappeared before old-root install");
                };
                child_process.pending_old_page_tables.push(old_page_table);
            }
            (child_pid, child_tid, published)
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
        core::mem::drop(child.2);
    }

    let quiesce_deadline = retirement_oracle_quiesce_deadline();
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
    let kstack_returns = pairing_child_pids
        .iter()
        .map(|pid| boot_test_pid_counts(*pid).kernel_stack_slot_returns)
        .sum::<u64>();
    #[cfg(target_arch = "x86_64")]
    let cohort_recorded = expected_tables * pairing_child_pids.len() as u64
        + expected_pending_old_tables;
    #[cfg(target_arch = "x86_64")]
    let allocator_balance = allocator_used_after as i64 - allocator_used_before as i64;
    #[cfg(target_arch = "aarch64")]
    crate::serial_println!(
        "[PT_RETIRE_ORACLE:aarch64:cycles=64:used_before={}:used_after={}:expected_tables={}:roots={}:returned={}:lost={}:kstack_returns={}]",
        allocator_used_before,
        allocator_used_after,
        expected_tables,
        roots_retired_delta,
        table_frames_returned_delta,
        table_frames_lost_delta,
        kstack_returns
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
        if counts.kernel_stack_slot_returns != 1 {
            return TestResult::Fail("retire cohort per-PID kernel-stack return was not exact");
        }
    }
    if kstack_returns != pairing_child_pids.len() as u64 {
        return TestResult::Fail("retire cohort kernel-stack return population was not exact");
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
        let cleanup_deadline = retirement_oracle_quiesce_deadline();
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
            "[PT_RETIRE_COHORT:x86:children={}:retired={}:returned={}:recorded={}:lost={}:no_arch={}:undecided={}:mid_retire={}:kstack_returns={}:balance={}]",
            pairing_child_pids.len(),
            roots_retired_delta,
            table_frames_returned_delta,
            cohort_recorded,
            table_frames_lost_delta,
            no_arch_delta,
            dropped_undecided_delta,
            dropped_mid_retire_delta,
            kstack_returns,
            allocator_balance
        );
    }
    TestResult::Pass
}

/// P6a gate extras (b), (f), (g) and the C3 arbiter — the two-event join, both
/// orders, in one run.
///
/// The join has two arms and each is dormant code unless something reaches it.
/// The natural fork/exit/reap cohort only ever produces one of them: the parent
/// collects the status long before grace elapses, so retirement is always the
/// second event. This oracle drives **both** orders deterministically on rows
/// that genuinely exited, and observes the `TOMBSTONE_RESIDENT` gauge nonzero
/// mid-run and back to its entry value afterwards — both halves of the
/// anti-vacuity burden, without a sleep race deciding a merge gate.
///
/// The rows carry no page table, so the oracle moves no frame, leaf or
/// kernel-stack accounting and none of the eight pinned oracle literals depend
/// on it.
#[cfg(feature = "boot_tests")]
pub fn tombstone_join_oracle_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    let _reclaim_owner = match crate::task::process_task::BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(_) => {
            return TestResult::Fail("reclaim queues not quiescent at tombstone join oracle start")
        }
    };

    /// Insert a row, terminate it, and defer its (empty) resources so exactly one
    /// retirement receipt names it. The row is a zombie on return: terminated,
    /// unreaped, one obligation outstanding.
    fn stage_zombie(
        name: &str,
    ) -> Result<
        (
            crate::process::ProcessId,
            crate::task::process_task::PendingProcessReclaim,
        ),
        &'static str,
    > {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return Err("process manager unavailable for tombstone join fixture");
        };
        let pid = manager.allocate_pid();
        let process = crate::process::Process::new(
            pid,
            alloc::string::String::from(name),
            VirtAddr::new(0x0040_0000),
        );
        manager.insert_process(pid, process);
        let Some(row) = manager.get_process_mut(pid) else {
            return Err("tombstone join fixture row disappeared after insert");
        };
        row.terminate_minimal(0);
        let reclaim = crate::task::process_task::defer_process_resources(row);
        Ok((pid, reclaim))
    }

    fn row_is_visible_to_live_queries(pid: crate::process::ProcessId) -> bool {
        let manager_guard = crate::process::manager();
        manager_guard
            .as_ref()
            .is_some_and(|manager| manager.get_process(pid).is_some())
    }

    fn tombstone_rows() -> usize {
        let manager_guard = crate::process::manager();
        manager_guard
            .as_ref()
            .map_or(0, |manager| manager.tombstone_row_count())
    }

    fn reap(
        pid: crate::process::ProcessId,
        reaper: crate::process::ProcessId,
    ) -> Result<bool, &'static str> {
        let mut evicted = None;
        let claimed = {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                return Err("process manager unavailable for tombstone join reap");
            };
            match manager.reap_row(pid, reaper, 0) {
                crate::process::manager::ReapOutcome::Claimed(row) => {
                    evicted = row;
                    true
                }
                crate::process::manager::ReapOutcome::Refused => false,
            }
        };
        drop(evicted);
        Ok(claimed)
    }

    /// Drain until `record_reclaim` has run for one more receipt, or the deadline
    /// passes. Returns whether the retirement was observed.
    fn drain_one_retirement(reclaims_before: u64) -> bool {
        let deadline = retirement_oracle_quiesce_deadline();
        loop {
            crate::task::scheduler::nudge_retirement_grace_for_test();
            let boundary =
                retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
            while retirement_oracle_clock_now() < boundary {
                core::hint::spin_loop();
            }
            crate::task::process_task::boot_reclaim_deferred_process_resources();
            if TEARDOWN_RECLAIM.aggregate() != reclaims_before {
                return true;
            }
            if retirement_oracle_clock_now() >= deadline {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    let reaper = crate::process::ProcessId::new(crate::process::RESERVED_INIT_PID);
    let resident_before = TOMBSTONE_RESIDENT.aggregate();
    let removed_before = TOMBSTONE_REMOVED.aggregate();
    let reap_second_before = TOMBSTONE_JOIN_REAP_SECOND.aggregate();
    let retire_second_before = TOMBSTONE_JOIN_RETIRE_SECOND.aggregate();
    let tombstone_rows_before = tombstone_rows();

    // ---- Arm 1: the reap lands first, retirement completes the join --------
    let (pid_a, reclaim_a) = match stage_zombie("tombstone_join_retire_second") {
        Ok(staged) => staged,
        Err(reason) => return TestResult::Fail(reason),
    };
    if !row_is_visible_to_live_queries(pid_a) {
        return TestResult::Fail("A1: a zombie was already invisible to live queries");
    }
    match reap(pid_a, reaper) {
        Ok(true) => {}
        Ok(false) => return TestResult::Fail("A2: the first reap of a zombie was refused"),
        Err(reason) => return TestResult::Fail(reason),
    }
    // Gate extra (b)/(f), first half: the gauge is observably nonzero here.
    if TOMBSTONE_RESIDENT.aggregate() != resident_before.wrapping_add(1) {
        return TestResult::Fail("A3: the reap did not make TOMBSTONE_RESIDENT nonzero");
    }
    if tombstone_rows() != tombstone_rows_before + 1 {
        return TestResult::Fail("A4: the reaped row did not stay resident as a tombstone");
    }
    if row_is_visible_to_live_queries(pid_a) {
        return TestResult::Fail("A5: a tombstone answered a live-process query");
    }
    if TOMBSTONE_REMOVED.aggregate() != removed_before {
        return TestResult::Fail("A6: the reap removed the row with retirement outstanding");
    }
    // Condition C3: the claim is the arbiter, so a second reaper is refused and
    // reports nothing. This is what a concurrent losing waiter observes.
    match reap(pid_a, reaper) {
        Ok(false) => {}
        Ok(true) => return TestResult::Fail("A7: a second reap of the same row was admitted"),
        Err(reason) => return TestResult::Fail(reason),
    }

    let reclaims_before_a = TEARDOWN_RECLAIM.aggregate();
    crate::task::process_task::enqueue_process_reclaim(reclaim_a);
    if !drain_one_retirement(reclaims_before_a) {
        return TestResult::Fail("A8: the tombstone's retirement never completed");
    }
    core::sync::atomic::fence(Ordering::Acquire);
    if TOMBSTONE_JOIN_RETIRE_SECOND.aggregate() != retire_second_before.wrapping_add(1) {
        return TestResult::Fail("A9: retirement did not complete the join as the second event");
    }
    if TOMBSTONE_JOIN_REAP_SECOND.aggregate() != reap_second_before {
        return TestResult::Fail("A10: the wrong join arm was credited");
    }
    if TOMBSTONE_REMOVED.aggregate() != removed_before.wrapping_add(1) {
        return TestResult::Fail("A11: the join did not remove the row");
    }
    // Gate extra (b)/(f), second half: the gauge returns to its entry value.
    if TOMBSTONE_RESIDENT.aggregate() != resident_before {
        return TestResult::Fail("A12: TOMBSTONE_RESIDENT did not return to its entry value");
    }
    if tombstone_rows() != tombstone_rows_before {
        return TestResult::Fail("A13: a tombstone survived its own join");
    }

    // ---- Arm 2: retirement lands first, the reap completes the join --------
    let (pid_b, reclaim_b) = match stage_zombie("tombstone_join_reap_second") {
        Ok(staged) => staged,
        Err(reason) => return TestResult::Fail(reason),
    };
    let reclaims_before_b = TEARDOWN_RECLAIM.aggregate();
    crate::task::process_task::enqueue_process_reclaim(reclaim_b);
    if !drain_one_retirement(reclaims_before_b) {
        return TestResult::Fail("B1: the zombie's retirement never completed");
    }
    core::sync::atomic::fence(Ordering::Acquire);
    if TOMBSTONE_REMOVED.aggregate() != removed_before.wrapping_add(1) {
        return TestResult::Fail("B2: retirement removed an unreaped row");
    }
    if !row_is_visible_to_live_queries(pid_b) {
        return TestResult::Fail("B3: a retired-but-unreaped row stopped answering waitpid");
    }
    match reap(pid_b, reaper) {
        Ok(true) => {}
        Ok(false) => return TestResult::Fail("B4: the reap of a retired zombie was refused"),
        Err(reason) => return TestResult::Fail(reason),
    }
    if TOMBSTONE_JOIN_REAP_SECOND.aggregate() != reap_second_before.wrapping_add(1) {
        return TestResult::Fail("B5: the reap did not complete the join as the second event");
    }
    if TOMBSTONE_JOIN_RETIRE_SECOND.aggregate() != retire_second_before.wrapping_add(1) {
        return TestResult::Fail("B6: the wrong join arm was credited");
    }
    if TOMBSTONE_REMOVED.aggregate() != removed_before.wrapping_add(2) {
        return TestResult::Fail("B7: the join did not remove the row");
    }
    if TOMBSTONE_RESIDENT.aggregate() != resident_before {
        return TestResult::Fail("B8: TOMBSTONE_RESIDENT did not return to its entry value");
    }
    if tombstone_rows() != tombstone_rows_before {
        return TestResult::Fail("B9: a tombstone survived its own join");
    }
    if row_is_visible_to_live_queries(pid_b) {
        return TestResult::Fail("B10: a removed row still answered a live-process query");
    }

    crate::serial_println!(
        "[TOMBSTONE_JOIN_ORACLE:{}:retire_second={}:reap_second={}:removed={}:resident_delta={}:tombstone_rows={}:PASS]",
        if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86"
        },
        TOMBSTONE_JOIN_RETIRE_SECOND
            .aggregate()
            .wrapping_sub(retire_second_before),
        TOMBSTONE_JOIN_REAP_SECOND
            .aggregate()
            .wrapping_sub(reap_second_before),
        TOMBSTONE_REMOVED.aggregate().wrapping_sub(removed_before),
        TOMBSTONE_RESIDENT.aggregate().wrapping_sub(resident_before),
        tombstone_rows(),
    );
    TestResult::Pass
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_tombstone_join_gate() {
    crate::serial_println!("[TEST:process:tombstone_join_oracle:START]");
    let result = tombstone_join_oracle_test();
    if !result.is_pass() {
        crate::serial_println!("[TEST:process:tombstone_join_oracle:FAIL:{:?}]", result);
    }
    assert!(result.is_pass(), "x86 tombstone join oracle gate failed");
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

    let quiesce_deadline = retirement_oracle_quiesce_deadline();
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
    let cleanup_deadline = retirement_oracle_quiesce_deadline();
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

    // The leaf residual below remains a manifestation of issue #583:
    // `GuardedStack::drop` does not reclaim user-stack frames. `stack_residual`
    // used to include a kernel-stack component too; `KernelStack::drop` now
    // releases that component, leaving the user-stack residue still owned by
    // #583.
    #[cfg(target_arch = "aarch64")]
    const EXPECTED_LEAF_RESIDUAL: u64 = 16;
    #[cfg(target_arch = "x86_64")]
    const EXPECTED_LEAF_RESIDUAL: u64 = 16;
    // Residual measured before kernel-stack frames were released in
    // `KernelStack::drop`.
    #[cfg(target_arch = "aarch64")]
    const EXPECTED_STACK_RESIDUAL_PRE_KSTACK_RELEASE: i64 = 18;
    #[cfg(target_arch = "x86_64")]
    const EXPECTED_STACK_RESIDUAL_PRE_KSTACK_RELEASE: i64 = 149;
    // x86: 149 - 128 * 1 = 21 (one stack dropped in the window, 128 frames per
    // stack). aarch64: 18 - 0 = 18 because HHDM-preallocated stacks release no
    // frames.
    #[cfg(target_arch = "aarch64")]
    const EXPECTED_STACK_RESIDUAL: i64 = 18;
    #[cfg(target_arch = "x86_64")]
    const EXPECTED_STACK_RESIDUAL: i64 = 21;
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
                .map(|(_, _, commit)| {
                    // #721 m1: deliberately not applied. This oracle reads detach/
                    // refusal state straight off the process row and tears the row
                    // down immediately after (retire_and_remove_owned_row /
                    // exit_and_remove_unowned_row), so there is no live syscall
                    // caller here for the scheduler-side commit to matter to —
                    // unlike every real exec path, which must apply it.
                    let _ = commit;
                })
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
        let cleanup_deadline = retirement_oracle_quiesce_deadline();
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
    let kstack_pool_before = crate::memory::kernel_stack::kernel_stack_pool_counters();
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
    // #721 B2: the live-sibling refusal is now armed on both architectures
    // (x86 exec_process/exec_process_with_argv gained the guard alongside
    // aarch64's), so this oracle exercises it on both arches too — the sibling
    // arm below is no longer aarch64-only.
    let mut sibling_refused = 0usize;
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

    let quiesce_deadline = retirement_oracle_quiesce_deadline();
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
    let kstack_pool_after = crate::memory::kernel_stack::kernel_stack_pool_counters();
    let stack_residual = allocator_used_after as i64 - allocator_used_before as i64;
    let Some(kstack_frames_released) = kstack_pool_after
        .frames_released
        .checked_sub(kstack_pool_before.frames_released)
    else {
        return TestResult::Fail("exec detach kernel-stack release counter regressed");
    };
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
    if sibling_refused != 2 && first_failure.is_none() {
        first_failure = Some("live-sibling refusal count was not exact");
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
    if EXPECTED_STACK_RESIDUAL_PRE_KSTACK_RELEASE - stack_residual != kstack_frames_released as i64
        && first_failure.is_none()
    {
        first_failure =
            Some("exec detach residual movement was not exactly kernel-stack frame release");
    }
    #[cfg(target_arch = "x86_64")]
    if kstack_frames_released % 128 != 0 && first_failure.is_none() {
        first_failure = Some("exec detach released a partial x86 kernel stack");
    }
    if stack_residual != EXPECTED_STACK_RESIDUAL && first_failure.is_none() {
        first_failure = Some("exec detach user-stack residual changed");
    }

    #[cfg(target_arch = "aarch64")]
    let arch = "aarch64";
    #[cfg(target_arch = "x86_64")]
    let arch = "x86";
    crate::serial_println!(
        "[EXEC_DETACH_ORACLE:{}:bodies={}:fail_preserved={}:sibling_refused={}:success_detached={}:fresh_root={}:tgid_self={}:custody_balance={}:leaf_residual={}:stack_residual={}:kstack_frames_released={}:old_group_reached_pre={}:old_group_missed_post={}:self_group_reached_post={}]",
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
        kstack_frames_released,
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

    fn reclaim_progress_sample() -> [u64; 4] {
        [
            PT_RETIRE_BUDGET_REQUEUED.aggregate(),
            PT_TABLE_FRAMES_RETURNED.aggregate(),
            PT_ROOTS_RETIRED.aggregate(),
            TEARDOWN_RECLAIM.aggregate(),
        ]
    }

    fn retire_and_remove_owned_row(
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

    let refusal_deadline = retirement_oracle_quiesce_deadline();
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

    let dispatch_deadline = retirement_oracle_quiesce_deadline();
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
    if let Err(reason) =
        retire_and_remove_owned_row(pid, probe_reference_clear, &mut retirement_blockers)
    {
        if first_failure.is_none() {
            first_failure = Some(reason);
        }
    }

    let cleanup_deadline = retirement_oracle_quiesce_deadline();
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

    let settle_deadline = retirement_oracle_quiesce_deadline();
    let mut settle_rounds = 0u64;
    let mut stable_rounds = 0u64;
    let mut settle_sample = reclaim_progress_sample();
    let mut settle_timed_out = false;
    loop {
        let grace_target = crate::task::scheduler::retirement_grace_target();
        crate::task::scheduler::nudge_retirement_grace_for_test();
        let grace_elapsed = loop {
            if crate::task::scheduler::retirement_grace_elapsed(&grace_target) {
                break true;
            }
            if retirement_oracle_clock_now() >= settle_deadline {
                break false;
            }
            core::hint::spin_loop();
        };
        if !grace_elapsed {
            settle_timed_out = true;
            break;
        }
        crate::task::process_task::boot_reclaim_deferred_process_resources();
        crate::task::scheduler::reclaim_terminated_threads();
        core::sync::atomic::fence(Ordering::Acquire);
        let next_sample = reclaim_progress_sample();
        settle_rounds = settle_rounds.saturating_add(1);
        if crate::task::process_task::boot_reclaim_queue_census() == (0, 0)
            && next_sample == settle_sample
        {
            stable_rounds = stable_rounds.saturating_add(1);
            if stable_rounds >= 3 {
                break;
            }
        } else {
            stable_rounds = 0;
        }
        settle_sample = next_sample;
        if retirement_oracle_clock_now() >= settle_deadline {
            settle_timed_out = true;
            break;
        }
        core::hint::spin_loop();
    }
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

    let quiesce_deadline = retirement_oracle_quiesce_deadline();
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

#[cfg(feature = "boot_tests")]
pub fn init_designation_oracle_test() -> crate::test_framework::registry::TestResult {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    use crate::test_framework::registry::TestResult;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    fn test_user_entry() {}

    fn test_thread(
        pid: crate::process::ProcessId,
        name: &'static str,
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
        thread.state = crate::task::thread::ThreadState::Ready;
        thread
    }

    fn synthetic_row(
        pid: crate::process::ProcessId,
        name: &'static str,
        thread_name: &'static str,
    ) -> crate::process::Process {
        let mut row = crate::process::Process::new(
            pid,
            alloc::string::String::from(name),
            VirtAddr::new(0x0040_0000),
        );
        row.set_main_thread(test_thread(pid, thread_name));
        row
    }

    fn too_small_image() -> [u8; 8] {
        [0; 8]
    }

    fn out_of_bounds_segment_image() -> [u8; 120] {
        #[cfg(target_arch = "x86_64")]
        let machine = 0x3eu16;
        #[cfg(target_arch = "aarch64")]
        let machine = 0xb7u16;

        let mut image = [0u8; 120];
        image[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        image[4] = 2;
        image[5] = 1;
        image[6] = 1;
        image[16..18].copy_from_slice(&2u16.to_le_bytes());
        image[18..20].copy_from_slice(&machine.to_le_bytes());
        image[20..24].copy_from_slice(&1u32.to_le_bytes());
        image[24..32].copy_from_slice(&0x0040_0000u64.to_le_bytes());
        image[32..40].copy_from_slice(&64u64.to_le_bytes());
        image[52..54].copy_from_slice(&64u16.to_le_bytes());
        image[54..56].copy_from_slice(&56u16.to_le_bytes());
        image[56..58].copy_from_slice(&1u16.to_le_bytes());

        image[64..68].copy_from_slice(&1u32.to_le_bytes());
        image[68..72].copy_from_slice(&5u32.to_le_bytes());
        image[72..80].copy_from_slice(&120u64.to_le_bytes());
        image[80..88].copy_from_slice(&0x0040_0000u64.to_le_bytes());
        image[88..96].copy_from_slice(&0x0040_0000u64.to_le_bytes());
        image[96..104].copy_from_slice(&1u64.to_le_bytes());
        image[104..112].copy_from_slice(&1u64.to_le_bytes());
        image[112..120].copy_from_slice(&4096u64.to_le_bytes());
        image
    }

    let reclaim_owner = match crate::task::process_task::BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(_) => {
            return TestResult::Fail(
                "reclaim queues not quiescent at init designation oracle start",
            )
        }
    };
    let construct_used_before = frame_allocator_used_frames();
    let construct_undecided_before = PT_ROOT_DROPPED_UNDECIDED.aggregate();
    let accepted_before = init_designation_accepted();
    let refused_before = init_designation_refused();
    let retired_before = init_designation_retired();
    let publications_before = init_publications();
    let reparent_children_before = init_reparent_children();
    let reparent_skipped_before = init_reparent_skipped_no_init();
    let ordinary_allocations_before = init_ordinary_pid_allocations();
    let reserved = crate::process::ProcessId::new(crate::process::RESERVED_INIT_PID);
    let mut construct_failed = 0u64;
    let mut refused = 0u64;
    let mut accepted = 0u64;
    let mut published = 0u64;
    let mut retired = 0u64;
    let mut held_error_removals = 0u64;
    let mut reparented = 0u64;
    let mut reparent_skipped = 0u64;
    let ordinary_allocated_expected = 5u64;
    let mut first_failure: Option<&'static str> = None;

    // A1: reject an image before the loader has a complete ELF header.
    {
        let image = too_small_image();
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A1");
        };
        #[cfg(target_arch = "x86_64")]
        let construction = manager.create_init_process(
            alloc::string::String::from("init_oracle_a1"),
            &image,
        );
        #[cfg(target_arch = "aarch64")]
        let construction = {
            let argv = [b"/sbin/init".as_slice()];
            manager.create_init_process_with_argv(
                alloc::string::String::from("init_oracle_a1"),
                &image,
                &argv,
            )
        };
        match construction {
            Err(_) => construct_failed += 1,
            Ok(_) if first_failure.is_none() => {
                first_failure = Some("init designation A1 constructor unexpectedly succeeded")
            }
            Ok(_) => {}
        }
        if manager.get_process(reserved).is_some() && first_failure.is_none() {
            first_failure = Some("init designation A1 left a reserved process row");
        }
        if manager.designated_init().is_some() && first_failure.is_none() {
            first_failure = Some("init designation A1 changed the init designation");
        }
        manager.remove_process(reserved);
    }

    // A2: reject a PT_LOAD whose file data lies outside the image, after the
    // process page table has been constructed.
    {
        let image = out_of_bounds_segment_image();
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A2");
        };
        #[cfg(target_arch = "x86_64")]
        let construction = manager.create_init_process(
            alloc::string::String::from("init_oracle_a2"),
            &image,
        );
        #[cfg(target_arch = "aarch64")]
        let construction = {
            let argv = [b"/sbin/init".as_slice()];
            manager.create_init_process_with_argv(
                alloc::string::String::from("init_oracle_a2"),
                &image,
                &argv,
            )
        };
        match construction {
            Err("Segment data out of bounds") => construct_failed += 1,
            Err(_) => {
                construct_failed += 1;
                if first_failure.is_none() {
                    first_failure = Some("init designation A2 returned the wrong loader error");
                }
            }
            Ok(_) if first_failure.is_none() => {
                first_failure = Some("init designation A2 constructor unexpectedly succeeded")
            }
            Ok(_) => {}
        }
        if manager.get_process(reserved).is_some() && first_failure.is_none() {
            first_failure = Some("init designation A2 left a reserved process row");
        }
        if manager.designated_init().is_some() && first_failure.is_none() {
            first_failure = Some("init designation A2 changed the init designation");
        }
        manager.remove_process(reserved);
    }

    let construct_used_after = frame_allocator_used_frames();
    let designation_used_before = construct_used_after;
    let construct_residual = construct_used_after as i64 - construct_used_before as i64;
    let construct_undecided = PT_ROOT_DROPPED_UNDECIDED
        .aggregate()
        .saturating_sub(construct_undecided_before);

    // A3: a ticket for an ordinary PID cannot designate init.
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A3");
        };
        let probe = manager.allocate_pid();
        if probe.as_u64() < crate::process::FIRST_ORDINARY_PID && first_failure.is_none() {
            first_failure = Some("init designation A3 ordinary PID used the reserved range");
        }
        manager.insert_process(
            probe,
            synthetic_row(probe, "init_oracle_a3", "init_oracle_a3_main"),
        );
        match manager.hold_init_publication(probe) {
            Ok(ticket) => match manager.designate_init(ticket) {
                Err(_) => refused += 1,
                Ok(_) if first_failure.is_none() => {
                    first_failure = Some("init designation A3 accepted an ordinary PID")
                }
                Ok(_) => {}
            },
            Err(_) if first_failure.is_none() => {
                first_failure = Some("init designation A3 failed to issue a ticket")
            }
            Err(_) => {}
        }
        if manager.designated_init().is_some() && first_failure.is_none() {
            first_failure = Some("init designation A3 changed the init designation");
        }
        manager.remove_process(probe);
    }

    // A4: a ticket cannot outlive the row it names.
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A4");
        };
        manager.insert_process(
            reserved,
            synthetic_row(reserved, "init_oracle_a4", "init_oracle_a4_main"),
        );
        match manager.hold_init_publication(reserved) {
            Ok(ticket) => {
                manager.remove_process(reserved);
                match manager.designate_init(ticket) {
                    Err(_) => refused += 1,
                    Ok(_) if first_failure.is_none() => {
                        first_failure = Some("init designation A4 accepted a missing row")
                    }
                    Ok(_) => {}
                }
            }
            Err(_) if first_failure.is_none() => {
                first_failure = Some("init designation A4 failed to issue a ticket")
            }
            Err(_) => {}
        }
        if manager.designated_init().is_some() && first_failure.is_none() {
            first_failure = Some("init designation A4 changed the init designation");
        }
    }

    // A5: a terminated reserved row cannot be designated.
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A5");
        };
        manager.insert_process(
            reserved,
            synthetic_row(reserved, "init_oracle_a5", "init_oracle_a5_main"),
        );
        match manager.hold_init_publication(reserved) {
            Ok(ticket) => {
                if let Some(row) = manager.get_process_mut(reserved) {
                    row.terminate_minimal(0);
                } else if first_failure.is_none() {
                    first_failure = Some("init designation A5 row disappeared before termination");
                }
                match manager.designate_init(ticket) {
                    Err(_) => refused += 1,
                    Ok(_) if first_failure.is_none() => {
                        first_failure = Some("init designation A5 accepted a terminated row")
                    }
                    Ok(_) => {}
                }
            }
            Err(_) if first_failure.is_none() => {
                first_failure = Some("init designation A5 failed to issue a ticket")
            }
            Err(_) => {}
        }
        if manager.designated_init().is_some() && first_failure.is_none() {
            first_failure = Some("init designation A5 changed the init designation");
        }
        manager.remove_from_ready_queue(reserved);
        manager.remove_process(reserved);
    }

    // A6: a clean reserved row is designated and published exactly once.
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A6");
        };
        manager.insert_process(
            reserved,
            synthetic_row(reserved, "init_oracle_a6", "init_oracle_a6_main"),
        );
        match manager.hold_init_publication(reserved) {
            Ok(ticket) => {
                if manager.remove_from_ready_queue(reserved) && first_failure.is_none() {
                    first_failure = Some("held init row reached the run queue before designation");
                }
                match manager.designate_init(ticket) {
                    Ok(publication) => {
                        if manager.remove_from_ready_queue(reserved) && first_failure.is_none() {
                            first_failure =
                                Some("designated init row reached the run queue before publication");
                        }
                        accepted += 1;
                        if publication.pid() != reserved && first_failure.is_none() {
                            first_failure =
                                Some("init designation A6 publication named the wrong PID");
                        }
                        if manager.designated_init() != Some(reserved) && first_failure.is_none() {
                            first_failure = Some("init designation A6 did not install the authority");
                        }
                        let thread = manager.publish_init(publication);
                        published += 1;
                        if !manager.remove_from_ready_queue(reserved) && first_failure.is_none() {
                            first_failure = Some("init designation A6 publication missed the ready queue");
                        }
                        drop(thread);
                    }
                    Err(_) if first_failure.is_none() => {
                        first_failure = Some("init designation A6 refused a clean reserved row")
                    }
                    Err(_) => {}
                }
            }
            Err(_) if first_failure.is_none() => {
                first_failure = Some("init designation A6 failed to issue a ticket")
            }
            Err(_) => {}
        }
    }

    // A7: designation is single-assignment while the designated row is live.
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A7");
        };
        match manager.hold_init_publication(reserved) {
            Ok(ticket) => match manager.designate_init(ticket) {
                Err(_) => refused += 1,
                Ok(_) if first_failure.is_none() => {
                    first_failure = Some("init designation A7 accepted re-designation")
                }
                Ok(_) => {}
            },
            Err(_) if first_failure.is_none() => {
                first_failure = Some("init designation A7 failed to issue a ticket")
            }
            Err(_) => {}
        }
        if manager.designated_init() != Some(reserved) && first_failure.is_none() {
            first_failure = Some("init designation A7 disturbed the installed authority");
        }
    }

    // A8: live children are reparented onto the designated init row.
    let (parent_pid, child_pid) = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A8");
        };
        let parent_pid = manager.allocate_pid();
        let child_pid = manager.allocate_pid();
        if parent_pid.as_u64() < crate::process::FIRST_ORDINARY_PID && first_failure.is_none() {
            first_failure = Some("init designation A8 parent PID used the reserved range");
        }
        if child_pid.as_u64() < crate::process::FIRST_ORDINARY_PID && first_failure.is_none() {
            first_failure = Some("init designation A8 child PID used the reserved range");
        }
        let mut parent = synthetic_row(
            parent_pid,
            "init_oracle_a8_parent",
            "init_oracle_a8_parent_main",
        );
        parent.children = alloc::vec![child_pid];
        let mut child = synthetic_row(
            child_pid,
            "init_oracle_a8_child",
            "init_oracle_a8_child_main",
        );
        child.parent = Some(parent_pid);
        manager.insert_process(parent_pid, parent);
        manager.insert_process(child_pid, child);
        if !manager.reparent_children_to_init(parent_pid, &[child_pid])
            && first_failure.is_none()
        {
            first_failure = Some("init designation A8 reparent operation reported no change");
        }
        if manager
            .get_process(child_pid)
            .and_then(|row| row.parent)
            != Some(reserved)
            && first_failure.is_none()
        {
            first_failure = Some("init designation A8 did not reparent the child");
        }
        if !manager
            .get_process(reserved)
            .map(|row| row.children.contains(&child_pid))
            .unwrap_or(false)
            && first_failure.is_none()
        {
            first_failure = Some("init designation A8 did not attach the child to init");
        }
        reparented += 1;
        (parent_pid, child_pid)
    };

    // A9: reaping the designated row retires the authority.
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A9");
        };
        manager.remove_process(reserved);
        if manager.designated_init().is_some() && first_failure.is_none() {
            first_failure = Some("init designation A9 did not retire the authority");
        }
        retired += 1;
    }

    // A10: without a designated init, reparenting is a defined no-op.
    let (parent2, child2) = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A10");
        };
        let parent2 = manager.allocate_pid();
        let child2 = manager.allocate_pid();
        if parent2.as_u64() < crate::process::FIRST_ORDINARY_PID && first_failure.is_none() {
            first_failure = Some("init designation A10 parent PID used the reserved range");
        }
        if child2.as_u64() < crate::process::FIRST_ORDINARY_PID && first_failure.is_none() {
            first_failure = Some("init designation A10 child PID used the reserved range");
        }
        let parent = synthetic_row(
            parent2,
            "init_oracle_a10_parent",
            "init_oracle_a10_parent_main",
        );
        let mut child = synthetic_row(
            child2,
            "init_oracle_a10_child",
            "init_oracle_a10_child_main",
        );
        child.parent = Some(parent2);
        manager.insert_process(parent2, parent);
        manager.insert_process(child2, child);
        if manager.reparent_children_to_init(parent2, &[child2]) && first_failure.is_none() {
            first_failure = Some("init designation A10 reparent operation reported a change");
        }
        if manager.get_process(child2).and_then(|row| row.parent) != Some(parent2)
            && first_failure.is_none()
        {
            first_failure = Some("init designation A10 changed the child parent without init");
        }
        reparent_skipped += 1;
        (parent2, child2)
    };

    // A11: a threadless row fails ticket construction and is removed through the row authority.
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation A11");
        };
        let epoch_before = crate::task::process_task::boot_row_removal_epoch();
        manager.insert_process(
            reserved,
            crate::process::Process::new(
                reserved,
                alloc::string::String::from("init_oracle_a11"),
                VirtAddr::new(0x0040_0000),
            ),
        );
        match manager.hold_init_publication(reserved) {
            Ok(ticket) => {
                drop(ticket);
                if first_failure.is_none() {
                    first_failure =
                        Some("init designation A11 issued a ticket for a threadless row");
                }
            }
            Err(_) => held_error_removals += 1,
        }
        if manager.get_process(reserved).is_some() && first_failure.is_none() {
            first_failure = Some("init designation A11 left the threadless row behind");
        }
        if crate::task::process_task::boot_row_removal_epoch() - epoch_before != 1
            && first_failure.is_none()
        {
            first_failure =
                Some("init designation A11 removed the row without the row-removal epoch bump");
        }
        if manager.designated_init().is_some() && first_failure.is_none() {
            first_failure = Some("init designation A11 disturbed the init designation");
        }
    }

    // Remove all synthetic rows and prove the oracle leaves no init identity behind.
    {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init designation cleanup");
        };
        for pid in [parent_pid, child_pid, parent2, child2] {
            manager.remove_process(pid);
        }
        if manager.get_process(reserved).is_some() && first_failure.is_none() {
            first_failure = Some("init designation cleanup left the reserved row live");
        }
        if manager.designated_init().is_some() && first_failure.is_none() {
            first_failure = Some("init designation cleanup left the authority installed");
        }
    }

    let accepted_delta = init_designation_accepted().saturating_sub(accepted_before);
    let refused_delta = init_designation_refused().saturating_sub(refused_before);
    let retired_delta = init_designation_retired().saturating_sub(retired_before);
    let publications_delta = init_publications().saturating_sub(publications_before);
    let reparent_children_delta =
        init_reparent_children().saturating_sub(reparent_children_before);
    let reparent_skipped_delta =
        init_reparent_skipped_no_init().saturating_sub(reparent_skipped_before);
    let ordinary_allocated =
        init_ordinary_pid_allocations().saturating_sub(ordinary_allocations_before);
    let reserved_collisions = init_reserved_pid_collisions_total();
    let designation_used_after = frame_allocator_used_frames();
    let designation_balance = designation_used_after as i64 - designation_used_before as i64;
    core::mem::drop(reclaim_owner);

    if accepted_delta != 1 && first_failure.is_none() {
        first_failure = Some("init designation accepted counter delta was not exact");
    }
    if refused_delta != 4 && first_failure.is_none() {
        first_failure = Some("init designation refused counter delta was not exact");
    }
    if retired_delta != 1 && first_failure.is_none() {
        first_failure = Some("init designation retired counter delta was not exact");
    }
    if publications_delta != 1 && first_failure.is_none() {
        first_failure = Some("init designation publication counter delta was not exact");
    }
    if reparent_children_delta != 1 && first_failure.is_none() {
        first_failure = Some("init designation reparent counter delta was not exact");
    }
    if reparent_skipped_delta != 1 && first_failure.is_none() {
        first_failure = Some("init designation skipped-reparent counter delta was not exact");
    }
    if ordinary_allocated != ordinary_allocated_expected && first_failure.is_none() {
        first_failure = Some("init designation ordinary PID allocation delta was not exact");
    }
    if reserved_collisions != 0 && first_failure.is_none() {
        first_failure = Some("ordinary PID allocation collided with the reserved init PID");
    }
    if construct_undecided != 2 && first_failure.is_none() {
        first_failure = Some("init designation construction undecided-drop delta was not exact");
    }
    if construct_residual < 0 && first_failure.is_none() {
        first_failure = Some("init designation failed construction over-freed frames");
    }
    if designation_balance != 0 && first_failure.is_none() {
        first_failure = Some("init designation synthetic arms changed frame accounting");
    }

    #[cfg(target_arch = "aarch64")]
    let arch = "aarch64";
    #[cfg(target_arch = "x86_64")]
    let arch = "x86";
    // `construct_residual` is the counted (never freed, never double-freed) frame residue of
    // the two failed constructions. It is a pre-existing property of the process-creation
    // failure path, not something P5a introduces; `construct_undecided` proves that residue is
    // counted rather than lost.
    crate::serial_println!(
        "[INIT_DESIGNATION_ORACLE:{}:construct_failed={}:construct_undecided={}:construct_residual={}:refused={}:accepted={}:published={}:retired={}:held_error_removals={}:reparented={}:reparent_skipped={}:ordinary_allocated={}:reserved_collisions={}:designation_balance={}]",
        arch,
        construct_failed,
        construct_undecided,
        construct_residual,
        refused,
        accepted,
        published,
        retired,
        held_error_removals,
        reparented,
        reparent_skipped,
        ordinary_allocated,
        reserved_collisions,
        designation_balance
    );
    if let Some(reason) = first_failure {
        return TestResult::Fail(reason);
    }

    crate::serial_println!("[TEST:process:init_designation_oracle:PASS]");
    TestResult::Pass
}

#[cfg(feature = "boot_tests")]
pub fn init_group_refusal_oracle_test() -> crate::test_framework::registry::TestResult {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    use crate::test_framework::registry::TestResult;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    fn test_user_entry() {}

    fn test_thread(
        pid: crate::process::ProcessId,
        name: &'static str,
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
        thread.state = crate::task::thread::ThreadState::Ready;
        thread
    }

    fn synthetic_row(
        pid: crate::process::ProcessId,
        name: &'static str,
        thread_name: &'static str,
    ) -> crate::process::Process {
        let mut row = crate::process::Process::new(
            pid,
            alloc::string::String::from(name),
            VirtAddr::new(0x0040_0000),
        );
        row.set_main_thread(test_thread(pid, thread_name));
        row
    }

    let reclaim_owner = match crate::task::process_task::BootReclaimTestGuard::enter() {
        Ok(guard) => guard,
        Err(_) => {
            return TestResult::Fail(
                "reclaim queues not quiescent at init group refusal oracle start",
            )
        }
    };
    let allocator_used_before = frame_allocator_used_frames();
    let refusal_counter_before = init_group_refusals_total();
    let reserved = crate::process::ProcessId::new(crate::process::RESERVED_INIT_PID);
    let mut first_failure: Option<&'static str> = None;

    let (
        none_probes,
        none_refusals,
        init_refused,
        alias_refused,
        alias_pid_refused,
        nonit_probes,
        nonit_refusals,
        rows_before,
        rows_after,
        designation_residual,
    ) = {
        let mut manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_mut() else {
            return TestResult::Fail("process manager unavailable for init group refusal oracle");
        };
        if manager.designated_init().is_some() {
            return TestResult::Fail(
                "a designation was already installed at init group refusal oracle start",
            );
        }
        let rows_before = manager.process_count();

        // A0: no designation means every effective thread-group ID is admitted.
        let other = manager.allocate_pid();
        let mut none_probes = 0u64;
        let mut none_refusals = 0u64;
        for derived_tg_id in [crate::process::RESERVED_INIT_PID, other.as_u64(), u64::MAX] {
            let refused = crate::syscall::clone::refuses_init_group_clone(
                manager,
                derived_tg_id,
            );
            none_probes += 1;
            none_refusals += u64::from(refused);
        }

        // Install and publish a synthetic designated init row.
        manager.insert_process(
            reserved,
            synthetic_row(
                reserved,
                "init_group_refusal_init",
                "init_group_refusal_init_main",
            ),
        );
        match manager.hold_init_publication(reserved) {
            Ok(ticket) => match manager.designate_init(ticket) {
                Ok(publication) => {
                    let thread = manager.publish_init(publication);
                    if !manager.remove_from_ready_queue(reserved) && first_failure.is_none() {
                        first_failure =
                            Some("init group refusal publication missed the synthetic ready queue");
                    }
                    drop(thread);
                }
                Err(_) if first_failure.is_none() => {
                    first_failure = Some("init group refusal synthetic designation was refused")
                }
                Err(_) => {}
            },
            Err(_) if first_failure.is_none() => {
                first_failure = Some("init group refusal synthetic ticket was refused")
            }
            Err(_) => {}
        }

        // A1: init's own effective group is refused.
        let init_refused = u64::from(crate::syscall::clone::refuses_init_group_clone(
            manager,
            crate::process::RESERVED_INIT_PID,
        ));

        // A2: ordinary and absent non-init groups remain admitted.
        manager.insert_process(
            other,
            synthetic_row(
                other,
                "init_group_refusal_other",
                "init_group_refusal_other_main",
            ),
        );
        let missing_other = manager.allocate_pid();
        let mut nonit_probes = 0u64;
        let mut nonit_refusals = 0u64;
        for derived_tg_id in [other.as_u64(), missing_other.as_u64()] {
            let refused = crate::syscall::clone::refuses_init_group_clone(manager, derived_tg_id);
            nonit_probes += 1;
            nonit_refusals += u64::from(refused);
        }

        // A3: compare the effective group IDs, never the designated init's PID.
        const INIT_GROUP_ALIAS: u64 = 0x5000_0000_0000_0001;
        if let Some(init_row) = manager.get_process_mut(reserved) {
            init_row.thread_group_id = Some(INIT_GROUP_ALIAS);
        } else if first_failure.is_none() {
            first_failure = Some("init group refusal synthetic init row disappeared");
        }
        let alias_refused = u64::from(crate::syscall::clone::refuses_init_group_clone(
            manager,
            INIT_GROUP_ALIAS,
        ));
        let alias_pid_refused = u64::from(crate::syscall::clone::refuses_init_group_clone(
            manager,
            crate::process::RESERVED_INIT_PID,
        ));
        if let Some(init_row) = manager.get_process_mut(reserved) {
            init_row.thread_group_id = None;
        } else if first_failure.is_none() {
            first_failure = Some("init group refusal synthetic init row vanished before restore");
        }

        manager.remove_from_ready_queue(other);
        manager.remove_process(other);
        manager.remove_from_ready_queue(reserved);
        manager.remove_process(reserved);

        let rows_after = manager.process_count();
        let designation_residual = usize::from(manager.designated_init().is_some());
        (
            none_probes,
            none_refusals,
            init_refused,
            alias_refused,
            alias_pid_refused,
            nonit_probes,
            nonit_refusals,
            rows_before,
            rows_after,
            designation_residual,
        )
    };

    let refusal_counter_after = init_group_refusals_total();
    let refusal_counter_delta = refusal_counter_after as i64 - refusal_counter_before as i64;
    let rows_delta = rows_after as i64 - rows_before as i64;
    let allocator_used_after = frame_allocator_used_frames();
    let balance = allocator_used_after as i64 - allocator_used_before as i64;
    core::mem::drop(reclaim_owner);

    if none_probes != 3 && first_failure.is_none() {
        first_failure = Some("init group refusal None-arm probe count was not exact");
    }
    if none_refusals != 0 && first_failure.is_none() {
        first_failure = Some("init group refusal None arm refused a group");
    }
    if init_refused != 1 && first_failure.is_none() {
        first_failure = Some("init group refusal did not refuse the designated init group");
    }
    if alias_refused != 1 && first_failure.is_none() {
        first_failure = Some("init group refusal did not refuse init's effective alias group");
    }
    if alias_pid_refused != 0 && first_failure.is_none() {
        first_failure = Some("init group refusal compared against init's PID instead of its group");
    }
    if nonit_probes != 2 && first_failure.is_none() {
        first_failure = Some("init group refusal non-init probe count was not exact");
    }
    if nonit_refusals != 0 && first_failure.is_none() {
        first_failure = Some("init group refusal rejected a non-init group");
    }
    if rows_delta != 0 && first_failure.is_none() {
        first_failure = Some("init group refusal oracle left synthetic process rows");
    }
    if refusal_counter_delta != 0 && first_failure.is_none() {
        first_failure = Some("init group refusal predicate moved the production counter");
    }
    if designation_residual != 0 && first_failure.is_none() {
        first_failure = Some("init group refusal oracle left the designation installed");
    }
    if balance != 0 && first_failure.is_none() {
        first_failure = Some("init group refusal oracle changed frame accounting");
    }

    #[cfg(target_arch = "aarch64")]
    let arch = "aarch64";
    #[cfg(target_arch = "x86_64")]
    let arch = "x86";
    crate::serial_println!(
        "[INIT_GROUP_REFUSAL_ORACLE:{}:none_probes={}:none_refusals={}:init_refused={}:alias_refused={}:alias_pid_refused={}:nonit_probes={}:nonit_refusals={}:rows_delta={}:refusal_counter_delta={}:designation_residual={}:balance={}]",
        arch,
        none_probes,
        none_refusals,
        init_refused,
        alias_refused,
        alias_pid_refused,
        nonit_probes,
        nonit_refusals,
        rows_delta,
        refusal_counter_delta,
        designation_residual,
        balance
    );
    if let Some(reason) = first_failure {
        return TestResult::Fail(reason);
    }

    crate::serial_println!("[TEST:process:init_group_refusal_oracle:PASS]");
    TestResult::Pass
}

#[cfg(feature = "boot_tests")]
pub fn kernel_stack_ownership_oracle_test() -> crate::test_framework::registry::TestResult {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    use crate::test_framework::registry::TestResult;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    const OWNERSHIP_STRESS_ITERATIONS: usize = 1000;
    const OWNERSHIP_STRESS_WARMUPS: usize = 8;

    #[derive(Default)]
    struct Measurements {
        creation_rows: u64,
        creation_owned: u64,
        one_owner: u64,
        two_owner: u64,
        zero_owner: u64,
        fork_rows: u64,
        fork_owned: u64,
        slot_returns_exact_one: u64,
        slot_alloc_delta: u64,
        slot_free_delta: u64,
        slot_balance: i64,
        cohort_enrolled: u64,
        cohort_returned: u64,
        cohort_double_return: u64,
        foreign_returned: u64,
        foreign_alloc_delta: u64,
        frames_mapped_delta: u64,
        frames_released_delta: u64,
        frame_balance: i64,
        frame_used_delta: i64,
        frame_used_bounded: u64,
        live_checks: u64,
        live_refusals_production: u64,
        live_refusals_injected: u64,
        drop_refused_live: u64,
        pte_overwrite_refusals: u64,
        pub_pooled: u64,
        pub_sched_owned: u64,
        pub_row_residual: u64,
        pub_unowned: u64,
        classifier_sched_owned: u64,
        classifier_row_residual: u64,
        classifier_unowned: u64,
        classifier_not_pooled: u64,
        sched_publications: u64,
        sched_pm_held_production: u64,
        sched_pm_held_injected: u64,
        reconciliation_diff: i64,
        reconciliation_skew_bound: i64,
    }

    fn note_failure(
        first_failure: &mut Option<&'static str>,
        violations: &mut u64,
        reason: &'static str,
    ) {
        *violations = violations.saturating_add(1);
        if first_failure.is_none() {
            *first_failure = Some(reason);
        }
    }

    fn create_ownership_stress_row(
        iteration: usize,
    ) -> Result<crate::task::thread::Thread, &'static str> {
        let pid = crate::process::ProcessId::new(0x4b53_0000 + iteration as u64);
        let mut process = crate::process::Process::new(
            pid,
            alloc::string::String::from("kernel_stack_ownership_stress"),
            VirtAddr::new(0x0040_0000),
        );
        let stack_top = VirtAddr::new(0x0080_0000);
        let mut manager_guard = crate::process::manager();
        let manager = manager_guard
            .as_mut()
            .ok_or("process manager unavailable during ownership constructor stress")?;
        #[cfg(target_arch = "x86_64")]
        {
            manager.create_main_thread_for_ownership_stress(&mut process, stack_top)
        }
        #[cfg(target_arch = "aarch64")]
        {
            let tls = VirtAddr::new(0);
            if iteration % 2 == 0 {
                manager.create_main_thread_for_ownership_stress(&mut process, stack_top, tls)
            } else {
                manager.create_main_thread_with_sp_for_ownership_stress(
                    &mut process,
                    stack_top,
                    VirtAddr::new(stack_top.as_u64() - 16),
                    tls,
                )
            }
        }
    }

    fn retire_and_remove_owned_row(pid: crate::process::ProcessId) -> Result<(), &'static str> {
        let reclaim = {
            let mut manager_guard = crate::process::manager();
            let manager = manager_guard
                .as_mut()
                .ok_or("process manager unavailable during ownership row retirement")?;
            let reclaim = {
                let process = manager
                    .get_process_mut(pid)
                    .ok_or("ownership row disappeared before retirement")?;
                let reclaim = crate::task::process_task::defer_process_resources(process);
                crate::task::process_task::release_process_resources(process);
                reclaim
            };
            manager.remove_from_ready_queue(pid);
            manager.remove_process(pid);
            reclaim
        };
        crate::task::process_task::enqueue_process_reclaim(reclaim);
        let cleanup_deadline = retirement_oracle_quiesce_deadline();
        while crate::task::process_task::boot_reclaim_locations(pid.as_u64()) != (false, false) {
            crate::task::scheduler::nudge_retirement_grace_for_test();
            let boundary_deadline =
                retirement_oracle_clock_now().saturating_add(retirement_oracle_clock_delta(1));
            while retirement_oracle_clock_now() < boundary_deadline {
                core::hint::spin_loop();
            }
            crate::task::process_task::boot_reclaim_deferred_process_resources();
            if retirement_oracle_clock_now() >= cleanup_deadline {
                return Err("ownership deferred cleanup did not quiesce");
            }
        }
        Ok(())
    }

    let mut measurements = Measurements::default();
    let mut first_failure = None;
    let mut violations = 0u64;

    // Arm A: drive every production classifier verdict against a genuine pool VA.
    match crate::memory::kernel_stack::allocate_kernel_stack() {
        Ok(stack) => {
            let pooled_top = stack.top().as_u64();
            if crate::memory::kernel_stack::classify_kernel_stack_ownership(
                Some(pooled_top),
                true,
                false,
            ) == crate::memory::kernel_stack::KernelStackOwnership::SchedulerOwned
            {
                measurements.classifier_sched_owned += 1;
            }
            if crate::memory::kernel_stack::classify_kernel_stack_ownership(
                Some(pooled_top),
                true,
                true,
            ) == crate::memory::kernel_stack::KernelStackOwnership::RowResidual
            {
                measurements.classifier_row_residual += 1;
            }
            if crate::memory::kernel_stack::classify_kernel_stack_ownership(
                Some(pooled_top),
                false,
                false,
            ) == crate::memory::kernel_stack::KernelStackOwnership::Unowned
            {
                measurements.classifier_unowned += 1;
            }
            if crate::memory::kernel_stack::classify_kernel_stack_ownership(None, false, false)
                == crate::memory::kernel_stack::KernelStackOwnership::NotPooled
            {
                measurements.classifier_not_pooled += 1;
            }
            core::mem::drop(stack);
        }
        Err(_) => note_failure(
            &mut first_failure,
            &mut violations,
            "classifier could not allocate a genuine pooled kernel stack",
        ),
    }

    // Arm B: inject one known-live candidate through the allocator's real guard.
    let guard_before = crate::memory::kernel_stack::kernel_stack_pool_counters();
    let injection_saw_live = match crate::memory::kernel_stack::current_live_stack_top_for_test() {
        Some(stack_top) => crate::memory::kernel_stack::probe_live_slot_guard_injection(stack_top),
        None => {
            note_failure(
                &mut first_failure,
                &mut violations,
                "live-slot guard had no current live stack top",
            );
            false
        }
    };
    if !injection_saw_live {
        note_failure(
            &mut first_failure,
            &mut violations,
            "live-slot guard accepted the injected live stack",
        );
    }

    // Arm C warm-up: use the exact production constructor/publication lifecycle,
    // but keep allocator growth and one-time TLS-vector growth out of the sample.
    let mut warmups_completed = 0u64;
    for iteration in 0..OWNERSHIP_STRESS_WARMUPS {
        match create_ownership_stress_row(iteration) {
            Ok(mut row) => {
                let row_owned = row.kernel_stack_allocation.is_some()
                    && row.kernel_stack_top.is_some_and(|top| {
                        crate::memory::kernel_stack::is_kernel_stack_va(top.as_u64())
                    });
                let published = row.publish_to_scheduler();
                let moved_once = row.kernel_stack_allocation.is_none()
                    && published.kernel_stack_allocation.is_some();
                if !row_owned {
                    note_failure(
                        &mut first_failure,
                        &mut violations,
                        "ownership warm-up constructor did not return a pooled owner",
                    );
                }
                if !moved_once {
                    note_failure(
                        &mut first_failure,
                        &mut violations,
                        "ownership warm-up publication did not move exactly one owner",
                    );
                }
                core::mem::drop(published);
                core::mem::drop(row);
                warmups_completed += 1;
            }
            Err(_) => note_failure(
                &mut first_failure,
                &mut violations,
                "ownership warm-up production constructor failed",
            ),
        }
    }

    let stress_before = crate::memory::kernel_stack::kernel_stack_pool_counters();
    // #646: the global slot counters move for every kernel stack in the system,
    // so they cannot carry the oracle's own allocation/return equality. Enrol the
    // slots this loop allocates and match the returns by slot identity instead.
    let cohort_before = crate::memory::kernel_stack::kernel_stack_cohort_counters();
    let frame_used_before = frame_allocator_used_frames();
    #[cfg(target_arch = "aarch64")]
    let mut aarch_plain_constructors = 0u64;
    #[cfg(target_arch = "aarch64")]
    let mut aarch_sp_constructors = 0u64;

    for iteration in 0..OWNERSHIP_STRESS_ITERATIONS {
        let row_result = create_ownership_stress_row(iteration);
        let mut row = match row_result {
            Ok(row) => row,
            Err(_) => {
                note_failure(
                    &mut first_failure,
                    &mut violations,
                    "ownership stress production constructor failed",
                );
                continue;
            }
        };
        measurements.creation_rows += 1;
        #[cfg(target_arch = "aarch64")]
        if iteration % 2 == 0 {
            aarch_plain_constructors += 1;
        } else {
            aarch_sp_constructors += 1;
        }

        if let Some(allocation) = row.kernel_stack_allocation.as_ref() {
            // Enrolment travels with the slot index, so it survives the
            // publication move into the scheduler-owned copy.
            allocation.enroll_in_measurement_cohort();
        }

        if row.kernel_stack_allocation.is_some()
            && row
                .kernel_stack_top
                .is_some_and(|top| crate::memory::kernel_stack::is_kernel_stack_va(top.as_u64()))
        {
            measurements.creation_owned += 1;
        } else {
            note_failure(
                &mut first_failure,
                &mut violations,
                "ownership stress constructor returned a row without a pooled owner",
            );
        }

        let published = row.publish_to_scheduler();
        match (
            row.kernel_stack_allocation.is_some(),
            published.kernel_stack_allocation.is_some(),
        ) {
            (false, true) => measurements.one_owner += 1,
            (true, true) => measurements.two_owner += 1,
            (false, false) => measurements.zero_owner += 1,
            (true, false) => note_failure(
                &mut first_failure,
                &mut violations,
                "ownership stress left the only owner in the process row",
            ),
        }
        core::mem::drop(published);
        core::mem::drop(row);
    }

    let stress_after = crate::memory::kernel_stack::kernel_stack_pool_counters();
    let cohort_after = crate::memory::kernel_stack::kernel_stack_cohort_counters();
    let frame_used_after = frame_allocator_used_frames();
    measurements.slot_alloc_delta = stress_after
        .slots_allocated
        .saturating_sub(stress_before.slots_allocated);
    measurements.slot_free_delta = stress_after
        .slots_freed
        .saturating_sub(stress_before.slots_freed);
    measurements.slot_balance =
        measurements.slot_alloc_delta as i64 - measurements.slot_free_delta as i64;
    measurements.cohort_enrolled = cohort_after.enrolled.saturating_sub(cohort_before.enrolled);
    measurements.cohort_returned = cohort_after.returned.saturating_sub(cohort_before.returned);
    measurements.cohort_double_return = cohort_after
        .double_returned
        .saturating_sub(cohort_before.double_returned);
    measurements.foreign_returned = cohort_after
        .foreign_returned
        .saturating_sub(cohort_before.foreign_returned);
    // Allocation-side attribution is only knowable once a slot is enrolled, so
    // the foreign allocation count is the residual of the exact global delta.
    measurements.foreign_alloc_delta = measurements
        .slot_alloc_delta
        .saturating_sub(measurements.cohort_enrolled);
    let online_cpus = crate::task::scheduler::online_cpu_count_snapshot() as i64;
    measurements.reconciliation_skew_bound = 2 * (online_cpus - 1).max(0);
    measurements.reconciliation_diff = measurements.slot_free_delta as i64
        - (measurements.cohort_returned + measurements.foreign_returned) as i64;
    measurements.frames_mapped_delta = stress_after
        .frames_mapped
        .saturating_sub(stress_before.frames_mapped);
    measurements.frames_released_delta = stress_after
        .frames_released
        .saturating_sub(stress_before.frames_released);
    measurements.frame_balance =
        measurements.frames_mapped_delta as i64 - measurements.frames_released_delta as i64;
    measurements.frame_used_delta = frame_used_after as i64 - frame_used_before as i64;
    measurements.frame_used_bounded = u64::from(measurements.frame_used_delta < 128);
    measurements.drop_refused_live = stress_after
        .drop_refused_live
        .saturating_sub(stress_before.drop_refused_live);
    measurements.pte_overwrite_refusals = stress_after
        .pte_overwrite_refusals
        .saturating_sub(stress_before.pte_overwrite_refusals);

    // Arm D: exercise every architecture's production fork constructor, then
    // prove one PID-stamped slot return after publishing and destroying the child.
    let reclaim_owner = crate::task::process_task::BootReclaimTestGuard::enter();
    match reclaim_owner {
        Ok(reclaim_owner) => {
            let pid_counts_guard = reset_boot_test_pid_counts();
            let parent_page_table = crate::memory::process_memory::ProcessPageTable::new();
            match parent_page_table {
                Ok(parent_page_table) => {
                    let parent_pid = {
                        let manager_guard = crate::process::manager();
                        manager_guard.as_ref().map(|manager| manager.allocate_pid())
                    };
                    match parent_pid {
                        Some(parent_pid) => {
                            fn fork_parent_entry() {}
                            let mut parent_process = crate::process::Process::new(
                                parent_pid,
                                alloc::string::String::from("kernel_stack_ownership_parent"),
                                VirtAddr::new(0x0040_0000),
                            );
                            let mut parent_thread = crate::task::thread::Thread::new(
                                alloc::string::String::from("kernel_stack_ownership_parent_main"),
                                fork_parent_entry,
                                VirtAddr::new(0x0080_0000),
                                VirtAddr::new(0x007f_0000),
                                VirtAddr::new(0x0001_0000),
                                crate::task::thread::ThreadPrivilege::User,
                            );
                            parent_thread.owner_pid = Some(parent_pid.as_u64());
                            #[cfg(target_arch = "aarch64")]
                            let parent_context = parent_thread.context.clone();
                            parent_process.page_table =
                                Some(alloc::boxed::Box::new(parent_page_table));
                            parent_process.set_main_thread(parent_thread);
                            {
                                let mut manager_guard = crate::process::manager();
                                if let Some(manager) = manager_guard.as_mut() {
                                    manager.insert_process(parent_pid, parent_process);
                                } else {
                                    note_failure(
                                        &mut first_failure,
                                        &mut violations,
                                        "process manager unavailable for ownership parent insert",
                                    );
                                }
                            }

                            #[cfg(target_arch = "x86_64")]
                            let expected_fork_rows = 2usize;
                            #[cfg(target_arch = "aarch64")]
                            let expected_fork_rows = 1usize;
                            for fork_index in 0..expected_fork_rows {
                                #[cfg(target_arch = "x86_64")]
                                let child_pid_result = if fork_index == 0 {
                                    match crate::memory::process_memory::ProcessPageTable::new() {
                                        Ok(child_page_table) => {
                                            let mut manager_guard = crate::process::manager();
                                            manager_guard
                                                .as_mut()
                                                .ok_or("process manager unavailable for complete_fork ownership arm")
                                                .and_then(|manager| {
                                                    manager.fork_process_with_page_table(
                                                        parent_pid,
                                                        None,
                                                        None,
                                                        alloc::boxed::Box::new(child_page_table),
                                                    )
                                                })
                                        }
                                        Err(_) => Err(
                                            "complete_fork ownership page-table allocation failed",
                                        ),
                                    }
                                } else {
                                    let mut manager_guard = crate::process::manager();
                                    manager_guard
                                        .as_mut()
                                        .ok_or("process manager unavailable for fork_process_with_context ownership arm")
                                        .and_then(|manager| {
                                            manager.fork_process_with_context(parent_pid, None)
                                        })
                                };
                                #[cfg(target_arch = "aarch64")]
                                let child_pid_result = if fork_index != 0 {
                                    Err("aarch64 ownership fork count exceeded one")
                                } else {
                                    match crate::memory::process_memory::ProcessPageTable::new() {
                                        Ok(child_page_table) => {
                                            let mut manager_guard = crate::process::manager();
                                            manager_guard
                                                .as_mut()
                                                .ok_or("process manager unavailable for aarch64 ownership fork")
                                                .and_then(|manager| {
                                                    manager.fork_process_aarch64(
                                                        parent_pid,
                                                        parent_context.clone(),
                                                        alloc::boxed::Box::new(child_page_table),
                                                    )
                                                })
                                        }
                                        Err(_) => Err(
                                            "aarch64 ownership fork page-table allocation failed",
                                        ),
                                    }
                                };

                                let child_pid = match child_pid_result {
                                    Ok(child_pid) => child_pid,
                                    Err(_) => {
                                        note_failure(
                                            &mut first_failure,
                                            &mut violations,
                                            "ownership fork constructor failed",
                                        );
                                        continue;
                                    }
                                };
                                measurements.fork_rows += 1;
                                let published = {
                                    let mut manager_guard = crate::process::manager();
                                    match manager_guard
                                        .as_mut()
                                        .and_then(|manager| manager.get_process_mut(child_pid))
                                        .and_then(|process| process.main_thread.as_mut())
                                    {
                                        Some(row) => {
                                            if row.kernel_stack_allocation.is_some()
                                                && row.kernel_stack_top.is_some_and(|top| {
                                                    crate::memory::kernel_stack::is_kernel_stack_va(
                                                        top.as_u64(),
                                                    )
                                                })
                                            {
                                                measurements.fork_owned += 1;
                                            } else {
                                                note_failure(
                                                    &mut first_failure,
                                                    &mut violations,
                                                    "ownership fork row did not hold a pooled allocation",
                                                );
                                            }
                                            let published = row.publish_to_scheduler();
                                            if row.kernel_stack_allocation.is_some()
                                                || published.kernel_stack_allocation.is_none()
                                            {
                                                note_failure(
                                                    &mut first_failure,
                                                    &mut violations,
                                                    "ownership fork publication did not move exactly one owner",
                                                );
                                            }
                                            Some(published)
                                        }
                                        None => {
                                            note_failure(
                                                &mut first_failure,
                                                &mut violations,
                                                "ownership fork child row had no main thread",
                                            );
                                            None
                                        }
                                    }
                                };

                                match teardown_pid_evidence(child_pid.as_u64()) {
                                    Some(evidence) if evidence.kernel_stack_slot_returns == 0 => {}
                                    Some(_) => note_failure(
                                        &mut first_failure,
                                        &mut violations,
                                        "ownership fork PID had a slot return before process death",
                                    ),
                                    None => note_failure(
                                        &mut first_failure,
                                        &mut violations,
                                        "ownership fork PID evidence slot was unavailable",
                                    ),
                                }
                                core::mem::drop(published);
                                {
                                    let mut manager_guard = crate::process::manager();
                                    if let Some(parent) = manager_guard
                                        .as_mut()
                                        .and_then(|manager| manager.get_process_mut(parent_pid))
                                    {
                                        parent.children.retain(|pid| *pid != child_pid);
                                    }
                                }
                                if retire_and_remove_owned_row(child_pid).is_err() {
                                    note_failure(
                                        &mut first_failure,
                                        &mut violations,
                                        "ownership fork child cleanup failed",
                                    );
                                }
                                match teardown_pid_evidence(child_pid.as_u64()) {
                                    Some(evidence) if evidence.kernel_stack_slot_returns == 1 => {
                                        measurements.slot_returns_exact_one += 1;
                                    }
                                    Some(_) => note_failure(
                                        &mut first_failure,
                                        &mut violations,
                                        "ownership fork PID slot-return count was not exactly one",
                                    ),
                                    None => note_failure(
                                        &mut first_failure,
                                        &mut violations,
                                        "ownership fork PID evidence disappeared after cleanup",
                                    ),
                                }
                            }

                            if retire_and_remove_owned_row(parent_pid).is_err() {
                                note_failure(
                                    &mut first_failure,
                                    &mut violations,
                                    "ownership parent cleanup failed",
                                );
                            }
                        }
                        None => note_failure(
                            &mut first_failure,
                            &mut violations,
                            "process manager unavailable for ownership parent PID",
                        ),
                    }
                }
                Err(_) => note_failure(
                    &mut first_failure,
                    &mut violations,
                    "ownership parent page-table allocation failed",
                ),
            }
            core::mem::drop(pid_counts_guard);
            core::mem::drop(reclaim_owner);
        }
        Err(_) => note_failure(
            &mut first_failure,
            &mut violations,
            "reclaim queues not quiescent at ownership oracle start",
        ),
    }

    // Arm E: prove the creation publication lock-order detector can observe a
    // process-manager guard held by this CPU.
    let creation_counters_before = crate::task::scheduler::creation_lock_order_counters();
    let guard = crate::process::manager();
    let injection_saw_pm_held =
        crate::task::scheduler::probe_publication_lock_order_injection();
    drop(guard);
    let creation_counters = crate::task::scheduler::creation_lock_order_counters();
    let injected_delta = creation_counters
        .pm_held_injected
        .saturating_sub(creation_counters_before.pm_held_injected);
    if !injection_saw_pm_held {
        note_failure(
            &mut first_failure,
            &mut violations,
            "publication lock-order detector could not see this CPU's process-manager guard",
        );
    }
    if injected_delta != 1 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "injected publication PM-held counter did not rise by exactly one",
        );
    }

    // Read boot-wide production counters only after all oracle workloads have
    // completed.
    let final_pool = crate::memory::kernel_stack::kernel_stack_pool_counters();
    measurements.live_checks = final_pool.live_slot_checks;
    measurements.live_refusals_injected = final_pool
        .live_slot_refusals_injected
        .saturating_sub(guard_before.live_slot_refusals_injected);
    measurements.live_refusals_production = final_pool
        .live_slot_refusals
        .saturating_sub(final_pool.live_slot_refusals_injected);
    measurements.pub_pooled = final_pool.publications_pooled;
    measurements.pub_sched_owned = final_pool.publications_scheduler_owned;
    measurements.pub_row_residual = final_pool.publications_row_residual;
    measurements.pub_unowned = final_pool.publications_unowned;
    measurements.sched_publications = creation_counters.publications;
    measurements.sched_pm_held_production =
        creation_counters.pm_held - creation_counters.pm_held_injected;
    measurements.sched_pm_held_injected = creation_counters.pm_held_injected;

    if measurements.classifier_sched_owned != 1 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "classifier SchedulerOwned arm was not exact",
        );
    }
    if measurements.classifier_row_residual != 1 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "classifier RowResidual arm was not exact",
        );
    }
    if measurements.classifier_unowned != 1 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "classifier Unowned arm was not exact",
        );
    }
    if measurements.classifier_not_pooled != 1 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "classifier NotPooled arm was not exact",
        );
    }
    if warmups_completed != OWNERSHIP_STRESS_WARMUPS as u64 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership warm-up count was not exact",
        );
    }
    if measurements.creation_rows != OWNERSHIP_STRESS_ITERATIONS as u64 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress creation row count was not exact",
        );
    }
    if measurements.creation_owned != OWNERSHIP_STRESS_ITERATIONS as u64 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress owned row count was not exact",
        );
    }
    if measurements.one_owner != OWNERSHIP_STRESS_ITERATIONS as u64 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress one-owner count was not exact",
        );
    }
    if measurements.two_owner != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress observed two owners",
        );
    }
    if measurements.zero_owner != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress observed zero owners",
        );
    }
    #[cfg(target_arch = "aarch64")]
    if aarch_plain_constructors != 500 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "aarch64 plain constructor count was not exact",
        );
    }
    #[cfg(target_arch = "aarch64")]
    if aarch_sp_constructors != 500 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "aarch64 explicit-SP constructor count was not exact",
        );
    }
    #[cfg(target_arch = "x86_64")]
    let expected_fork_rows = 2u64;
    #[cfg(target_arch = "aarch64")]
    let expected_fork_rows = 1u64;
    if measurements.fork_rows != expected_fork_rows {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership fork row count was not exact",
        );
    }
    if measurements.fork_owned != expected_fork_rows {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership fork owned count was not exact",
        );
    }
    if measurements.slot_returns_exact_one != expected_fork_rows {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership fork exact slot-return population was not exact",
        );
    }
    if measurements.slot_alloc_delta < OWNERSHIP_STRESS_ITERATIONS as u64 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress slot allocation workload was too small",
        );
    }
    if measurements.slot_free_delta < OWNERSHIP_STRESS_ITERATIONS as u64 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress slot return workload was too small",
        );
    }
    // #646: the oracle's own allocation/return balance is asserted by slot
    // identity, not by a global before/after delta. Every stress row enrols the
    // slot it allocated, and `KernelStack::drop` matches the return against that
    // enrolment, so a thread reaped on another CPU inside this window can no
    // longer move the quantities these checks read.
    if measurements.cohort_enrolled != OWNERSHIP_STRESS_ITERATIONS as u64 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress did not enrol one cohort slot per iteration",
        );
    }
    if measurements.cohort_returned != measurements.cohort_enrolled {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress cohort slot allocation/return equality failed",
        );
    }
    // The reading the old global delta could never separate from ordinary
    // concurrency: a slot returned twice with no intervening allocation.
    if measurements.cohort_double_return != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress returned a cohort slot twice",
        );
    }
    // Global/cohort reconciliation. The pool and cohort counters are read as
    // separate snapshots at both boundaries, while `KernelStack::drop` writes
    // the cohort classification before bumping the global freed counter. A
    // foreign drop that lands in either boundary's read window can desynchronize
    // the two accountings. The check therefore tolerates the derived
    // `2 * (online_cpu_count - 1)` skew, on the assumption that each other
    // online CPU completes at most one `KernelStack::drop` inside a two-load
    // read window; nothing enforces that assumption, so the bound is a
    // best-effort derivation, not a proof. Any deviation beyond the bound still
    // means the accountings drifted, but a drift within the bound is invisible
    // to this check and to the gate regex (which accepts any
    // `reconciliation_diff` value) alike — a disclosed blind spot, not an exact
    // identity.
    if measurements.reconciliation_diff.abs() > measurements.reconciliation_skew_bound {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress slot return accounting did not reconcile",
        );
    }
    if measurements.slot_alloc_delta < measurements.cohort_enrolled {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress enrolled more slots than the allocator handed out",
        );
    }
    // Frame accounting is per allocation and per return, so pinning it to the
    // global slot deltas is exact on both arches whatever else is running;
    // the old mapped-equals-released pair carried the same window race as the
    // slot equality did.
    #[cfg(target_arch = "x86_64")]
    if measurements.frames_mapped_delta != 128 * measurements.slot_alloc_delta {
        note_failure(
            &mut first_failure,
            &mut violations,
            "x86 ownership stress did not map 128 frames per stack",
        );
    }
    #[cfg(target_arch = "x86_64")]
    if measurements.frames_released_delta != 128 * measurements.slot_free_delta {
        note_failure(
            &mut first_failure,
            &mut violations,
            "x86 ownership stress did not release 128 frames per returned stack",
        );
    }
    #[cfg(target_arch = "aarch64")]
    if measurements.frames_released_delta != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "aarch64 ownership stress unexpectedly released frames",
        );
    }
    #[cfg(target_arch = "aarch64")]
    if measurements.frames_mapped_delta != 0 {
        // AArch64 kernel stacks are HHDM-preallocated and therefore legitimately
        // map no frames during allocation.
        note_failure(
            &mut first_failure,
            &mut violations,
            "aarch64 ownership stress unexpectedly mapped frames",
        );
    }
    #[cfg(target_arch = "aarch64")]
    if measurements.frames_released_delta != 0 {
        // The paired release count is legitimately zero for the same HHDM pool.
        note_failure(
            &mut first_failure,
            &mut violations,
            "aarch64 ownership stress unexpectedly released frames",
        );
    }
    if measurements.frame_used_delta >= 128 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress orphaned at least one stack of frames",
        );
    }
    if measurements.frame_used_bounded != 1 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress frame-used bound flag was not asserted",
        );
    }
    if measurements.drop_refused_live != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress drop liveness refusal counter moved",
        );
    }
    if measurements.pte_overwrite_refusals != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "ownership stress PTE overwrite refusal counter moved",
        );
    }
    if measurements.live_checks == 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "kernel-stack live-slot checks had no allocation workload",
        );
    }
    if final_pool.live_slot_refusals < final_pool.live_slot_refusals_injected {
        note_failure(
            &mut first_failure,
            &mut violations,
            "injected live-slot refusals exceeded total refusals",
        );
    }
    if measurements.live_refusals_production != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "production allocator selected a live kernel-stack slot",
        );
    }
    if measurements.live_refusals_injected != 1 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "injected live-slot refusal delta was not exactly one",
        );
    }
    if final_pool.publications == 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "kernel-stack publication workload was absent",
        );
    }
    if measurements.pub_pooled == 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "pooled kernel-stack publication workload was absent",
        );
    }
    if measurements.pub_sched_owned != measurements.pub_pooled {
        note_failure(
            &mut first_failure,
            &mut violations,
            "pooled publications were not all scheduler-owned",
        );
    }
    if measurements.pub_row_residual != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "a publication retained row ownership",
        );
    }
    if measurements.pub_unowned != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "a pooled publication had no owner",
        );
    }
    if measurements.sched_publications == 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "scheduler publication workload was absent",
        );
    }
    if measurements.sched_pm_held_production != 0 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "production scheduler publication occurred while PM was held",
        );
    }
    if measurements.sched_pm_held_injected != 1 {
        note_failure(
            &mut first_failure,
            &mut violations,
            "injected scheduler publication PM-held count was not exactly one",
        );
    }

    #[cfg(target_arch = "aarch64")]
    let arch = "aarch64";
    #[cfg(target_arch = "x86_64")]
    let arch = "x86";
    let balance = violations;
    crate::serial_println!(
        "[KSTACK_OWNER_ORACLE:{}:creation_rows={}:creation_owned={}:one_owner={}:two_owner={}:zero_owner={}:fork_rows={}:fork_owned={}:slot_returns_exact_one={}:slot_alloc_delta={}:slot_free_delta={}:slot_balance={}:cohort_enrolled={}:cohort_returned={}:cohort_double_return={}:foreign_alloc_delta={}:foreign_returned={}:frames_mapped_delta={}:frames_released_delta={}:frame_balance={}:frame_used_delta={}:frame_used_bounded={}:live_checks={}:live_refusals_production={}:live_refusals_injected={}:drop_refused_live={}:pte_overwrite_refusals={}:pub_pooled={}:pub_sched_owned={}:pub_row_residual={}:pub_unowned={}:classifier_sched_owned={}:classifier_row_residual={}:classifier_unowned={}:classifier_not_pooled={}:sched_publications={}:sched_pm_held_production={}:sched_pm_held_injected={}:reconciliation_diff={}:reconciliation_skew_bound={}:balance={}]",
        arch,
        measurements.creation_rows,
        measurements.creation_owned,
        measurements.one_owner,
        measurements.two_owner,
        measurements.zero_owner,
        measurements.fork_rows,
        measurements.fork_owned,
        measurements.slot_returns_exact_one,
        measurements.slot_alloc_delta,
        measurements.slot_free_delta,
        measurements.slot_balance,
        measurements.cohort_enrolled,
        measurements.cohort_returned,
        measurements.cohort_double_return,
        measurements.foreign_alloc_delta,
        measurements.foreign_returned,
        measurements.frames_mapped_delta,
        measurements.frames_released_delta,
        measurements.frame_balance,
        measurements.frame_used_delta,
        measurements.frame_used_bounded,
        measurements.live_checks,
        measurements.live_refusals_production,
        measurements.live_refusals_injected,
        measurements.drop_refused_live,
        measurements.pte_overwrite_refusals,
        measurements.pub_pooled,
        measurements.pub_sched_owned,
        measurements.pub_row_residual,
        measurements.pub_unowned,
        measurements.classifier_sched_owned,
        measurements.classifier_row_residual,
        measurements.classifier_unowned,
        measurements.classifier_not_pooled,
        measurements.sched_publications,
        measurements.sched_pm_held_production,
        measurements.sched_pm_held_injected,
        measurements.reconciliation_diff,
        measurements.reconciliation_skew_bound,
        balance
    );

    if balance != 0 {
        return TestResult::Fail(
            first_failure.unwrap_or("kernel-stack ownership balance was nonzero"),
        );
    }
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

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_init_designation_gate() {
    crate::serial_println!("[TEST:process:init_designation_oracle:START]");
    let result = init_designation_oracle_test();
    if !result.is_pass() {
        crate::serial_println!(
            "[TEST:process:init_designation_oracle:FAIL:{:?}]",
            result
        );
    }
    assert!(result.is_pass(), "x86 init designation oracle gate failed");
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_init_group_refusal_gate() {
    crate::serial_println!("[TEST:process:init_group_refusal_oracle:START]");
    let result = init_group_refusal_oracle_test();
    if !result.is_pass() {
        crate::serial_println!("[TEST:process:init_group_refusal_oracle:FAIL:{:?}]", result);
    }
    assert!(
        result.is_pass(),
        "x86 init group refusal oracle gate failed"
    );
}

#[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
pub fn run_x86_kernel_stack_ownership_gate() {
    let _ = crate::memory::kernel_stack::kernel_stack_quiesce_baseline_outstanding();
    crate::memory::kernel_stack::kernel_stack_quiesce_start_watch();
    crate::serial_println!("[TEST:process:kernel_stack_ownership_oracle:START]");
    let result = kernel_stack_ownership_oracle_test();
    crate::memory::kernel_stack::kernel_stack_quiesce_stop_watch();
    if !result.is_pass() {
        crate::serial_println!(
            "[TEST:process:kernel_stack_ownership_oracle:FAIL:{:?}]",
            result
        );
    }
    assert!(
        result.is_pass(),
        "x86 kernel-stack ownership oracle gate failed"
    );
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
