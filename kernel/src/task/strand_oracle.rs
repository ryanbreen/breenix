//! Boot-test scheduler strand detector and deterministic #589 stimulus.

#![cfg(feature = "boot_tests")]

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
const INJECT_SCORE_WAIT_MS: u64 = 2_000;
#[cfg(target_arch = "aarch64")]
const INJECT_DEADLINE_MS: u64 = 6_000;

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
) {
    crate::serial_println!(
        "[SCHED_STRAND_ORACLE:{}:samples={}:checked={}:stranded={}:running_shape={}:ready_shape={}:resolved_production={}:resolved_exercised={}:worst_dwell_ms={}:overflow={}]",
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
    let b_scored = state == INJECT_B_SCORED_RECOVERED || state == INJECT_B_SCORED_LOST;
    let a_scored_lost = state == INJECT_A_SCORED_LOST;
    let deadline = INJECT_DEADLINE.load(Ordering::Acquire);
    a_scored_lost || b_scored || (deadline != 0 && now_ms >= deadline)
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
    let deadline = monotonic_now_ms().saturating_add(INJECT_DEADLINE_MS);
    INJECT_DEADLINE.store(deadline, Ordering::Release);

    while monotonic_now_ms() < deadline {
        let state = INJECT_ARMED.load(Ordering::Acquire);
        if state == INJECT_B_SCORED_RECOVERED || state == INJECT_B_SCORED_LOST {
            break;
        }
        super::scheduler::yield_current();
        crate::arch_impl::aarch64::context_switch::schedule_from_kernel();
        VICTIM_PROGRESS.fetch_add(1, Ordering::AcqRel);
        crate::arch_halt_with_interrupts();
    }
}

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

fn strand_oracle_thread() {
    let mut slots = [DwellSlot::EMPTY; STRAND_CENSUS_CAPACITY];
    let mut candidates = [StrandCandidate {
        tid: 0,
        shape: StrandShape::Running,
        privilege: ThreadPrivilege::Kernel,
        state: ThreadState::Running,
    }; STRAND_CENSUS_CAPACITY];
    let mut samples = 0u64;
    let mut checked = 0u64;
    let mut stranded = 0u64;
    let mut running_shape = 0u64;
    let mut ready_shape = 0u64;
    let mut worst_dwell_ms = 0u64;
    let mut overflow = 0u64;
    let mut first_attribution_reported = false;
    let mut first_strand = None;
    let mut strand_nonzero_reported = false;
    let mut next_strand_report_ms = monotonic_now_ms().saturating_add(3_000);
    #[cfg(target_arch = "aarch64")]
    let mut injection_scoring = InjectionScoring::default();

    loop {
        if let Some(census) = collect_strand_census(&mut candidates) {
            samples += 1;
            checked += census.checked;
            overflow += census.overflow;
            update_dwell(
                &candidates,
                census,
                monotonic_now_ms(),
                &mut slots,
                &mut stranded,
                &mut running_shape,
                &mut ready_shape,
                &mut worst_dwell_ms,
                &mut first_strand,
            );
        }

        let now_ms = monotonic_now_ms();
        if !first_attribution_reported {
            if let Some(first) = first_strand {
                report_first_strand(first);
                first_attribution_reported = true;
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            update_injection_scoring(now_ms, &mut injection_scoring);
            if !INJECT_REPORT_EMITTED.load(Ordering::Acquire)
                && injection_marker_ready(now_ms)
                && INJECT_REPORT_EMITTED
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            {
                report_injection();
            }
        }

        let immediate_strand_report = stranded != 0 && !strand_nonzero_reported;
        if immediate_strand_report || now_ms >= next_strand_report_ms {
            report_strand(
                samples,
                checked,
                stranded,
                running_shape,
                ready_shape,
                worst_dwell_ms,
                overflow,
            );
            if immediate_strand_report {
                strand_nonzero_reported = true;
            }
            if now_ms >= next_strand_report_ms {
                next_strand_report_ms = now_ms.saturating_add(5_000);
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
