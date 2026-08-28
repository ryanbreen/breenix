use core::sync::atomic::Ordering;

use crate::task::scheduler::{
    self, StrandCandidate, StrandCensus, StrandShape, UnblockOutcome, STRAND_CENSUS_CAPACITY,
};
use crate::task::thread::{ThreadPrivilege, ThreadState};

use super::quiesce::{Controller, Mode};
use super::{record, rng, stimulus};

pub const COMPONENT_A: u8 = b'A';

const WALL_CLOCK_BUDGET_NS: u64 = 12_000_000_000;
const ITERATION_CAP: u64 = 1_000_000;
const CENSUS_CADENCE: u64 = 256;

// These indices name counters pinned at zero by the production-profile gate.
// PT_ROOT_ABANDONED_NO_ARCH: losing its architecture handle is never expected.
// RECLAIM_ABANDONED_UNQUEUED: a reclaim item must retain a queue owner.
// Other production-zero root/tombstone counters legitimately move while the
// concurrent boot-test cohort creates and tears down processes, so they are
// intentionally excluded from this harness-window subset.
const TEARDOWN_MUST_STAY_FLAT: [usize; 2] = [49, 77];

#[derive(Clone, Copy)]
struct LivenessBaseline {
    nonprogress: usize,
    queued_on_nondispatching_cpu: u64,
    worst_queued_nondispatch_ms: u64,
}

struct Baseline {
    cpu_identity_split: u64,
    percpu_stack_alien: u64,
    teardown: [u64; crate::tracing::providers::teardown::COUNTER_COUNT],
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
        teardown: crate::tracing::providers::teardown::snapshot(),
        liveness: liveness_baseline(),
    }
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

    let teardown = crate::tracing::providers::teardown::snapshot();
    if let Some(index) = TEARDOWN_MUST_STAY_FLAT
        .iter()
        .copied()
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
    let driver_cpu = crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    let victim_tid = scheduler::current_thread_id().unwrap_or(0);
    let baseline = baseline();

    record::emit_seed_line(seed, mode, online_cpus);
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
        let probe = scheduler::coreproof_departure_probe();
        crate::proof_point!(DriverPostCycle);
        super::disarm();

        if let Some(Ok(probe)) = probe {
            score_probe(seed, iterations, &vector, probe, &mut reported);
        }
        score_existing_markers(seed, iterations, &vector, &baseline, &mut reported);
        if iterations % CENSUS_CADENCE == 0 {
            score_liveness(seed, iterations, &vector, baseline.liveness, &mut reported);
        }

        if iterations % release_cadence == 0 {
            crate::proof_point!(DriverPreQuiesce);
            controller.release_and_reform();
        }
        iterations = iterations.wrapping_add(1);
    }

    super::disarm();
    crate::proof_point!(DriverPreQuiesce);
    controller.finish();
    record::emit_run(seed, iterations, mode, online_cpus);
}
