//! Interval timer (setitimer/getitimer) syscall test program (std version)
//!
//! Tests the setitimer() and getitimer() syscalls:
//! 1. setitimer(ITIMER_REAL) fires SIGALRM after the specified interval
//! 2. Interval timers rearm automatically (it_interval field)
//! 3. getitimer() returns the current timer value
//! 4. Setting a zero timer value disables the timer
//! 5. ITIMER_VIRTUAL and ITIMER_PROF return ENOSYS (not implemented)

use std::sync::atomic::{AtomicU32, Ordering};

use libbreenix::signal::{SIGALRM, ITIMER_REAL, ITIMER_VIRTUAL, ITIMER_PROF};
use libbreenix::{sigaction, setitimer, getitimer, Sigaction, Itimerval, Timeval};
use libbreenix::process::yield_now;

/// Static counter to track SIGALRM deliveries
static ALARM_COUNT: AtomicU32 = AtomicU32::new(0);

/// SIGALRM handler
extern "C" fn sigalrm_handler(_sig: i32) {
    ALARM_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn main() {
    println!("=== Interval Timer (setitimer/getitimer) Test ===");

    // Test 1: Register SIGALRM handler
    println!("\nTest 1: Register SIGALRM handler");
    let action = Sigaction::new(sigalrm_handler);

    if sigaction(SIGALRM, Some(&action), None).is_err() {
        println!("  FAIL: sigaction returned error");
        println!("ITIMER_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered handler");

    // Test 2: ITIMER_VIRTUAL should return ENOSYS
    println!("\nTest 2: ITIMER_VIRTUAL should return ENOSYS");
    let zero_timer = Itimerval {
        it_interval: Timeval { tv_sec: 0, tv_usec: 0 },
        it_value: Timeval { tv_sec: 1, tv_usec: 0 },
    };
    if setitimer(ITIMER_VIRTUAL, &zero_timer, None).is_ok() {
        println!("  FAIL: ITIMER_VIRTUAL should not be implemented");
        println!("ITIMER_TEST_FAILED");
        std::process::exit(1);
    } else {
        println!("  PASS: ITIMER_VIRTUAL returned error");
    }

    // Test 3: ITIMER_PROF should return ENOSYS
    println!("\nTest 3: ITIMER_PROF should return ENOSYS");
    if setitimer(ITIMER_PROF, &zero_timer, None).is_ok() {
        println!("  FAIL: ITIMER_PROF should not be implemented");
        println!("ITIMER_TEST_FAILED");
        std::process::exit(1);
    } else {
        println!("  PASS: ITIMER_PROF returned error");
    }

    // Test 4: Set repeating interval timer (100ms initial, 50ms interval)
    println!("\nTest 4: Set repeating ITIMER_REAL");
    let repeating_timer = Itimerval {
        it_interval: Timeval { tv_sec: 0, tv_usec: 50_000 }, // 50ms repeat
        it_value: Timeval { tv_sec: 0, tv_usec: 100_000 },   // 100ms initial
    };

    let mut old_value = Itimerval::default();
    if setitimer(ITIMER_REAL, &repeating_timer, Some(&mut old_value)).is_ok() {
        println!("  PASS: setitimer(ITIMER_REAL) succeeded");
        println!("  Previous timer: {}s {}us", old_value.it_value.tv_sec, old_value.it_value.tv_usec);
    } else {
        println!("  FAIL: setitimer returned error");
        println!("ITIMER_TEST_FAILED");
        std::process::exit(1);
    }

    // Test 5: Wait for multiple SIGALRM deliveries
    println!("\nTest 5: Waiting for multiple SIGALRM deliveries...");
    println!("  (Should see 4-6 alarms in ~400ms)");

    for _ in 0..500 {
        let _ = yield_now();
        if ALARM_COUNT.load(Ordering::SeqCst) >= 4 {
            break;
        }
    }

    let count = ALARM_COUNT.load(Ordering::SeqCst);
    if count >= 4 {
        println!("  Received {} SIGALRM deliveries", count);
        println!("  PASS: Repeating timer works!");
    } else {
        println!("  FAIL: Only received {} alarms (expected >= 4)", count);
        println!("ITIMER_TEST_FAILED");
        std::process::exit(1);
    }

    // Test 6: getitimer() should return non-zero remaining time
    println!("\nTest 6: getitimer() returns current timer value");
    let mut curr_value = Itimerval::default();
    if getitimer(ITIMER_REAL, &mut curr_value).is_ok() {
        println!("  Current timer: {}s {}us", curr_value.it_value.tv_sec, curr_value.it_value.tv_usec);
        println!("  Interval: {}s {}us", curr_value.it_interval.tv_sec, curr_value.it_interval.tv_usec);

        // Should have non-zero interval (50ms = 50000us)
        if curr_value.it_interval.tv_usec > 0 {
            println!("  PASS: getitimer() shows active timer");
        } else {
            println!("  FAIL: it_interval is zero (expected 50000us)");
            println!("ITIMER_TEST_FAILED");
            std::process::exit(1);
        }
    } else {
        println!("  FAIL: getitimer returned error");
        println!("ITIMER_TEST_FAILED");
        std::process::exit(1);
    }

    // Test 7: Cancel timer by setting to zero
    println!("\nTest 7: Cancel timer with zero value");
    let cancel_timer = Itimerval::default(); // All zeros
    if setitimer(ITIMER_REAL, &cancel_timer, None).is_ok() {
        println!("  PASS: setitimer(0) succeeded");
    } else {
        println!("  FAIL: setitimer(0) returned error");
        println!("ITIMER_TEST_FAILED");
        std::process::exit(1);
    }

    // Test 8: Verify no more SIGALRM after cancellation
    println!("\nTest 8: Verify no more SIGALRM after cancellation");
    let count_before = ALARM_COUNT.load(Ordering::SeqCst);
    println!("  Alarms before wait: {}", count_before);

    // Wait ~200ms (should NOT get any more alarms)
    for _ in 0..200 {
        let _ = yield_now();
    }

    let count_after = ALARM_COUNT.load(Ordering::SeqCst);
    println!("  Alarms after wait: {}", count_after);

    if count_after == count_before {
        println!("  PASS: No new alarms after cancellation");
    } else {
        println!("  FAIL: Got {} unexpected alarms", count_after - count_before);
        println!("ITIMER_TEST_FAILED");
        std::process::exit(1);
    }

    // Test 9: Verify getitimer() shows timer is disabled
    println!("\nTest 9: getitimer() confirms timer is disabled");
    let mut disabled_value = Itimerval::default();
    if getitimer(ITIMER_REAL, &mut disabled_value).is_ok() {
        if disabled_value.it_value.tv_sec == 0 && disabled_value.it_value.tv_usec == 0 {
            println!("  PASS: Timer is disabled (it_value = 0)");
        } else {
            println!("  FAIL: Timer still active: {}s {}us",
                     disabled_value.it_value.tv_sec, disabled_value.it_value.tv_usec);
            println!("ITIMER_TEST_FAILED");
            std::process::exit(1);
        }
    } else {
        println!("  FAIL: getitimer returned error");
        println!("ITIMER_TEST_FAILED");
        std::process::exit(1);
    }

    // All tests passed
    println!();
    println!("=== All Interval Timer Tests PASSED ===");
    println!("ITIMER_TEST_PASSED");
    std::process::exit(0);
}
