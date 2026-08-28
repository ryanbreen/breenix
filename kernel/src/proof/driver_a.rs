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

use super::quiesce::{Controller, Mode};
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
    liveness: Option<LivenessBaseline>,
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
        liveness: liveness_baseline(),
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

    let lock_order = u64::from(scheduler::SCHED_AFTER_PM_VIOLATIONS.load(Ordering::Relaxed) != 0)
        | (u64::from(scheduler::EXEC_COMMIT_UNPINNED.load(Ordering::Relaxed) != 0) << 1)
        | (u64::from(scheduler::EXEC_COMMIT_MISSING_THREAD.load(Ordering::Relaxed) != 0) << 2);
    if lock_order != 0 && reported.first(REPORTED_EXEC_LOCK) {
        record::violation(seed, iteration, vector, "EXEC_LOCK_ORDER", lock_order);
    }
}

fn score_liveness(
    seed: u64,
    iteration: u64,
    vector: &rng::DrawVector,
    baseline: Option<LivenessBaseline>,
    reported: &mut Reported,
) {
    let (Some(baseline), Some(census)) = (baseline, strand_census()) else {
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
    let online_cpus = scheduler::online_cpu_count_snapshot();

    record::emit_seed_line(seed, mode, online_cpus);

    // The seed is already on the wire, so a boot that dies waiting still names
    // the run it was going to make.
    if !wait_for_boot_tests() {
        record::emit_run(seed, 0, mode, online_cpus);
        return;
    }

    // Read the identity and the baselines only now. Before the cohort finished,
    // the driver was an ordinary sleeping kthread that any CPU could pick up;
    // from here the pen keeps it where it is, and the baselines describe the
    // window the run actually covers rather than the boot that preceded it.
    let driver_cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let victim_tid = scheduler::current_thread_id().unwrap_or(0);
    let baseline = baseline();

    let controller =
        Controller::begin(mode, seed, COMPONENT_A, driver_cpu, victim_tid, online_cpus);

    let cadence_vector = rng::draw(seed, COMPONENT_A, driver_cpu as u8, u64::MAX);
    let release_cadence = u64::from(cadence_vector.cycles % 64) + 1;
    let started_at = monotonic_now_ns();
    let mut iterations = 0u64;
    let mut reported = Reported(0);

    while iterations < ITERATION_CAP
        && monotonic_now_ns().saturating_sub(started_at) < WALL_CLOCK_BUDGET_NS
    {
        let vector =
            stimulus::materialize(rng::draw(seed, COMPONENT_A, driver_cpu as u8, iterations));
        super::arm(&vector);
        crate::proof_point!(DriverPreCycle);
        let probe = scheduler::block_current_coreproof_probe();
        crate::proof_point!(DriverPostCycle);
        super::disarm();

        if let Some(Ok(probe)) = probe {
            score_probe(seed, iterations, &vector, probe, &mut reported);
        }
        // Both of these read shared state — the census takes the scheduler
        // lock, and the marker sweep touches per-CPU aggregate counters — so
        // they run at a cadence rather than per iteration. More iterations
        // inside one boot is the harness's actual lever (more BOOTS is
        // explicitly the wrong one), and paying a sweep per trial would spend
        // most of the budget re-reading counters that move on a millisecond
        // timescale, not a microsecond one.
        if iterations % CENSUS_CADENCE == 0 {
            score_existing_markers(seed, iterations, &vector, &baseline, &mut reported);
            score_liveness(seed, iterations, &vector, baseline.liveness, &mut reported);
        }

        if iterations % release_cadence == 0 {
            crate::proof_point!(DriverPreQuiesce);
            controller.release_and_reform();
        }
        iterations = iterations.wrapping_add(1);
    }

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
    score_liveness(
        seed,
        iterations,
        &closing_vector,
        baseline.liveness,
        &mut reported,
    );
    crate::proof_point!(DriverPreQuiesce);
    controller.finish();
    record::emit_run(seed, iterations, mode, online_cpus);
}
