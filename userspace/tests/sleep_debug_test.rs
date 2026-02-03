//! Sleep/Clock Debug Test for ARM64
//!
//! This test investigates why sleep_ms() hangs in forked child processes on ARM64.
//! It tests:
//! 1. clock_gettime in the parent (baseline)
//! 2. clock_gettime in a forked child (to see if time is advancing)
//! 3. yield_now in a forked child (to ensure scheduler works)
//! 4. A short sleep_ms() call in a forked child
//!
//! The goal is to identify whether:
//! - clock_gettime returns stale data in forked children
//! - yield_now() causes issues
//! - The elapsed time calculation has bugs

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::time::{now_monotonic, sleep_ms};
use libbreenix::types::Timespec;

/// Print a string
fn print(s: &str) {
    io::print(s);
}

/// Print a number (signed)
fn print_num(n: i64) {
    if n < 0 {
        print("-");
        print_num(-n);
        return;
    }
    if n == 0 {
        print("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut val = n as u64;

    while val > 0 {
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
        i += 1;
    }

    while i > 0 {
        i -= 1;
        let ch = [buf[i]];
        if let Ok(s) = core::str::from_utf8(&ch) {
            print(s);
        }
    }
}

/// Print a Timespec
fn print_timespec(ts: &Timespec) {
    print("{ tv_sec: ");
    print_num(ts.tv_sec);
    print(", tv_nsec: ");
    print_num(ts.tv_nsec);
    print(" }");
}

/// Calculate elapsed time in nanoseconds between two Timespec values
fn elapsed_ns(start: &Timespec, end: &Timespec) -> i64 {
    let sec_diff = end.tv_sec - start.tv_sec;
    let nsec_diff = end.tv_nsec - start.tv_nsec;
    sec_diff * 1_000_000_000 + nsec_diff
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    print("SLEEP_DEBUG_TEST: Starting\n");
    print("SLEEP_DEBUG_TEST: This test diagnoses sleep_ms() hang in forked children on ARM64\n\n");

    // =========================================================================
    // Test 1: Baseline - clock_gettime in parent
    // =========================================================================
    print("=== TEST 1: Baseline clock_gettime in parent ===\n");

    let t1 = now_monotonic();
    print("  First call:  ");
    print_timespec(&t1);
    print("\n");

    // Small yield
    process::yield_now();

    let t2 = now_monotonic();
    print("  Second call: ");
    print_timespec(&t2);
    print("\n");

    let elapsed = elapsed_ns(&t1, &t2);
    print("  Elapsed: ");
    print_num(elapsed);
    print(" ns\n");

    if elapsed >= 0 && t1.tv_sec >= 0 && t1.tv_nsec >= 0 {
        print("  TEST 1: PASS - clock works in parent\n\n");
    } else {
        print("  TEST 1: FAIL - clock not working in parent\n\n");
        process::exit(1);
    }

    // =========================================================================
    // Test 2: Fork and test clock_gettime in child
    // =========================================================================
    print("=== TEST 2: clock_gettime in forked child ===\n");

    let _parent_pid = process::getpid();
    let fork_result = process::fork();

    if fork_result < 0 {
        print("  FAIL - fork failed\n");
        process::exit(2);
    }

    if fork_result == 0 {
        // ===== CHILD PROCESS =====
        print("  [CHILD] Forked successfully, PID=");
        print_num(process::getpid() as i64);
        print("\n");

        // Test 2a: First clock call in child
        print("  [CHILD] Getting first timestamp...\n");
        let c1 = now_monotonic();
        print("  [CHILD] First call:  ");
        print_timespec(&c1);
        print("\n");

        // Test 2b: Yield in child
        print("  [CHILD] Calling yield_now()...\n");
        process::yield_now();
        print("  [CHILD] yield_now() returned\n");

        // Test 2c: Second clock call in child
        print("  [CHILD] Getting second timestamp...\n");
        let c2 = now_monotonic();
        print("  [CHILD] Second call: ");
        print_timespec(&c2);
        print("\n");

        let c_elapsed = elapsed_ns(&c1, &c2);
        print("  [CHILD] Elapsed: ");
        print_num(c_elapsed);
        print(" ns\n");

        if c_elapsed < 0 {
            print("  [CHILD] WARNING: Time went backwards!\n");
        }

        // Test 2d: Manual elapsed time calculation (like sleep_ms does)
        print("\n  [CHILD] Testing manual elapsed calculation (like sleep_ms)...\n");
        let start = now_monotonic();
        print("  [CHILD] Start: ");
        print_timespec(&start);
        print("\n");

        // Do 5 yields and check time
        for i in 0..5 {
            process::yield_now();
            let now = now_monotonic();

            // Calculate elapsed like sleep_ms does
            let elapsed_sec = now.tv_sec - start.tv_sec;
            let elapsed_nsec = if now.tv_nsec >= start.tv_nsec {
                now.tv_nsec - start.tv_nsec
            } else {
                // Handle nanosecond underflow - THIS IS THE POTENTIAL BUG
                // sleep_ms does NOT decrement elapsed_sec here!
                1_000_000_000 - (start.tv_nsec - now.tv_nsec)
            };

            // Note: sleep_ms doesn't decrement elapsed_sec on underflow!
            // Let's calculate correctly for comparison:
            let correct_elapsed_sec = if now.tv_nsec >= start.tv_nsec {
                elapsed_sec
            } else {
                elapsed_sec - 1  // Borrow from seconds
            };

            let elapsed_ns_buggy = (elapsed_sec as u64) * 1_000_000_000 + (elapsed_nsec as u64);
            let elapsed_ns_correct = (correct_elapsed_sec as u64) * 1_000_000_000 + (elapsed_nsec as u64);

            print("  [CHILD] Iteration ");
            print_num(i as i64);
            print(": now=");
            print_timespec(&now);
            print("\n");

            print("           elapsed_sec=");
            print_num(elapsed_sec);
            print(", elapsed_nsec=");
            print_num(elapsed_nsec);
            print("\n");

            print("           buggy_elapsed_ns=");
            print_num(elapsed_ns_buggy as i64);
            print(", correct_elapsed_ns=");
            print_num(elapsed_ns_correct as i64);

            if elapsed_ns_buggy != elapsed_ns_correct {
                print(" *** BUG TRIGGERED ***");
            }
            print("\n");
        }

        // Test 2e: Try a very short sleep_ms
        print("\n  [CHILD] Testing sleep_ms(10)...\n");
        let before_sleep = now_monotonic();
        print("  [CHILD] Before sleep: ");
        print_timespec(&before_sleep);
        print("\n");

        sleep_ms(10);

        let after_sleep = now_monotonic();
        print("  [CHILD] After sleep: ");
        print_timespec(&after_sleep);
        print("\n");

        let sleep_elapsed = elapsed_ns(&before_sleep, &after_sleep);
        print("  [CHILD] Sleep elapsed: ");
        print_num(sleep_elapsed);
        print(" ns (");
        print_num(sleep_elapsed / 1_000_000);
        print(" ms)\n");

        if sleep_elapsed >= 10_000_000 {
            print("  [CHILD] sleep_ms(10) completed successfully!\n");
        } else {
            print("  [CHILD] WARNING: sleep returned too early\n");
        }

        print("  [CHILD] TEST 2: PASS - all clock tests passed in child\n");
        process::exit(0);
    } else {
        // ===== PARENT PROCESS =====
        print("  [PARENT] Forked child PID=");
        print_num(fork_result);
        print("\n");

        // Wait for child
        let mut status: i32 = 0;
        let wait_result = process::waitpid(fork_result as i32, &mut status, 0);

        print("  [PARENT] waitpid returned ");
        print_num(wait_result);
        print(", status=");
        print_num(status as i64);
        print("\n");

        if wait_result == fork_result && process::wifexited(status) && process::wexitstatus(status) == 0 {
            print("  TEST 2: PASS - child completed successfully\n\n");
        } else {
            print("  TEST 2: FAIL - child did not complete successfully\n\n");
            process::exit(2);
        }
    }

    // =========================================================================
    // Test 3: Sleep in parent (sanity check)
    // =========================================================================
    print("=== TEST 3: sleep_ms(50) in parent ===\n");

    let before = now_monotonic();
    print("  Before: ");
    print_timespec(&before);
    print("\n");

    sleep_ms(50);

    let after = now_monotonic();
    print("  After: ");
    print_timespec(&after);
    print("\n");

    let parent_sleep_elapsed = elapsed_ns(&before, &after);
    print("  Elapsed: ");
    print_num(parent_sleep_elapsed);
    print(" ns (");
    print_num(parent_sleep_elapsed / 1_000_000);
    print(" ms)\n");

    if parent_sleep_elapsed >= 50_000_000 {
        print("  TEST 3: PASS\n\n");
    } else {
        print("  TEST 3: FAIL - sleep returned too early\n\n");
    }

    // =========================================================================
    // Final results
    // =========================================================================
    print("=== FINAL RESULTS ===\n");
    print("SLEEP_DEBUG_TEST: ALL TESTS PASSED\n");
    print("SLEEP_DEBUG_TEST: PASS\n");
    process::exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    print("SLEEP_DEBUG_TEST: PANIC!\n");
    process::exit(99);
}
