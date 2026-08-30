//! Component C — per-CPU identity + stack custody, driven and scored.
//!
//! ## What rung 1 already proved and this driver does not re-derive
//!
//! Every postcondition below is a read of a counter that already fails a gate
//! (`kernel/src/arch_impl/aarch64/percpu.rs`'s `CPU_IDENTITY_SPLIT_EVENTS` /
//! `PERCPU_STACK_ALIEN_REFUSALS`, `context_switch.rs`'s `RET_STAGE_REFUSALS`).
//! No new bookkeeping is added anywhere in production code for this driver;
//! see rung 2's spec §1 for the full citation of each marker's sole emitter.
//! The one genuinely new thing this rung contributes is stimulus: a driver
//! whose job is PRODUCING the live, cross-CPU race these markers are already
//! wired to detect, not defining what "caught" means.
//!
//! ## Why this driver's shape differs from Component A's
//!
//! Component A's engine is a synchronous, self-contained probe: the driver
//! arms its OWN cpu's slot, calls a manufactured single-lock primitive, and
//! disarms — the whole cycle costs microseconds and never leaves the driver's
//! own execution. Component C's target defect is not reachable that way. The
//! vulnerable window (`schedule_from_kernel`'s pre-mask identity read,
//! `context_switch.rs`) sits inside a permanently seam-prohibited file, and
//! producing the damage needs a thread PREEMPTED there to resume on ANOTHER
//! cpu — a live, cross-CPU race, not a manufactured critical section.
//!
//! So this driver does not probe anything itself. It arms the ONE upstream
//! seam this rung is allowed to place (`ScheduleEntry`, at the top of
//! `scheduler::schedule()`, `SiteClass::Open`) on every online PEER cpu, every
//! cycle, so that whichever peer next executes `scheduler::schedule()` — via
//! the existing `KernelSchedule` or `Steal` adversarial ops, unmodified, which
//! the pilot's own round-3 measurement already put through this exact function
//! 50k-54k times per boot — finds a freshly drawn vector waiting for it. A
//! `TimerSqueeze` armed there fires the timer within a handful of cycles of the
//! seam, aiming for the still-unmasked window a few instructions further down
//! `schedule_from_kernel`'s own entry. This is aim, not a guarantee: the
//! residual gap between "fires very soon after the seam" and "fires inside the
//! exact pre-mask window" is real, and is exactly what `iters` and a replay hit
//! rate measure rather than assert. See rung 2's spec §3.1 for the full
//! reasoning and the pre-registered reading of a miss.
//!
//! `victim_tid` is carried through to `Controller::begin` exactly as Component
//! A uses it (the driver's own current thread id at settle time), which keeps
//! `Unblock`/`Steal`'s unblock half inert here since this driver's own thread
//! is never blocked mid-loop — the useful half of `Steal` for this component is
//! the dispatch attempt it always makes on the stealer's own cpu, which itself
//! reaches `ScheduleEntry` exactly like `KernelSchedule` does. This is a
//! disclosed simplification, not a claim that a dedicated victim thread
//! wouldn't sharpen the aim further; it is the honest starting point, matching
//! the "reuse existing antagonist machinery verbatim" constraint this rung was
//! built under.

use core::sync::atomic::Ordering;

use crate::task::scheduler;

use super::quiesce::{Controller, Mode, Window};
use super::record::Phase;
use super::{record, rng, stimulus};

pub const COMPONENT_C: u8 = b'C';

const WALL_CLOCK_BUDGET_NS: u64 = 8_000_000_000;

/// How long the driver will wait for the boot-test cohort to publish its
/// verdict before giving up and reporting a run it never made. Same budget as
/// Component A's driver, for the same reason (`driver_a.rs`'s own doc).
const COHORT_WAIT_BUDGET_NS: u64 = 30_000_000_000;
const COHORT_POLL_NS: u64 = 100_000_000;
const ITERATION_CAP: u64 = 1_000_000;
const CENSUS_CADENCE: u64 = 256;

/// The three markers this component's contract reads. See the module header
/// and rung 2's spec §1 for each one's sole emitter.
struct Baseline {
    cpu_identity_split: u64,
    percpu_stack_alien: u64,
    ret_stage_refusals: u64,
}

fn baseline() -> Baseline {
    Baseline {
        cpu_identity_split: crate::arch_impl::aarch64::percpu::CPU_IDENTITY_SPLIT_EVENTS
            .load(Ordering::Relaxed),
        percpu_stack_alien: crate::arch_impl::aarch64::percpu::PERCPU_STACK_ALIEN_REFUSALS
            .load(Ordering::Relaxed),
        ret_stage_refusals: crate::arch_impl::aarch64::context_switch::RET_STAGE_REFUSALS
            .load(Ordering::Relaxed),
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

const REPORTED_CPU_IDENTITY: u8 = 1 << 0;
const REPORTED_STACK_ALIEN: u8 = 1 << 1;
const REPORTED_RET_STAGE: u8 = 1 << 2;

/// Score the three existing markers against the run's baseline.
///
/// `CPU_IDENTITY_SPLIT` is postcondition 1, M2's own predicate. `RET_STAGE_REFUSED`
/// is postcondition 1's corroborating signal (rung 2's spec §1): when it moves
/// IN THE SAME window as an identity split, that is folded into the identity
/// record's own `detail` (bit 63) rather than filed as a second predicate, per
/// the spec's explicit "reported alongside a finding, not in place of one."
/// When it moves ALONE — a shape the spec does not anticipate — it is still
/// reported as its own finding rather than silently dropped; the project's
/// standing rule is that unexplained evidence is never hidden.
/// `PERCPU_STACK_ALIEN` is postcondition 2's own predicate, read here as a
/// first-class check rather than the side channel Component A's driver used it
/// as.
fn score_existing_markers(
    seed: u64,
    iteration: u64,
    vector: &rng::DrawVector,
    baseline: &Baseline,
    reported: &mut Reported,
) {
    let identity =
        crate::arch_impl::aarch64::percpu::CPU_IDENTITY_SPLIT_EVENTS.load(Ordering::Relaxed);
    let identity_moved = identity != baseline.cpu_identity_split;

    let ret_stage =
        crate::arch_impl::aarch64::context_switch::RET_STAGE_REFUSALS.load(Ordering::Relaxed);
    let ret_stage_moved = ret_stage != baseline.ret_stage_refusals;

    if identity_moved && reported.first(REPORTED_CPU_IDENTITY) {
        let detail = identity | (u64::from(ret_stage_moved) << 63);
        record::violation(
            seed,
            iteration,
            vector,
            "CPU_IDENTITY_SPLIT",
            detail,
            COMPONENT_C,
        );
    }

    let alien =
        crate::arch_impl::aarch64::percpu::PERCPU_STACK_ALIEN_REFUSALS.load(Ordering::Relaxed);
    if alien != baseline.percpu_stack_alien && reported.first(REPORTED_STACK_ALIEN) {
        record::violation(
            seed,
            iteration,
            vector,
            "PERCPU_STACK_ALIEN",
            alien,
            COMPONENT_C,
        );
    }

    if ret_stage_moved && !identity_moved && reported.first(REPORTED_RET_STAGE) {
        record::violation(
            seed,
            iteration,
            vector,
            "RET_STAGE_REFUSED",
            ret_stage,
            COMPONENT_C,
        );
    }
}

fn monotonic_now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// Block until the boot-test cohort has published its verdict. Identical in
/// shape to Component A's driver's own helper — see `driver_a.rs` for why this
/// blocks on a timer rather than spinning.
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

pub fn run() {
    let seed = rng::root_seed();
    let mode = Mode::selected();
    let window = Window::selected(mode);
    let online_cpus = scheduler::online_cpu_count_snapshot();

    // Seed on the wire before the cohort wait — see `driver_a.rs`'s identical
    // reasoning, unchanged here.
    record::emit_seed_line(
        seed,
        crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize,
        mode,
        window,
        online_cpus,
        Phase::Open,
        COMPONENT_C,
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
            COMPONENT_C,
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
        COMPONENT_C,
    );
    let baseline_snapshot = baseline();

    let controller = Controller::begin(
        mode,
        window,
        seed,
        COMPONENT_C,
        driver_cpu,
        victim_tid,
        online_cpus,
    );

    let cadence_vector = rng::draw(seed, COMPONENT_C, driver_cpu as u8, u64::MAX);
    let release_cadence = u64::from(cadence_vector.cycles % 64) + 1;
    let started_at = monotonic_now_ns();
    let mut iterations = 0u64;
    let mut reported = Reported(0);
    super::coverage::open_window();

    while iterations < ITERATION_CAP
        && monotonic_now_ns().saturating_sub(started_at) < WALL_CLOCK_BUDGET_NS
    {
        // Re-arm every online peer's OWN slot at `ScheduleEntry` every cycle.
        // `sites::ALL` names exactly one site for this component, so every
        // draw already targets `ScheduleEntry` — nothing here overrides the
        // drawn site. Each peer's stream is keyed by its own cpu id, so a
        // replay of iteration I arms the same vector on the same peer without
        // re-running any predecessor (rng.rs's own counter-derived contract).
        if !super::loop_disarmed() {
            for cpu in online_peer_cpus(driver_cpu, online_cpus) {
                let vector =
                    stimulus::materialize(rng::draw(seed, COMPONENT_C, cpu as u8, iterations));
                super::arm_cpu(cpu, &vector);
            }
        }

        if iterations % CENSUS_CADENCE == 0 {
            let sample_vector = rng::draw(seed, COMPONENT_C, driver_cpu as u8, iterations);
            score_existing_markers(
                seed,
                iterations,
                &sample_vector,
                &baseline_snapshot,
                &mut reported,
            );
        }

        if iterations % release_cadence == 0 {
            controller.release_and_reform();
        }
        if window == Window::Overlap {
            scheduler::yield_current();
        }
        iterations = iterations.wrapping_add(1);
    }
    super::coverage::close_window();

    for cpu in online_peer_cpus(driver_cpu, online_cpus) {
        super::disarm_cpu(cpu);
    }

    // One closing sweep, so a marker that moved after the last cadence tick is
    // scored rather than dropped on the floor — Component A's driver does the
    // same for the same reason.
    let closing_vector = rng::draw(
        seed,
        COMPONENT_C,
        driver_cpu as u8,
        iterations.saturating_sub(1),
    );
    score_existing_markers(
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
        COMPONENT_C,
    );
}
