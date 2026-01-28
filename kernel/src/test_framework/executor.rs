//! Test executor - spawns kthreads to run tests in parallel
//!
//! Each subsystem gets its own kthread, allowing tests to run concurrently.
//! Tests within a subsystem run sequentially to avoid test interference.
//!
//! # Serial Output Protocol
//!
//! The executor emits structured markers to serial output for external monitoring:
//!
//! ```text
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

use crate::task::kthread::{kthread_run, kthread_join, KthreadHandle};
use crate::serial_println;
use super::registry::{SUBSYSTEMS, Subsystem, SubsystemId, TestResult};
use super::progress::{init_subsystem, mark_started, increment_completed, mark_failed, get_progress, get_overall_progress};

/// Run all registered tests in parallel
///
/// Spawns one kthread per subsystem with tests. Returns when all tests complete.
/// Returns the total number of failed tests.
pub fn run_all_tests() -> u32 {
    // Use serial_println! for test markers (works on both x86_64 and ARM64)
    // log::info!() is silently discarded on ARM64 due to lack of logger backend
    serial_println!("[BOOT_TESTS:START]");

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

    serial_println!("[BOOT_TESTS:TOTAL:{}]", total_test_count);

    // Render initial display state (all subsystems pending)
    super::display::render_progress();

    // Collect handles for subsystems that have tests
    let mut handles: Vec<(SubsystemId, KthreadHandle)> = Vec::new();

    for subsystem in SUBSYSTEMS.iter() {
        // Count tests that match the current architecture
        let test_count = count_arch_filtered_tests(subsystem);

        if test_count == 0 {
            // No tests for this subsystem on this architecture
            continue;
        }

        // Initialize progress counters
        init_subsystem(subsystem.id, test_count);

        // Spawn a kthread for this subsystem
        let subsystem_id = subsystem.id;
        let thread_name = format!("test_{}", subsystem.id.name());

        match kthread_run(
            move || run_subsystem_tests(subsystem_id),
            &thread_name,
        ) {
            Ok(handle) => {
                handles.push((subsystem.id, handle));
                // Debug output for spawn - not a critical marker
                log::debug!(
                    "Spawned test thread for {} ({} tests)",
                    subsystem.name,
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

    // Emit final summary
    let (completed, total, failed) = get_overall_progress();
    if failed == 0 {
        serial_println!("[TESTS_COMPLETE:{}/{}]", completed, total);
        serial_println!("[BOOT_TESTS:PASS]");
    } else {
        serial_println!("[TESTS_COMPLETE:{}/{}:FAILED:{}]", completed, total, failed);
        serial_println!("[BOOT_TESTS:FAIL:{}]", failed);
    }

    // Final display render showing complete state
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

/// Run all tests for a single subsystem
///
/// This is the kthread entry point. Tests run sequentially within the subsystem.
fn run_subsystem_tests(id: SubsystemId) {
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

    // Emit subsystem start marker
    serial_println!("[SUBSYSTEM:{}:START]", id_name);
    mark_started(id);

    let mut passed_count = 0u32;
    let mut failed_count = 0u32;
    let total_tests = count_arch_filtered_tests(subsystem);

    for test in subsystem.tests.iter() {
        // Skip tests not for this architecture
        if !test.arch.matches_current() {
            continue;
        }

        let test_name = test.name;

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

        // Refresh display after each test
        super::display::request_refresh();
    }

    // Emit subsystem complete marker with pass/total
    let (_completed, _total, _) = get_progress(id);
    serial_println!(
        "[SUBSYSTEM:{}:COMPLETE:{}/{}]",
        id_name,
        passed_count,
        total_tests
    );

    // Log summary for humans (debug info, not critical markers)
    if failed_count == 0 {
        log::debug!(
            "{}: all {} tests passed",
            subsystem_name,
            passed_count
        );
    } else {
        log::warn!(
            "{}: {}/{} tests failed",
            subsystem_name,
            failed_count,
            total_tests
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
