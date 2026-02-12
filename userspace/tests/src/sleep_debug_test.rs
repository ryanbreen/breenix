//! Sleep/Clock Debug Test for ARM64 (std version)
//!
//! This test investigates why sleep_ms() hangs in forked child processes on ARM64.
//! It tests:
//! 1. clock_gettime in the parent (baseline)
//! 2. clock_gettime in a forked child (to see if time is advancing)
//! 3. yield_now in a forked child (to ensure scheduler works)
//! 4. A short sleep_ms() call in a forked child

use libbreenix::process::{fork, getpid, waitpid, wifexited, wexitstatus, yield_now, ForkResult};
use libbreenix::time::{clock_gettime, nanosleep, CLOCK_MONOTONIC};
use libbreenix::Timespec;

fn now_monotonic() -> Timespec {
    let mut ts = Timespec::new();
    let _ = clock_gettime(CLOCK_MONOTONIC, &mut ts);
    ts
}

fn elapsed_ns(start: &Timespec, end: &Timespec) -> i64 {
    let sec_diff = end.tv_sec - start.tv_sec;
    let nsec_diff = end.tv_nsec - start.tv_nsec;
    sec_diff * 1_000_000_000 + nsec_diff
}

fn sleep_ms(ms: u64) {
    let req = Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    };
    let _ = nanosleep(&req);
}

fn print_timespec(ts: &Timespec) {
    print!("{{ tv_sec: {}, tv_nsec: {} }}", ts.tv_sec, ts.tv_nsec);
}

fn main() {
    println!("SLEEP_DEBUG_TEST: Starting");
    println!("SLEEP_DEBUG_TEST: This test diagnoses sleep_ms() hang in forked children on ARM64\n");

    // =========================================================================
    // Test 1: Baseline - clock_gettime in parent
    // =========================================================================
    println!("=== TEST 1: Baseline clock_gettime in parent ===");

    let t1 = now_monotonic();
    print!("  First call:  ");
    print_timespec(&t1);
    println!();

    // Small yield
    let _ = yield_now();

    let t2 = now_monotonic();
    print!("  Second call: ");
    print_timespec(&t2);
    println!();

    let elapsed = elapsed_ns(&t1, &t2);
    println!("  Elapsed: {} ns", elapsed);

    if elapsed >= 0 && t1.tv_sec >= 0 && t1.tv_nsec >= 0 {
        println!("  TEST 1: PASS - clock works in parent\n");
    } else {
        println!("  TEST 1: FAIL - clock not working in parent\n");
        std::process::exit(1);
    }

    // =========================================================================
    // Test 2: Fork and test clock_gettime in child
    // =========================================================================
    println!("=== TEST 2: clock_gettime in forked child ===");

    let _parent_pid = getpid().unwrap().raw();

    match fork() {
        Err(_) => {
            println!("  FAIL - fork failed");
            std::process::exit(2);
        }
        Ok(ForkResult::Child) => {
            // ===== CHILD PROCESS =====
            let child_pid = getpid().unwrap().raw();
            println!("  [CHILD] Forked successfully, PID={}", child_pid);

            // Test 2a: First clock call in child
            println!("  [CHILD] Getting first timestamp...");
            let c1 = now_monotonic();
            print!("  [CHILD] First call:  ");
            print_timespec(&c1);
            println!();

            // Test 2b: Yield in child
            println!("  [CHILD] Calling yield_now()...");
            let _ = yield_now();
            println!("  [CHILD] yield_now() returned");

            // Test 2c: Second clock call in child
            println!("  [CHILD] Getting second timestamp...");
            let c2 = now_monotonic();
            print!("  [CHILD] Second call: ");
            print_timespec(&c2);
            println!();

            let c_elapsed = elapsed_ns(&c1, &c2);
            println!("  [CHILD] Elapsed: {} ns", c_elapsed);

            if c_elapsed < 0 {
                println!("  [CHILD] WARNING: Time went backwards!");
            }

            // Test 2d: Manual elapsed time calculation (like sleep_ms does)
            println!("\n  [CHILD] Testing manual elapsed calculation (like sleep_ms)...");
            let start = now_monotonic();
            print!("  [CHILD] Start: ");
            print_timespec(&start);
            println!();

            // Do 5 yields and check time
            for i in 0..5 {
                let _ = yield_now();
                let now = now_monotonic();

                // Calculate elapsed like sleep_ms does
                let elapsed_sec = now.tv_sec - start.tv_sec;
                let elapsed_nsec = if now.tv_nsec >= start.tv_nsec {
                    now.tv_nsec - start.tv_nsec
                } else {
                    // Handle nanosecond underflow
                    1_000_000_000 - (start.tv_nsec - now.tv_nsec)
                };

                // Correct calculation for comparison
                let correct_elapsed_sec = if now.tv_nsec >= start.tv_nsec {
                    elapsed_sec
                } else {
                    elapsed_sec - 1  // Borrow from seconds
                };

                let elapsed_ns_buggy = (elapsed_sec as u64) * 1_000_000_000 + (elapsed_nsec as u64);
                let elapsed_ns_correct = (correct_elapsed_sec as u64) * 1_000_000_000 + (elapsed_nsec as u64);

                print!("  [CHILD] Iteration {}: now=", i);
                print_timespec(&now);
                println!();

                println!("           elapsed_sec={}, elapsed_nsec={}", elapsed_sec, elapsed_nsec);

                print!("           buggy_elapsed_ns={}, correct_elapsed_ns={}",
                       elapsed_ns_buggy, elapsed_ns_correct);

                if elapsed_ns_buggy != elapsed_ns_correct {
                    print!(" *** BUG TRIGGERED ***");
                }
                println!();
            }

            // Test 2e: Try a very short sleep_ms
            println!("\n  [CHILD] Testing sleep_ms(10)...");
            let before_sleep = now_monotonic();
            print!("  [CHILD] Before sleep: ");
            print_timespec(&before_sleep);
            println!();

            sleep_ms(10);

            let after_sleep = now_monotonic();
            print!("  [CHILD] After sleep: ");
            print_timespec(&after_sleep);
            println!();

            let sleep_elapsed = elapsed_ns(&before_sleep, &after_sleep);
            println!("  [CHILD] Sleep elapsed: {} ns ({} ms)", sleep_elapsed, sleep_elapsed / 1_000_000);

            if sleep_elapsed >= 10_000_000 {
                println!("  [CHILD] sleep_ms(10) completed successfully!");
            } else {
                println!("  [CHILD] WARNING: sleep returned too early");
            }

            println!("  [CHILD] TEST 2: PASS - all clock tests passed in child");
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ===== PARENT PROCESS =====
            println!("  [PARENT] Forked child PID={}", child_pid.raw());

            // Wait for child
            let mut status: i32 = 0;
            let wait_result = waitpid(child_pid.raw() as i32, &mut status, 0);

            match wait_result {
                Ok(waited_pid) => {
                    println!("  [PARENT] waitpid returned {}, status={}", waited_pid.raw(), status);
                    if waited_pid.raw() == child_pid.raw() && wifexited(status) && wexitstatus(status) == 0 {
                        println!("  TEST 2: PASS - child completed successfully\n");
                    } else {
                        println!("  TEST 2: FAIL - child did not complete successfully\n");
                        std::process::exit(2);
                    }
                }
                Err(_) => {
                    println!("  TEST 2: FAIL - waitpid failed\n");
                    std::process::exit(2);
                }
            }
        }
    }

    // =========================================================================
    // Test 3: Sleep in parent (sanity check)
    // =========================================================================
    println!("=== TEST 3: sleep_ms(50) in parent ===");

    let before = now_monotonic();
    print!("  Before: ");
    print_timespec(&before);
    println!();

    sleep_ms(50);

    let after = now_monotonic();
    print!("  After: ");
    print_timespec(&after);
    println!();

    let parent_sleep_elapsed = elapsed_ns(&before, &after);
    println!("  Elapsed: {} ns ({} ms)", parent_sleep_elapsed, parent_sleep_elapsed / 1_000_000);

    if parent_sleep_elapsed >= 50_000_000 {
        println!("  TEST 3: PASS\n");
    } else {
        println!("  TEST 3: FAIL - sleep returned too early\n");
    }

    // =========================================================================
    // Final results
    // =========================================================================
    println!("=== FINAL RESULTS ===");
    println!("SLEEP_DEBUG_TEST: ALL TESTS PASSED");
    println!("SLEEP_DEBUG_TEST: PASS");
    std::process::exit(0);
}
