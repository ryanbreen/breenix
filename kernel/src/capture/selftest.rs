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
/// Per architecture, and both numbers are MEASURED rather than chosen. The
/// requirement pulls two ways: late enough that the boot's own serial traffic
/// has quietened, because the capture is ~4.5 KB written a byte at a time by
/// a lock-free writer that a peer CPU's locked `serial_println!` can
/// interleave into (#847), and early enough that the profile actually reaches
/// it.
///
/// **aarch64: 3000 ms.** 13 of 13 strict-gate boots at this checkpoint
/// produced an uncorrupted `[BXCAP:END ...]` line. At 1100 ms, tried first for symmetry
/// with x86, 1 of 4 boots had another CPU's `[TEST:...]` line spliced into the
/// middle of the `END` record and a second boot lost PR-2's `[RING_SPAN:` line
/// the same way -- the gate's own #847 signature. The aarch64 gate is `-smp 4`
/// and its boot-test output storm is still running at 1100 ms.
///
/// **x86_64: 1100 ms.** The x86 `boot_tests` profile does not reach 3000 ms: a
/// `boot_tests,testing,external_test_bins,capture_selftest` build on the beast
/// container emitted 0 `[BXCAP:` bytes across both of
/// `run-x86-boot-tests.sh`'s serials, while the same build's
/// `[RING_SPAN:cpu=0:...]` line -- whose checkpoint is 1000 ms -- was present.
/// x86 ticks at `PIT_HZ` = 200 Hz (`MS_PER_TICK` = 5), so 3000 ms is tick 600
/// and 1100 ms is tick 220. The interleaving that ruled 1100 ms out on aarch64
/// does not arise there: both x86 gates run `-smp 1`, so there is no peer CPU
/// writing the same UART.
///
/// The round doc's section 6.7 records both measurements.
#[cfg(target_arch = "aarch64")]
const CAPTURE_AT_MS: u64 = 3_000;

#[cfg(not(target_arch = "aarch64"))]
const CAPTURE_AT_MS: u64 = 1_100;

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
