//! WNOHANG timing test (std version)
//!
//! Tests that WNOHANG correctly returns 0 when child is still running:
//! 1. Fork child that does some work before exiting
//! 2. Immediately call waitpid with WNOHANG (should return 0 - child still running)
//! 3. Wait for child to actually exit
//! 4. Call waitpid again (should return child PID)
//!
//! This validates the distinction between:
//! - 0: children exist but none have exited yet
//! - -ECHILD: no children exist at all

const WNOHANG: i32 = 1;

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn sched_yield() -> i32;
}

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

fn main() {
    println!("=== WNOHANG Timing Test ===");

    // Step 1: Fork child that will do some work before exiting
    println!("\nStep 1: Forking child that loops before exiting...");
    let fork_result = unsafe { fork() };

    if fork_result < 0 {
        println!("  FAIL: fork() failed with error {}", fork_result);
        println!("WNOHANG_TIMING_TEST_FAILED");
        std::process::exit(1);
    }

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        println!("[CHILD] Started, doing work before exit...");

        // Do some work - yield multiple times to simulate busy work
        // This ensures the parent has time to call WNOHANG while we're running
        for i in 0..50 {
            unsafe { sched_yield(); }
            // Volatile read to prevent optimization
            std::hint::black_box(i);
        }

        println!("[CHILD] Work complete, exiting with code 99");
        std::process::exit(99);
    } else {
        // ========== PARENT PROCESS ==========
        println!("[PARENT] Forked child PID: {}", fork_result);

        // Step 2: IMMEDIATELY call waitpid with WNOHANG
        // Child should still be running (doing its loop)
        println!("\nStep 2: Immediate WNOHANG (child should still be running)...");
        let mut status: i32 = 0;
        let wnohang_result = unsafe { waitpid(fork_result, &mut status, WNOHANG) };

        println!("  waitpid(-1, WNOHANG) returned: {}", wnohang_result);

        // WNOHANG should return 0 if child exists but hasn't exited yet
        // Note: In a fast system, child might already be done, so we accept either 0 or the child PID
        let early_wait_ok = if wnohang_result == 0 {
            println!("  PASS: Returned 0 (child still running)");
            true
        } else if wnohang_result == fork_result {
            // Child finished very quickly - this is acceptable
            println!("  OK: Child already finished (fast execution)");
            false // Already reaped, skip step 3
        } else if wnohang_result == -10 {
            // ECHILD - this would be wrong since we just forked
            println!("  FAIL: Returned ECHILD but child should exist!");
            println!("WNOHANG_TIMING_TEST_FAILED");
            std::process::exit(1);
        } else {
            println!("  FAIL: Unexpected return value");
            println!("WNOHANG_TIMING_TEST_FAILED");
            std::process::exit(1);
        };

        if early_wait_ok {
            // Step 3: Now wait for child to actually finish (blocking wait)
            println!("\nStep 3: Blocking wait for child to finish...");

            let mut status2: i32 = 0;
            let wait_result = unsafe { waitpid(fork_result, &mut status2, 0) };

            println!("  waitpid returned: {}", wait_result);

            if wait_result != fork_result {
                println!("  FAIL: waitpid returned wrong PID");
                println!("WNOHANG_TIMING_TEST_FAILED");
                std::process::exit(1);
            }

            println!("  PASS: waitpid returned correct child PID");

            // Verify exit status
            if wifexited(status2) {
                let exit_code = wexitstatus(status2);
                println!("  Child exit code: {}", exit_code);

                if exit_code != 99 {
                    println!("  FAIL: Expected exit code 99");
                    println!("WNOHANG_TIMING_TEST_FAILED");
                    std::process::exit(1);
                }
                println!("  PASS: Exit code verified");
            } else {
                println!("  FAIL: Child did not exit normally");
                println!("WNOHANG_TIMING_TEST_FAILED");
                std::process::exit(1);
            }
        } else {
            // Child was already reaped in step 2, verify the status
            if wifexited(status) {
                let exit_code = wexitstatus(status);
                println!("  Child exit code: {}", exit_code);

                if exit_code != 99 {
                    println!("  FAIL: Expected exit code 99");
                    println!("WNOHANG_TIMING_TEST_FAILED");
                    std::process::exit(1);
                }
                println!("  PASS: Exit code verified");
            }
        }

        // Step 4: Now that child is reaped, WNOHANG should return ECHILD
        println!("\nStep 4: WNOHANG after child reaped (should return ECHILD)...");
        let mut status3: i32 = 0;
        let final_result = unsafe { waitpid(-1, &mut status3, WNOHANG) };

        println!("  waitpid(-1, WNOHANG) returned: {}", final_result);

        if final_result == -10 {
            println!("  PASS: Correctly returned ECHILD (no more children)");
        } else {
            println!("  FAIL: Expected -10 (ECHILD) but got different value");
            println!("WNOHANG_TIMING_TEST_FAILED");
            std::process::exit(1);
        }

        println!("\n=== All WNOHANG timing tests passed! ===");
        println!("WNOHANG_TIMING_TEST_PASSED");
        std::process::exit(0);
    }
}
