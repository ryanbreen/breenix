//! Test executor - spawns kthreads to run tests in parallel
//!
//! Each subsystem gets its own kthread, allowing tests to run concurrently.
//! Tests within a subsystem run sequentially to avoid test interference.
//!
//! # Staged Execution
//!
//! Tests declare which boot stage they require (EarlyBoot, PostScheduler,
//! ProcessContext, Userspace). The executor tracks the current stage and
//! only runs tests whose requirements are met. Call `advance_to_stage()`
//! at appropriate points in the boot sequence to run staged tests.
//!
//! # Serial Output Protocol
//!
//! The executor emits structured markers to serial output for external monitoring:
//!
//! ```text
//! [STAGE:EarlyBoot:ADVANCE]
//! [SUBSYSTEM:Memory:START]
//! [TEST:Memory:heap_alloc:START]
//! [TEST:Memory:heap_alloc:PASS]
//! [TEST:Memory:frame_alloc:START]
//! [TEST:Memory:frame_alloc:FAIL:allocation failed]
//! [SUBSYSTEM:Memory:COMPLETE:1/2]
//! [TESTS_COMPLETE:45/48]
//! ```
//!
//! These markers can be parsed by external tools to track test progress.
//!
//! # Architecture Support
//!
//! Test markers use `serial_println!` directly instead of `log::info!()` because:
//! - ARM64 has no logger backend (logger.rs is x86_64-only)
//! - `log::info!()` calls are silently discarded on ARM64
//! - `serial_println!` works on both architectures via their respective serial implementations

use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::task::kthread::{kthread_run, kthread_join, KthreadHandle};
use crate::serial_println;
use super::registry::{SUBSYSTEMS, Subsystem, SubsystemId, TestResult, TestStage};
use super::progress::{init_subsystem, init_subsystem_stage, mark_started, increment_completed, increment_stage_completed, mark_failed, get_overall_progress};

/// Current boot stage - tests with stage <= this can run
static CURRENT_STAGE: AtomicU8 = AtomicU8::new(TestStage::EarlyBoot as u8);

/// Track which tests have already run (by subsystem + test index)
/// This is a simple bitmap: each subsystem gets 64 bits (max 64 tests per subsystem)
static TESTS_RUN: [AtomicU64; SubsystemId::COUNT] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; SubsystemId::COUNT]
};

use core::sync::atomic::AtomicU64;

/// Get the current test stage
pub fn current_stage() -> TestStage {
    TestStage::from_u8(CURRENT_STAGE.load(Ordering::Acquire))
        .unwrap_or(TestStage::EarlyBoot)
}

/// Advance to a new stage and run any tests waiting for that stage
///
/// Call this at appropriate points in the boot sequence:
/// - PostScheduler: after scheduler and kthreads are working
/// - ProcessContext: after first user process is created
/// - Userspace: after first userspace syscall is confirmed
///
/// Returns the number of failed tests at the new stage.
pub fn advance_to_stage(stage: TestStage) -> u32 {
    let current = current_stage();
    if stage <= current {
        // Already at or past this stage
        return 0;
    }

    serial_println!("[STAGE:{}:ADVANCE]", stage.name());
    CURRENT_STAGE.store(stage as u8, Ordering::Release);

    // Run any tests that were waiting for this stage
    run_staged_tests(stage)
}

/// Advance to a new stage without running tests
///
/// Use this when in syscall context where spawning kthreads would block.
/// Emits the stage marker but does not run any tests.
pub fn advance_stage_marker_only(stage: TestStage) {
    let current = current_stage();
    if stage <= current {
        // Already at or past this stage
        return;
    }

    serial_println!("[STAGE:{}:ADVANCE]", stage.name());
    CURRENT_STAGE.store(stage as u8, Ordering::Release);

    // Note: We don't call run_staged_tests() here because we're in syscall context.
    // Tests for this stage should verify the stage was reached via other means
    // (e.g., checking is_el0_confirmed() or is_ring3_confirmed()).

    // Emit completion marker since no tests run
    let (completed, total, failed) = get_overall_progress();
    if failed == 0 {
        serial_println!("[TESTS_COMPLETE:{}/{}]", completed, total);
        serial_println!("[BOOT_TESTS:PASS]");
    } else {
        serial_println!("[TESTS_COMPLETE:{}/{}:FAILED:{}]", completed, total, failed);
        serial_println!("[BOOT_TESTS:FAIL:{}]", failed);
    }
}

/// Run all registered tests in parallel (EarlyBoot stage only)
///
/// Spawns one kthread per subsystem with tests. Returns when all EarlyBoot
/// tests complete. Later stages run via advance_to_stage().
/// Returns the total number of failed tests.
pub fn run_all_tests() -> u32 {
    // Use serial_println! for test markers (works on both x86_64 and ARM64)
    // log::info!() is silently discarded on ARM64 due to lack of logger backend
    serial_println!("[BOOT_TESTS:START]");
    serial_println!("[STAGE:{}:ADVANCE]", TestStage::EarlyBoot.name());

    // Initialize graphical display if framebuffer is available
    super::display::init();

    // Count total tests across all subsystems for final summary
    let total_test_count: u32 = SUBSYSTEMS.iter()
        .map(|s| count_arch_filtered_tests(s))
        .sum();

    if total_test_count == 0 {
        serial_println!("[BOOT_TESTS:SKIP] No tests registered for current architecture");
        serial_println!("[TESTS_COMPLETE:0/0]");
        return 0;
    }

    // Count tests by stage for reporting
    let early_boot_count: u32 = SUBSYSTEMS.iter()
        .map(|s| count_stage_filtered_tests(s, TestStage::EarlyBoot))
        .sum();
    let later_stage_count = total_test_count - early_boot_count;

    serial_println!("[BOOT_TESTS:TOTAL:{}]", total_test_count);
    serial_println!("[BOOT_TESTS:EARLY_BOOT:{}]", early_boot_count);
    if later_stage_count > 0 {
        serial_println!("[BOOT_TESTS:STAGED:{} tests waiting for later stages]", later_stage_count);
    }

    // Render initial display state (all subsystems pending)
    super::display::render_progress();

    // Initialize progress counters for ALL tests (not just current stage)
    for subsystem in SUBSYSTEMS.iter() {
        let test_count = count_arch_filtered_tests(subsystem);
        if test_count > 0 {
            init_subsystem(subsystem.id, test_count);
            // Initialize per-stage totals for color-coded display
            for stage_idx in 0..TestStage::COUNT {
                if let Some(stage) = TestStage::from_u8(stage_idx as u8) {
                    let stage_count = count_stage_filtered_tests(subsystem, stage);
                    init_subsystem_stage(subsystem.id, stage, stage_count);
                }
            }
        }
    }

    // Run EarlyBoot tests
    let early_failures = run_staged_tests(TestStage::EarlyBoot);

    // Now advance to PostScheduler stage - by this point kthreads are working
    // (we just used them to run EarlyBoot tests)
    serial_println!("[STAGE:{}:ADVANCE]", TestStage::PostScheduler.name());
    CURRENT_STAGE.store(TestStage::PostScheduler as u8, Ordering::Release);
    let post_failures = run_staged_tests(TestStage::PostScheduler);

    early_failures + post_failures
}

/// Run tests for a specific stage (and mark them as run)
fn run_staged_tests(target_stage: TestStage) -> u32 {
    let mut handles: Vec<(SubsystemId, KthreadHandle)> = Vec::new();

    for subsystem in SUBSYSTEMS.iter() {
        // Count tests that match architecture AND stage
        let test_count = count_stage_filtered_tests(subsystem, target_stage);

        if test_count == 0 {
            // No tests for this subsystem at this stage
            continue;
        }

        // Spawn a kthread for this subsystem's staged tests
        let subsystem_id = subsystem.id;
        let thread_name = format!("test_{}_{}", subsystem.id.name(), target_stage.name());

        match kthread_run(
            move || run_subsystem_stage_tests(subsystem_id, target_stage),
            &thread_name,
        ) {
            Ok(handle) => {
                handles.push((subsystem.id, handle));
                log::debug!(
                    "Spawned test thread for {}:{} ({} tests)",
                    subsystem.name,
                    target_stage.name(),
                    test_count
                );
            }
            Err(e) => {
                serial_println!(
                    "[SUBSYSTEM:{}:SPAWN_ERROR:{:?}]",
                    subsystem.name,
                    e
                );
            }
        }
    }

    // Wait for all test threads to complete
    let mut total_failed = 0u32;
    for (id, handle) in handles {
        match kthread_join(&handle) {
            Ok(exit_code) => {
                if exit_code != 0 {
                    total_failed += exit_code as u32;
                }
            }
            Err(e) => {
                serial_println!("[SUBSYSTEM:{}:JOIN_ERROR:{:?}]", id.name(), e);
                total_failed += 1;
            }
        }
    }

    // Emit stage summary
    let (completed, total, failed) = get_overall_progress();

    // Check if all tests are complete
    let all_complete = completed == total;

    if all_complete {
        if failed == 0 {
            serial_println!("[TESTS_COMPLETE:{}/{}]", completed, total);
            serial_println!("[BOOT_TESTS:PASS]");
        } else {
            serial_println!("[TESTS_COMPLETE:{}/{}:FAILED:{}]", completed, total, failed);
            serial_println!("[BOOT_TESTS:FAIL:{}]", failed);
        }
    } else {
        serial_println!("[STAGE:{}:COMPLETE:{}/{}]", target_stage.name(), completed, total);
    }

    // Refresh display
    super::display::render_progress();

    total_failed
}

/// Count tests that match the current architecture
fn count_arch_filtered_tests(subsystem: &Subsystem) -> u32 {
    subsystem
        .tests
        .iter()
        .filter(|t| t.arch.matches_current())
        .count() as u32
}

/// Count tests that match architecture AND specific stage (not already run)
fn count_stage_filtered_tests(subsystem: &Subsystem, stage: TestStage) -> u32 {
    let subsystem_idx = subsystem.id as usize;
    let already_run = TESTS_RUN[subsystem_idx].load(Ordering::Acquire);

    subsystem
        .tests
        .iter()
        .enumerate()
        .filter(|(idx, t)| {
            t.arch.matches_current()
                && t.stage == stage
                && (already_run & (1u64 << idx)) == 0 // Not already run
        })
        .count() as u32
}

/// Count pending tests (not yet run) across all stages up to current
#[allow(dead_code)]
fn count_pending_tests(subsystem: &Subsystem) -> u32 {
    let current = current_stage();
    let subsystem_idx = subsystem.id as usize;
    let already_run = TESTS_RUN[subsystem_idx].load(Ordering::Acquire);

    subsystem
        .tests
        .iter()
        .enumerate()
        .filter(|(idx, t)| {
            t.arch.matches_current()
                && t.stage <= current
                && (already_run & (1u64 << idx)) == 0
        })
        .count() as u32
}

/// Run tests for a single subsystem at a specific stage
///
/// This is the kthread entry point. Tests run sequentially within the subsystem.
fn run_subsystem_stage_tests(id: SubsystemId, target_stage: TestStage) {
    // Get the subsystem definition
    let subsystem = match SUBSYSTEMS.iter().find(|s| s.id == id) {
        Some(s) => s,
        None => {
            serial_println!("[SUBSYSTEM:{:?}:NOT_FOUND]", id);
            return;
        }
    };

    let subsystem_name = subsystem.name;
    let id_name = id.name();
    let subsystem_idx = id as usize;

    // Emit subsystem start marker (include stage)
    serial_println!("[SUBSYSTEM:{}:{}:START]", id_name, target_stage.name());
    mark_started(id);

    let mut passed_count = 0u32;
    let mut failed_count = 0u32;
    let mut run_count = 0u32;

    for (test_idx, test) in subsystem.tests.iter().enumerate() {
        // Skip tests not for this architecture
        if !test.arch.matches_current() {
            continue;
        }

        // Skip tests not for this stage
        if test.stage != target_stage {
            continue;
        }

        // Check if already run (atomic CAS to mark as running)
        let bit = 1u64 << test_idx;
        let old = TESTS_RUN[subsystem_idx].fetch_or(bit, Ordering::AcqRel);
        if (old & bit) != 0 {
            // Already run by another thread (shouldn't happen, but be safe)
            continue;
        }

        let test_name = test.name;
        run_count += 1;

        // Emit test start marker
        serial_println!("[TEST:{}:{}:START]", id_name, test_name);

        // Run the test (timeout handling will be added in Phase 5)
        let result = run_single_test(test.func);

        // Emit test result marker
        match result {
            TestResult::Pass => {
                serial_println!("[TEST:{}:{}:PASS]", id_name, test_name);
                passed_count += 1;
            }
            TestResult::Fail(msg) => {
                serial_println!("[TEST:{}:{}:FAIL:{}]", id_name, test_name, msg);
                mark_failed(id);
                failed_count += 1;
            }
            TestResult::Timeout => {
                serial_println!("[TEST:{}:{}:TIMEOUT]", id_name, test_name);
                mark_failed(id);
                failed_count += 1;
            }
            TestResult::Panic => {
                serial_println!("[TEST:{}:{}:PANIC]", id_name, test_name);
                mark_failed(id);
                failed_count += 1;
            }
        }

        increment_completed(id);
        increment_stage_completed(id, target_stage);

        // Refresh display after each test
        super::display::request_refresh();
    }

    // Emit subsystem stage complete marker with pass/total
    serial_println!(
        "[SUBSYSTEM:{}:{}:COMPLETE:{}/{}]",
        id_name,
        target_stage.name(),
        passed_count,
        run_count
    );

    // Log summary for humans (debug info, not critical markers)
    if failed_count == 0 {
        log::debug!(
            "{}:{}: all {} tests passed",
            subsystem_name,
            target_stage.name(),
            passed_count
        );
    } else {
        log::warn!(
            "{}:{}: {}/{} tests failed",
            subsystem_name,
            target_stage.name(),
            failed_count,
            run_count
        );
    }
}

/// Run a single test function
///
/// Currently just calls the function directly. Panic catching and timeout
/// handling will be added in Phase 5.
fn run_single_test(func: fn() -> TestResult) -> TestResult {
    // TODO (Phase 5): Add panic catching with catch_unwind equivalent
    // TODO (Phase 5): Add timeout handling
    func()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_arch_filtered() {
        // Memory subsystem now has sanity tests
        let memory = SUBSYSTEMS.iter().find(|s| s.id == SubsystemId::Memory).unwrap();
        // Should have at least the framework_sanity and heap_alloc_basic tests
        assert!(count_arch_filtered_tests(memory) >= 2);
    }
}
