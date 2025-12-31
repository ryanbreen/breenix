//! Waitpid syscall test program
//!
//! Tests that waitpid() correctly waits for a child process:
//! - Fork creates a child process
//! - Child exits with a specific exit code (42)
//! - Parent calls waitpid() to wait for child
//! - Verify the returned PID matches the child PID
//! - Verify the exit status is correct (wexitstatus == 42)

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::types::fd;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to string and print it
unsafe fn print_number(prefix: &str, num: u64) {
    io::print(prefix);

    let mut n = num;
    let mut i = 0;

    if n == 0 {
        BUFFER[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            BUFFER[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = BUFFER[j];
            BUFFER[j] = BUFFER[i - j - 1];
            BUFFER[i - j - 1] = tmp;
        }
    }

    io::write(fd::STDOUT, &BUFFER[..i]);
    io::print("\n");
}

/// Print a signed number
unsafe fn print_signed_number(prefix: &str, num: i64) {
    io::print(prefix);

    if num < 0 {
        io::print("-");
        print_number("", (-num) as u64);
    } else {
        print_number("", num as u64);
    }
}

/// Helper to exit with error message
fn fail(msg: &str) -> ! {
    io::print("WAITPID_TEST: FAIL - ");
    io::print(msg);
    io::print("\n");
    process::exit(1);
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Waitpid Syscall Test ===\n");

        // Phase 1: Fork to create child process
        io::print("Phase 1: Forking process...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            print_signed_number("  fork() failed with error: ", fork_result);
            fail("fork failed");
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Process started\n");
            print_number("[CHILD] PID: ", process::getpid());

            // Exit with a specific code that the parent will verify
            io::print("[CHILD] Exiting with code 42\n");
            process::exit(42);
        } else {
            // ========== PARENT PROCESS ==========
            io::print("[PARENT] Process continuing\n");
            print_number("[PARENT] PID: ", process::getpid());
            print_number("[PARENT] Child PID: ", fork_result as u64);

            // Phase 2: Wait for child process
            io::print("[PARENT] Phase 2: Calling waitpid()...\n");
            let mut status: i32 = 0;
            let result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

            print_signed_number("[PARENT] waitpid returned: ", result);
            print_number("[PARENT] status value: ", status as u64);

            // Verify waitpid returned the child PID
            if result != fork_result {
                io::print("[PARENT] ERROR: waitpid returned wrong PID\n");
                print_signed_number("  Expected: ", fork_result);
                print_signed_number("  Got: ", result);
                fail("waitpid returned wrong PID");
            }

            io::print("[PARENT] waitpid returned correct child PID\n");

            // Verify child exited normally
            if !process::wifexited(status) {
                io::print("[PARENT] ERROR: child did not exit normally\n");
                print_number("  status: ", status as u64);
                fail("child did not exit normally");
            }

            io::print("[PARENT] Child exited normally (WIFEXITED=true)\n");

            // Verify exit code
            let exit_code = process::wexitstatus(status);
            print_number("[PARENT] Child exit code (WEXITSTATUS): ", exit_code as u64);

            if exit_code != 42 {
                io::print("[PARENT] ERROR: child exit code wrong\n");
                io::print("  Expected: 42\n");
                print_number("  Got: ", exit_code as u64);
                fail("child exit code wrong");
            }

            io::print("[PARENT] Child exit code verified: 42\n");

            // Phase 3: Test WNOHANG with no more children
            io::print("[PARENT] Phase 3: Testing WNOHANG with no children...\n");
            let mut status2: i32 = 0;
            let wnohang_result = process::waitpid(-1, &mut status2 as *mut i32, process::WNOHANG);

            // With no children, waitpid(-1, ..., WNOHANG) MUST return -ECHILD (errno 10)
            // POSIX requires ECHILD when there are no child processes to wait for.
            // Returning 0 here would be incorrect - 0 means "children exist but none exited yet"
            print_signed_number("[PARENT] waitpid(-1, WNOHANG) returned: ", wnohang_result);

            if wnohang_result == -10 {
                io::print("[PARENT] Correctly returned ECHILD for no children\n");
            } else {
                io::print("[PARENT] ERROR: Expected -10 (ECHILD) but got different value\n");
                fail("waitpid with no children must return ECHILD");
            }

            // All tests passed!
            io::print("\n=== All waitpid tests passed! ===\n");
            io::print("WAITPID_TEST_PASSED\n");
            process::exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in waitpid_test!\n");
    process::exit(255);
}
