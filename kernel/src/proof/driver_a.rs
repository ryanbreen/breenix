//! Component A — the ready-queue departure protocol, driven and scored.
//!
//! The engine is a probe that runs under ONE scheduler-lock acquisition with
//! interrupts masked (`scheduler::block_current_coreproof_probe`), modelled on the
//! existing `block_current_departure_gate`: it manufactures the one state that
//! makes the departure observable — the current thread present in its own CPU's
//! ready queue while it publishes `Blocked` — proves the primitive removed it,
//! and restores the planted state exactly before the lock is released. That is
//! why a trial costs microseconds instead of a scheduling round trip, and why
//! `iters` reaches five figures inside one boot.
//!
//! ## Postconditions read markers that already exist
//!
//! Three of them are genuinely new, and they are the component's own contract:
//! queue membership after a block, `unblock` idempotence on a runnable thread,
//! and conservation of total queue cardinality. Everything else in the table
//! below is a READ of a counter that already fails a gate somewhere in this
//! tree. Inventing a second notion of "a CPU pivoted onto a foreign stack" or
//! "the reclaim bracket held" would be a parallel truth, and a parallel truth
//! that disagrees with the original is worse than no check at all.
//!
//! `REDISPATCH_LIVENESS` is the clearest case. A departed thread that is never
//! re-dispatched is exactly what `collect_strand_census`'s `nonprogress`,
//! `queued_on_nondispatching_cpu` and `worst_queued_nondispatch_ms` fields
//! already measure, so the liveness check scores those against a baseline taken
//! at harness start rather than running a private timer. This is the one
//! postcondition with no fault marker to key off — the silent-hang case — and
//! it is the reason component A was chosen for the pilot.
//!
//! ## Where a postcondition is sampled is part of the postcondition
//!
//! Round 2 measured four planted defects executing inside the window and no
//! predicate firing on any of them. Two of those misses were a SAMPLING
//! mistake, not a missing marker, and round 3 fixes them here:
//!
//! * `RECLAIM_CLAIM_UNBRACKETED` (#653) scores an invariant that exists only
//!   while the machine is moving. The unbracketed claim is a handful of
//!   instructions wide and is gone by the time anything quiesces, so it is
//!   sampled IN-WINDOW on every iteration while the adversarial peers drive
//!   drains — and it is read out of the claim word and the per-CPU preempt
//!   counts, the two pieces of state the bracket is made of.
//! * `FUTEX_HANDOFF_RESCUED` (#584) is the opposite case. The futex handoff
//!   oracle's `rescues` census is LATCHED for the boot, so it is read at the
//!   existing cadence and once more at close; a per-iteration read would cost a
//!   load in the hot loop and learn nothing extra.
//!
//! Neither adds a truth. The first reads `RECLAIM_DRAIN_ACTIVE` and
//! `PerCpuData::preempt_count`; the second reads the counter behind the
//! `rescues=` field of the futex handoff oracle's marker line.
//!
//! ## First occurrence only
//!
//! At most one record is emitted per predicate per run. The gate fails on the
//! presence of any violation line, so nothing is lost, and a predicate that
//! fires on every one of tens of thousands of iterations cannot bury the serial
//! (and with it the RUN record the gate needs to read).

use core::sync::atomic::Ordering;

use crate::task::scheduler::{
    self, StrandCandidate, StrandCensus, StrandShape, UnblockOutcome, STRAND_CENSUS_CAPACITY,
};
use crate::task::thread::{ThreadPrivilege, ThreadState};

use super::quiesce::{Controller, Mode, Window};
use super::record::Phase;
use super::{record, rng, stimulus};

pub const COMPONENT_A: u8 = b'A';

const WALL_CLOCK_BUDGET_NS: u64 = 8_000_000_000;

/// How long the driver will wait for the boot-test cohort to publish its
/// verdict before giving up and reporting a run it never made.
const COHORT_WAIT_BUDGET_NS: u64 = 30_000_000_000;

/// Sleep interval while waiting for the cohort, in nanoseconds.
const COHORT_POLL_NS: u64 = 100_000_000;
const ITERATION_CAP: u64 = 1_000_000;
const CENSUS_CADENCE: u64 = 256;

/// Teardown counters that must not move while the harness window is open.
///
/// Read BY NAME, never by index into `teardown::snapshot()`. An index is a
/// literal that silently re-points at a different counter the moment one is
/// inserted into the registration array, which would leave the predicate
/// passing while measuring something else entirely — the brittle-literal
/// failure this campaign has been bitten by three times (#549, #551, #527-r1).
///
/// Both are pinned at zero by the production-profile gate
/// (`docker/qemu/run-x86-prod-profile-boot-test.sh`):
///
/// * `PT_ROOT_ABANDONED_NO_ARCH` — a page-table root losing its architecture
///   handle is never expected.
/// * `RECLAIM_ABANDONED_UNQUEUED` — a reclaim item must retain a queue owner;
///   this is the counter the reclaim-bracket family moves.
///
/// The other production-zero root and tombstone counters legitimately move
/// while the concurrent boot-test cohort creates and tears down processes, so
/// they are deliberately outside this subset. Narrowing it is stated here
/// rather than left implicit.
fn teardown_flat_counters() -> [u64; 2] {
    use crate::tracing::providers::teardown::{
        PT_ROOT_ABANDONED_NO_ARCH, RECLAIM_ABANDONED_UNQUEUED,
    };
    [
        PT_ROOT_ABANDONED_NO_ARCH.aggregate(),
        RECLAIM_ABANDONED_UNQUEUED.aggregate(),
    ]
}

#[derive(Clone, Copy)]
struct LivenessBaseline {
    nonprogress: usize,
    queued_on_nondispatching_cpu: u64,
    worst_queued_nondispatch_ms: u64,
}

struct Baseline {
    cpu_identity_split: u64,
    percpu_stack_alien: u64,
    teardown: [u64; 2],
    futex_rescues: u64,
}

struct LivenessMonitor {
    baseline: Option<LivenessBaseline>,
    waiting_for_cohort: bool,
}

impl LivenessMonitor {
    fn new(window: Window) -> Self {
        Self {
            baseline: liveness_baseline(),
            waiting_for_cohort: window == Window::Overlap && !super::boot_tests_complete(),
        }
    }

    /// Keep a rolling baseline while the ordinary boot cohort is active.
    ///
    /// Cohort tests deliberately create transient queued and nonprogressing
    /// threads, so comparing their later census to a snapshot from cohort start
    /// makes normal boot growth look like a harness finding. Coverage remains
    /// open during this interval; only the liveness comparison waits. When the
    /// verdict arrives, freeze the most recent real census and score subsequent
    /// samples against it without changing the existing strand predicates.
    fn baseline_for_score(&mut self) -> Option<LivenessBaseline> {
        if self.waiting_for_cohort {
            self.baseline = liveness_baseline();
            if !super::boot_tests_complete() {
                return None;
            }
            self.waiting_for_cohort = false;
            return None;
        }
        self.baseline
    }
}

struct Reported(u16);

impl Reported {
    fn first(&mut self, predicate_bit: u16) -> bool {
        let first = self.0 & predicate_bit == 0;
        self.0 |= predicate_bit;
        first
    }
}

const REPORTED_BLOCKED_READYQ: u16 = 1 << 0;
const REPORTED_UNBLOCK_RUNNABLE: u16 = 1 << 1;
const REPORTED_CARDINALITY: u16 = 1 << 2;
const REPORTED_LIVENESS: u16 = 1 << 3;
const REPORTED_CPU_IDENTITY: u16 = 1 << 4;
const REPORTED_STACK_ALIEN: u16 = 1 << 5;
const REPORTED_TEARDOWN: u16 = 1 << 6;
const REPORTED_EXEC_LOCK: u16 = 1 << 7;
const REPORTED_RECLAIM_BRACKET: u16 = 1 << 8;
const REPORTED_FUTEX_RESCUE: u16 = 1 << 9;

fn strand_census() -> Option<StrandCensus> {
    const EMPTY: StrandCandidate = StrandCandidate {
        tid: 0,
        shape: StrandShape::Running,
        privilege: ThreadPrivilege::Kernel,
        state: ThreadState::Running,
    };
    let mut candidates = [EMPTY; STRAND_CENSUS_CAPACITY];
    let mut nonprogress = [0u64; STRAND_CENSUS_CAPACITY];
    scheduler::collect_strand_census(&mut candidates, &mut nonprogress)
}

fn liveness_baseline() -> Option<LivenessBaseline> {
    strand_census().map(|census| LivenessBaseline {
        nonprogress: census.nonprogress,
        queued_on_nondispatching_cpu: census.queued_on_nondispatching_cpu,
        worst_queued_nondispatch_ms: census.worst_queued_nondispatch_ms,
    })
}

fn baseline() -> Baseline {
    Baseline {
        cpu_identity_split: crate::arch_impl::aarch64::percpu::CPU_IDENTITY_SPLIT_EVENTS
            .load(Ordering::Relaxed),
        percpu_stack_alien: crate::arch_impl::aarch64::percpu::PERCPU_STACK_ALIEN_REFUSALS
            .load(Ordering::Relaxed),
        teardown: teardown_flat_counters(),
        futex_rescues: crate::syscall::futex_oracle::rescues(),
    }
}

/// Block until the boot-test cohort has published its verdict.
///
/// The harness pens every other online CPU and runs a tight loop on its own, so
/// it must not overlap the cohort: the first smoke boot proved that concretely
/// by reddening `census_widen_oracle`, which needs a quiet strand census and
/// places its probe on the CPU the driver had taken. Waiting here — rather than
/// relaxing the oracle — is the fix, because the oracle was measuring correctly.
///
/// This BLOCKS on a timer rather than spinning. A driver that busy-waited for
/// the cohort would starve exactly what it is waiting for.
///
/// Returns false if the verdict never came, in which case the boot has larger
/// problems than the harness and the run record says `iters=0`.
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

fn monotonic_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

fn score_probe(
    seed: u64,
    iteration: u64,
    vector: &rng::DrawVector,
    probe: scheduler::DepartureProbe,
    reported: &mut Reported,
) {
    if probe.state_is_blocked && probe.queued_after_block && reported.first(REPORTED_BLOCKED_READYQ)
    {
        record::violation(seed, iteration, vector, "BLOCKED_NOT_IN_READYQ", 1);
    }
    if (probe.unblock_outcome != UnblockOutcome::AlreadyRunnable || probe.membership_changed)
        && reported.first(REPORTED_UNBLOCK_RUNNABLE)
    {
        let detail = u64::from(probe.membership_changed)
            | match probe.unblock_outcome {
                UnblockOutcome::AlreadyRunnable => 0,
                UnblockOutcome::Transitioned => 2,
                UnblockOutcome::NotFound => 4,
            };
        record::violation(seed, iteration, vector, "UNBLOCK_ALREADY_RUNNABLE", detail);
    }
    if probe.cardinality_before != probe.cardinality_after && reported.first(REPORTED_CARDINALITY) {
        let detail = ((probe.cardinality_before as u64) << 32) | probe.cardinality_after as u64;
        record::violation(seed, iteration, vector, "QUEUE_CARDINALITY", detail);
    }
}

fn score_existing_markers(
    seed: u64,
    iteration: u64,
    vector: &rng::DrawVector,
    baseline: &Baseline,
    reported: &mut Reported,
) {
    let identity =
        crate::arch_impl::aarch64::percpu::CPU_IDENTITY_SPLIT_EVENTS.load(Ordering::Relaxed);
    if identity != baseline.cpu_identity_split && reported.first(REPORTED_CPU_IDENTITY) {
        record::violation(seed, iteration, vector, "CPU_IDENTITY_SPLIT", identity);
    }

    let alien =
        crate::arch_impl::aarch64::percpu::PERCPU_STACK_ALIEN_REFUSALS.load(Ordering::Relaxed);
    if alien != baseline.percpu_stack_alien && reported.first(REPORTED_STACK_ALIEN) {
        record::violation(seed, iteration, vector, "PERCPU_STACK_ALIEN", alien);
    }

    let teardown = teardown_flat_counters();
    if let Some(index) = (0..teardown.len())
        .find(|index| teardown[*index] != baseline.teardown[*index])
        .filter(|_| reported.first(REPORTED_TEARDOWN))
    {
        let detail = ((index as u64) << 56) | (teardown[index] & 0x00ff_ffff_ffff_ffff);
        record::violation(seed, iteration, vector, "TEARDOWN_COUNTERS", detail);
    }

    // #584's observable, read out of the futex handoff oracle's own census
    // rather than re-derived. A rescue means a wait outlived its stage budget
    // with no wake reaching it — the lost handoff — and the counter is LATCHED,
    // so a cadence sample cannot miss one that has already happened. Sampling
    // it per iteration would buy nothing and cost a load in the hot loop.
    let futex_rescues = crate::syscall::futex_oracle::rescues();
    if futex_rescues != baseline.futex_rescues && reported.first(REPORTED_FUTEX_RESCUE) {
        record::violation(seed, iteration, vector, "FUTEX_HANDOFF_RESCUED", futex_rescues);
    }

    let lock_order = u64::from(scheduler::SCHED_AFTER_PM_VIOLATIONS.load(Ordering::Relaxed) != 0)
        | (u64::from(scheduler::EXEC_COMMIT_UNPINNED.load(Ordering::Relaxed) != 0) << 1)
        | (u64::from(scheduler::EXEC_COMMIT_MISSING_THREAD.load(Ordering::Relaxed) != 0) << 2);
    if lock_order != 0 && reported.first(REPORTED_EXEC_LOCK) {
        record::violation(seed, iteration, vector, "EXEC_LOCK_ORDER", lock_order);
    }
}

/// Whether every online CPU currently shows an empty preemption bracket.
///
/// A CPU whose count is non-zero is inside SOME non-preemptible region, which
/// is enough to explain a held reclaim claim; the predicate below only reports
/// when nobody at all is holding one, so any bracket anywhere makes this sample
/// inconclusive rather than clean.
fn no_cpu_holds_a_preempt_bracket(cpus: usize) -> bool {
    (0..cpus).all(|cpu| crate::per_cpu_aarch64::preempt_count_snapshot(cpu).unwrap_or(0) == 0)
}

/// #653's bracket invariant, scored from a CPU that is not the one draining.
///
/// PR #655 did not add a counter; it established an INVARIANT — the production
/// drain takes its preemption bracket BEFORE the compare-exchange and drops it
/// after the release, so `RECLAIM_DRAIN_ACTIVE == true` implies some CPU has a
/// non-zero preempt count. A claim held with no bracket anywhere is the exact
/// state the fix removed: the interval in which a scheduler handoff can abandon
/// the claim, latch the flag for the rest of the boot, and turn production
/// reclamation off (`process_task::RECLAIM_DRAIN_ACTIVE`'s own documentation).
///
/// This reads the claim word and the preempt counts THEMSELVES. It is not a
/// second notion of "the bracket held" — the driver's header rules that out and
/// is right to — it is the two pieces of state the bracket consists of.
///
/// SAMPLED IN-WINDOW, EVERY ITERATION, and that is forced by the state: the
/// unbracketed interval is a handful of instructions inside a call the
/// adversarial peers make tens of thousands of times, and it is GONE at
/// quiescence, when no drain is running at all. A cadence sample would be
/// sampling for a state that only exists while the peers are hammering.
///
/// Three flag reads around two sweeps, with the pass id pinned across all of
/// them, is what makes a positive sound. `RECLAIM_PASS_ID` advances on every
/// entry to the drain — refusals included — so an unchanged pass id proves no
/// CPU entered during the sample, and therefore that the claim seen at the
/// start is the claim seen at the end. Without that, a claim released and
/// retaken between two reads could show a bracket-free instant that never
/// existed.
fn reclaim_claim_unbracketed(online_cpus: usize) -> Option<u64> {
    let cpus = online_cpus.min(crate::arch_impl::aarch64::smp::MAX_CPUS);
    let (held, pass) = crate::task::process_task::reclaim_drain_claim_snapshot();
    if !held || !no_cpu_holds_a_preempt_bracket(cpus) {
        return None;
    }
    let (still_held, mid_pass) = crate::task::process_task::reclaim_drain_claim_snapshot();
    if !still_held || mid_pass != pass || !no_cpu_holds_a_preempt_bracket(cpus) {
        return None;
    }
    let (finally_held, end_pass) = crate::task::process_task::reclaim_drain_claim_snapshot();
    if !finally_held || end_pass != pass {
        return None;
    }
    Some((u64::from(pass) << 32) | cpus as u64)
}

fn score_reclaim_bracket(
    seed: u64,
    iteration: u64,
    vector: &rng::DrawVector,
    online_cpus: usize,
    reported: &mut Reported,
) {
    if let Some(detail) = reclaim_claim_unbracketed(online_cpus) {
        if reported.first(REPORTED_RECLAIM_BRACKET) {
            record::violation(
                seed,
                iteration,
                vector,
                "RECLAIM_CLAIM_UNBRACKETED",
                detail,
            );
        }
    }
}

fn score_liveness(
    seed: u64,
    iteration: u64,
    vector: &rng::DrawVector,
    monitor: &mut LivenessMonitor,
    reported: &mut Reported,
) {
    let (Some(baseline), Some(census)) = (monitor.baseline_for_score(), strand_census()) else {
        return;
    };
    if census.nonprogress > baseline.nonprogress
        || census.queued_on_nondispatching_cpu > baseline.queued_on_nondispatching_cpu
        || census.worst_queued_nondispatch_ms > baseline.worst_queued_nondispatch_ms
    {
        if !reported.first(REPORTED_LIVENESS) {
            return;
        }
        let detail = census
            .worst_queued_nondispatch_ms
            .max(census.nonprogress as u64)
            .max(census.queued_on_nondispatching_cpu);
        record::violation(seed, iteration, vector, "REDISPATCH_LIVENESS", detail);
    }
}

pub fn run() {
    let seed = rng::root_seed();
    let mode = Mode::selected();
    let window = Window::selected(mode);
    let online_cpus = scheduler::online_cpu_count_snapshot();

    // The seed goes on the wire BEFORE the cohort wait, and therefore before
    // anything in this boot that can die, so a run that never reaches its
    // window still names the run it was going to make. That is design 4.1's
    // requirement and it survives here rather than being traded away for a
    // settled `dcpu`.
    //
    // `dcpu` in THIS record is provisional. The driver is spawned unpinned and
    // sleeps through the cohort wait, so it may be resumed on another CPU; the
    // authoritative draw domain is the one in the settled record emitted after
    // the wait and in the closing record, which is the one the gate adjudicates.
    record::emit_seed_line(
        seed,
        crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize,
        mode,
        window,
        online_cpus,
        Phase::Open,
    );

    if window == Window::PostCohort && !wait_for_boot_tests() {
        let driver_cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
        record::emit_run(seed, driver_cpu, 0, mode, window, online_cpus, Phase::Close);
        return;
    }

    // Read the identity and the baselines only now. Before the cohort finished,
    // the driver was an ordinary sleeping kthread that any CPU could pick up;
    // from here the pen keeps it where it is, and the baselines describe the
    // window the run actually covers rather than the boot that preceded it.
    let driver_cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let victim_tid = scheduler::current_thread_id().unwrap_or(0);
    record::emit_seed_line(seed, driver_cpu, mode, window, online_cpus, Phase::Settled);
    let baseline = baseline();
    let mut liveness = LivenessMonitor::new(window);

    let controller = Controller::begin(
        mode,
        window,
        seed,
        COMPONENT_A,
        driver_cpu,
        victim_tid,
        online_cpus,
    );

    let cadence_vector = rng::draw(seed, COMPONENT_A, driver_cpu as u8, u64::MAX);
    let release_cadence = u64::from(cadence_vector.cycles % 64) + 1;
    let started_at = monotonic_now_ns();
    let mut iterations = 0u64;
    let mut reported = Reported(0);
    super::coverage::open_window();

    while iterations < ITERATION_CAP
        && monotonic_now_ns().saturating_sub(started_at) < WALL_CLOCK_BUDGET_NS
    {
        let vector =
            stimulus::materialize(rng::draw(seed, COMPONENT_A, driver_cpu as u8, iterations));
        if !super::loop_disarmed() {
            super::arm(&vector);
        }
        crate::proof_point!(DriverPreCycle);
        let probe = scheduler::block_current_coreproof_probe();
        crate::proof_point!(DriverPostCycle);
        super::disarm();

        if let Some(Ok(probe)) = probe {
            score_probe(seed, iterations, &vector, probe, &mut reported);
        }
        score_reclaim_bracket(seed, iterations, &vector, online_cpus, &mut reported);
        // Both of these read shared state — the census takes the scheduler
        // lock, and the marker sweep touches per-CPU aggregate counters — so
        // they run at a cadence rather than per iteration. More iterations
        // inside one boot is the harness's actual lever (more BOOTS is
        // explicitly the wrong one), and paying a sweep per trial would spend
        // most of the budget re-reading counters that move on a millisecond
        // timescale, not a microsecond one.
        if iterations % CENSUS_CADENCE == 0 {
            score_existing_markers(seed, iterations, &vector, &baseline, &mut reported);
            score_liveness(seed, iterations, &vector, &mut liveness, &mut reported);
        }

        if iterations % release_cadence == 0 {
            crate::proof_point!(DriverPreQuiesce);
            controller.release_and_reform();
        }
        if window == Window::Overlap {
            scheduler::yield_current();
        }
        iterations = iterations.wrapping_add(1);
    }
    super::coverage::close_window();

    super::disarm();
    // One closing sweep, so a marker that moved after the last cadence tick is
    // scored rather than dropped on the floor.
    let closing_vector = stimulus::materialize(rng::draw(
        seed,
        COMPONENT_A,
        driver_cpu as u8,
        iterations.saturating_sub(1),
    ));
    score_existing_markers(seed, iterations, &closing_vector, &baseline, &mut reported);
    score_reclaim_bracket(seed, iterations, &closing_vector, online_cpus, &mut reported);
    score_liveness(
        seed,
        iterations,
        &closing_vector,
        &mut liveness,
        &mut reported,
    );
    crate::proof_point!(DriverPreQuiesce);
    controller.finish();
    record::emit_run(
        seed,
        driver_cpu,
        iterations,
        mode,
        window,
        online_cpus,
        Phase::Close,
    );
}
