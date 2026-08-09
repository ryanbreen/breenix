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

use super::progress::{
    get_overall_progress, increment_completed, increment_stage_completed, init_subsystem,
    init_subsystem_stage, mark_failed, mark_started,
};
use super::registry::{Subsystem, SubsystemId, TestResult, TestStage, SUBSYSTEMS};
use crate::serial_println;
use crate::task::kthread::{kthread_join, kthread_run, KthreadHandle};

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const BOOT_TEST_STAGE_JOIN_CEILING_MILLISECONDS: u64 = 60_000;
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
// Leave one full gate per-wait ceiling after the aggregate verdict for its
// evidence/result/exit path to run even under severe host-vCPU starvation.
const BOOT_TEST_EXIT_KICK_JOIN_MARGIN_MILLISECONDS: u64 = 15_000;
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const BOOT_TEST_JOIN_REKICK_INTERVAL_MILLISECONDS: u64 = 50;
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
const BOOT_TEST_JOIN_CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS: u64 = 100_000;

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
type BootTestJoinAnchor = u64;
#[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
type BootTestJoinAnchor = ();

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn boot_test_join_anchor_now() -> BootTestJoinAnchor {
    crate::arch_impl::aarch64::timer::rdtsc_serialized()
}

#[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
const fn boot_test_join_anchor_now() -> BootTestJoinAnchor {}

#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
enum BootTestJoinFailure {
    Join(crate::task::kthread::KthreadError),
    Watchdog,
}

/// Join one subsystem worker without allowing the boot-test harness to hang
/// silently on a host-starved aarch64 vCPU.
///
/// `run_all_tests()` captures one anchor immediately before its EarlyBoot stage
/// runner and reuses it for PostScheduler, so ordinary subsystem joins cannot stack
/// their 60-second budgets. The Process/PostScheduler worker is special because it
/// contains the exit-kick gate: once that gate publishes its exact start timestamp,
/// this outer join defers to the gate's 45-second ceiling plus a strict 15-second
/// completion margin. Before publication, the ordinary shared deadline still catches
/// a worker that never reaches the gate. See P21 in docs/polling-allowlist.md.
#[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
fn join_boot_test_kthread_bounded(
    subsystem: SubsystemId,
    target_stage: TestStage,
    handle: &KthreadHandle,
    boot_join_started_at: u64,
) -> Result<i32, BootTestJoinFailure> {
    use crate::arch_impl::aarch64::{
        constants::SGI_RESCHEDULE, gic, percpu::Aarch64PerCpu, smp, timer,
    };
    use crate::task::kthread::kthread_has_exited_for_test;
    use crate::tracing::providers::teardown::{
        exit_kick_gate_started_at_for_test, EXIT_KICK_GATE_CEILING_MILLISECONDS,
    };

    if kthread_has_exited_for_test(handle) {
        return kthread_join(handle).map_err(BootTestJoinFailure::Join);
    }

    let counter_frequency_hz = timer::frequency_hz();
    if counter_frequency_hz == 0 {
        serial_println!(
            "[boot_tests] subsystem_join_watchdog_failure subsystem={} stage={} cause=counter_frequency_unavailable elapsed_ms=0 re_kick_sgis=0 cpus_online={}",
            subsystem.name(),
            target_stage.name(),
            smp::cpus_online(),
        );
        serial_println!("[BOOT_TESTS:FAIL:subsystem_join_watchdog]");
        return Err(BootTestJoinFailure::Watchdog);
    }

    let boot_join_ceiling_ticks =
        (counter_frequency_hz.saturating_mul(BOOT_TEST_STAGE_JOIN_CEILING_MILLISECONDS) / 1_000)
            .max(1);
    let exit_kick_gate_join_ceiling_milliseconds = EXIT_KICK_GATE_CEILING_MILLISECONDS
        .saturating_add(BOOT_TEST_EXIT_KICK_JOIN_MARGIN_MILLISECONDS);
    let exit_kick_gate_join_ceiling_ticks =
        (counter_frequency_hz.saturating_mul(exit_kick_gate_join_ceiling_milliseconds) / 1_000)
            .max(1);
    let re_kick_ticks =
        (counter_frequency_hz.saturating_mul(BOOT_TEST_JOIN_REKICK_INTERVAL_MILLISECONDS) / 1_000)
            .max(1);
    let wait_started_at = timer::rdtsc_serialized();
    let mut last_counter_sample = wait_started_at;
    let mut last_re_kick = wait_started_at;
    let mut iterations = 0u64;
    let mut re_kick_sgis = 0u64;

    loop {
        if kthread_has_exited_for_test(handle) {
            return kthread_join(handle).map_err(BootTestJoinFailure::Join);
        }

        let now = timer::rdtsc_serialized();
        iterations = iterations.wrapping_add(1);
        if iterations % BOOT_TEST_JOIN_CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS == 0 {
            let counter_delta = now.wrapping_sub(last_counter_sample);
            if counter_delta == 0 {
                if kthread_has_exited_for_test(handle) {
                    return kthread_join(handle).map_err(BootTestJoinFailure::Join);
                }
                serial_println!(
                    "[boot_tests] subsystem_join_watchdog_failure subsystem={} stage={} cause=cntvct_stall elapsed_ms={} re_kick_sgis={} cpus_online={}",
                    subsystem.name(),
                    target_stage.name(),
                    now.wrapping_sub(wait_started_at).saturating_mul(1_000)
                        / counter_frequency_hz,
                    re_kick_sgis,
                    smp::cpus_online(),
                );
                serial_println!("[BOOT_TESTS:FAIL:subsystem_join_watchdog]");
                return Err(BootTestJoinFailure::Watchdog);
            }
            last_counter_sample = now;
        }

        let exit_kick_gate_started_at =
            if subsystem == SubsystemId::Process && target_stage == TestStage::PostScheduler {
                exit_kick_gate_started_at_for_test()
            } else {
                None
            };
        let (deadline_expired, deadline_scope, deadline_budget_ms) =
            if let Some(gate_started_at) = exit_kick_gate_started_at {
                (
                    now.wrapping_sub(gate_started_at) >= exit_kick_gate_join_ceiling_ticks,
                    "exit_kick_gate",
                    exit_kick_gate_join_ceiling_milliseconds,
                )
            } else {
                (
                    now.wrapping_sub(boot_join_started_at) >= boot_join_ceiling_ticks,
                    "boot_test_stages",
                    BOOT_TEST_STAGE_JOIN_CEILING_MILLISECONDS,
                )
            };
        if deadline_expired {
            // Close the deadline race before emitting a permanent failure marker.
            if kthread_has_exited_for_test(handle) {
                return kthread_join(handle).map_err(BootTestJoinFailure::Join);
            }
            // The gate can publish between the deadline calculation and this
            // verdict. Re-enter the loop so its full 45s + 15s outer budget wins.
            if exit_kick_gate_started_at.is_none()
                && subsystem == SubsystemId::Process
                && target_stage == TestStage::PostScheduler
                && exit_kick_gate_started_at_for_test().is_some()
            {
                continue;
            }

            let cpus_online = smp::cpus_online();
            let elapsed_ms =
                now.wrapping_sub(wait_started_at).saturating_mul(1_000) / counter_frequency_hz;
            let boot_join_elapsed_ms =
                now.wrapping_sub(boot_join_started_at).saturating_mul(1_000)
                    / counter_frequency_hz;
            serial_println!(
                "[boot_tests] subsystem_join_timeout subsystem={} stage={} deadline_scope={} deadline_budget_ms={} elapsed_ms={} boot_join_elapsed_ms={} re_kick_sgis={} cpus_online={}",
                subsystem.name(),
                target_stage.name(),
                deadline_scope,
                deadline_budget_ms,
                elapsed_ms,
                boot_join_elapsed_ms,
                re_kick_sgis,
                cpus_online,
            );
            serial_println!("[BOOT_TESTS:FAIL:subsystem_join_timeout]");
            return Err(BootTestJoinFailure::Watchdog);
        }

        if now.wrapping_sub(last_re_kick) >= re_kick_ticks {
            let coordinator_cpu = Aarch64PerCpu::cpu_id() as usize;
            for cpu in 0..smp::MAX_CPUS {
                if cpu != coordinator_cpu && smp::is_cpu_online(cpu) {
                    gic::send_sgi(SGI_RESCHEDULE as u8, cpu as u8);
                    re_kick_sgis = re_kick_sgis.saturating_add(1);
                }
            }
            last_re_kick = now;
        }

        // Match the exit-kick gate's reachable CNTVCT-stall watchdog: request
        // rescheduling and keep sampling instead of sleeping in WFI on the same
        // counter source whose failure this loop must detect.
        crate::task::scheduler::yield_current();
        core::hint::spin_loop();
    }
}

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
    TestStage::from_u8(CURRENT_STAGE.load(Ordering::Acquire)).unwrap_or(TestStage::EarlyBoot)
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
    run_staged_tests(stage, boot_test_join_anchor_now())
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
    let total_test_count: u32 = SUBSYSTEMS
        .iter()
        .map(|s| count_arch_filtered_tests(s))
        .sum();

    if total_test_count == 0 {
        serial_println!("[BOOT_TESTS:SKIP] No tests registered for current architecture");
        serial_println!("[TESTS_COMPLETE:0/0]");
        return 0;
    }

    // Count tests by stage for reporting
    let early_boot_count: u32 = SUBSYSTEMS
        .iter()
        .map(|s| count_stage_filtered_tests(s, TestStage::EarlyBoot))
        .sum();
    let later_stage_count = total_test_count - early_boot_count;

    serial_println!("[BOOT_TESTS:TOTAL:{}]", total_test_count);
    serial_println!("[BOOT_TESTS:EARLY_BOOT:{}]", early_boot_count);
    if later_stage_count > 0 {
        serial_println!(
            "[BOOT_TESTS:STAGED:{} tests waiting for later stages]",
            later_stage_count
        );
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

    // One aarch64 boot-test anchor governs both join phases. Capturing it here,
    // rather than inside run_staged_tests(), prevents PostScheduler from
    // re-arming the watchdog after a host-starved EarlyBoot phase.
    let boot_join_started_at = boot_test_join_anchor_now();

    // Run EarlyBoot tests
    let early_failures = run_staged_tests(TestStage::EarlyBoot, boot_join_started_at);

    // Now advance to PostScheduler stage - by this point kthreads are working
    // (we just used them to run EarlyBoot tests)
    serial_println!("[STAGE:{}:ADVANCE]", TestStage::PostScheduler.name());
    CURRENT_STAGE.store(TestStage::PostScheduler as u8, Ordering::Release);
    let post_failures = run_staged_tests(TestStage::PostScheduler, boot_join_started_at);

    early_failures + post_failures
}

/// Run tests for a specific stage (and mark them as run)
fn run_staged_tests(target_stage: TestStage, boot_join_started_at: BootTestJoinAnchor) -> u32 {
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
                serial_println!("[SUBSYSTEM:{}:SPAWN_ERROR:{:?}]", subsystem.name, e);
            }
        }
    }

    // Wait for all test threads to complete. On aarch64 boot-test builds the
    // caller-provided timestamp is shared by every sequential join. run_all_tests
    // also shares it across EarlyBoot and PostScheduler.
    #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
    let _ = boot_join_started_at;
    let mut total_failed = 0u32;
    for (id, handle) in handles {
        #[cfg(all(feature = "boot_tests", target_arch = "aarch64"))]
        match join_boot_test_kthread_bounded(id, target_stage, &handle, boot_join_started_at) {
            Ok(exit_code) => {
                if exit_code != 0 {
                    total_failed += exit_code as u32;
                }
            }
            Err(BootTestJoinFailure::Join(e)) => {
                serial_println!("[SUBSYSTEM:{}:JOIN_ERROR:{:?}]", id.name(), e);
                total_failed += 1;
            }
            Err(BootTestJoinFailure::Watchdog) => {
                mark_failed(id);
                total_failed += 1;
                // There is no safe kthread cancellation primitive. Stop here
                // rather than cascade the already-expired deadline across every
                // remaining handle; those workers are intentionally detached and
                // the terminal BOOT_TESTS failure tells the harness to end QEMU.
                break;
            }
        }

        #[cfg(not(all(feature = "boot_tests", target_arch = "aarch64")))]
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
        serial_println!(
            "[STAGE:{}:COMPLETE:{}/{}]",
            target_stage.name(),
            completed,
            total
        );
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
            t.arch.matches_current() && t.stage == stage && (already_run & (1u64 << idx)) == 0
            // Not already run
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
            t.arch.matches_current() && t.stage <= current && (already_run & (1u64 << idx)) == 0
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
        let memory = SUBSYSTEMS
            .iter()
            .find(|s| s.id == SubsystemId::Memory)
            .unwrap();
        // Should have at least the framework_sanity and heap_alloc_basic tests
        assert!(count_arch_filtered_tests(memory) >= 2);
    }
}
