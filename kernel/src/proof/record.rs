//! The two marker lines the harness puts on the serial.
//!
//! Both use the tree's existing bracketed-marker convention, so the gate
//! scripts' classifiers consume them exactly as they consume
//! `[FUTEX_HANDOFF_ORACLE:...]` and `[PERCPU_STACK_CUSTODY_ORACLE:...]`.
//!
//! ```text
//! [COREPROOF:RUN:v1:comp=A:phase=close:mut=none:seed=0x...:dcpu=N:iters=N:sites_declared=N:
//!  sites_visited=N:mode=pen:window=post_cohort:disarmed=0:degraded=0:profile=...:
//!  smp=N:downgraded=N:fire_dropped=N:violated_predicates=N:cov=name=N,...]
//! [COREPROOF:VIOLATION:v1:comp=A:seed=0x...:iter=N:site=...:action=...:ticks=N:
//!  order=before:acpu=N:pred=...:detail=N:fired_cpu=N:fired_iter=N]
//! ```
//!
//! Up to three RUN records per run, each naming its own `phase`, and the first
//! one matters: `phase=open` is emitted BEFORE the cohort wait — before anything
//! in the boot that can die — so a boot which never reaches its window still
//! carries its seed. Its `dcpu` is PROVISIONAL, because the driver is spawned
//! unpinned and sleeps through that wait; `phase=settled` repeats the seed with
//! the authoritative draw domain once the window is about to open, and
//! `phase=close` carries the achieved counts.
//!
//! The phase field is what the gate keys on, in both of its jobs: it waits for
//! `phase=close` to know the run finished, and it adjudicates that record. It
//! exists because counting RUN lines cannot tell "the run is over" from "the run
//! is about to start" — a gate that counted them killed the guest the instant
//! the settled record appeared and then adjudicated a run of zero iterations as
//! if that were the result.
//!
//! `degraded=1` says a rendezvous in the pen exhausted its budget and the run
//! fell back to ambient. It is reported rather than hidden: a degraded run is a
//! weaker measurement than a penned one and must not be read as the same thing.
//!
//! `iters` is a MEASURED output, never a target. The leverage arithmetic in the
//! design is judged against whatever number appears here.
//!
//! `violated_predicates` is named for what it counts: at most one record is
//! emitted per predicate per run, so this is the number of DISTINCT predicates
//! that fired, not a tally of occurrences. Calling it `violations` would have
//! read as a count and quietly understated a run where one predicate fired
//! thousands of times. The gate keys on the presence of any VIOLATION line, so
//! nothing is lost by reporting first occurrences only, and the serial stays
//! readable.
//!
//! ## `component`, threaded rather than hardcoded
//!
//! Rung 1 hardcoded `comp=A` in both format strings. Rung 2 adds Component C,
//! whose runs must say `comp=C` in the same two records, so every public
//! function here now takes a `component: u8` byte (`driver_a::COMPONENT_A` /
//! `driver_c::COMPONENT_C`) and prints it as the one ASCII character it is.
//! There is deliberately no third notion of "which component" invented here —
//! the byte is the driver's own public constant, passed straight through.

use core::sync::atomic::{AtomicU64, Ordering};

use super::quiesce::{Mode, Window};
use super::rng::DrawVector;
use super::sites;
use super::stimulus;

static VIOLATIONS: AtomicU64 = AtomicU64::new(0);

/// The CPU profile this boot is running under.
///
/// `platform_config` distinguishes hypervisors (QEMU / Parallels / VMware) but
/// nothing in the tree reads MIDR_EL1 to tell `-cpu max` from `-cpu cortex-a72`,
/// which is the distinction the gate matrix cares about. Reporting the
/// hypervisor and leaving the model as `unknown` is the honest answer: the gate
/// script knows which profile it launched, and a fabricated model in the record
/// would be worse than an absent one.
fn profile() -> &'static str {
    if crate::platform_config::is_parallels() {
        "parallels"
    } else if crate::platform_config::is_qemu() {
        "qemu-unknown-model"
    } else {
        "unknown"
    }
}

/// Which of a run's records this is. See the module header.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Before the cohort wait; the seed is on the wire, `dcpu` is provisional.
    Open,
    /// The window is about to open; the draw domain is now authoritative.
    Settled,
    /// The run is over; every field is final and this is the adjudicated record.
    Close,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Settled => "settled",
            Self::Close => "close",
        }
    }
}

/// Put the seed on the wire before the first iteration.
#[allow(clippy::too_many_arguments)]
pub fn emit_seed_line(
    seed: u64,
    driver_cpu: usize,
    mode: Mode,
    window: Window,
    smp: usize,
    phase: Phase,
    component: u8,
) {
    emit_run(seed, driver_cpu, 0, mode, window, smp, phase, component);
}

#[allow(clippy::too_many_arguments)]
pub fn emit_run(
    seed: u64,
    driver_cpu: usize,
    iterations: u64,
    mode: Mode,
    window: Window,
    smp: usize,
    phase: Phase,
    component: u8,
) {
    let coverage = super::coverage::counts();
    crate::serial_println!(
        "[COREPROOF:RUN:v1:comp={}:phase={}:mut={}:seed=0x{:016x}:dcpu={}:iters={}:sites_declared={}:sites_visited={}:mode={}:window={}:disarmed={}:degraded={}:profile={}:smp={}:downgraded={}:fire_dropped={}:violated_predicates={}:cov={}]",
        component as char,
        phase.name(),
        super::mutations::armed_name(),
        seed,
        driver_cpu,
        iterations,
        sites::DECLARED,
        sites::visited_count(),
        mode.name(),
        window.name(),
        u8::from(super::loop_disarmed()),
        u8::from(super::quiesce::degraded()),
        profile(),
        smp,
        stimulus::downgraded_count(),
        super::fire_dropped_count(),
        violated_predicate_count(),
        super::coverage::display_counts(&coverage),
    );
}

/// Emit one violation record naming its site, action, order, predicate, and optional
/// cross-CPU fire attribution.
///
/// `fired_cpu`/`fired_iter` are emitted as `u64::MAX` when the violation has no cross-CPU
/// fire to attribute. Component A's self-armed synchronous predicates always pass
/// `None, None`: there is no cross-CPU gap to bridge for them.
#[allow(clippy::too_many_arguments)]
pub fn violation(
    seed: u64,
    iteration: u64,
    vector: &DrawVector,
    predicate: &str,
    detail: u64,
    component: u8,
    fired_cpu: Option<usize>,
    fired_iter: Option<u64>,
) {
    VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[COREPROOF:VIOLATION:v1:comp={}:seed=0x{:016x}:iter={}:site={}:action={}:ticks={}:order={}:acpu={}:pred={}:detail={}:fired_cpu={}:fired_iter={}]",
        component as char,
        seed,
        iteration,
        vector.site.name(),
        stimulus::effective_action(vector).name(),
        vector.ticks,
        vector.order.name(),
        vector.antagonist_cpu,
        predicate,
        detail,
        fired_cpu.map(|cpu| cpu as u64).unwrap_or(u64::MAX),
        fired_iter.unwrap_or(u64::MAX),
    );
}

/// Distinct predicates that have fired this run. See the module header.
pub fn violated_predicate_count() -> u64 {
    VIOLATIONS.load(Ordering::Relaxed)
}
