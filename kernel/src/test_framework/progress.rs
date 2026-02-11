//! Lock-free progress tracking for parallel test execution
//!
//! Uses atomic counters to track test completion without requiring locks,
//! which is essential since test kthreads run concurrently with potential
//! timer interrupts.
//!
//! Tracks per-stage completion counts for color-coded progress display.

use core::sync::atomic::{AtomicU32, Ordering};
use super::registry::{SubsystemId, TestStage};

/// Progress counters for a single subsystem
///
/// All fields are atomic to allow lock-free updates from test kthreads
/// and lock-free reads from the display thread.
struct SubsystemProgress {
    /// Number of tests completed (pass or fail)
    completed: AtomicU32,
    /// Total number of tests in this subsystem
    total: AtomicU32,
    /// Number of tests that failed
    failed: AtomicU32,
    /// Whether this subsystem has started executing
    started: AtomicU32, // Using u32 for alignment, 0 = false, 1 = true
    /// Per-stage completion counts for color-coded display
    /// Index by TestStage as usize
    stage_completed: [AtomicU32; TestStage::COUNT],
    /// Per-stage total counts
    stage_total: [AtomicU32; TestStage::COUNT],
}

impl SubsystemProgress {
    const fn new() -> Self {
        Self {
            completed: AtomicU32::new(0),
            total: AtomicU32::new(0),
            failed: AtomicU32::new(0),
            started: AtomicU32::new(0),
            stage_completed: [
                AtomicU32::new(0), // EarlyBoot
                AtomicU32::new(0), // PostScheduler
                AtomicU32::new(0), // ProcessContext
                AtomicU32::new(0), // Userspace
            ],
            stage_total: [
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
                AtomicU32::new(0),
            ],
        }
    }
}

/// Static array of progress counters, one per subsystem
///
/// Index by `SubsystemId as usize` to get the corresponding progress.
static PROGRESS: [SubsystemProgress; SubsystemId::COUNT] = [
    SubsystemProgress::new(), // Memory
    SubsystemProgress::new(), // Scheduler
    SubsystemProgress::new(), // Interrupts
    SubsystemProgress::new(), // Filesystem
    SubsystemProgress::new(), // Network
    SubsystemProgress::new(), // Ipc
    SubsystemProgress::new(), // Process
    SubsystemProgress::new(), // Syscall
    SubsystemProgress::new(), // Timer
    SubsystemProgress::new(), // Logging
    SubsystemProgress::new(), // System
];

/// Initialize progress counters for a subsystem
///
/// Called by the executor before spawning the test kthread.
pub fn init_subsystem(id: SubsystemId, total_tests: u32) {
    let idx = id as usize;
    PROGRESS[idx].total.store(total_tests, Ordering::Release);
    PROGRESS[idx].completed.store(0, Ordering::Release);
    PROGRESS[idx].failed.store(0, Ordering::Release);
    PROGRESS[idx].started.store(0, Ordering::Release);
    // Reset per-stage counters
    for stage_idx in 0..TestStage::COUNT {
        PROGRESS[idx].stage_completed[stage_idx].store(0, Ordering::Release);
        PROGRESS[idx].stage_total[stage_idx].store(0, Ordering::Release);
    }
}

/// Initialize per-stage totals for a subsystem
///
/// Called by the executor to set up stage-specific test counts.
pub fn init_subsystem_stage(id: SubsystemId, stage: TestStage, count: u32) {
    let idx = id as usize;
    let stage_idx = stage as usize;
    PROGRESS[idx].stage_total[stage_idx].store(count, Ordering::Release);
}

/// Mark a subsystem as started
///
/// Called by the kthread when it begins executing tests.
pub fn mark_started(id: SubsystemId) {
    let idx = id as usize;
    PROGRESS[idx].started.store(1, Ordering::Release);
}

/// Increment the completed counter for a subsystem
///
/// Called after each test finishes (regardless of pass/fail).
pub fn increment_completed(id: SubsystemId) {
    let idx = id as usize;
    PROGRESS[idx].completed.fetch_add(1, Ordering::AcqRel);
}

/// Increment the completed counter for a specific stage
///
/// Called after each test finishes to track per-stage progress.
pub fn increment_stage_completed(id: SubsystemId, stage: TestStage) {
    let idx = id as usize;
    let stage_idx = stage as usize;
    PROGRESS[idx].stage_completed[stage_idx].fetch_add(1, Ordering::AcqRel);
}

/// Increment the failed counter for a subsystem
///
/// Called when a test fails, times out, or panics.
pub fn mark_failed(id: SubsystemId) {
    let idx = id as usize;
    PROGRESS[idx].failed.fetch_add(1, Ordering::AcqRel);
}

/// Get progress for a specific subsystem
///
/// Returns (completed, total, failed) tuple.
pub fn get_progress(id: SubsystemId) -> (u32, u32, u32) {
    let idx = id as usize;
    let completed = PROGRESS[idx].completed.load(Ordering::Acquire);
    let total = PROGRESS[idx].total.load(Ordering::Acquire);
    let failed = PROGRESS[idx].failed.load(Ordering::Acquire);
    (completed, total, failed)
}

/// Get per-stage progress for a subsystem
///
/// Returns array of (completed, total) for each stage.
/// Index by TestStage as usize.
pub fn get_stage_progress(id: SubsystemId) -> [(u32, u32); TestStage::COUNT] {
    let idx = id as usize;
    let mut result = [(0u32, 0u32); TestStage::COUNT];
    for stage_idx in 0..TestStage::COUNT {
        result[stage_idx] = (
            PROGRESS[idx].stage_completed[stage_idx].load(Ordering::Acquire),
            PROGRESS[idx].stage_total[stage_idx].load(Ordering::Acquire),
        );
    }
    result
}

/// Check if a subsystem has started executing
pub fn is_started(id: SubsystemId) -> bool {
    let idx = id as usize;
    PROGRESS[idx].started.load(Ordering::Acquire) != 0
}

/// Check if a subsystem has completed all tests
pub fn is_complete(id: SubsystemId) -> bool {
    let (completed, total, _) = get_progress(id);
    total > 0 && completed >= total
}

/// Get overall progress across all subsystems
///
/// Returns (completed, total, failed) aggregated across all subsystems.
pub fn get_overall_progress() -> (u32, u32, u32) {
    let mut total_completed = 0u32;
    let mut total_tests = 0u32;
    let mut total_failed = 0u32;

    for idx in 0..SubsystemId::COUNT {
        total_completed += PROGRESS[idx].completed.load(Ordering::Acquire);
        total_tests += PROGRESS[idx].total.load(Ordering::Acquire);
        total_failed += PROGRESS[idx].failed.load(Ordering::Acquire);
    }

    (total_completed, total_tests, total_failed)
}

/// Check if all subsystems have completed (public API for future use)
#[allow(dead_code)]
pub fn all_complete() -> bool {
    for idx in 0..SubsystemId::COUNT {
        let total = PROGRESS[idx].total.load(Ordering::Acquire);
        let completed = PROGRESS[idx].completed.load(Ordering::Acquire);
        // Skip subsystems with no tests
        if total > 0 && completed < total {
            return false;
        }
    }
    true
}

/// Reset all progress counters (for test isolation)
#[cfg(test)]
pub fn reset_all() {
    for idx in 0..SubsystemId::COUNT {
        PROGRESS[idx].completed.store(0, Ordering::Release);
        PROGRESS[idx].total.store(0, Ordering::Release);
        PROGRESS[idx].failed.store(0, Ordering::Release);
        PROGRESS[idx].started.store(0, Ordering::Release);
    }
}
