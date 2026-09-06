//! The deterministic `edge=PANIC` terminal-edge oracle.
//!
//! Behind `--features capture_panic_oracle`, which no gate builds with, so
//! this module is not compiled into a shipped or gated kernel. Its purpose
//! is to make PR-4's claim -- "a kernel panic emits a complete `BXCAP`
//! record" -- testable on demand, on both architectures, instead of waiting
//! for a real panic.
//!
//! # Why it lives here and not under `kernel/src/capture/`
//!
//! `kernel/src/capture/` is a critical-path directory: both
//! `scripts/check-critical-path-violations.sh` and
//! `tests/capture_path_lock_free_structure.rs` forbid `panic!` there,
//! because a panic raised from inside a capture re-enters the very path the
//! capture is reporting from. This module's whole job is to raise one, so it
//! is a sibling of that directory rather than a member of it. The denylist
//! is not being worked around: the emitter still may not panic, and no code
//! in this module runs inside it.
//!
//! # Why the timer tick
//!
//! Same two reasons `capture::selftest` fires from `trace_timer_tick`: the
//! path exists on both architectures at the same point in boot, and it needs
//! no scheduler to be running to spawn onto. It also puts the panic in
//! hard-IRQ context, which is where the aarch64 EL1-fault family and the
//! x86 double-fault/GPF panics this PR is aimed at actually originate.
//!
//! That last property has a cost, and it is the cost the round doc discloses
//! rather than hides: the panic handler's own banner is a
//! `serial_println!`, which takes `SERIAL1`'s spin mutex. A panic raised
//! from a timer interrupt that preempted a holder of that lock ON THE SAME
//! CPU deadlocks in the banner, before the capture is reached. That hazard
//! is the panic handler's, not this oracle's -- it predates the capture and
//! the capture is placed after the banner deliberately -- but this oracle
//! is the thing most likely to meet it, so it is stated here too.

use core::sync::atomic::{AtomicBool, Ordering};

/// Guest uptime at which the oracle panics, in milliseconds.
///
/// The same checkpoint `capture::selftest` uses, for the reason recorded in
/// `kernel/src/capture/selftest.rs`: it is late enough that the boot's own
/// serial traffic has quietened and early enough that both architectures'
/// `boot_tests` profiles reach it. The two features are independent and
/// neither implies the other, so a build fires at most one of them.
const PANIC_AT_MS: u64 = 3_000;

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
    if elapsed_ms < PANIC_AT_MS {
        return;
    }
    if FIRED.swap(true, Ordering::AcqRel) {
        return;
    }
    fire(elapsed_ms, tick_count);
}

/// The panic itself, out of line so the tick path carries only the latch.
///
/// The message names the oracle so a reader of a serial log can tell this
/// panic from a real one at a glance, and carries the two numbers the
/// `[BXCAP:EDGE]` record cannot: this is the only place they are formatted,
/// and it is not on the capture path.
#[cold]
#[inline(never)]
fn fire(elapsed_ms: u64, tick_count: u64) -> ! {
    panic!("capture_panic_oracle: deliberate panic at uptime_ms={elapsed_ms} tick={tick_count}");
}
