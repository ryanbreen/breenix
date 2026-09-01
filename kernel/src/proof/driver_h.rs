//! Component H — dispatch admission, driven and scored.
//!
//! H's contract is the two-part `INLINE_SCHEDULE_STATE` handoff-slot discipline and the
//! `pending_next` conservation discipline behind `#605`, `#589`, `#607`, and `#645`.
//! Every postcondition reads an existing production marker — the null-fallback trace,
//! `strand_oracle`'s resolution/strand counters — plus the single coreproof-only slot-entry
//! attribution instrument introduced by rung 3 section 1.4. The driver does not define a
//! parallel truth for behavior those markers already judge.
//!
//! ## A/C hybrid
//!
//! The `pending_next` half is already stimulated by the unconditionally shared
//! `ThreadChurn` antagonist in `quiesce.rs`, which calls
//! `exercise_pending_next_coreproof_probe` regardless of the driving component. H therefore
//! reads the resulting `RESOLVED_PRODUCTION`/`RESOLVED_EXERCISED` counters and does not invoke
//! that probe directly. The `INLINE_SCHEDULE_STATE` half needs live cross-CPU scheduling
//! pressure, so H reuses Component C's peer-armed shape: every cycle it targets
//! `IncomingHandoffCommit`/`PendingNextResolveEntry` (or the driver census draw) on every
//! online peer through `arm_cpu`/`disarm_cpu`.
//!
//! `LastFired` retains its disclosed advisory-only one-writer-per-cpu caveat: an `Open` seam
//! could be preempted and resume on another cpu between its fires, creating a second writer
//! for a slot whose snapshot protocol assumes one. H's two scheduler seams are `Masked`, so
//! they add no exposure. `DriverPreCycle` is `Open`, matching Component C, and carries the
//! pre-existing caveat forward unchanged; it has no kernel consequence and rung 3 does not
//! attempt a structural fix unrelated to H's contract.
//!
//! ## Deliberately no forced victim/stealer pair
//!
//! Component C forces two peers into fixed roles (`assign_roles = component == b'C'`). H's
//! component byte is `b'H'`, so `assign_roles` intentionally remains false. `#605`, `#607`,
//! and `#589` describe one CPU racing with its own handoff slot across back-to-back
//! `schedule()` calls: scheduling density per cpu is the lever, not a designated two-party
//! contest. Ordinary independent antagonist draws are therefore a deliberate deviation from
//! C's precedent, not a missing copy of its role assignment.

use core::sync::atomic::Ordering;

use crate::arch_impl::aarch64::context_switch::{
    coreproof_reset_self_retracted_tags, COREPROOF_INLINE_SLOT_ALREADY_CONSUMED_UNEXPLAINED,
    INLINE_SCHED_NULL_FALLBACK,
};
use crate::task::scheduler;
use crate::task::strand_oracle;

use super::coverage::MutSite;
use super::quiesce::{Controller, Mode, Window};
use super::record::Phase;
use super::{coverage, record, rng, stimulus};

pub const COMPONENT_H: u8 = b'H';

const WALL_CLOCK_BUDGET_NS: u64 = 8_000_000_000;
const COHORT_WAIT_BUDGET_NS: u64 = 30_000_000_000;
const COHORT_POLL_NS: u64 = 100_000_000;
const ITERATION_CAP: u64 = 1_000_000;
const CENSUS_CADENCE: u64 = 256;

struct Baseline {
    inline_slot_unexplained: u64,
    resolved_production: u64,
    resolved_exercised: u64,
    stranded: u64,
}

fn baseline() -> Baseline {
    Baseline {
        inline_slot_unexplained: COREPROOF_INLINE_SLOT_ALREADY_CONSUMED_UNEXPLAINED
            .load(Ordering::Relaxed),
        resolved_production: strand_oracle::RESOLVED_PRODUCTION.load(Ordering::Acquire),
        resolved_exercised: strand_oracle::RESOLVED_EXERCISED.load(Ordering::Acquire),
        stranded: strand_oracle::stranded_count(),
    }
}

struct Reported(u8);

impl Reported {
    fn first(&mut self, predicate_bit: u8) -> bool {
        let first = self.0 & predicate_bit == 0;
        self.0 |= predicate_bit;
        first
    }
}

const REPORTED_INLINE_SLOT_UNEXPLAINED: u8 = 1 << 0;
const REPORTED_PENDING_NEXT_UNRESOLVED: u8 = 1 << 1;
const REPORTED_OUTGOING_HANDOFF_STRANDED: u8 = 1 << 2;

fn score_inline_slot(
    seed: u64,
    iteration: u64,
    vector: &rng::DrawVector,
    baseline: &Baseline,
    fired_cpu: Option<usize>,
    fired_iter: Option<u64>,
    reported: &mut Reported,
) {
    let unexplained = COREPROOF_INLINE_SLOT_ALREADY_CONSUMED_UNEXPLAINED.load(Ordering::Relaxed);
    if unexplained != baseline.inline_slot_unexplained
        && reported.first(REPORTED_INLINE_SLOT_UNEXPLAINED)
    {
        record::violation(
            seed,
            iteration,
            vector,
            "INLINE_SLOT_UNEXPLAINED",
            unexplained,
            COMPONENT_H,
            fired_cpu,
            fired_iter,
        );
    }
}

fn score_outgoing_handoff_stranded(
    seed: u64,
    iteration: u64,
    vector: &rng::DrawVector,
    baseline: &Baseline,
    fired_cpu: Option<usize>,
    fired_iter: Option<u64>,
    reported: &mut Reported,
) {
    let stranded = strand_oracle::stranded_count();
    if stranded != baseline.stranded && reported.first(REPORTED_OUTGOING_HANDOFF_STRANDED) {
        record::violation(
            seed,
            iteration,
            vector,
            "OUTGOING_HANDOFF_STRANDED",
            stranded,
            COMPONENT_H,
            fired_cpu,
            fired_iter,
        );
    }
}

fn corroborating_nonprogress_bit() -> bool {
    use crate::task::scheduler::{StrandCandidate, StrandShape, STRAND_CENSUS_CAPACITY};
    use crate::task::thread::{ThreadPrivilege, ThreadState};

    const EMPTY: StrandCandidate = StrandCandidate {
        tid: 0,
        shape: StrandShape::Running,
        privilege: ThreadPrivilege::Kernel,
        state: ThreadState::Running,
    };
    let mut candidates = [EMPTY; STRAND_CENSUS_CAPACITY];
    let mut nonprogress = [0u64; STRAND_CENSUS_CAPACITY];
    scheduler::collect_strand_census(&mut candidates, &mut nonprogress)
        .map(|census| census.nonprogress > 0)
        .unwrap_or(false)
}

fn score_pending_next_resolution(
    seed: u64,
    iteration: u64,
    vector: &rng::DrawVector,
    baseline: &Baseline,
    reported: &mut Reported,
) {
    let production = strand_oracle::RESOLVED_PRODUCTION.load(Ordering::Acquire);
    let exercised = strand_oracle::RESOLVED_EXERCISED.load(Ordering::Acquire);
    if production == baseline.resolved_production
        && exercised == baseline.resolved_exercised
        && reported.first(REPORTED_PENDING_NEXT_UNRESOLVED)
    {
        let detail = production | (u64::from(corroborating_nonprogress_bit()) << 63);
        record::violation(
            seed,
            iteration,
            vector,
            "PENDING_NEXT_UNRESOLVED",
            detail,
            COMPONENT_H,
            None,
            None,
        );
    }
}

fn note_outgoing_fallback_coverage(last_seen: &mut u64) {
    let current = INLINE_SCHED_NULL_FALLBACK.aggregate();
    let delta = current.saturating_sub(*last_seen);
    for _ in 0..delta {
        coverage::note(MutSite::OutgoingFallbackRequeue);
    }
    *last_seen = current;
}

fn monotonic_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

fn wait_for_boot_tests() -> bool {
    let started_at = monotonic_now_ns();
    while !super::boot_tests_complete() {
        if monotonic_now_ns().saturating_sub(started_at) >= COHORT_WAIT_BUDGET_NS {
            return false;
        }
        let Some(tid) = scheduler::current_thread_id() else {
            crate::arch_halt_with_interrupts();
            continue;
        };
        let wake_at = monotonic_now_ns().saturating_add(COHORT_POLL_NS);
        scheduler::with_scheduler(|scheduler| {
            if scheduler.get_thread(tid).is_some() {
                scheduler.block_current_for_timer(wake_at);
            }
        });
        crate::task::scheduler::schedule();
    }
    true
}

/// Every online cpu except the driver's own, capped at `MAX_CPUS`.
fn online_peer_cpus(driver_cpu: usize, online_cpus: usize) -> impl Iterator<Item = usize> {
    let cap = online_cpus.min(crate::arch_impl::aarch64::smp::MAX_CPUS);
    (0..cap).filter(move |&cpu| cpu != driver_cpu)
}

/// The vector that actually fired on some peer cpu since this driver started (and which
/// cpu/sequence number it fired at), or an explicit "nothing has fired yet" placeholder if
/// none has — same non-fabrication rule `driver_c.rs`'s own `last_fired_stimulus` documents
/// (rung 2 review, M3). Unlike `driver_c.rs`'s pre-rung-3 version, the (cpu, seq) pair is
/// THREADED THROUGH to the caller rather than discarded (rung 3 review, m2): H's own
/// predicate is specifically about WHICH peer's entry produced a given classification, so
/// dropping that attribution here would be a real loss, not a convenience.
fn last_fired_stimulus(
    driver_cpu: usize,
    online_cpus: usize,
) -> (Option<usize>, Option<u64>, rng::DrawVector) {
    match super::most_recent_fired(online_peer_cpus(driver_cpu, online_cpus)) {
        Some((cpu, seq, vector)) => (Some(cpu), Some(seq), vector),
        None => (
            None,
            None,
            rng::DrawVector {
                site: super::ALL[0],
                action: stimulus::Action::None,
                ticks: 0,
                cycles: 0,
                antagonist_op: rng::AntagonistOp::Unblock,
                antagonist_cpu: 0,
                order: super::ALL[0].order(),
            },
        ),
    }
}

pub fn run() {
    let seed = rng::root_seed();
    let mode = Mode::selected();
    let window = Window::selected(mode);
    let online_cpus = scheduler::online_cpu_count_snapshot();

    record::emit_seed_line(
        seed,
        crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize,
        mode,
        window,
        online_cpus,
        Phase::Open,
        COMPONENT_H,
    );

    if window == Window::PostCohort && !wait_for_boot_tests() {
        let driver_cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
        record::emit_run(
            seed,
            driver_cpu,
            0,
            mode,
            window,
            online_cpus,
            Phase::Close,
            COMPONENT_H,
        );
        return;
    }

    let driver_cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let victim_tid = scheduler::current_thread_id().unwrap_or(0);
    record::emit_seed_line(
        seed,
        driver_cpu,
        mode,
        window,
        online_cpus,
        Phase::Settled,
        COMPONENT_H,
    );
    let baseline_snapshot = baseline();

    let controller = Controller::begin(
        mode,
        window,
        seed,
        COMPONENT_H,
        driver_cpu,
        victim_tid,
        online_cpus,
    );

    let cadence_vector = rng::draw(seed, COMPONENT_H, driver_cpu as u8, u64::MAX);
    let release_cadence = u64::from(cadence_vector.cycles % 64) + 1;
    let started_at = monotonic_now_ns();
    let mut iterations = 0u64;
    let mut reported = Reported(0);
    super::coverage::open_window();
    // Bound the self-retraction attribution tag's lifetime to this measured
    // window (rung 3 review, M1): a `true` left by pre-window activity must
    // never be available to absorb a later, unrelated null read as ATTRIBUTED.
    coreproof_reset_self_retracted_tags();
    let mut last_null_fallback = INLINE_SCHED_NULL_FALLBACK.aggregate();

    while iterations < ITERATION_CAP
        && monotonic_now_ns().saturating_sub(started_at) < WALL_CLOCK_BUDGET_NS
    {
        if !super::loop_disarmed() {
            for cpu in online_peer_cpus(driver_cpu, online_cpus) {
                let vector =
                    stimulus::materialize(rng::draw(seed, COMPONENT_H, cpu as u8, iterations));
                super::arm_cpu(cpu, &vector);
            }
        }

        crate::proof_point!(DriverPreCycle);

        if iterations % CENSUS_CADENCE == 0 {
            let (fired_cpu, fired_iter, vector) = last_fired_stimulus(driver_cpu, online_cpus);
            score_inline_slot(
                seed,
                iterations,
                &vector,
                &baseline_snapshot,
                fired_cpu,
                fired_iter,
                &mut reported,
            );
            score_outgoing_handoff_stranded(
                seed,
                iterations,
                &vector,
                &baseline_snapshot,
                fired_cpu,
                fired_iter,
                &mut reported,
            );
            note_outgoing_fallback_coverage(&mut last_null_fallback);
        }

        if iterations % release_cadence == 0 {
            controller.release_and_reform();
        }
        if window == Window::Overlap {
            scheduler::yield_current();
        }
        iterations = iterations.wrapping_add(1);
    }

    note_outgoing_fallback_coverage(&mut last_null_fallback);
    super::coverage::close_window();

    for cpu in online_peer_cpus(driver_cpu, online_cpus) {
        super::disarm_cpu(cpu);
    }

    let (fired_cpu, fired_iter, closing_vector) = last_fired_stimulus(driver_cpu, online_cpus);
    score_inline_slot(
        seed,
        iterations,
        &closing_vector,
        &baseline_snapshot,
        fired_cpu,
        fired_iter,
        &mut reported,
    );
    score_outgoing_handoff_stranded(
        seed,
        iterations,
        &closing_vector,
        &baseline_snapshot,
        fired_cpu,
        fired_iter,
        &mut reported,
    );
    score_pending_next_resolution(
        seed,
        iterations,
        &closing_vector,
        &baseline_snapshot,
        &mut reported,
    );

    controller.finish();
    record::emit_run(
        seed,
        driver_cpu,
        iterations,
        mode,
        window,
        online_cpus,
        Phase::Close,
        COMPONENT_H,
    );
}
