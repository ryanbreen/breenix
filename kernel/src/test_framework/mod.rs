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

#[cfg(feature = "boot_tests")]
pub mod registry;
#[cfg(feature = "boot_tests")]
pub mod executor;
#[cfg(feature = "boot_tests")]
pub mod progress;
#[cfg(feature = "boot_tests")]
pub mod display;

#[cfg(feature = "boot_tests")]
pub use executor::{run_all_tests, advance_to_stage, advance_stage_marker_only, current_stage};
#[cfg(feature = "boot_tests")]
pub use registry::TestStage;
#[cfg(feature = "boot_tests")]
pub use progress::get_overall_progress;
#[cfg(feature = "boot_tests")]
pub use display::{init as init_display, render_progress, is_ready as display_ready};
