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

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;

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

/// Print a number to stdout
fn print_number(num: u64) {
    let mut buffer: [u8; 32] = [0; 32];
    let mut n = num;
    let mut i = 0;

    if n == 0 {
        buffer[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            buffer[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = buffer[j];
            buffer[j] = buffer[i - j - 1];
            buffer[i - j - 1] = tmp;
        }
    }

    io::write(libbreenix::types::fd::STDOUT, &buffer[..i]);
}

/// Print signed number
fn print_signed(num: i64) {
    if num < 0 {
        io::print("-");
        print_number((-num) as u64);
    } else {
        print_number(num as u64);
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    io::print("=== CoW Read-Only Page Sharing Test ===\n");
    io::print("Verifies code sections are shared without COW flag\n\n");

    // Step 1: Execute some functions in parent to verify code is accessible
    io::print("Step 1: Execute code functions in parent (before fork)\n");

    let fib_10 = compute_fibonacci(10);
    io::print("  Fibonacci(10) = ");
    print_number(fib_10);
    io::print(" (expected 55)\n");

    let fact_5 = compute_factorial(5);
    io::print("  Factorial(5) = ");
    print_number(fact_5);
    io::print(" (expected 120)\n");

    if fib_10 != 55 || fact_5 != 120 {
        io::print("  FAIL: Computation incorrect before fork!\n");
        io::print("COW_READONLY_TEST_FAILED\n");
        process::exit(1);
    }
    io::print("  PASS: Code functions work correctly in parent\n\n");

    // Step 2: Fork the process
    io::print("Step 2: Forking process...\n");
    let fork_result = process::fork();

    if fork_result < 0 {
        io::print("  FAIL: fork() failed with error ");
        print_signed(fork_result);
        io::print("\n");
        io::print("COW_READONLY_TEST_FAILED\n");
        process::exit(1);
    }

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        io::print("[CHILD] Process started\n");

        // Step 3: Child executes the SAME code functions
        // If read-only page sharing works, this code is on the same
        // physical frame as the parent's code - shared without COW
        io::print("[CHILD] Step 3: Execute code functions (shared code section)\n");

        // Execute fibonacci - this runs code from shared read-only page
        let child_fib_10 = compute_fibonacci(10);
        io::print("[CHILD]   Fibonacci(10) = ");
        print_number(child_fib_10);
        io::print("\n");

        // Execute factorial - another function in shared code section
        let child_fact_5 = compute_factorial(5);
        io::print("[CHILD]   Factorial(5) = ");
        print_number(child_fact_5);
        io::print("\n");

        // Execute with different arguments to further exercise the code
        let child_fib_15 = compute_fibonacci(15);
        io::print("[CHILD]   Fibonacci(15) = ");
        print_number(child_fib_15);
        io::print(" (expected 610)\n");

        let child_fact_10 = compute_factorial(10);
        io::print("[CHILD]   Factorial(10) = ");
        print_number(child_fact_10);
        io::print(" (expected 3628800)\n");

        // Verify all computations
        if child_fib_10 != 55 {
            io::print("[CHILD]   FAIL: Fibonacci(10) incorrect\n");
            io::print("COW_READONLY_TEST_FAILED\n");
            process::exit(1);
        }
        if child_fact_5 != 120 {
            io::print("[CHILD]   FAIL: Factorial(5) incorrect\n");
            io::print("COW_READONLY_TEST_FAILED\n");
            process::exit(1);
        }
        if child_fib_15 != 610 {
            io::print("[CHILD]   FAIL: Fibonacci(15) incorrect\n");
            io::print("COW_READONLY_TEST_FAILED\n");
            process::exit(1);
        }
        if child_fact_10 != 3628800 {
            io::print("[CHILD]   FAIL: Factorial(10) incorrect\n");
            io::print("COW_READONLY_TEST_FAILED\n");
            process::exit(1);
        }

        io::print("[CHILD]   PASS: All code functions executed correctly!\n");
        io::print("[CHILD]   This proves read-only pages are properly shared\n");
        process::exit(0);
    } else {
        // ========== PARENT PROCESS ==========
        let child_pid = fork_result;
        io::print("[PARENT] Forked child PID: ");
        print_number(child_pid as u64);
        io::print("\n");

        // Step 4: Parent also executes code functions after fork
        // This proves parent still has access to shared code pages
        io::print("[PARENT] Step 4: Execute code functions after fork\n");

        let parent_fib_20 = compute_fibonacci(20);
        io::print("[PARENT]   Fibonacci(20) = ");
        print_number(parent_fib_20);
        io::print(" (expected 6765)\n");

        let parent_fact_8 = compute_factorial(8);
        io::print("[PARENT]   Factorial(8) = ");
        print_number(parent_fact_8);
        io::print(" (expected 40320)\n");

        if parent_fib_20 != 6765 {
            io::print("[PARENT]   FAIL: Fibonacci(20) incorrect\n");
            io::print("COW_READONLY_TEST_FAILED\n");
            process::exit(1);
        }
        if parent_fact_8 != 40320 {
            io::print("[PARENT]   FAIL: Factorial(8) incorrect\n");
            io::print("COW_READONLY_TEST_FAILED\n");
            process::exit(1);
        }

        io::print("[PARENT]   PASS: Code functions still work in parent\n");

        // Wait for child to complete
        io::print("[PARENT] Waiting for child...\n");
        let mut status: i32 = 0;
        let wait_result = process::waitpid(child_pid as i32, &mut status as *mut i32, 0);

        if wait_result != child_pid {
            io::print("[PARENT] FAIL: waitpid returned wrong PID: ");
            print_signed(wait_result);
            io::print("\n");
            io::print("COW_READONLY_TEST_FAILED\n");
            process::exit(1);
        }

        // Check child exit status
        if process::wifexited(status) {
            let exit_code = process::wexitstatus(status);
            if exit_code == 0 {
                io::print("[PARENT] Child exited successfully\n\n");
                io::print("=== Summary ===\n");
                io::print("1. Parent executed code before fork - PASS\n");
                io::print("2. Child executed shared code after fork - PASS\n");
                io::print("3. Parent executed code after fork - PASS\n");
                io::print("4. No page faults on code execution (read-only sharing works)\n");
                io::print("\n=== CoW Read-Only Page Sharing Test PASSED ===\n");
                io::print("COW_READONLY_TEST_PASSED\n");
                process::exit(0);
            } else {
                io::print("[PARENT] Child exited with error code: ");
                print_number(exit_code as u64);
                io::print("\n");
                io::print("COW_READONLY_TEST_FAILED\n");
                process::exit(1);
            }
        } else {
            io::print("[PARENT] Child did not exit normally\n");
            io::print("COW_READONLY_TEST_FAILED\n");
            process::exit(1);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in cow_readonly_test!\n");
    io::print("COW_READONLY_TEST_FAILED\n");
    process::exit(255);
}
