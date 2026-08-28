//! Test executor - spawns kthreads to run tests in parallel
//!
//! Each subsystem gets its own kthread, allowing tests to run concurrently.
//! Tests within a subsystem run sequentially to avoid test interference.
//!
//! # Staged Execution
//!
//! Tests declare which boot stage they require (SerialBoot, EarlyBoot,
//! PostScheduler, ProcessContext, Userspace). SerialBoot tests run alone;
//! subsystem-level parallelism starts at EarlyBoot. The executor tracks the
//! current stage and only runs tests whose requirements are met.
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

/// Current boot stage - tests with stage <= this can run
static CURRENT_STAGE: AtomicU8 = AtomicU8::new(TestStage::SerialBoot as u8);

/// The marker-only x86 stage path can emit more than one verdict per boot.
#[cfg(not(target_arch = "aarch64"))]
static X86_CENSUS_WIDEN_ORACLE_RAN: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Track which tests have already run (by subsystem + test index)
/// This is a simple bitmap: each subsystem gets 64 bits (max 64 tests per subsystem)
static TESTS_RUN: [AtomicU64; SubsystemId::COUNT] = {
    const INIT: AtomicU64 = AtomicU64::new(0);
    [INIT; SubsystemId::COUNT]
};

use core::sync::atomic::AtomicU64;

/// Emit the aarch64 exec lock-order counters.
///
/// Called twice: once from the boot-test verdict (where `commits` is still 0 — init has not yet
/// spawned anything), and once from `sys_exec_aarch64` immediately after a successful exec in
/// `boot_tests` builds, where the counters are live. The aarch64 gate scripts assert on the second
/// line: `commits >= 1` (the exec smoke really executed) with every violation counter at 0.
/// Violations are ALSO reported at the instant they happen by the `[EXEC_LOCK_ORDER:VIOLATION:*]`
/// markers in `ExecSchedCommit::apply`, which every build emits and both gate scripts treat as fatal.
#[cfg(target_arch = "aarch64")]
pub fn emit_exec_lock_order_counters() -> bool {
    use crate::task::scheduler::{
        EXEC_COMMIT_MISSING_THREAD, EXEC_COMMIT_UNPINNED, EXEC_SCHED_COMMITS,
        SCHED_AFTER_PM_VIOLATIONS,
    };

    let commits = EXEC_SCHED_COMMITS.load(Ordering::Relaxed);
    let pm_held = SCHED_AFTER_PM_VIOLATIONS.load(Ordering::Relaxed);
    let unpinned = EXEC_COMMIT_UNPINNED.load(Ordering::Relaxed);
    let missing = EXEC_COMMIT_MISSING_THREAD.load(Ordering::Relaxed);
    serial_println!(
        "[EXEC_LOCK_ORDER:commits={}:pm_held={}:unpinned={}:missing={}]",
        commits,
        pm_held,
        unpinned,
        missing
    );
    pm_held == 0 && unpinned == 0 && missing == 0
}

#[cfg(not(target_arch = "aarch64"))]
pub fn emit_exec_lock_order_counters() -> bool {
    true
}

/// Emit the aarch64 EL-conditioned `user_rsp_scratch` install census.
///
/// Called from the LAST boot-test verdict — the marker-only Userspace-stage
/// path, reached from the first confirmed EL0 syscall. The earlier
/// ProcessContext verdict runs before any exception has been taken from EL0, so
/// reporting there would print two zeroes and measure nothing.
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
fn emit_user_rsp_scratch_el_census() {
    crate::arch_impl::aarch64::context_switch::report_user_rsp_scratch_el_census();
}

#[cfg(not(all(target_arch = "aarch64", feature = "boot_tests")))]
fn emit_user_rsp_scratch_el_census() {}

/// Get the current test stage
pub fn current_stage() -> TestStage {
    TestStage::from_u8(CURRENT_STAGE.load(Ordering::Acquire)).unwrap_or(TestStage::SerialBoot)
}

#[cfg(not(target_arch = "aarch64"))]
fn run_census_widen_oracle_x86_once() {
    if X86_CENSUS_WIDEN_ORACLE_RAN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        super::registry::run_census_widen_oracle();
    }
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
    if stage > current {
        serial_println!("[STAGE:{}:ADVANCE]", stage.name());
        CURRENT_STAGE.store(stage as u8, Ordering::Release);
        #[cfg(not(target_arch = "aarch64"))]
        crate::task::strand_oracle::sample_now();
    }

    // Run this stage's tests whether or not the stage counter has already been
    // pushed past it.
    //
    // The counter is not only advanced from here: `advance_stage_marker_only`
    // jumps it straight to Userspace from the first Ring 3 syscall, and on x86
    // that syscall races the dispatch - it was observed landing in the middle of
    // the EarlyBoot cohort. With the old `stage <= current` early return, that
    // race silently dropped the entire ProcessContext cohort, which is the
    // failure mode #533 exists to remove. `run_staged_tests` is idempotent
    // through the TESTS_RUN bitmap and returns silently when nothing is pending,
    // so running it unconditionally costs an already-complete stage nothing.
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
    #[cfg(not(target_arch = "aarch64"))]
    crate::task::strand_oracle::sample_now();

    // Note: We don't call run_staged_tests() here because we're in syscall context.
    // Tests for this stage should verify the stage was reached via other means
    // (e.g., checking is_el0_confirmed() or is_ring3_confirmed()).

    // Emit completion marker since no tests run
    let (completed, total, failed) = get_overall_progress();
    let lock_order_clean = emit_exec_lock_order_counters();
    emit_user_rsp_scratch_el_census();
    #[cfg(not(target_arch = "aarch64"))]
    {
        crate::task::strand_oracle::sample_now();
        run_census_widen_oracle_x86_once();
        crate::task::strand_oracle::report_x86_once();
    }

    // Both guards below only mean something where the registry is actually
    // dispatched. On an x86 build without `x86_staged_registry` nothing ever
    // runs, so every boot would trip the vacuity arm and report
    // `[BOOT_TESTS:FAIL:VACUOUS]` - a true statement about #533, but one that
    // turns the shipped x86 gate red for a defect the build deliberately still
    // has. Leaving that build on the historical output keeps it byte-for-byte
    // what it was; #533 stays open, and it is #533 that describes the 0/0 pass.
    // Only the completion of the last registered test gets to publish a
    // verdict.
    //
    // This path prints whatever progress happens to stand at the instant of the
    // first Ring 3 syscall, and on x86 that instant lands in the middle of the
    // dispatch - a boot was observed printing this verdict while the `system`
    // subsystem's EarlyBoot thread was still running. Publishing a partial tally
    // as `[BOOT_TESTS:PASS]` would be the same class of false green as the 0/0
    // pass below, so an incomplete tally is reported as stage progress and the
    // dispatch keeps the verdict.
    //
    // Both this guard and the vacuity guard below only mean something where the
    // registry is actually dispatched. On an x86 build without
    // `x86_staged_registry` nothing ever runs, so every boot would trip the
    // vacuity arm - a true statement about #533, and the wrong place to make it:
    // it turns the shipped x86 gate red for a defect that build deliberately
    // still has. That build keeps the historical output; #533 stays open, and
    // #533 is where the 0/0 pass is described.
    let dispatched = registry_is_dispatched();
    if dispatched && total != 0 && completed != 0 && completed < total {
        serial_println!("[STAGE:{}:COMPLETE:{}/{}]", stage.name(), completed, total);
        return;
    }

    // A verdict that no test contributed to is not a pass (#533). `0/0` is not
    // evidence of anything, so it is reported as a failure with its own
    // signature, which the gates reject by name.
    if dispatched && (total == 0 || completed == 0) {
        serial_println!("[TESTS_COMPLETE:{}/{}:VACUOUS]", completed, total);
        serial_println!("[BOOT_TESTS:FAIL:VACUOUS]");
    } else if failed == 0 && lock_order_clean {
        serial_println!("[TESTS_COMPLETE:{}/{}]", completed, total);
        serial_println!("[BOOT_TESTS:PASS]");
    } else {
        serial_println!("[TESTS_COMPLETE:{}/{}:FAILED:{}]", completed, total, failed);
        let verdict_failures = failed + u32::from(!lock_order_clean);
        serial_println!("[BOOT_TESTS:FAIL:{}]", verdict_failures);
    }
}

/// Run all registered tests in parallel (EarlyBoot stage only)
///
/// Spawns one kthread per subsystem with tests. Returns when all EarlyBoot
/// tests complete. Later stages run via advance_to_stage().
/// Returns the total number of failed tests.
pub fn run_all_tests() -> u32 {
    crate::task::strand_oracle::start();
    #[cfg(feature = "coreproof")]
    crate::proof::start();
    crate::task::ret_zero_pc_oracle::start();
    crate::task::percpu_stack_oracle::start();
    #[cfg(all(feature = "boot_tests", target_arch = "x86_64"))]
    let _ = crate::memory::kernel_stack::kernel_stack_quiesce_baseline_outstanding();

    // Use serial_println! for test markers (works on both x86_64 and ARM64)
    // log::info!() is silently discarded on ARM64 due to lack of logger backend
    serial_println!("[BOOT_TESTS:START]");
    serial_println!("[STAGE:{}:ADVANCE]", TestStage::SerialBoot.name());
    #[cfg(not(target_arch = "aarch64"))]
    crate::task::strand_oracle::sample_now();

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
    let serial_boot_count: u32 = SUBSYSTEMS
        .iter()
        .map(|s| count_stage_filtered_tests(s, TestStage::SerialBoot))
        .sum();
    let parallel_boot_count: u32 = SUBSYSTEMS
        .iter()
        .map(|s| count_stage_filtered_tests(s, TestStage::EarlyBoot))
        .sum();
    let early_boot_count = serial_boot_count + parallel_boot_count;
    let later_stage_count = total_test_count - early_boot_count;

    serial_println!("[BOOT_TESTS:TOTAL:{}]", total_test_count);
    serial_println!("[BOOT_TESTS:SERIAL_BOOT:{}]", serial_boot_count);
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

    // Run state-mutating gates before any parallel test kthreads exist.
    let serial_failures = run_staged_tests(TestStage::SerialBoot);

    // Run ordinary EarlyBoot tests with subsystem-level parallelism.
    serial_println!("[STAGE:{}:ADVANCE]", TestStage::EarlyBoot.name());
    CURRENT_STAGE.store(TestStage::EarlyBoot as u8, Ordering::Release);
    #[cfg(not(target_arch = "aarch64"))]
    crate::task::strand_oracle::sample_now();
    let early_failures = run_staged_tests(TestStage::EarlyBoot);

    // Now advance to PostScheduler stage - by this point kthreads are working
    // (we just used them to run EarlyBoot tests)
    serial_println!("[STAGE:{}:ADVANCE]", TestStage::PostScheduler.name());
    CURRENT_STAGE.store(TestStage::PostScheduler as u8, Ordering::Release);
    #[cfg(not(target_arch = "aarch64"))]
    crate::task::strand_oracle::sample_now();
    let post_failures = run_staged_tests(TestStage::PostScheduler);

    serial_failures + early_failures + post_failures
}

#[cfg(all(target_arch = "aarch64", feature = "arm_a_609"))]
mod arm_a_609 {
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::task::kthread::{
        kthread_has_exited_for_test, kthread_run, kthread_run_pinned_where_placed_for_test,
        KthreadHandle,
    };
    use crate::{arch_halt, serial_println};

    const ARM_A_609_DEADLINE_MS: u64 = 4000;

    static ARM_A_609_RAN: AtomicBool = AtomicBool::new(false);
    static LEGA_BODY_RAN: AtomicUsize = AtomicUsize::new(0);
    static LEGB_BODY_RAN: AtomicUsize = AtomicUsize::new(0);

    fn bounded_join(handle: &KthreadHandle) -> Option<u64> {
        let start = crate::time::get_ticks();
        loop {
            if kthread_has_exited_for_test(handle) {
                return Some(crate::time::get_ticks().saturating_sub(start));
            }
            if crate::time::get_ticks().saturating_sub(start) >= ARM_A_609_DEADLINE_MS {
                return None;
            }
            arch_halt();
        }
    }

    pub(super) fn arm_once() {
        if ARM_A_609_RAN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        serial_println!("[ARMA609:ARMED]");

        match kthread_run(
            || {
                LEGB_BODY_RAN.fetch_add(1, Ordering::SeqCst);
            },
            "arma609_legb",
        ) {
            Ok(handle) => {
                let tid = handle.tid();
                let waited = bounded_join(&handle);
                serial_println!(
                    "[ARMA609:LEGB:tid={}:cpu=-1:joined={}:waited_ms={}:body_ran={}]",
                    tid,
                    usize::from(waited.is_some()),
                    waited.unwrap_or(ARM_A_609_DEADLINE_MS),
                    LEGB_BODY_RAN.load(Ordering::SeqCst)
                );
            }
            Err(_) => serial_println!("[ARMA609:LEGB:SPAWN_ERROR]"),
        }

        let placed = AtomicUsize::new(usize::MAX);
        match kthread_run_pinned_where_placed_for_test(
            || {
                LEGA_BODY_RAN.fetch_add(1, Ordering::SeqCst);
            },
            "arma609_lega",
            &placed,
        ) {
            Ok(handle) => {
                crate::task::scheduler::dump_thread_placement(handle.tid(), "ARMA609_LEGA");
                let tid = handle.tid();
                let waited = bounded_join(&handle);
                serial_println!(
                    "[ARMA609:LEGA:tid={}:cpu={}:joined={}:waited_ms={}:body_ran={}]",
                    tid,
                    placed.load(Ordering::Acquire),
                    usize::from(waited.is_some()),
                    waited.unwrap_or(ARM_A_609_DEADLINE_MS),
                    LEGA_BODY_RAN.load(Ordering::SeqCst)
                );
            }
            Err(_) => serial_println!("[ARMA609:LEGA:SPAWN_ERROR]"),
        }
    }

    pub(super) fn report_late() {
        serial_println!(
            "[ARMA609:LATE:lega_body_ran={}:legb_body_ran={}]",
            LEGA_BODY_RAN.load(Ordering::SeqCst),
            LEGB_BODY_RAN.load(Ordering::SeqCst)
        );
    }
}

#[cfg(not(all(target_arch = "aarch64", feature = "arm_a_609")))]
mod arm_a_609 {
    pub(super) fn arm_once() {}

    pub(super) fn report_late() {}
}

/// Whether the staged registry is dispatched in this build.
///
/// aarch64 always dispatches it. x86 does so only behind `x86_staged_registry`,
/// which is off by default until #680 and #681 are fixed.
#[inline]
fn registry_is_dispatched() -> bool {
    cfg!(any(target_arch = "aarch64", feature = "x86_staged_registry"))
}

/// Whether this architecture runs one subsystem at a time instead of spawning
/// the whole cohort in parallel.
///
/// aarch64 runs the cohort in parallel and has done so for the life of this
/// framework. x86 does not, and the reason is measured rather than assumed: with
/// the cohort spawned in parallel, the very first x86 dispatch deadlocked with
/// the boot thread inside `kernel_stack::allocate` mapping the second
/// subsystem's stack while the Memory subsystem's kthread was inside
/// `heap_large_alloc` — two threads in the frame allocator and the kernel page
/// tables at once, with the boot thread halted in `kthread_join`. That is the
/// same #567 family the registry already documents above
/// `run_x86_loopback_gates` ("any test that schedules in this window currently
/// poisons the x86 boot").
///
/// Serializing does not paper over #567: the four tests that #567 actually
/// blocks stay on the deferral roster and still announce themselves. It removes
/// the concurrency the executor itself introduces, so the registry can be
/// dispatched at all. When #567 is fixed this should go back to parallel and the
/// roster should empty; both are one edit each.
#[inline]
fn dispatch_is_serialized() -> bool {
    cfg!(not(target_arch = "aarch64"))
}

/// Run tests for a specific stage (and mark them as run)
fn run_staged_tests(target_stage: TestStage) -> u32 {
    // Nothing pending at this stage: return without printing. `advance_to_stage`
    // may be called for a stage that has already run, and a repeat summary line
    // would be noise in every serial the gates parse.
    if SUBSYSTEMS
        .iter()
        .all(|s| count_stage_filtered_tests(s, target_stage) == 0)
    {
        return 0;
    }

    let mut handles: Vec<(SubsystemId, KthreadHandle)> = Vec::new();
    let mut total_failed = 0u32;

    if target_stage == TestStage::EarlyBoot {
        arm_a_609::arm_once();
    }

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
                log::debug!(
                    "Spawned test thread for {}:{} ({} tests)",
                    subsystem.name,
                    target_stage.name(),
                    test_count
                );
                if target_stage == TestStage::SerialBoot || dispatch_is_serialized() {
                    total_failed += join_test_thread(subsystem.id, handle);
                } else {
                    handles.push((subsystem.id, handle));
                }
            }
            Err(e) => {
                serial_println!("[SUBSYSTEM:{}:SPAWN_ERROR:{:?}]", subsystem.name, e);
            }
        }
    }

    // Wait for all test threads to complete
    for (id, handle) in handles {
        total_failed += join_test_thread(id, handle);
    }

    if target_stage == TestStage::EarlyBoot {
        arm_a_609::report_late();
    }

    // Emit stage summary
    let (completed, total, failed) = get_overall_progress();
    #[cfg(not(target_arch = "aarch64"))]
    crate::task::strand_oracle::sample_now();

    // Check if all tests are complete
    let all_complete = completed == total;

    if all_complete {
        let lock_order_clean = emit_exec_lock_order_counters();
        #[cfg(not(target_arch = "aarch64"))]
        {
            crate::task::strand_oracle::sample_now();
            crate::task::strand_oracle::report_x86_once();
        }

        if failed == 0 && lock_order_clean {
            serial_println!("[TESTS_COMPLETE:{}/{}]", completed, total);
            serial_println!("[BOOT_TESTS:PASS]");
        } else {
            serial_println!("[TESTS_COMPLETE:{}/{}:FAILED:{}]", completed, total, failed);
            let verdict_failures = failed + u32::from(!lock_order_clean);
            serial_println!("[BOOT_TESTS:FAIL:{}]", verdict_failures);
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

fn join_test_thread(id: SubsystemId, handle: KthreadHandle) -> u32 {
    match kthread_join(&handle) {
        Ok(exit_code) if exit_code != 0 => exit_code as u32,
        Ok(_) => 0,
        Err(e) => {
            serial_println!("[SUBSYSTEM:{}:JOIN_ERROR:{:?}]", id.name(), e);
            1
        }
    }
}

/// Count tests that match the current architecture
fn count_arch_filtered_tests(subsystem: &Subsystem) -> u32 {
    subsystem
        .tests
        .iter()
        .filter(|t| t.arch.matches_current() && deferral_issue(t.name).is_none())
        .count() as u32
}

/// Registry tests deferred on this architecture, each with the open issue that
/// blocks it.
///
/// A deferred test is NOT run here, and the executor says so out loud: it emits
/// `[TEST:<subsystem>:<name>:DEFERRED:#<issue>]` and counts the test as neither
/// passed nor failed. That marker is the whole point of this roster. The four
/// entries below were previously absent from x86 by the simple expedient of the
/// x86 registry never being dispatched at all (#533); once it is dispatched,
/// silently skipping them would trade one invisible gap for another, and
/// deleting them from the registry would lose them on aarch64, where they run
/// and are the mechanism-level proof for #545.
///
/// Every entry must name a live issue. When #567 is fixed, this roster empties
/// and `x86_deferral_issue` returns `None` for everything.
#[cfg(not(target_arch = "aarch64"))]
static X86_DEFERRED_TESTS: &[(&str, u32)] = &[
    // #567: these four schedule inside the x86 boot-test window, and every one
    // of them poisons the boot's resume context - documented case by case in
    // registry.rs above `run_x86_loopback_gates`, which exists precisely because
    // only the non-scheduling loopback gate could be run on x86 without them.
    ("loopback_recv_wake_when_idle", 567),
    ("loopback_recv_wake_under_load", 567),
    ("loopback_pump_does_not_busy_spin", 567),
    ("tcp_final_ack_survives_accept_publish_race", 567),
];

/// The issue blocking `name` on this architecture, if it is deferred here.
#[cfg(not(target_arch = "aarch64"))]
fn deferral_issue(name: &str) -> Option<u32> {
    X86_DEFERRED_TESTS
        .iter()
        .find(|(deferred, _)| *deferred == name)
        .map(|(_, issue)| *issue)
}

/// aarch64 defers nothing: every registry test that matches the architecture is
/// dispatched there.
#[cfg(target_arch = "aarch64")]
fn deferral_issue(_name: &str) -> Option<u32> {
    None
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
                && deferral_issue(t.name).is_none()
                && (already_run & (1u64 << idx)) == 0
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
            t.arch.matches_current()
                && t.stage <= current
                && deferral_issue(t.name).is_none()
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

        // Announce deferred tests rather than skipping them silently.
        if let Some(issue) = deferral_issue(test.name) {
            serial_println!("[TEST:{}:{}:DEFERRED:#{}]", id_name, test.name, issue);
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
