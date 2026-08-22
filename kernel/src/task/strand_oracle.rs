//! Boot-test scheduler strand detector and deterministic #589 stimulus.

#![cfg(feature = "boot_tests")]

#[cfg(not(target_arch = "aarch64"))]
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::scheduler::{
    collect_strand_census, StrandCandidate, StrandCensus, StrandShape, STRAND_CENSUS_CAPACITY,
};
use super::thread::{ThreadPrivilege, ThreadState};

pub const STRAND_DWELL_MS: u64 = 2_000;

static STARTED: AtomicBool = AtomicBool::new(false);

/// Round-B rollback counters are wired into the marker now and remain zero in
/// Round A. Production and exercised resolution are intentionally separate so
/// a test-only recovery leg cannot inflate the production count.
pub static RESOLVED_PRODUCTION: AtomicU64 = AtomicU64::new(0);
pub static RESOLVED_EXERCISED: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
pub(crate) fn note_pending_next_resolved(tid: u64) {
    if tid == VICTIM_TID.load(Ordering::Acquire) {
        RESOLVED_EXERCISED.fetch_add(1, Ordering::Relaxed);
    } else {
        RESOLVED_PRODUCTION.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(target_arch = "aarch64")]
const INJECT_IDLE: u64 = 0;
#[cfg(target_arch = "aarch64")]
const INJECT_A_ARMED: u64 = 1;
#[cfg(target_arch = "aarch64")]
const INJECT_A_FIRED: u64 = 2;
#[cfg(target_arch = "aarch64")]
const INJECT_A_SCORED_LOST: u64 = 4;
#[cfg(target_arch = "aarch64")]
const INJECT_B_ARMED: u64 = 5;
#[cfg(target_arch = "aarch64")]
const INJECT_B_FIRED: u64 = 6;
#[cfg(target_arch = "aarch64")]
const INJECT_B_SCORED_RECOVERED: u64 = 7;
#[cfg(target_arch = "aarch64")]
const INJECT_B_SCORED_LOST: u64 = 8;

#[cfg(target_arch = "aarch64")]
/// First census emission: early enough that a boot which dies in the first
/// seconds still carries one, so "stranded=0 in every gate boot" is a claim
/// about every boot rather than only the ones that survived the detector's
/// first report.
const STRAND_FIRST_REPORT_MS: u64 = 500;
#[cfg(target_arch = "aarch64")]
/// Steady-state census cadence after the first emission.
const STRAND_REPORT_PERIOD_MS: u64 = 5_000;

#[cfg(target_arch = "aarch64")]
const INJECT_SCORE_WAIT_MS: u64 = 1_000;
#[cfg(target_arch = "aarch64")]
const INJECT_DEADLINE_MS: u64 = 6_000;
#[cfg(target_arch = "aarch64")]
const INJECT_REPORT_CAP_MS: u64 = INJECT_DEADLINE_MS + 2 * INJECT_SCORE_WAIT_MS;

#[cfg(target_arch = "aarch64")]
static VICTIM_TID: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static VICTIM_PROGRESS: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static INJECT_ARMED: AtomicU64 = AtomicU64::new(INJECT_IDLE);
#[cfg(target_arch = "aarch64")]
static INJECT_DEADLINE: AtomicU64 = AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static INJECT_REPORT_EMITTED: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum InjectionLeg {
    A,
    B,
}

/// Consume the one-shot test arm for the selected victim.
///
/// This function is called from `inline_schedule_trampoline` immediately
/// after its state reads. It deliberately contains only the two relaxed loads
/// needed to identify the event, comparisons, and one compare-exchange. The
/// state transition itself records which leg fired; the victim TID is already
/// the published event record.
#[cfg(target_arch = "aarch64")]
pub(crate) fn inject_if_armed(new_id: u64) -> Option<InjectionLeg> {
    let armed = INJECT_ARMED.load(Ordering::Relaxed);
    let victim_tid = VICTIM_TID.load(Ordering::Relaxed);
    if new_id != victim_tid {
        return None;
    }

    let (next, leg) = match armed {
        INJECT_A_ARMED => (INJECT_A_FIRED, InjectionLeg::A),
        INJECT_B_ARMED => (INJECT_B_FIRED, InjectionLeg::B),
        _ => return None,
    };
    INJECT_ARMED
        .compare_exchange(armed, next, Ordering::AcqRel, Ordering::Relaxed)
        .ok()
        .map(|_| leg)
}

#[derive(Clone, Copy)]
struct DwellSlot {
    tid: u64,
    shape: StrandShape,
    first_seen_ms: u64,
    live: bool,
    stranded_recorded: bool,
}

#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
struct FirstStrand {
    candidate: StrandCandidate,
    dwell_ms: u64,
}

impl DwellSlot {
    const EMPTY: Self = Self {
        tid: 0,
        shape: StrandShape::Running,
        first_seen_ms: 0,
        live: false,
        stranded_recorded: false,
    };
}

struct OracleState {
    slots: [DwellSlot; STRAND_CENSUS_CAPACITY],
    samples: u64,
    checked: u64,
    stranded: u64,
    running_shape: u64,
    ready_shape: u64,
    worst_dwell_ms: u64,
    worst_nonprogress_ms: u64,
    nonprogress: usize,
    queued_on_nondispatching_cpu: u64,
    worst_queued_nondispatch_ms: u64,
    worst_cpu_scheduler_silence_ms: u64,
    worst_silence_cpu: u64,
    overflow: u64,
    #[cfg(target_arch = "aarch64")]
    first_strand: Option<FirstStrand>,
}

impl OracleState {
    const fn new() -> Self {
        Self {
            slots: [DwellSlot::EMPTY; STRAND_CENSUS_CAPACITY],
            samples: 0,
            checked: 0,
            stranded: 0,
            running_shape: 0,
            ready_shape: 0,
            worst_dwell_ms: 0,
            worst_nonprogress_ms: 0,
            nonprogress: 0,
            queued_on_nondispatching_cpu: 0,
            worst_queued_nondispatch_ms: 0,
            worst_cpu_scheduler_silence_ms: 0,
            worst_silence_cpu: 0,
            overflow: 0,
            #[cfg(target_arch = "aarch64")]
            first_strand: None,
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
struct X86OracleState(UnsafeCell<OracleState>);

#[cfg(not(target_arch = "aarch64"))]
// The state is accessed only through X86_ORACLE_STATE_BUSY, which is a
// non-blocking try-lock. The boot-test executor is the sole x86 sampler.
unsafe impl Sync for X86OracleState {}

#[cfg(not(target_arch = "aarch64"))]
static X86_ORACLE_STATE: X86OracleState = X86OracleState(UnsafeCell::new(OracleState::new()));

#[cfg(not(target_arch = "aarch64"))]
static X86_ORACLE_STATE_BUSY: AtomicBool = AtomicBool::new(false);

#[cfg(not(target_arch = "aarch64"))]
static X86_REPORT_EMITTED: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "aarch64")]
#[derive(Default)]
struct InjectionScoring {
    a_baseline: u64,
    a_deadline_ms: u64,
    b_baseline: u64,
    b_deadline_ms: u64,
}

fn monotonic_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

fn monotonic_now_ms() -> u64 {
    monotonic_now_ns() / 1_000_000
}

fn update_dwell(
    candidates: &[StrandCandidate; STRAND_CENSUS_CAPACITY],
    census: StrandCensus,
    now_ms: u64,
    slots: &mut [DwellSlot; STRAND_CENSUS_CAPACITY],
    stranded: &mut u64,
    running_shape: &mut u64,
    ready_shape: &mut u64,
    worst_dwell_ms: &mut u64,
    #[cfg(target_arch = "aarch64")]
    first_strand: &mut Option<FirstStrand>,
) {
    let mut seen = [false; STRAND_CENSUS_CAPACITY];

    for candidate in candidates.iter().take(census.candidates) {
        let existing = slots.iter().position(|slot| slot.tid == candidate.tid);
        let slot_index = existing.or_else(|| slots.iter().position(|slot| !slot.live));
        let Some(slot_index) = slot_index else {
            continue;
        };

        let slot = &mut slots[slot_index];
        if !slot.live || slot.shape != candidate.shape {
            *slot = DwellSlot {
                tid: candidate.tid,
                shape: candidate.shape,
                first_seen_ms: now_ms,
                live: true,
                stranded_recorded: false,
            };
        } else {
            slot.live = true;
        }
        seen[slot_index] = true;

        let dwell_ms = now_ms.saturating_sub(slot.first_seen_ms);
        *worst_dwell_ms = (*worst_dwell_ms).max(dwell_ms);
        if dwell_ms >= STRAND_DWELL_MS && !slot.stranded_recorded {
            slot.stranded_recorded = true;
            *stranded += 1;
            #[cfg(target_arch = "aarch64")]
            if first_strand.is_none() {
                *first_strand = Some(FirstStrand {
                    candidate: *candidate,
                    dwell_ms,
                });
            }
            match slot.shape {
                StrandShape::Running => *running_shape += 1,
                StrandShape::Ready => *ready_shape += 1,
            }
        }
    }

    for (index, slot) in slots.iter_mut().enumerate() {
        if !seen[index] {
            slot.live = false;
            slot.stranded_recorded = false;
        }
    }
}

fn sample_once(state: &mut OracleState) {
    let mut candidates = [StrandCandidate {
        tid: 0,
        shape: StrandShape::Running,
        privilege: ThreadPrivilege::Kernel,
        state: ThreadState::Running,
    }; STRAND_CENSUS_CAPACITY];
    let mut nonprogress = [0u64; STRAND_CENSUS_CAPACITY];

    if let Some(census) = collect_strand_census(&mut candidates, &mut nonprogress) {
        state.samples += 1;
        state.checked += census.checked;
        state.overflow += census.overflow;
        state.worst_nonprogress_ms = state.worst_nonprogress_ms.max(census.worst_nonprogress_ms);
        state.nonprogress = state.nonprogress.max(census.nonprogress);
        state.queued_on_nondispatching_cpu = state
            .queued_on_nondispatching_cpu
            .max(census.queued_on_nondispatching_cpu);
        state.worst_queued_nondispatch_ms = state
            .worst_queued_nondispatch_ms
            .max(census.worst_queued_nondispatch_ms);
        if census.worst_cpu_scheduler_silence_ms > state.worst_cpu_scheduler_silence_ms {
            state.worst_cpu_scheduler_silence_ms = census.worst_cpu_scheduler_silence_ms;
            state.worst_silence_cpu = census.worst_silence_cpu;
        }
        update_dwell(
            &candidates,
            census,
            monotonic_now_ms(),
            &mut state.slots,
            &mut state.stranded,
            &mut state.running_shape,
            &mut state.ready_shape,
            &mut state.worst_dwell_ms,
            #[cfg(target_arch = "aarch64")]
            &mut state.first_strand,
        );
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn with_x86_oracle_state<R>(f: impl FnOnce(&mut OracleState) -> R) -> Option<R> {
    if X86_ORACLE_STATE_BUSY
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return None;
    }

    let result = unsafe { f(&mut *X86_ORACLE_STATE.0.get()) };
    X86_ORACLE_STATE_BUSY.store(false, Ordering::Release);
    Some(result)
}

/// Take one non-blocking synchronous census for the x86 boot-test executor.
///
/// AArch64 sampling remains owned by its dedicated 50 ms kthread. This
/// function is intentionally allocation-free and emits no output; callers
/// report the accumulated x86 state only after the final boot-test sample.
pub fn sample_now() {
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = with_x86_oracle_state(sample_once);
    }
}

#[cfg(not(target_arch = "aarch64"))]
pub fn report_x86_once() {
    let Some(report) = with_x86_oracle_state(|state| {
        (
            state.samples,
            state.checked,
            state.stranded,
            state.running_shape,
            state.ready_shape,
            state.worst_dwell_ms,
            state.overflow,
            state.worst_nonprogress_ms,
            state.nonprogress,
            state.queued_on_nondispatching_cpu,
            state.worst_queued_nondispatch_ms,
            state.worst_cpu_scheduler_silence_ms,
            state.worst_silence_cpu,
        )
    }) else {
        return;
    };

    if X86_REPORT_EMITTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        report_strand(
            report.0, report.1, report.2, report.3, report.4, report.5, report.6, report.7,
            report.8, report.9, report.10, report.11, report.12,
        );
    }
}

#[cfg(target_arch = "aarch64")]
pub fn report_x86_once() {}

#[cfg(target_arch = "aarch64")]
fn report_first_strand(first: FirstStrand) {
    let shape = match first.candidate.shape {
        StrandShape::Running => "running",
        StrandShape::Ready => "ready",
    };
    let privilege = match first.candidate.privilege {
        ThreadPrivilege::User => "user",
        ThreadPrivilege::Kernel => "kernel",
    };
    let state = match first.candidate.state {
        ThreadState::Ready => 0,
        ThreadState::Running => 1,
        ThreadState::Blocked => 2,
        ThreadState::BlockedOnSignal => 3,
        ThreadState::BlockedOnChildExit => 4,
        ThreadState::BlockedOnTimer => 5,
        ThreadState::Terminated => 6,
        ThreadState::BlockedOnIO => 7,
    };
    crate::serial_println!(
        "[SCHED_STRAND_FIRST:tid={}:shape={}:priv={}:state={}:dwell_ms={}]",
        first.candidate.tid,
        shape,
        privilege,
        state,
        first.dwell_ms,
    );
}

fn report_strand(
    samples: u64,
    checked: u64,
    stranded: u64,
    running_shape: u64,
    ready_shape: u64,
    worst_dwell_ms: u64,
    overflow: u64,
    worst_nonprogress_ms: u64,
    nonprogress: usize,
    queued_on_nondispatching_cpu: u64,
    worst_queued_nondispatch_ms: u64,
    worst_cpu_scheduler_silence_ms: u64,
    worst_silence_cpu: u64,
) {
    crate::serial_println!(
        "[SCHED_STRAND_ORACLE:{}:samples={}:checked={}:stranded={}:running_shape={}:ready_shape={}:resolved_production={}:resolved_exercised={}:worst_dwell_ms={}:overflow={}:worst_nonprogress_ms={}:nonprogress={}:queued_on_nondispatching_cpu={}:worst_queued_nondispatch_ms={}:worst_cpu_scheduler_silence_ms={}:worst_silence_cpu={}]",
        if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86"
        },
        samples,
        checked,
        stranded,
        running_shape,
        ready_shape,
        RESOLVED_PRODUCTION.load(Ordering::Acquire),
        RESOLVED_EXERCISED.load(Ordering::Acquire),
        worst_dwell_ms,
        overflow,
        worst_nonprogress_ms,
        nonprogress,
        queued_on_nondispatching_cpu,
        worst_queued_nondispatch_ms,
        worst_cpu_scheduler_silence_ms,
        worst_silence_cpu,
    );
}

#[cfg(target_arch = "aarch64")]
fn update_injection_scoring(now_ms: u64, scoring: &mut InjectionScoring) {
    let state = INJECT_ARMED.load(Ordering::Acquire);
    if state == INJECT_A_FIRED {
        if scoring.a_deadline_ms == 0 {
            scoring.a_baseline = VICTIM_PROGRESS.load(Ordering::Acquire);
            scoring.a_deadline_ms = now_ms.saturating_add(INJECT_SCORE_WAIT_MS);
        } else if now_ms >= scoring.a_deadline_ms {
            let recovered = VICTIM_PROGRESS.load(Ordering::Acquire) > scoring.a_baseline;
            if recovered {
                INJECT_ARMED.store(INJECT_B_ARMED, Ordering::Release);
            } else {
                INJECT_ARMED.store(INJECT_A_SCORED_LOST, Ordering::Release);
            }
        }
    }

    let state = INJECT_ARMED.load(Ordering::Acquire);
    if state == INJECT_B_FIRED {
        if scoring.b_deadline_ms == 0 {
            scoring.b_baseline = VICTIM_PROGRESS.load(Ordering::Acquire);
            scoring.b_deadline_ms = now_ms.saturating_add(INJECT_SCORE_WAIT_MS);
        } else if now_ms >= scoring.b_deadline_ms {
            let recovered = VICTIM_PROGRESS.load(Ordering::Acquire) > scoring.b_baseline;
            if recovered {
                INJECT_ARMED.store(INJECT_B_SCORED_RECOVERED, Ordering::Release);
            } else {
                INJECT_ARMED.store(INJECT_B_SCORED_LOST, Ordering::Release);
            }
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn injection_marker_ready(now_ms: u64) -> bool {
    let state = INJECT_ARMED.load(Ordering::Acquire);
    let deadline = INJECT_DEADLINE.load(Ordering::Acquire);
    if deadline == 0 {
        return false;
    }

    let terminal = state == INJECT_A_SCORED_LOST
        || state == INJECT_B_SCORED_RECOVERED
        || state == INJECT_B_SCORED_LOST;
    if terminal {
        return true;
    }

    let report_cap_ms = deadline.saturating_add(2 * INJECT_SCORE_WAIT_MS);
    if now_ms >= report_cap_ms {
        return true;
    }

    now_ms >= deadline && state != INJECT_A_FIRED && state != INJECT_B_FIRED
}

#[cfg(target_arch = "aarch64")]
fn report_injection() {
    let state = INJECT_ARMED.load(Ordering::Acquire);
    let leg_a_exercised = state >= INJECT_A_FIRED;
    let leg_a_recovered = state == INJECT_B_ARMED
        || state == INJECT_B_FIRED
        || state == INJECT_B_SCORED_RECOVERED
        || state == INJECT_B_SCORED_LOST;
    let leg_b_exercised = state >= INJECT_B_FIRED;
    let leg_b_recovered = state == INJECT_B_SCORED_RECOVERED;
    let stranded = u64::from(leg_a_exercised && !leg_a_recovered)
        + u64::from(leg_b_exercised && !leg_b_recovered);

    crate::serial_println!(
        "[STRAND_INJECT_ORACLE:aarch64:legA_exercised={}:legA_recovered={}:legB_exercised={}:legB_recovered={}:stranded={}]",
        u64::from(leg_a_exercised),
        u64::from(leg_a_recovered),
        u64::from(leg_b_exercised),
        u64::from(leg_b_recovered),
        stranded,
    );
}

#[cfg(target_arch = "aarch64")]
fn strand_victim() {
    let Some(tid) = super::scheduler::current_thread_id() else {
        return;
    };
    VICTIM_TID.store(tid, Ordering::Release);
    let start = monotonic_now_ms();
    let deadline = start.saturating_add(INJECT_DEADLINE_MS);
    let report_cap_ms = start.saturating_add(INJECT_REPORT_CAP_MS);
    INJECT_DEADLINE.store(deadline, Ordering::Release);

    // The old yield_current() + schedule_from_kernel() +
    // arch_halt_with_interrupts() body drove the inline schedule path at roughly
    // 1 kHz for six seconds. A 50-boot control arm with injection fully disarmed
    // but this loop running reproduced the whole collateral bucket (7 aborts and
    // 4 hangs in 49 boots), versus 1 abort in 50 without the loop. Its drive rate
    // is not part of this oracle: the victim only needs to be dispatchable and
    // make observable forward progress.
    while monotonic_now_ms() < report_cap_ms {
        let state = INJECT_ARMED.load(Ordering::Acquire);
        if state == INJECT_B_SCORED_RECOVERED || state == INJECT_B_SCORED_LOST {
            break;
        }
        VICTIM_PROGRESS.fetch_add(1, Ordering::AcqRel);
        sleep_sample_period();
    }
}

#[cfg(target_arch = "aarch64")]
fn sleep_sample_period() {
    let Some(tid) = super::scheduler::current_thread_id() else {
        crate::arch_halt_with_interrupts();
        return;
    };
    let wake_time_ns = monotonic_now_ns().saturating_add(50_000_000);
    super::scheduler::with_scheduler(|scheduler| {
        if scheduler.get_thread(tid).is_some() {
            scheduler.block_current_for_timer(wake_time_ns);
        }
    });
    super::scheduler::yield_current();

    loop {
        crate::arch_halt_with_interrupts();
        let blocked = super::scheduler::with_scheduler(|scheduler| {
            scheduler
                .get_thread(tid)
                .is_some_and(|thread| thread.state == ThreadState::BlockedOnTimer)
        })
        .unwrap_or(false);
        if !blocked {
            break;
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn strand_oracle_thread() {
    let mut state = OracleState::new();
    let mut first_attribution_reported = false;
    let mut strand_nonzero_reported = false;
    let mut next_strand_report_ms = monotonic_now_ms().saturating_add(STRAND_FIRST_REPORT_MS);
    let mut injection_scoring = InjectionScoring::default();

    loop {
        sample_once(&mut state);

        let now_ms = monotonic_now_ms();
        if !first_attribution_reported {
            if let Some(first) = state.first_strand {
                report_first_strand(first);
                first_attribution_reported = true;
            }
        }
        update_injection_scoring(now_ms, &mut injection_scoring);
        if !INJECT_REPORT_EMITTED.load(Ordering::Acquire)
            && injection_marker_ready(now_ms)
            && INJECT_REPORT_EMITTED
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            report_injection();
        }

        let immediate_strand_report = state.stranded != 0 && !strand_nonzero_reported;
        if immediate_strand_report || now_ms >= next_strand_report_ms {
            report_strand(
                state.samples,
                state.checked,
                state.stranded,
                state.running_shape,
                state.ready_shape,
                state.worst_dwell_ms,
                state.overflow,
                state.worst_nonprogress_ms,
                state.nonprogress,
                state.queued_on_nondispatching_cpu,
                state.worst_queued_nondispatch_ms,
                state.worst_cpu_scheduler_silence_ms,
                state.worst_silence_cpu,
            );
            if immediate_strand_report {
                strand_nonzero_reported = true;
            }
            if now_ms >= next_strand_report_ms {
                next_strand_report_ms = now_ms.saturating_add(STRAND_REPORT_PERIOD_MS);
            }
        }

        sleep_sample_period();
    }
}

/// Start the always-on detector and, on AArch64, its deterministic victim.
pub fn start() {
    if STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    #[cfg(target_arch = "aarch64")]
    {
        if let Ok(victim) = super::kthread::kthread_run(strand_victim, "strand-victim") {
            // Publish and arm before the victim can be selected for its first
            // dispatch. The victim repeats the publication after it starts, but
            // this ordering makes the one-shot stimulus deterministic even when
            // the first dispatch is the event being tested.
            VICTIM_TID.store(victim.tid(), Ordering::Release);
            VICTIM_PROGRESS.store(0, Ordering::Release);
            INJECT_ARMED.store(INJECT_A_ARMED, Ordering::Release);
        }
        let _ = super::kthread::kthread_run(strand_oracle_thread, "strand-oracle");
    }
}
