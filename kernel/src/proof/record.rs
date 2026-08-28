//! The two marker lines the harness puts on the serial.
//!
//! Both use the tree's existing bracketed-marker convention, so the gate
//! scripts' classifiers consume them exactly as they consume
//! `[FUTEX_HANDOFF_ORACLE:...]` and `[PERCPU_STACK_CUSTODY_ORACLE:...]`.
//!
//! ```text
//! [COREPROOF:RUN:v1:comp=A:mut=none:seed=0x...:iters=N:sites_declared=N:sites_visited=N:
//!  mode=pen:degraded=0:profile=...:smp=N:downgraded=N:violated_predicates=N]
//! [COREPROOF:VIOLATION:v1:comp=A:seed=0x...:iter=N:site=...:action=...:ticks=N:
//!  order=before:acpu=N:pred=...:detail=N]
//! ```
//!
//! Two RUN records per run, and the first one matters: it is emitted BEFORE the
//! first iteration so that a boot which dies mid-flight still carries its seed.
//! The gate adjudicates the last record, which is the closing one.
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

use core::sync::atomic::{AtomicU64, Ordering};

use super::quiesce::Mode;
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

/// Put the seed on the wire before the first iteration.
pub fn emit_seed_line(seed: u64, mode: Mode, smp: usize) {
    emit_run(seed, 0, mode, smp);
}

pub fn emit_run(seed: u64, iterations: u64, mode: Mode, smp: usize) {
    crate::serial_println!(
        "[COREPROOF:RUN:v1:comp=A:mut={}:seed=0x{:016x}:iters={}:sites_declared={}:sites_visited={}:mode={}:degraded={}:profile={}:smp={}:downgraded={}:violated_predicates={}]",
        super::mutations::armed_name(),
        seed,
        iterations,
        sites::DECLARED,
        sites::visited_count(),
        mode.name(),
        u8::from(super::quiesce::degraded()),
        profile(),
        smp,
        stimulus::downgraded_count(),
        violated_predicate_count(),
    );
}

/// Emit one violation record naming its site, action, order and predicate.
pub fn violation(seed: u64, iteration: u64, vector: &DrawVector, predicate: &str, detail: u64) {
    VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[COREPROOF:VIOLATION:v1:comp=A:seed=0x{:016x}:iter={}:site={}:action={}:ticks={}:order={}:acpu={}:pred={}:detail={}]",
        seed,
        iteration,
        vector.site.name(),
        stimulus::effective_action(vector).name(),
        vector.ticks,
        vector.order.name(),
        vector.antagonist_cpu,
        predicate,
        detail,
    );
}

/// Distinct predicates that have fired this run. See the module header.
pub fn violated_predicate_count() -> u64 {
    VIOLATIONS.load(Ordering::Relaxed)
}
