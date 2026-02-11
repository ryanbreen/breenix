//! Shared workqueue tests for x86_64 and ARM64.
//!
//! Tests the Linux-style work queue implementation:
//! 1. Basic work execution via system workqueue
//! 2. Multiple work items execute in order
//! 3. Flush waits for all pending work
//! 4. Re-queue rejection while work is pending
//! 5. Multi-item flush
//! 6. Shutdown with new workqueue
//! 7. Error path re-queue

use alloc::sync::Arc;
use crate::task::kthread::{arch_disable_interrupts, arch_enable_interrupts, arch_halt};
use crate::task::workqueue::{
    flush_system_workqueue, schedule_work, schedule_work_fn, Work, Workqueue, WorkqueueFlags,
};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub fn test_workqueue() {
    static EXEC_COUNT: AtomicU32 = AtomicU32::new(0);
    static EXEC_ORDER: [AtomicU32; 3] = [
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
    ];

    // Reset counters
    EXEC_COUNT.store(0, Ordering::SeqCst);
    for order in &EXEC_ORDER {
        order.store(0, Ordering::SeqCst);
    }

    log::info!("=== WORKQUEUE TEST: Starting workqueue test ===");

    // Enable interrupts so worker thread can run
    unsafe { arch_enable_interrupts(); }

    // Test 1: Basic execution
    log::info!("WORKQUEUE_TEST: Testing basic execution...");
    let work1 = schedule_work_fn(
        || {
            EXEC_COUNT.fetch_add(1, Ordering::SeqCst);
            log::info!("WORKQUEUE_TEST: work1 executed");
        },
        "test_work1",
    );

    // Wait for work1 to complete
    work1.wait();
    let count = EXEC_COUNT.load(Ordering::SeqCst);
    assert_eq!(count, 1, "work1 should have executed once");
    log::info!("WORKQUEUE_TEST: basic execution passed");

    // Test 2: Multiple work items
    log::info!("WORKQUEUE_TEST: Testing multiple work items...");
    let work2 = schedule_work_fn(
        || {
            let order = EXEC_COUNT.fetch_add(1, Ordering::SeqCst);
            EXEC_ORDER[0].store(order, Ordering::SeqCst);
            log::info!("WORKQUEUE_TEST: work2 executed (order={})", order);
        },
        "test_work2",
    );

    let work3 = schedule_work_fn(
        || {
            let order = EXEC_COUNT.fetch_add(1, Ordering::SeqCst);
            EXEC_ORDER[1].store(order, Ordering::SeqCst);
            log::info!("WORKQUEUE_TEST: work3 executed (order={})", order);
        },
        "test_work3",
    );

    let work4 = schedule_work_fn(
        || {
            let order = EXEC_COUNT.fetch_add(1, Ordering::SeqCst);
            EXEC_ORDER[2].store(order, Ordering::SeqCst);
            log::info!("WORKQUEUE_TEST: work4 executed (order={})", order);
        },
        "test_work4",
    );

    // Wait for all work items
    work2.wait();
    work3.wait();
    work4.wait();

    let final_count = EXEC_COUNT.load(Ordering::SeqCst);
    assert_eq!(final_count, 4, "all 4 work items should have executed");

    // Verify execution order (work2 < work3 < work4)
    let order2 = EXEC_ORDER[0].load(Ordering::SeqCst);
    let order3 = EXEC_ORDER[1].load(Ordering::SeqCst);
    let order4 = EXEC_ORDER[2].load(Ordering::SeqCst);
    assert!(order2 < order3, "work2 should execute before work3");
    assert!(order3 < order4, "work3 should execute before work4");
    log::info!("WORKQUEUE_TEST: multiple work items passed");

    // Test 3: Flush functionality
    log::info!("WORKQUEUE_TEST: Testing flush...");
    static FLUSH_WORK_DONE: AtomicU32 = AtomicU32::new(0);
    FLUSH_WORK_DONE.store(0, Ordering::SeqCst);

    let _flush_work = schedule_work_fn(
        || {
            FLUSH_WORK_DONE.fetch_add(1, Ordering::SeqCst);
            log::info!("WORKQUEUE_TEST: flush_work executed");
        },
        "flush_work",
    );

    // Flush should wait for the work to complete
    flush_system_workqueue();

    let flush_done = FLUSH_WORK_DONE.load(Ordering::SeqCst);
    assert_eq!(flush_done, 1, "flush should have waited for work to complete");
    log::info!("WORKQUEUE_TEST: flush completed");

    // Test 4: Re-queue rejection test
    log::info!("WORKQUEUE_TEST: Testing re-queue rejection...");
    static REQUEUE_BLOCK: AtomicBool = AtomicBool::new(false);
    REQUEUE_BLOCK.store(false, Ordering::SeqCst);
    let requeue_work = schedule_work_fn(
        || {
            while !REQUEUE_BLOCK.load(Ordering::Acquire) {
                arch_halt();
            }
        },
        "requeue_work",
    );
    let requeue_work_clone = Arc::clone(&requeue_work);
    let requeue_accepted = schedule_work(requeue_work_clone);
    assert!(
        !requeue_accepted,
        "re-queue should be rejected while work is pending"
    );
    REQUEUE_BLOCK.store(true, Ordering::Release);
    requeue_work.wait();
    log::info!("WORKQUEUE_TEST: re-queue rejection passed");

    // Test 5: Multi-item flush test
    log::info!("WORKQUEUE_TEST: Testing multi-item flush...");
    static MULTI_FLUSH_COUNT: AtomicU32 = AtomicU32::new(0);
    MULTI_FLUSH_COUNT.store(0, Ordering::SeqCst);
    for _ in 0..6 {
        let _work = schedule_work_fn(
            || {
                MULTI_FLUSH_COUNT.fetch_add(1, Ordering::SeqCst);
            },
            "multi_flush_work",
        );
    }
    flush_system_workqueue();
    let multi_flush_done = MULTI_FLUSH_COUNT.load(Ordering::SeqCst);
    assert_eq!(
        multi_flush_done, 6,
        "multi-item flush should execute all work items"
    );
    log::info!("WORKQUEUE_TEST: multi-item flush passed");

    // Test 6: Shutdown test - creates a NEW workqueue (not system workqueue)
    // This tests that new kthreads created later in boot can execute
    log::info!("WORKQUEUE_TEST: Testing shutdown with new workqueue...");
    {
        static SHUTDOWN_WORK_DONE: AtomicBool = AtomicBool::new(false);
        SHUTDOWN_WORK_DONE.store(false, Ordering::SeqCst);

        // Create a NEW workqueue (not the system workqueue)
        let test_wq = Workqueue::new("test_wq", WorkqueueFlags::default());
        log::info!("WORKQUEUE_TEST: Created new workqueue 'test_wq'");

        // Queue work to it - this will spawn a NEW kworker thread
        let work = Work::new(
            || {
                log::info!("WORKQUEUE_TEST: shutdown work executing!");
                SHUTDOWN_WORK_DONE.store(true, Ordering::SeqCst);
            },
            "shutdown_work",
        );

        log::info!("WORKQUEUE_TEST: Queuing work to new workqueue...");
        let queued = test_wq.queue(Arc::clone(&work));
        assert!(queued, "work should be queued successfully");

        // Wait for the work to complete
        log::info!("WORKQUEUE_TEST: Waiting for work completion...");
        work.wait();

        // Verify work executed
        let done = SHUTDOWN_WORK_DONE.load(Ordering::SeqCst);
        assert!(done, "shutdown work should have executed");

        // Destroy the workqueue (tests clean shutdown)
        log::info!("WORKQUEUE_TEST: Destroying workqueue...");
        test_wq.destroy();

        // Test idempotent destroy - calling destroy() twice should be safe
        test_wq.destroy(); // Second call - should be no-op, not panic or hang
        log::info!("WORKQUEUE_TEST: idempotent destroy passed");

        // Test flush after destroy - should return immediately, not hang
        test_wq.flush();
        log::info!("WORKQUEUE_TEST: flush after destroy passed");

        log::info!("WORKQUEUE_TEST: shutdown test passed");
    }

    // Test 7: Error path test
    log::info!("WORKQUEUE_TEST: Testing error path re-queue...");
    static ERROR_PATH_BLOCK: AtomicBool = AtomicBool::new(false);
    ERROR_PATH_BLOCK.store(false, Ordering::SeqCst);
    let error_work = Work::new(
        || {
            while !ERROR_PATH_BLOCK.load(Ordering::Acquire) {
                arch_halt();
            }
        },
        "error_path_work",
    );
    let first_schedule = schedule_work(Arc::clone(&error_work));
    assert!(first_schedule, "schedule_work should accept idle work");
    let second_schedule = schedule_work(Arc::clone(&error_work));
    assert!(
        !second_schedule,
        "schedule_work should reject re-queue while work is pending"
    );
    ERROR_PATH_BLOCK.store(true, Ordering::Release);
    error_work.wait();
    log::info!("WORKQUEUE_TEST: error path test passed");

    unsafe { arch_disable_interrupts(); }
    log::info!("WORKQUEUE_TEST: all tests passed");
    log::info!("=== WORKQUEUE TEST: Completed ===");
}
