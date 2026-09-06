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
/// MEASURED, and one value for both architectures. The requirement pulls two
/// ways: late enough that the boot's own serial traffic has quietened,
/// because the capture is ~4.5 KB written a byte at a time by a lock-free
/// writer that a peer CPU's locked `serial_println!` can interleave into
/// (#847), and early enough that the profile actually reaches it.
///
/// 3000 ms satisfies both, measured on each architecture:
///
/// * aarch64, `run-aarch64-boot-test-strict.sh` with
///   `boot_tests,capture_selftest`: 15 of 15 boots across this round produced
///   an uncorrupted `[BXCAP:END ...]` line (those boots span intermediate
///   revisions of the emitter; what the checkpoint is being judged on is the
///   interleaving, which is a property of the boot's serial traffic). 1100 ms was tried first and rejected -- of 4
///   boots there, 1 had another CPU's `[TEST:...]` line spliced into the
///   middle of the `END` record and a second lost PR-2's `[RING_SPAN:` line
///   the same way. That gate is `-smp 4` and its boot-test output storm is
///   still running at 1100 ms.
/// * x86_64, `run-x86-boot-tests.sh` with the same feature added: a complete
///   capture, `verdict=complete records=76 bytes=4424 truncated=0
///   sections_skipped=0x0`, at `uptime_ms=3000`. That gate is `-smp 1`, so
///   the interleaving above does not arise there.
///
/// The x86 measurement needed the gate's own feature list patched for the
/// run: `run-x86-boot-tests.sh` REBUILDS the kernel with a hard-coded
/// `--features boot_tests,testing,external_test_bins`, discarding whatever
/// was built before it, where `run-aarch64-boot-test-strict.sh` validates the
/// kernel it is handed. An earlier revision of this file recorded "the x86
/// boot_tests profile does not reach 3000 ms" on the strength of an
/// unpatched run; that was an artifact of the rebuild, not a property of the
/// profile, and the round doc's section 6.7 carries the correction.
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
