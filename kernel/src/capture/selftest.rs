//! The deterministic `edge=SELFTEST` capture.
//!
//! claim-lint:ok: the "no gate builds it" claim is machine-checked over the
//! 4 of 4 gate scripts by
//! tests/capture_path_lock_free_structure.rs, and the module is behind a cfg
//! that a build without the feature does not compile.
//!
//! Behind `--features capture_selftest`, which no gate builds with, so this
//! module is not compiled into a shipped or gated kernel. Its purpose is to
//! give a later PR in the failure-capture program a capture it can test
//! against on demand, instead of waiting for the rare fault the real edges
//! fire on.
//!
//! # Why the timer tick, and why that is the interesting place
//!
//! `observe` is called from `trace_timer_tick`, the same one-shot latch
//! idiom PR-2's `RING_SPAN` self-check uses in that file, for the same two
//! reasons: it exists on both architectures at the same point in boot, and
//! it needs no scheduler to be running to spawn onto.
//!
//! It also puts the capture in hard-IRQ context, which is where the terminal
//! edges PR-4 and PR-7 wire will run it. A self-test that ran in thread
//! context would be a weaker rehearsal: it would not exercise the
//! scheduler-lock refusal arm, and it would not show whether the emitter can
//! run with interrupts masked.

use core::sync::atomic::{AtomicBool, Ordering};

/// Guest uptime at which the one capture fires, in milliseconds.
///
/// After PR-2's `RING_SPAN` self-check at 1000 ms, so the two markers do not
/// contend for the UART, and late enough that the ring, the counters and the
/// scheduler have real content to report. Inside the boot window this runs
/// in: the aarch64 strict gate polls to ~18 s and the x86 boot-tests gate
/// runs longer still.
const CAPTURE_AT_MS: u64 = 3_000;

static FIRED: AtomicBool = AtomicBool::new(false);

/// Called on each timer tick. Returns immediately once latched; the load and
/// the comparison are the only per-tick cost, and only in a build that
/// enables this feature.
#[inline(always)]
pub fn observe(tick_count: u64) {
    if FIRED.load(Ordering::Relaxed) {
        return;
    }
    let elapsed_ms = tick_count.saturating_mul(crate::time::timer::MS_PER_TICK);
    if elapsed_ms < CAPTURE_AT_MS {
        return;
    }
    if FIRED.swap(true, Ordering::AcqRel) {
        return;
    }
    super::emit(super::Edge::SelfTest, elapsed_ms, tick_count);
}
