//! Parallel boot test framework
//!
//! Runs kernel tests concurrently during boot, one kthread per subsystem.
//! Progress is tracked via atomic counters and displayed graphically.
//!
//! # Architecture
//!
//! The framework consists of four main components:
//!
//! - **Registry**: Static test definitions organized by subsystem
//! - **Executor**: Spawns kthreads to run tests in parallel
//! - **Progress**: Lock-free atomic counters for tracking completion
//! - **Display**: Graphical progress bars rendered to framebuffer
//!
//! # Usage
//!
//! Tests are registered statically in `registry.rs`. During boot, call
//! `run_all_tests()` to spawn test kthreads. The display module renders
//! real-time progress bars to the framebuffer if available.

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Published floor, in milliseconds, for the initialization watchdog.
///
/// This watchdog bounds pre-test initialization only -- concretely, the
/// aarch64 secondary-CPU bring-up wait in `main_aarch64.rs`. It is anchored at
/// kernel entry, strictly before that wait's own `start` (`timer::rdtsc()` at
/// the top of the bring-up loop), because real initialization work -- memory
/// management, the generic timer, the RTC, the GIC -- runs in between. Because
/// the anchors differ, a watchdog of exactly this constant's magnitude, timed
/// from kernel entry, reaches its deadline strictly before a same-magnitude
/// local ceiling timed from the later `start` would: anchoring earlier at
/// equal magnitude is definitionally a tighter bound, not an equal one, for
/// any nonzero gap between the two anchors. #522 C5 review round 1 published
/// this constant at the SAME magnitude as the local ceiling
/// (`SMP_ONLINE_ABSOLUTE_CEILING_SECONDS`) and claimed the two were therefore
/// equally tight; that claim was false in four places (this docstring, the
/// `main_aarch64.rs` bring-up comment, the round doc, and the round's commit
/// message), and its practical effect was that the local ceiling's own
/// `[smp] Timeout waiting for CPUs: absolute ceiling ...` diagnostic became
/// unreachable in every `boot_tests` boot, silently narrowing a live-but-slow
/// CPU's promised local window by the (unmeasured, undisclosed) gap between
/// the two anchors. claim-lint:ok: #522 C5 fix pass V-1; reproduced live in
/// `tests/teardown_structure.rs::fix_pass_v1_watchdog_not_tighter_than_local_ceiling`'s
/// mutation transcript, which reddens on reintroducing this exact claim.
///
/// This constant is therefore a FLOOR, not the watchdog's whole story: the
/// bring-up wait in `main_aarch64.rs` computes the watchdog's real, per-boot
/// ceiling as `max(this constant, local_absolute_ceiling + measured_gap +
/// margin)`, where `measured_gap` is the actual elapsed time (via `rdtsc`)
/// between the kernel-entry anchor and this wait's own `start`, sampled fresh
/// every boot rather than assumed. That construction guarantees, for any
/// measured gap, that the watchdog's deadline is never earlier than the local
/// ceiling's own deadline -- so the local ceiling stays reachable and a
/// live-but-slow CPU keeps its full local window regardless of how long
/// earlier initialization took. This constant's value is sized so that, for
/// the gap this repository actually observes, the `max(...)` rarely needs to
/// extend past it; see `INITIALIZATION_WATCHDOG_LOCAL_CEILING_MARGIN_SECONDS`
/// in `main_aarch64.rs` for the margin and the `[smp] initialization_watchdog`
/// breadcrumb for the per-boot measured value.
/// claim-lint:ok: #522 C5 fix pass (V-1); the constant, the local ceiling it
/// must not undercut, and the `max(...)` construction are pinned in
/// tests/teardown_structure.rs.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub const INITIALIZATION_WATCHDOG_BUDGET_MILLISECONDS: u64 = 20_000;

/// Wall-clock budget for the boot-test phase's own liveness watchdog waits.
///
/// This budget is anchored at test-phase entry (`run_all_tests`), NOT at kernel
/// entry, so however long initialization took, the test phase's watchdogs still
/// get their promised local allowance.
///
/// It exceeds the P20 exit-kick gate's own 45-second gate ceiling by fifteen
/// seconds. That headroom is what makes the gate's local allowance real: the
/// gate is entered part-way into the test phase, and the measured distance from
/// test-phase entry to gate entry is about 2.4-2.7 seconds, so the remaining
/// budget still covers the full 45-second ceiling and a genuine whole-gate
/// overrun is classified as the gate ceiling rather than pre-empted here. The
/// same budget is also spent, sequentially after the real gate, by
/// `exit_kick_worker_window_isolation_test` (about 39 seconds of deliberate
/// per-scenario ceilings) and `exit_kick_budget_anchor_isolation_test` (about
/// 11 seconds), so a healthy boot's real consumption of this budget is closer
/// to 52 seconds than to the real gate's own 2-3 seconds; this value is not
/// free to shrink much further without risking a healthy boot's own deliberate
/// self-tests running into the enclosing ceiling.
///
/// Neither of the two budgets this pair replaces the previous single 65-second
/// clock with is larger than that clock: the pre-#522-C5-fix-pass pair (a
/// 40,000ms initialization watchdog plus this 60,000ms test-phase budget)
/// summed to 100,000ms, past the strict harness's 90,000ms hard timeout
/// (`docker/qemu/run-aarch64-boot-test-strict.sh`'s `BREENIX_STRICT_TIMEOUT_SECONDS`).
/// #522 C5 review round 1 (V-3) found that a pathological boot that exhausted
/// both watchdogs in full would therefore be killed by the harness's own
/// external timeout -- an unattributed `ended_by=hard_timeout` -- rather than
/// reaching either watchdog's own attributed in-kernel verdict, reproducing
/// the same class of failure the harness's `ended_by=hard_timeout` boot-facts
/// field already exists to name. `INITIALIZATION_WATCHDOG_BUDGET_MILLISECONDS`
/// was reduced from 40,000 to 20,000ms (with
/// `SMP_ONLINE_ABSOLUTE_CEILING_SECONDS` correspondingly reduced from 40 to 15
/// in `main_aarch64.rs`, still 1.5x the documented realistic worst case and
/// 1.875x the longest individually observed wait) so the pair now sums to
/// 80,000ms, ten full seconds inside the 90,000ms hard timeout.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub const TEST_PHASE_LIVENESS_BUDGET_MILLISECONDS: u64 = 60_000;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static INITIALIZATION_WATCHDOG_STARTED_AT: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static INITIALIZATION_WATCHDOG_HAS_STARTED: AtomicBool = AtomicBool::new(false);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static TEST_PHASE_LIVENESS_STARTED_AT: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static TEST_PHASE_LIVENESS_HAS_STARTED: AtomicBool = AtomicBool::new(false);

/// Publish the initialization watchdog anchor once, on the boot CPU.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn begin_initialization_watchdog(started_at: u64) {
    // `kernel_main` calls this once before SMP release, so there is one writer.
    // Keep the separate release flag because CNTVCT may legitimately read zero.
    if INITIALIZATION_WATCHDOG_HAS_STARTED.load(Ordering::Acquire) {
        return;
    }
    INITIALIZATION_WATCHDOG_STARTED_AT.store(started_at, Ordering::Relaxed);
    INITIALIZATION_WATCHDOG_HAS_STARTED.store(true, Ordering::Release);
}

/// Return the immutable initialization watchdog anchor after it is published.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn initialization_watchdog_started_at() -> Option<u64> {
    if INITIALIZATION_WATCHDOG_HAS_STARTED.load(Ordering::Acquire) {
        Some(INITIALIZATION_WATCHDOG_STARTED_AT.load(Ordering::Relaxed))
    } else {
        None
    }
}

/// Publish the test-phase liveness anchor once, at test-phase entry.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn begin_test_phase_liveness_budget(started_at: u64) {
    // `run_all_tests` calls this once, before any test kthread is spawned.
    if TEST_PHASE_LIVENESS_HAS_STARTED.load(Ordering::Acquire) {
        return;
    }
    TEST_PHASE_LIVENESS_STARTED_AT.store(started_at, Ordering::Relaxed);
    TEST_PHASE_LIVENESS_HAS_STARTED.store(true, Ordering::Release);
}

/// Return the immutable test-phase liveness anchor after it is published.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn test_phase_liveness_started_at() -> Option<u64> {
    if TEST_PHASE_LIVENESS_HAS_STARTED.load(Ordering::Acquire) {
        Some(TEST_PHASE_LIVENESS_STARTED_AT.load(Ordering::Relaxed))
    } else {
        None
    }
}

#[cfg(feature = "boot_tests")]
pub mod display;
#[cfg(feature = "boot_tests")]
pub mod executor;
#[cfg(feature = "boot_tests")]
pub mod progress;
#[cfg(feature = "boot_tests")]
pub mod registry;

#[cfg(feature = "boot_tests")]
pub use display::{init as init_display, is_ready as display_ready, render_progress};
#[cfg(feature = "boot_tests")]
pub use executor::{
    advance_stage_marker_only, advance_to_stage, current_stage, emit_exec_lock_order_counters,
    run_all_tests,
};
#[cfg(feature = "boot_tests")]
pub use progress::get_overall_progress;
#[cfg(feature = "boot_tests")]
pub use registry::TestStage;

// BTRT (Boot Test Result Table) modules
#[cfg(feature = "btrt")]
pub mod btrt;
#[cfg(feature = "btrt")]
pub mod catalog;
#[cfg(feature = "btrt")]
pub mod ktap;
