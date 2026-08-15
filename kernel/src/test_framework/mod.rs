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

/// Shared wall-clock budget for Phase-1 liveness watchdog waits.
///
/// The external full-test harness allows 90 seconds for Phase 1. Capping the
/// P17/P20 pre-`[BOOT_TESTS:PASS]` liveness waits at 65 seconds from one early
/// kernel anchor reserves 90s - 65s = 25s for loader/process overhead,
/// initialization, and the rest of the boot-test suite.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub const PHASE_ONE_LIVENESS_BUDGET_MILLISECONDS: u64 = 65_000;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static PHASE_ONE_LIVENESS_STARTED_AT: AtomicU64 = AtomicU64::new(0);
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
static PHASE_ONE_LIVENESS_HAS_STARTED: AtomicBool = AtomicBool::new(false);

/// Publish the shared Phase-1 liveness anchor once, on the boot CPU.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn begin_phase_one_liveness_budget(started_at: u64) {
    // `kernel_main` calls this once before SMP release, so there is one writer.
    // Keep the separate release flag because CNTVCT may legitimately read zero.
    if PHASE_ONE_LIVENESS_HAS_STARTED.load(Ordering::Acquire) {
        return;
    }
    PHASE_ONE_LIVENESS_STARTED_AT.store(started_at, Ordering::Relaxed);
    PHASE_ONE_LIVENESS_HAS_STARTED.store(true, Ordering::Release);
}

/// Return the immutable Phase-1 liveness anchor after it has been published.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
pub fn phase_one_liveness_started_at() -> Option<u64> {
    if PHASE_ONE_LIVENESS_HAS_STARTED.load(Ordering::Acquire) {
        Some(PHASE_ONE_LIVENESS_STARTED_AT.load(Ordering::Relaxed))
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
