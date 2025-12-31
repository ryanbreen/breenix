//! WNOHANG timing test
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

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;

/// Print a number to stdout
unsafe fn print_number(num: u64) {
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
unsafe fn print_signed(num: i64) {
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
    unsafe {
        io::print("=== WNOHANG Timing Test ===\n");

        // Step 1: Fork child that will do some work before exiting
        io::print("\nStep 1: Forking child that loops before exiting...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            io::print("  FAIL: fork() failed with error ");
            print_signed(fork_result);
            io::print("\n");
            io::print("WNOHANG_TIMING_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Started, doing work before exit...\n");

            // Do some work - yield multiple times to simulate busy work
            // This ensures the parent has time to call WNOHANG while we're running
            for i in 0..50 {
                process::yield_now();
                // Volatile read to prevent optimization
                core::hint::black_box(i);
            }

            io::print("[CHILD] Work complete, exiting with code 99\n");
            process::exit(99);
        } else {
            // ========== PARENT PROCESS ==========
            io::print("[PARENT] Forked child PID: ");
            print_number(fork_result as u64);
            io::print("\n");

            // Step 2: IMMEDIATELY call waitpid with WNOHANG
            // Child should still be running (doing its loop)
            io::print("\nStep 2: Immediate WNOHANG (child should still be running)...\n");
            let mut status: i32 = 0;
            let wnohang_result = process::waitpid(fork_result as i32, &mut status as *mut i32, process::WNOHANG);

            io::print("  waitpid(-1, WNOHANG) returned: ");
            print_signed(wnohang_result);
            io::print("\n");

            // WNOHANG should return 0 if child exists but hasn't exited yet
            // Note: In a fast system, child might already be done, so we accept either 0 or the child PID
            let early_wait_ok = if wnohang_result == 0 {
                io::print("  PASS: Returned 0 (child still running)\n");
                true
            } else if wnohang_result == fork_result {
                // Child finished very quickly - this is acceptable
                io::print("  OK: Child already finished (fast execution)\n");
                false // Already reaped, skip step 3
            } else if wnohang_result == -10 {
                // ECHILD - this would be wrong since we just forked
                io::print("  FAIL: Returned ECHILD but child should exist!\n");
                io::print("WNOHANG_TIMING_TEST_FAILED\n");
                process::exit(1);
            } else {
                io::print("  FAIL: Unexpected return value\n");
                io::print("WNOHANG_TIMING_TEST_FAILED\n");
                process::exit(1);
            };

            if early_wait_ok {
                // Step 3: Now wait for child to actually finish (blocking wait)
                io::print("\nStep 3: Blocking wait for child to finish...\n");

                let mut status2: i32 = 0;
                let wait_result = process::waitpid(fork_result as i32, &mut status2 as *mut i32, 0);

                io::print("  waitpid returned: ");
                print_signed(wait_result);
                io::print("\n");

                if wait_result != fork_result {
                    io::print("  FAIL: waitpid returned wrong PID\n");
                    io::print("WNOHANG_TIMING_TEST_FAILED\n");
                    process::exit(1);
                }

                io::print("  PASS: waitpid returned correct child PID\n");

                // Verify exit status
                if process::wifexited(status2) {
                    let exit_code = process::wexitstatus(status2);
                    io::print("  Child exit code: ");
                    print_number(exit_code as u64);
                    io::print("\n");

                    if exit_code != 99 {
                        io::print("  FAIL: Expected exit code 99\n");
                        io::print("WNOHANG_TIMING_TEST_FAILED\n");
                        process::exit(1);
                    }
                    io::print("  PASS: Exit code verified\n");
                } else {
                    io::print("  FAIL: Child did not exit normally\n");
                    io::print("WNOHANG_TIMING_TEST_FAILED\n");
                    process::exit(1);
                }
            } else {
                // Child was already reaped in step 2, verify the status
                if process::wifexited(status) {
                    let exit_code = process::wexitstatus(status);
                    io::print("  Child exit code: ");
                    print_number(exit_code as u64);
                    io::print("\n");

                    if exit_code != 99 {
                        io::print("  FAIL: Expected exit code 99\n");
                        io::print("WNOHANG_TIMING_TEST_FAILED\n");
                        process::exit(1);
                    }
                    io::print("  PASS: Exit code verified\n");
                }
            }

            // Step 4: Now that child is reaped, WNOHANG should return ECHILD
            io::print("\nStep 4: WNOHANG after child reaped (should return ECHILD)...\n");
            let mut status3: i32 = 0;
            let final_result = process::waitpid(-1, &mut status3 as *mut i32, process::WNOHANG);

            io::print("  waitpid(-1, WNOHANG) returned: ");
            print_signed(final_result);
            io::print("\n");

            if final_result == -10 {
                io::print("  PASS: Correctly returned ECHILD (no more children)\n");
            } else {
                io::print("  FAIL: Expected -10 (ECHILD) but got different value\n");
                io::print("WNOHANG_TIMING_TEST_FAILED\n");
                process::exit(1);
            }

            io::print("\n=== All WNOHANG timing tests passed! ===\n");
            io::print("WNOHANG_TIMING_TEST_PASSED\n");
            process::exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in WNOHANG timing test!\n");
    io::print("WNOHANG_TIMING_TEST_FAILED\n");
    process::exit(255);
}
