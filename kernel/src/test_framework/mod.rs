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

/// Wall-clock budget for the initialization watchdog.
///
/// This watchdog bounds pre-test initialization only -- concretely, the
/// aarch64 secondary-CPU bring-up wait in `main_aarch64.rs`. It is anchored at
/// kernel entry. It is deliberately equal to that wait's own `boot_tests`
/// absolute ceiling (`SMP_ONLINE_ABSOLUTE_CEILING_SECONDS = 40`), so the shared
/// watchdog is not a tighter bound on bring-up than the local ceiling already
/// in force, so a live-but-slow CPU is given up on no sooner than before.
/// claim-lint:ok: #522 C5; the two constants are pinned in
/// tests/teardown_structure.rs and read in docs/planning/green-program/tracing/522-C5-2026-09-07.md.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub const INITIALIZATION_WATCHDOG_BUDGET_MILLISECONDS: u64 = 40_000;

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
/// overrun is classified as the gate ceiling rather than pre-empted here.
///
/// Neither of the two budgets this pair replaces the previous single 65-second
/// clock with is larger than that clock. They are sequential rather than
/// shared: initialization is bounded by its own 40-second watchdog, and the
/// test phase by this one. In the pathological case where both run to
/// exhaustion the strict script's own 90-second `timeout` is the outer backstop,
/// as it already is for any boot whose deliberate test waits run long.
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
