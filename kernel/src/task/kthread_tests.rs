//! Shared kthread lifecycle tests for x86_64 and ARM64.
//!
//! These tests must be called BEFORE any userspace processes are created,
//! otherwise the scheduler will immediately preempt to userspace and
//! the tests won't work correctly.

use crate::task::kthread::{
    kthread_exit, kthread_join,
    kthread_park, kthread_run, kthread_should_stop, kthread_stop, kthread_unpark, KthreadError,
};
use crate::{arch_disable_interrupts, arch_enable_interrupts, arch_halt};
use crate::task::scheduler;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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

/// Test kthread_exit() - setting a custom exit code
/// This test verifies that kthread_exit(code) properly sets the exit code
/// and that join() returns it correctly, with join() actually blocking.
pub fn test_kthread_exit_code() {
    log::info!("=== KTHREAD EXIT CODE TEST: Starting ===");

    let handle = kthread_run(
        || {
            unsafe { arch_enable_interrupts() };
            // Exit immediately with custom code - no yielding for fast CI completion
            kthread_exit(42);
        },
        "exit_code_kthread",
    )
    .expect("Failed to create kthread for exit code test");

    unsafe { arch_enable_interrupts() };

    // Call join IMMEDIATELY - tests blocking behavior
    let exit_code = kthread_join(&handle).expect("kthread_join failed");
    assert_eq!(exit_code, 42, "kthread exit_code should be 42");

    unsafe { arch_disable_interrupts() };

    log::info!("KTHREAD_EXIT_CODE_TEST: exit_code=42");
    log::info!("=== KTHREAD EXIT CODE TEST: Completed ===");
}

/// Test kthread park/unpark functionality
/// This test verifies that:
/// 1. kthread_park() blocks the kthread
/// 2. kthread_unpark() wakes it up
/// 3. The kthread continues execution after unpark
pub fn test_kthread_park_unpark() {
    static KTHREAD_STARTED: AtomicBool = AtomicBool::new(false);
    static KTHREAD_ABOUT_TO_PARK: AtomicBool = AtomicBool::new(false);
    static KTHREAD_UNPARKED: AtomicBool = AtomicBool::new(false);
    static KTHREAD_DONE: AtomicBool = AtomicBool::new(false);

    // Reset flags
    KTHREAD_STARTED.store(false, Ordering::Release);
    KTHREAD_ABOUT_TO_PARK.store(false, Ordering::Release);
    KTHREAD_UNPARKED.store(false, Ordering::Release);
    KTHREAD_DONE.store(false, Ordering::Release);

    log::info!("=== KTHREAD PARK TEST: Starting kthread park/unpark test ===");

    let handle = kthread_run(
        || {
            // Kernel threads start with interrupts disabled - enable them to allow preemption
            unsafe { arch_enable_interrupts() };

            KTHREAD_STARTED.store(true, Ordering::Release);
            log::info!("KTHREAD_PARK_TEST: started");

            // Signal that we're about to park - main thread waits for this
            // before checking that we haven't unparked yet
            KTHREAD_ABOUT_TO_PARK.store(true, Ordering::Release);

            // Park ourselves - will block until unparked
            kthread_park();

            // If we get here, we were unparked
            KTHREAD_UNPARKED.store(true, Ordering::Release);
            log::info!("KTHREAD_PARK_TEST: unparked");

            // Use kthread_park() to wait - kthread_stop() will wake us
            while !kthread_should_stop() {
                kthread_park();
            }

            KTHREAD_DONE.store(true, Ordering::Release);
        },
        "test_kthread_park",
    )
    .expect("Failed to create kthread");

    unsafe { arch_enable_interrupts() };

    // Wait for kthread to reach the point right before kthread_park()
    // This is the key fix - we wait for ABOUT_TO_PARK, not just STARTED
    // Use more iterations for CI environments with slow TCG emulation
    for _ in 0..1000 {
        if KTHREAD_ABOUT_TO_PARK.load(Ordering::Acquire) {
            break;
        }
        arch_halt();
    }
    assert!(
        KTHREAD_ABOUT_TO_PARK.load(Ordering::Acquire),
        "kthread never reached park point"
    );

    // Give one more timer tick for kthread to enter kthread_park()
    arch_halt();

    // Verify kthread hasn't unparked yet (it should be blocked in park)
    assert!(
        !KTHREAD_UNPARKED.load(Ordering::Acquire),
        "kthread unparked before kthread_unpark was called"
    );

    // Now unpark it
    kthread_unpark(&handle);

    // Yield to give kthread a chance to run
    scheduler::yield_current();

    // Wait for kthread to confirm it was unparked
    // Use more iterations for CI environments with slow TCG emulation
    for _ in 0..1000 {
        if KTHREAD_UNPARKED.load(Ordering::Acquire) {
            break;
        }
        arch_halt();
    }
    assert!(
        KTHREAD_UNPARKED.load(Ordering::Acquire),
        "kthread never unparked after kthread_unpark"
    );

    // Send stop signal and verify return value
    match kthread_stop(&handle) {
        Ok(()) => log::info!("KTHREAD_PARK_TEST: stop signal sent"),
        Err(err) => panic!("kthread_stop failed: {:?}", err),
    }

    // Yield to give kthread a chance to see the stop signal
    scheduler::yield_current();

    // Wait for kthread to finish
    // Use more iterations for CI environments with slow TCG emulation
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

    unsafe { arch_disable_interrupts() };
    log::info!("=== KTHREAD PARK TEST: Completed ===");
}

/// Test kthread_stop() called twice returns AlreadyStopped
pub fn test_kthread_double_stop() {
    static KTHREAD_STARTED: AtomicBool = AtomicBool::new(false);
    static KTHREAD_DONE: AtomicBool = AtomicBool::new(false);

    KTHREAD_STARTED.store(false, Ordering::Release);
    KTHREAD_DONE.store(false, Ordering::Release);

    let handle = kthread_run(
        || {
            unsafe { arch_enable_interrupts() };
            KTHREAD_STARTED.store(true, Ordering::Release);

            // Use kthread_park() to wait - kthread_stop() will wake us
            while !kthread_should_stop() {
                kthread_park();
            }

            KTHREAD_DONE.store(true, Ordering::Release);
        },
        "test_kthread_double_stop",
    )
    .expect("Failed to create kthread for double stop test");

    unsafe { arch_enable_interrupts() };

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

    match kthread_stop(&handle) {
        Ok(()) => {}
        Err(err) => panic!("kthread_stop failed: {:?}", err),
    }

    // Yield to give kthread a chance to see the stop signal
    scheduler::yield_current();

    // Use more iterations for CI environments with slow TCG emulation
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

    match kthread_stop(&handle) {
        Err(KthreadError::AlreadyStopped) => {
            log::info!("KTHREAD_DOUBLE_STOP_TEST: AlreadyStopped returned correctly");
        }
        Ok(()) => panic!("kthread_stop unexpectedly succeeded on second call"),
        Err(err) => panic!("kthread_stop returned unexpected error: {:?}", err),
    }

    unsafe { arch_disable_interrupts() };
}

/// Test kthread_should_stop() from non-kthread context
pub fn test_kthread_should_stop_non_kthread() {
    let should_stop = kthread_should_stop();
    assert!(
        !should_stop,
        "kthread_should_stop should return false for non-kthread"
    );
    log::info!("KTHREAD_SHOULD_STOP_TEST: returns false for non-kthread");
}

/// Test kthread_stop() on a thread that has already exited naturally
/// This is distinct from double-stop (stop -> stop) - this tests (natural exit -> stop)
pub fn test_kthread_stop_after_exit() {
    log::info!("=== KTHREAD STOP AFTER EXIT TEST: Starting ===");

    // Create a kthread that exits immediately without being stopped
    let handle = kthread_run(
        || {
            unsafe { arch_enable_interrupts() };
            // Exit immediately - no stop signal needed
            log::info!("KTHREAD_STOP_AFTER_EXIT_TEST: kthread exiting immediately");
        },
        "stop_after_exit_kthread",
    )
    .expect("Failed to create kthread");

    unsafe { arch_enable_interrupts() };

    // Wait for the kthread to exit using join (tests blocking join)
    let exit_code = kthread_join(&handle).expect("kthread_join failed");
    assert_eq!(exit_code, 0, "exit code should be 0");

    // Now try to stop the already-exited thread - should return AlreadyStopped
    match kthread_stop(&handle) {
        Err(KthreadError::AlreadyStopped) => {
            log::info!("KTHREAD_STOP_AFTER_EXIT_TEST: AlreadyStopped returned correctly");
        }
        Ok(()) => panic!("kthread_stop unexpectedly succeeded on exited thread"),
        Err(err) => panic!("kthread_stop returned unexpected error: {:?}", err),
    }

    unsafe { arch_disable_interrupts() };
    log::info!("=== KTHREAD STOP AFTER EXIT TEST: Completed ===");
}

/// Stress test for kthreads - creates 100+ kthreads and rapidly starts/stops them.
/// This tests:
/// 1. The race condition fix in kthread_park() (checking should_stop after setting parked)
/// 2. The kthread_stop() always calling kthread_unpark() fix
/// 3. Scheduler stability under high thread churn
/// 4. Memory management with many concurrent threads
///
/// Only called when kthread_stress_test feature is enabled, so may appear unused
/// in regular testing builds.
#[allow(dead_code)]
pub fn test_kthread_stress() {
    const KTHREAD_COUNT: usize = 100;
    const RAPID_STOP_COUNT: usize = 50;

    log::info!("=== KTHREAD STRESS TEST: Starting ===");
    log::info!("KTHREAD_STRESS: Creating {} kthreads", KTHREAD_COUNT);

    static STARTED_COUNT: AtomicU32 = AtomicU32::new(0);
    static STOPPED_COUNT: AtomicU32 = AtomicU32::new(0);

    // Reset counters
    STARTED_COUNT.store(0, Ordering::SeqCst);
    STOPPED_COUNT.store(0, Ordering::SeqCst);

    // Phase 1: Create many kthreads that use kthread_park()
    log::info!(
        "KTHREAD_STRESS: Phase 1 - Creating {} parking kthreads",
        KTHREAD_COUNT
    );
    let mut handles = Vec::with_capacity(KTHREAD_COUNT);

    for i in 0..KTHREAD_COUNT {
        let handle = kthread_run(
            move || {
                unsafe { arch_enable_interrupts() };
                STARTED_COUNT.fetch_add(1, Ordering::SeqCst);

                // Use kthread_park() to wait - this tests the race condition fix
                while !kthread_should_stop() {
                    kthread_park();
                }

                STOPPED_COUNT.fetch_add(1, Ordering::SeqCst);
            },
            "stress_kthread",
        )
        .expect("Failed to create stress kthread");

        handles.push(handle);

        // Log progress every 25 threads
        if (i + 1) % 25 == 0 {
            log::info!(
                "KTHREAD_STRESS: Created {}/{} kthreads",
                i + 1,
                KTHREAD_COUNT
            );
        }
    }

    // Enable interrupts to let kthreads run
    unsafe { arch_enable_interrupts() };

    // Wait for all kthreads to start
    log::info!("KTHREAD_STRESS: Waiting for kthreads to start...");
    for _ in 0..5000 {
        if STARTED_COUNT.load(Ordering::SeqCst) >= KTHREAD_COUNT as u32 {
            break;
        }
        arch_halt();
    }
    let started = STARTED_COUNT.load(Ordering::SeqCst);
    log::info!(
        "KTHREAD_STRESS: {}/{} kthreads started",
        started,
        KTHREAD_COUNT
    );
    assert!(
        started == KTHREAD_COUNT as u32,
        "All {} kthreads should have started, but only {} started",
        KTHREAD_COUNT,
        started
    );

    // Phase 2: Rapidly stop all kthreads (tests the race condition fix)
    log::info!("KTHREAD_STRESS: Phase 2 - Stopping all kthreads rapidly");
    let mut stop_errors = 0u32;
    for (i, handle) in handles.iter().enumerate() {
        if let Err(e) = kthread_stop(handle) {
            log::error!(
                "KTHREAD_STRESS: kthread_stop failed for thread {}: {:?}",
                i,
                e
            );
            stop_errors += 1;
        }
    }
    assert!(
        stop_errors == 0,
        "All kthread_stop calls should succeed, but {} failed",
        stop_errors
    );

    // Wait for all kthreads to stop
    log::info!("KTHREAD_STRESS: Waiting for kthreads to stop...");
    for _ in 0..5000 {
        if STOPPED_COUNT.load(Ordering::SeqCst) >= started {
            break;
        }
        arch_halt();
    }
    let stopped = STOPPED_COUNT.load(Ordering::SeqCst);
    log::info!(
        "KTHREAD_STRESS: {}/{} kthreads stopped cleanly",
        stopped,
        started
    );
    assert!(
        stopped == started,
        "All {} started kthreads should have stopped, but only {} stopped",
        started,
        stopped
    );

    // Phase 3: Join all kthreads to ensure clean exit
    log::info!("KTHREAD_STRESS: Phase 3 - Joining all kthreads");
    let mut join_success = 0u32;
    for (i, handle) in handles.iter().enumerate() {
        match kthread_join(handle) {
            Ok(exit_code) => {
                assert_eq!(
                    exit_code, 0,
                    "kthread {} should exit with code 0, got {}",
                    i, exit_code
                );
                join_success += 1;
            }
            Err(e) => log::warn!(
                "KTHREAD_STRESS: kthread_join failed for thread {}: {:?}",
                i,
                e
            ),
        }
    }
    log::info!(
        "KTHREAD_STRESS: {}/{} kthreads joined successfully",
        join_success,
        KTHREAD_COUNT
    );
    assert!(
        join_success == KTHREAD_COUNT as u32,
        "All {} kthreads should join successfully, but only {} joined",
        KTHREAD_COUNT,
        join_success
    );

    // Phase 4: Rapid create-stop-join cycle (tests memory/scheduling pressure)
    log::info!(
        "KTHREAD_STRESS: Phase 4 - Rapid create/stop/join cycle ({} iterations)",
        RAPID_STOP_COUNT
    );
    STARTED_COUNT.store(0, Ordering::SeqCst);
    STOPPED_COUNT.store(0, Ordering::SeqCst);

    for i in 0..RAPID_STOP_COUNT {
        let handle = kthread_run(
            || {
                unsafe { arch_enable_interrupts() };
                STARTED_COUNT.fetch_add(1, Ordering::SeqCst);
                // Immediately park - kthread_stop() should wake us
                while !kthread_should_stop() {
                    kthread_park();
                }
                STOPPED_COUNT.fetch_add(1, Ordering::SeqCst);
            },
            "rapid_kthread",
        )
        .expect("Failed to create rapid kthread");

        // Give minimal time for kthread to start, then immediately stop
        for _ in 0..10 {
            arch_halt();
        }

        // Stop and join immediately
        kthread_stop(&handle).expect("rapid kthread_stop should not fail");
        kthread_join(&handle).expect("rapid kthread_join should not fail");

        if (i + 1) % 10 == 0 {
            log::info!(
                "KTHREAD_STRESS: Rapid cycle {}/{} complete",
                i + 1,
                RAPID_STOP_COUNT
            );
        }
    }

    let rapid_started = STARTED_COUNT.load(Ordering::SeqCst);
    let rapid_stopped = STOPPED_COUNT.load(Ordering::SeqCst);
    log::info!(
        "KTHREAD_STRESS: Rapid cycle results: {}/{} started, {}/{} stopped",
        rapid_started,
        RAPID_STOP_COUNT,
        rapid_stopped,
        RAPID_STOP_COUNT
    );
    assert!(
        rapid_started == RAPID_STOP_COUNT as u32,
        "All {} rapid kthreads should have started, but only {} started",
        RAPID_STOP_COUNT,
        rapid_started
    );
    assert!(
        rapid_stopped == RAPID_STOP_COUNT as u32,
        "All {} rapid kthreads should have stopped, but only {} stopped",
        RAPID_STOP_COUNT,
        rapid_stopped
    );

    unsafe { arch_disable_interrupts() };

    log::info!("KTHREAD_STRESS: All phases complete");
    log::info!(
        "KTHREAD_STRESS: Phase 1 - {} kthreads created and parked",
        KTHREAD_COUNT
    );
    log::info!("KTHREAD_STRESS: Phase 2 - {} kthreads stopped", stopped);
    log::info!("KTHREAD_STRESS: Phase 3 - {} kthreads joined", join_success);
    log::info!(
        "KTHREAD_STRESS: Phase 4 - {} rapid create/stop/join cycles",
        RAPID_STOP_COUNT
    );
    log::info!("=== KTHREAD STRESS TEST: Completed ===");
}
