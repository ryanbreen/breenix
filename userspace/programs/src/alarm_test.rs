//! Alarm syscall test program (std version)
//!
//! Tests the alarm() syscall:
//! 1. Set an alarm for 1 second
//! 2. Register SIGALRM handler
//! 3. Wait for the alarm to fire
//! 4. Verify the handler was called

use std::sync::atomic::{AtomicU32, Ordering};

use libbreenix::signal::SIGALRM;
use libbreenix::{alarm, sigaction, Sigaction};
use libbreenix::process::yield_now;

/// Static counter to track how many SIGALRM signals were received
static ALARM_COUNT: AtomicU32 = AtomicU32::new(0);

/// SIGALRM handler
extern "C" fn sigalrm_handler(_sig: i32) {
    let count = ALARM_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    println!("  HANDLER: SIGALRM received (count={})", count);
}

fn main() {
    println!("=== Alarm Syscall Test ===");

    // Test 1: Register SIGALRM handler
    println!("\nTest 1: Register SIGALRM handler");
    let action = Sigaction::new(sigalrm_handler);

    if sigaction(SIGALRM, Some(&action), None).is_err() {
        println!("  FAIL: sigaction returned error");
        println!("ALARM_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered handler");

    // Test 2: Set alarm for 1 second
    println!("\nTest 2: Set alarm for 1 second");
    let prev = alarm(1);
    println!("  Previous alarm value: {} seconds", prev);
    println!("  PASS: alarm(1) called");

    // Test 3: Wait for alarm (busy wait with yields)
    println!("\nTest 3: Waiting for SIGALRM delivery...");

    // Wait up to ~3 seconds (3000 yields at ~1ms each)
    for i in 0..3000 {
        let _ = yield_now();

        if ALARM_COUNT.load(Ordering::SeqCst) > 0 {
            println!("  Alarm received after ~{}.{} seconds", i / 1000, (i % 1000) / 100);
            break;
        }
    }

    // Test 4: Verify alarm was received
    println!("\nTest 4: Verify SIGALRM delivery");
    if ALARM_COUNT.load(Ordering::SeqCst) > 0 {
        println!("  PASS: SIGALRM was delivered!");
    } else {
        println!("  FAIL: SIGALRM was NOT received within timeout");
        println!("ALARM_TEST_FAILED");
        std::process::exit(1);
    }

    // Test 5: alarm(0) cancels pending alarm
    println!("\nTest 5: alarm(0) cancels pending alarm");
    ALARM_COUNT.store(0, Ordering::SeqCst);
    let prev = alarm(5);  // Set alarm for 5 seconds
    println!("  Set alarm(5), previous value: {}", prev);

    let cancelled = alarm(0);  // Cancel with alarm(0)
    println!("  Called alarm(0), returned: {} seconds remaining", cancelled);

    // Wait ~2 seconds to ensure alarm would have fired if not cancelled
    for _ in 0..2000 {
        let _ = yield_now();
    }

    if ALARM_COUNT.load(Ordering::SeqCst) == 0 {
        println!("  PASS: No SIGALRM after alarm(0) cancellation");
    } else {
        println!("  FAIL: SIGALRM was received after cancellation");
        println!("ALARM_TEST_FAILED");
        std::process::exit(1);
    }

    // Test 6: alarm() returns remaining seconds from previous alarm
    println!("\nTest 6: alarm() returns remaining seconds");
    ALARM_COUNT.store(0, Ordering::SeqCst);
    let prev = alarm(10);  // Set alarm for 10 seconds
    println!("  Set alarm(10), previous value: {}", prev);

    // Yield a few times (~100ms)
    for _ in 0..100 {
        let _ = yield_now();
    }

    let remaining = alarm(5);  // Replace with alarm(5)
    println!("  Called alarm(5) after brief wait, returned: {} seconds remaining", remaining);

    if remaining > 0 && remaining <= 10 {
        println!("  PASS: alarm() returned remaining seconds from previous alarm");
    } else {
        println!("  FAIL: Expected remaining > 0 and <= 10, got {}", remaining);
        println!("ALARM_TEST_FAILED");
        std::process::exit(1);
    }

    // Cancel the pending alarm before next test
    alarm(0);

    // Test 7: alarm() replaces existing alarm
    println!("\nTest 7: alarm() replaces existing alarm");
    ALARM_COUNT.store(0, Ordering::SeqCst);
    let prev = alarm(10);  // Set alarm for 10 seconds
    println!("  Set alarm(10), previous value: {}", prev);

    let prev2 = alarm(1);  // Replace with alarm(1)
    println!("  Set alarm(1), previous value: {}", prev2);

    // Wait ~2.5 seconds - should see exactly 1 SIGALRM from the alarm(1)
    for _ in 0..2500 {
        let _ = yield_now();
    }

    let count = ALARM_COUNT.load(Ordering::SeqCst);
    println!("  SIGALRM count after ~2.5 seconds: {}", count);

    if count == 1 {
        println!("  PASS: Exactly 1 SIGALRM received (alarm replaced)");
    } else {
        println!("  FAIL: Expected exactly 1 SIGALRM, got {}", count);
        println!("ALARM_TEST_FAILED");
        std::process::exit(1);
    }

    // All tests passed
    println!();
    println!("=== All Alarm Tests PASSED ===");
    println!("ALARM_TEST_PASSED");
    std::process::exit(0);
}
