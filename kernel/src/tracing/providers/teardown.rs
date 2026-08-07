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
    TEARDOWN_LOCK_ORDER_SUSPECT,
    "Suspect teardown lock ordering"
);
counter!(ROOT_PROOF_BLOCKED_EPOCH, "Retirements blocked by epoch");
counter!(ROOT_PROOF_BLOCKED_HW, "Retirements blocked by local TTBR0");
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

pub const COUNTER_COUNT: usize = 47;

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

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const BOOT_TEST_PID_COUNT_SLOTS: usize = 256;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
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
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
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
        }
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_TEST_PID_COUNTS: [BootTestPidCountSlot; BOOT_TEST_PID_COUNT_SLOTS] =
    [const { BootTestPidCountSlot::new() }; BOOT_TEST_PID_COUNT_SLOTS];

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static BOOT_TEST_PID_COUNTS_ACTIVE: AtomicU64 = AtomicU64::new(0);

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
struct BootTestPidCountsGuard;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
impl Drop for BootTestPidCountsGuard {
    fn drop(&mut self) {
        BOOT_TEST_PID_COUNTS_ACTIVE.store(0, Ordering::Release);
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
enum BootTestPidCountKind {
    Defer,
    Reclaim,
    Quarantine,
    SgiSent,
    KickObserved(u64),
    MaskedFramesWalked,
    Report,
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
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
            }
            return;
        }
        if slot_pid == 0 {
            return;
        }
    }
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
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
    }
    BOOT_TEST_PID_COUNTS_ACTIVE.store(1, Ordering::Release);
    BootTestPidCountsGuard
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
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    record_boot_test_pid_count(pid, BootTestPidCountKind::Defer);
}

#[inline(always)]
pub fn record_reclaim(pid: u64) {
    crate::trace_count!(TEARDOWN_RECLAIM);
    crate::trace_event!(TEARDOWN_PROVIDER, TEARDOWN_RECLAIM_EVENT, pid as u32);
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    record_boot_test_pid_count(pid, BootTestPidCountKind::Reclaim);
}

#[inline(always)]
pub fn record_quarantine(pid: u64) {
    crate::trace_count!(TEARDOWN_QUARANTINE);
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    record_boot_test_pid_count(pid, BootTestPidCountKind::Quarantine);
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = pid;
}

#[inline(always)]
pub fn record_exit_sgi_sent(pid: u64, batch: u64) {
    crate::trace_event!(TEARDOWN_PROVIDER, EXIT_SGI_SENT_EVENT, pid as u32);
    crate::trace_event!(TEARDOWN_PROVIDER, EXIT_SGI_BATCH_EVENT, batch as u32);
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
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
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    record_boot_test_pid_count(pid, BootTestPidCountKind::KickObserved(interval));
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = interval;
}

#[inline(always)]
pub fn record_masked_frames_walked(pid: u64) {
    crate::trace_count!(TEARDOWN_MASKED_FRAMES_WALKED);
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    record_boot_test_pid_count(pid, BootTestPidCountKind::MaskedFramesWalked);
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = pid;
}

#[inline(always)]
pub fn record_report(pid: u64) {
    #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
    record_boot_test_pid_count(pid, BootTestPidCountKind::Report);
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = pid;
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
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

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
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
    let pid_counts_guard = reset_boot_test_pid_counts();
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
        if defer_count == 0 {
            return TestResult::Fail("adapted-site per-PID defer proof was absent");
        }
        if defer_count > 1 {
            return TestResult::Fail("adapted-site per-PID defer proof was duplicated");
        }
        if reclaim_count == 0 {
            return TestResult::Fail("adapted-site per-PID reclaim proof was absent");
        }
        if reclaim_count > 1 {
            return TestResult::Fail("adapted-site per-PID reclaim proof was duplicated");
        }
    }
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

    core::mem::drop(pid_counts_guard);
    TestResult::Pass
}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn exit_kick_protocol_gate_test() -> crate::test_framework::registry::TestResult {
    use crate::test_framework::registry::TestResult;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use core::sync::atomic::AtomicBool;

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
    let reservation_bucket = RESERVATION_PID_A as usize % EXIT_KICK_BUCKETS;
    let reservation_slot = &EXIT_KICK_SLOTS[reservation_bucket];
    let payload_before = (
        reservation_slot.pid.load(Ordering::Relaxed),
        reservation_slot.at.load(Ordering::Relaxed),
    );
    let published_before = EXIT_KICK_PUBLISHED.aggregate();
    let collision_before = EXIT_KICK_BUCKET_COLLISION.aggregate();
    let sgi_before = EXIT_SGI_SENT.aggregate();
    let publisher_a_done = Arc::new(AtomicU64::new(0));
    let publisher_b_done = Arc::new(AtomicU64::new(0));
    let publisher_b_cpu = Arc::new(AtomicU64::new(u64::MAX));
    let hook = ExitKickTestHookGuard::arm(RESERVATION_PID_A);

    let a_done = Arc::clone(&publisher_a_done);
    let publisher_a = match crate::task::kthread::kthread_run(
        move || {
            crate::task::scheduler::Scheduler::send_exit_expedite_sgi(
                RESERVATION_PID_A,
                crate::task::scheduler::GroupBatchId::for_single_victim(RESERVATION_PID_A),
            );
            a_done.store(1, Ordering::Release);
        },
        "exit_kick_reserve_a",
    ) {
        Ok(handle) => handle,
        Err(_) => return TestResult::Fail("failed to spawn held exit-kick publisher A"),
    };

    while EXIT_KICK_TEST_HOOK_RESERVED.load(Ordering::Acquire) == 0 {
        crate::task::scheduler::yield_current();
        core::hint::spin_loop();
    }

    let b_done = Arc::clone(&publisher_b_done);
    let b_cpu = Arc::clone(&publisher_b_cpu);
    let publisher_b = match crate::task::kthread::kthread_run(
        move || {
            b_cpu.store(
                crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as u64,
                Ordering::Relaxed,
            );
            crate::task::scheduler::Scheduler::send_exit_expedite_sgi(
                RESERVATION_PID_B,
                crate::task::scheduler::GroupBatchId::for_single_victim(RESERVATION_PID_B),
            );
            b_done.store(1, Ordering::Release);
        },
        "exit_kick_reserve_b",
    ) {
        Ok(handle) => handle,
        Err(_) => {
            hook.release();
            let _ = crate::task::kthread::kthread_join(&publisher_a);
            return TestResult::Fail("failed to spawn colliding exit-kick publisher B");
        }
    };

    while publisher_b_done.load(Ordering::Acquire) == 0 {
        crate::task::scheduler::yield_current();
        core::hint::spin_loop();
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
    let joined = crate::task::kthread::kthread_join(&publisher_b).is_ok()
        && crate::task::kthread::kthread_join(&publisher_a).is_ok();
    core::mem::drop(hook);

    if !joined
        || publisher_a_done.load(Ordering::Acquire) != 1
        || publisher_a_cpu == u64::MAX
        || publisher_b_cpu == u64::MAX
        || publisher_a_cpu == publisher_b_cpu
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
        publisher_a_cpu_mask: AtomicU64,
        publisher_b_cpu_mask: AtomicU64,
        observer_cpu_mask: AtomicU64,
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
        publisher_a_cpu_mask: AtomicU64::new(0),
        publisher_b_cpu_mask: AtomicU64::new(0),
        observer_cpu_mask: AtomicU64::new(0),
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
                accounting.workers_ready.fetch_add(1, Ordering::Release);
                while !accounting.start.load(Ordering::Acquire) {
                    crate::task::scheduler::yield_current();
                    core::hint::spin_loop();
                }
                while !accounting.observer_running.load(Ordering::Acquire) {
                    crate::task::scheduler::yield_current();
                    core::hint::spin_loop();
                }
                if pid == PID_B {
                    while accounting.observations.load(Ordering::Acquire) == 0 {
                        crate::task::scheduler::yield_current();
                        core::hint::spin_loop();
                    }
                }
                for attempt in 0..ATTEMPTS_PER_PUBLISHER {
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
                            crate::task::scheduler::yield_current();
                            core::hint::spin_loop();
                        }
                    }
                    if attempt & 63 == 63 {
                        crate::task::scheduler::yield_current();
                    }
                }
                accounting.publishers_done.fetch_add(1, Ordering::Release);
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
    // the three workers on the other three CPUs. This preserves a CPU on which
    // the coordinator can release the start barrier. The RAII guard restores
    // preemption on every error return.
    let spawn_guard = StormSpawnPreemptGuard::enter();

    let online_cpus = crate::arch_impl::aarch64::smp::cpus_online() as usize;
    let coordinator_cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let mut worker_cpus = [usize::MAX; 3];
    let mut worker_count = 0;
    for cpu in 0..online_cpus {
        if cpu != coordinator_cpu && worker_count < worker_cpus.len() {
            worker_cpus[worker_count] = cpu;
            worker_count += 1;
        }
    }
    if worker_count != worker_cpus.len() {
        return TestResult::Fail("exit-kick storm requires four online CPUs");
    }

    let publisher_a = match spawn_publisher(PID_A, "exit_kick_pub_a", worker_cpus[0]) {
        Ok(handle) => handle,
        Err(_) => return TestResult::Fail("failed to spawn exit-kick publisher A"),
    };
    let publisher_b = match spawn_publisher(PID_B, "exit_kick_pub_b", worker_cpus[1]) {
        Ok(handle) => handle,
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
                .workers_ready
                .fetch_add(1, Ordering::Release);
            while !observer_accounting.start.load(Ordering::Acquire) {
                crate::task::scheduler::yield_current();
                core::hint::spin_loop();
            }
            observer_accounting
                .observer_running
                .store(true, Ordering::Release);
            while observer_accounting.publishers_done.load(Ordering::Acquire) != 2 {
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
                }
                core::hint::spin_loop();
            }
            observer_accounting
                .observer_done
                .store(true, Ordering::Release);
        },
        "exit_kick_observer",
        worker_cpus[2],
    ) {
        Ok(handle) => handle,
        Err(_) => return TestResult::Fail("failed to spawn exit-kick observer"),
    };
    core::mem::drop(spawn_guard);

    while accounting.workers_ready.load(Ordering::Acquire) != 3 {
        crate::task::scheduler::yield_current();
        core::hint::spin_loop();
    }
    accounting.start.store(true, Ordering::Release);

    if crate::task::kthread::kthread_join(&publisher_a).is_err()
        || crate::task::kthread::kthread_join(&publisher_b).is_err()
        || crate::task::kthread::kthread_join(&observer).is_err()
    {
        return TestResult::Fail("exit-kick storm kthread join failed");
    }

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
