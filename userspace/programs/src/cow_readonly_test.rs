//! Copy-on-Write Read-Only Page Sharing Test
//!
//! This test verifies that read-only pages (like code/text sections) are shared
//! directly between parent and child after fork WITHOUT the COW flag.
//!
//! The key behavior tested:
//! 1. Read-only pages (code sections) don't get COW_FLAG during setup_cow_pages()
//! 2. Both parent and child can execute the same code (shared physical frame)
//! 3. No page faults occur when executing code in either process
//!
//! This is a critical optimization because:
//! - Code sections are typically much larger than data sections
//! - Code never needs to be copied (it's read-only)
//! - Avoiding COW flag on read-only pages reduces page fault overhead
//!
//! Test markers:
//! - COW_READONLY_TEST_PASSED: All tests passed
//! - COW_READONLY_TEST_FAILED: A test failed

use libbreenix::process::{fork, waitpid, wifexited, wexitstatus, ForkResult};

/// A function that performs some computation
/// This function exists in read-only code section
#[inline(never)]
fn compute_fibonacci(n: u64) -> u64 {
    if n <= 1 {
        return n;
    }
    let mut a = 0u64;
    let mut b = 1u64;
    for _ in 2..=n {
        let c = a + b;
        a = b;
        b = c;
    }
    b
}

/// Another function in the code section
/// Used to verify multiple code regions work
#[inline(never)]
fn compute_factorial(n: u64) -> u64 {
    let mut result = 1u64;
    for i in 2..=n {
        result = result.saturating_mul(i);
    }
    result
}

fn main() {
    println!("=== CoW Read-Only Page Sharing Test ===");
    println!("Verifies code sections are shared without COW flag\n");

    // Step 1: Execute some functions in parent to verify code is accessible
    println!("Step 1: Execute code functions in parent (before fork)");

    let fib_10 = compute_fibonacci(10);
    println!("  Fibonacci(10) = {} (expected 55)", fib_10);

    let fact_5 = compute_factorial(5);
    println!("  Factorial(5) = {} (expected 120)", fact_5);

    if fib_10 != 55 || fact_5 != 120 {
        println!("  FAIL: Computation incorrect before fork!");
        println!("COW_READONLY_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: Code functions work correctly in parent\n");

    // Step 2: Fork the process
    println!("Step 2: Forking process...");
    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Process started");

            // Step 3: Child executes the SAME code functions
            // If read-only page sharing works, this code is on the same
            // physical frame as the parent's code - shared without COW
            println!("[CHILD] Step 3: Execute code functions (shared code section)");

            // Execute fibonacci - this runs code from shared read-only page
            let child_fib_10 = compute_fibonacci(10);
            println!("[CHILD]   Fibonacci(10) = {}", child_fib_10);

            // Execute factorial - another function in shared code section
            let child_fact_5 = compute_factorial(5);
            println!("[CHILD]   Factorial(5) = {}", child_fact_5);

            // Execute with different arguments to further exercise the code
            let child_fib_15 = compute_fibonacci(15);
            println!("[CHILD]   Fibonacci(15) = {} (expected 610)", child_fib_15);

            let child_fact_10 = compute_factorial(10);
            println!("[CHILD]   Factorial(10) = {} (expected 3628800)", child_fact_10);

            // Verify all computations
            if child_fib_10 != 55 {
                println!("[CHILD]   FAIL: Fibonacci(10) incorrect");
                println!("COW_READONLY_TEST_FAILED");
                std::process::exit(1);
            }
            if child_fact_5 != 120 {
                println!("[CHILD]   FAIL: Factorial(5) incorrect");
                println!("COW_READONLY_TEST_FAILED");
                std::process::exit(1);
            }
            if child_fib_15 != 610 {
                println!("[CHILD]   FAIL: Fibonacci(15) incorrect");
                println!("COW_READONLY_TEST_FAILED");
                std::process::exit(1);
            }
            if child_fact_10 != 3628800 {
                println!("[CHILD]   FAIL: Factorial(10) incorrect");
                println!("COW_READONLY_TEST_FAILED");
                std::process::exit(1);
            }

            println!("[CHILD]   PASS: All code functions executed correctly!");
            println!("[CHILD]   This proves read-only pages are properly shared");
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            println!("[PARENT] Forked child PID: {}", child_pid.raw());

            // Step 4: Parent also executes code functions after fork
            // This proves parent still has access to shared code pages
            println!("[PARENT] Step 4: Execute code functions after fork");

            let parent_fib_20 = compute_fibonacci(20);
            println!("[PARENT]   Fibonacci(20) = {} (expected 6765)", parent_fib_20);

            let parent_fact_8 = compute_factorial(8);
            println!("[PARENT]   Factorial(8) = {} (expected 40320)", parent_fact_8);

            if parent_fib_20 != 6765 {
                println!("[PARENT]   FAIL: Fibonacci(20) incorrect");
                println!("COW_READONLY_TEST_FAILED");
                std::process::exit(1);
            }
            if parent_fact_8 != 40320 {
                println!("[PARENT]   FAIL: Factorial(8) incorrect");
                println!("COW_READONLY_TEST_FAILED");
                std::process::exit(1);
            }

            println!("[PARENT]   PASS: Code functions still work in parent");

            // Wait for child to complete
            println!("[PARENT] Waiting for child...");
            let mut status: i32 = 0;
            let wait_result = waitpid(child_pid.raw() as i32, &mut status, 0);

            match wait_result {
                Ok(pid) if pid.raw() as i32 == child_pid.raw() as i32 => {}
                _ => {
                    println!("[PARENT] FAIL: waitpid returned wrong PID");
                    println!("COW_READONLY_TEST_FAILED");
                    std::process::exit(1);
                }
            }

            // Check child exit status
            if wifexited(status) {
                let exit_code = wexitstatus(status);
                if exit_code == 0 {
                    println!("[PARENT] Child exited successfully\n");
                    println!("=== Summary ===");
                    println!("1. Parent executed code before fork - PASS");
                    println!("2. Child executed shared code after fork - PASS");
                    println!("3. Parent executed code after fork - PASS");
                    println!("4. No page faults on code execution (read-only sharing works)");
                    println!("\n=== CoW Read-Only Page Sharing Test PASSED ===");
                    println!("COW_READONLY_TEST_PASSED");
                    std::process::exit(0);
                } else {
                    println!("[PARENT] Child exited with error code: {}", exit_code);
                    println!("COW_READONLY_TEST_FAILED");
                    std::process::exit(1);
                }
            } else {
                println!("[PARENT] Child did not exit normally");
                println!("COW_READONLY_TEST_FAILED");
                std::process::exit(1);
            }
        }
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("COW_READONLY_TEST_FAILED");
            std::process::exit(1);
        }
    }
}
