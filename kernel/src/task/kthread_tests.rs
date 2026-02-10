//! Shared kthread lifecycle tests for x86_64 and ARM64.
//!
//! These tests must be called BEFORE any userspace processes are created,
//! otherwise the scheduler will immediately preempt to userspace and
//! the tests won't work correctly.

use crate::task::kthread::{
    arch_disable_interrupts, arch_enable_interrupts, arch_halt, kthread_park, kthread_run,
    kthread_should_stop, kthread_stop,
};
use crate::task::scheduler;
use core::sync::atomic::{AtomicBool, Ordering};

/// Test kernel thread lifecycle
///
/// 1. Creates a kthread (added to ready queue)
/// 2. Enables interrupts so the kthread can be scheduled
/// 3. Waits for kthread to run and complete
/// 4. Disables interrupts for cleanup
pub fn test_kthread_lifecycle() {
    static KTHREAD_STARTED: AtomicBool = AtomicBool::new(false);
    static KTHREAD_DONE: AtomicBool = AtomicBool::new(false);

    // Reset flags in case test runs multiple times
    KTHREAD_STARTED.store(false, Ordering::Release);
    KTHREAD_DONE.store(false, Ordering::Release);

    log::info!("=== KTHREAD TEST: Starting kernel thread lifecycle test ===");

    let handle = kthread_run(
        || {
            KTHREAD_STARTED.store(true, Ordering::Release);
            log::info!("KTHREAD_RUN: kthread running");

            // Kernel threads start with interrupts disabled - enable them to allow preemption
            unsafe { arch_enable_interrupts() };

            // Use kthread_park() instead of bare HLT. This allows kthread_stop()
            // to wake us immediately via kthread_unpark(), rather than waiting for
            // a timer interrupt (which can be slow on TCG emulation in CI).
            while !kthread_should_stop() {
                kthread_park();
            }

            // Verify kthread_should_stop() is actually true
            let stopped = kthread_should_stop();
            assert!(stopped, "kthread_should_stop() must be true after loop exit");
            log::info!("KTHREAD_VERIFY: kthread_should_stop() = {}", stopped);
            log::info!("KTHREAD_STOP: kthread received stop signal");
            KTHREAD_DONE.store(true, Ordering::Release);
            log::info!("KTHREAD_EXIT: kthread exited cleanly");
        },
        "test_kthread",
    )
    .expect("Failed to create kthread");

    log::info!("KTHREAD_CREATE: kthread created");

    // Enable interrupts so the scheduler can run the kthread
    unsafe { arch_enable_interrupts() };

    // Step 1: Wait for kthread to START running
    // Use more iterations for CI environments with slow TCG emulation
    for _ in 0..1000 {
        if KTHREAD_STARTED.load(Ordering::Acquire) {
            break;
        }
        arch_halt();
    }
    assert!(
        KTHREAD_STARTED.load(Ordering::Acquire),
        "kthread never started"
    );

    // Step 2: Send stop signal
    // kthread_stop() now always calls kthread_unpark(), so the kthread
    // will be woken immediately from kthread_park() to check should_stop.
    match kthread_stop(&handle) {
        Ok(()) => log::info!("KTHREAD_STOP_SENT: stop signal sent successfully"),
        Err(err) => panic!("kthread_stop failed: {:?}", err),
    }

    // Yield to give kthread a chance to run
    scheduler::yield_current();
    arch_halt();

    // Step 3: Wait for kthread to finish
    for _ in 0..1000 {
        if KTHREAD_DONE.load(Ordering::Acquire) {
            break;
        }
        arch_halt();
    }
    assert!(
        KTHREAD_DONE.load(Ordering::Acquire),
        "kthread never finished after stop signal"
    );

    // Disable interrupts before returning to kernel initialization
    unsafe { arch_disable_interrupts() };
    log::info!("=== KTHREAD TEST: Completed ===");
}

/// Test kthread_join() - waiting for a kthread to exit
/// This test verifies that join() actually BLOCKS until the kthread exits,
/// not just that it returns the correct exit code.
pub fn test_kthread_join() {
    use crate::task::kthread::kthread_join;

    log::info!("=== KTHREAD JOIN TEST: Starting ===");

    // Create a kthread that exits immediately - no yielding to avoid
    // slow scheduling on TCG emulation in CI environments
    let handle = kthread_run(
        || {
            // Kernel threads start with interrupts disabled - enable them for preemption
            unsafe { arch_enable_interrupts() };
            log::info!("KTHREAD_JOIN_TEST: kthread about to exit");
            // Exit immediately - don't yield, to ensure fast completion on slow TCG
        },
        "join_test_kthread",
    )
    .expect("Failed to create kthread for join test");

    // Enable interrupts so the kthread can be scheduled
    unsafe { arch_enable_interrupts() };

    // Call join IMMEDIATELY - this tests the blocking behavior.
    // join() uses HLT internally, which allows timer interrupts to schedule
    // the kthread. We do NOT pre-wait for the kthread to exit.
    let exit_code = kthread_join(&handle).expect("kthread_join failed");
    assert_eq!(exit_code, 0, "kthread exit_code should be 0");

    // Disable interrupts before returning
    unsafe { arch_disable_interrupts() };

    log::info!("KTHREAD_JOIN_TEST: join returned exit_code={}", exit_code);
    log::info!("=== KTHREAD JOIN TEST: Completed ===");
}
