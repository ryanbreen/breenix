//! Fork state inheritance test
//!
//! Tests that copy_process_state correctly copies all inherited state:
//! 1. File descriptors - pipe FD works across fork (shared file position)
//! 2. Signal handlers - child inherits parent's handler
//! 3. Process group ID - child inherits parent's pgid
//! 4. Session ID - child inherits parent's sid
//!
//! POSIX requires all of these to be inherited by the child process.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;
use libbreenix::types::fd;

/// Static flag to track if handler was called
static mut HANDLER_CALLED: bool = false;

/// Signal handler for SIGUSR1
extern "C" fn sigusr1_handler(_sig: i32) {
    unsafe {
        HANDLER_CALLED = true;
    }
}

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

    io::write(fd::STDOUT, &buffer[..i]);
}

/// Helper to exit with failure
fn fail(msg: &str) -> ! {
    io::print("FORK_STATE_TEST FAIL: ");
    io::print(msg);
    io::print("\n");
    io::print("FORK_STATE_COPY_FAILED\n");
    process::exit(1);
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Fork State Copy Test ===\n\n");

        // =============================================
        // STEP 1: Set up parent state before fork
        // =============================================
        io::print("Step 1: Setting up parent state...\n");

        // 1a. Create a pipe (FD inheritance test)
        io::print("  1a. Creating pipe for FD inheritance test\n");
        let mut pipefd: [i32; 2] = [0, 0];
        let ret = io::pipe(&mut pipefd);
        if ret < 0 {
            fail("pipe creation failed");
        }
        let read_fd = pipefd[0] as u64;
        let write_fd = pipefd[1] as u64;
        io::print("      Pipe created: read_fd=");
        print_number(read_fd);
        io::print(", write_fd=");
        print_number(write_fd);
        io::print("\n");

        // 1b. Register signal handler (signal inheritance test)
        io::print("  1b. Registering SIGUSR1 handler\n");
        let action = signal::Sigaction::new(sigusr1_handler);
        if signal::sigaction(signal::SIGUSR1, Some(&action), None).is_err() {
            fail("sigaction failed");
        }
        io::print("      SIGUSR1 handler registered\n");

        // 1c. Get parent's pgid and sid
        io::print("  1c. Getting parent pgid and sid\n");
        let parent_pid = process::getpid();
        let parent_pgid = process::getpgid(0);
        let parent_sid = process::getsid(0);
        io::print("      Parent PID=");
        print_number(parent_pid);
        io::print(", PGID=");
        print_number(parent_pgid as u64);
        io::print(", SID=");
        print_number(parent_sid as u64);
        io::print("\n");

        // Write test data to pipe before fork
        io::print("  1d. Writing test data to pipe\n");
        let test_data = b"FORK_TEST_DATA";
        let written = io::write(write_fd, test_data);
        if written != test_data.len() as i64 {
            fail("pipe write failed");
        }
        io::print("      Wrote ");
        print_number(written as u64);
        io::print(" bytes to pipe\n");

        // =============================================
        // STEP 2: Fork
        // =============================================
        io::print("\nStep 2: Forking process...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            fail("fork failed");
        }

        if fork_result == 0 {
            // =============================================
            // CHILD PROCESS
            // =============================================
            io::print("\n[CHILD] Started (fork returned 0)\n");
            let child_pid = process::getpid();
            io::print("[CHILD] PID=");
            print_number(child_pid);
            io::print("\n");

            let mut tests_passed = 0;
            let total_tests = 4;

            // Test 1: FD inheritance - read from inherited pipe
            io::print("\n[CHILD] Test 1: File descriptor inheritance\n");
            let mut read_buf = [0u8; 32];
            // Retry on EAGAIN
            let mut bytes_read: i64 = -11;
            let mut retries = 0;
            while bytes_read == -11 && retries < 50 {
                bytes_read = io::read(read_fd, &mut read_buf);
                if bytes_read == -11 {
                    process::yield_now();
                    retries += 1;
                }
            }
            if bytes_read == test_data.len() as i64 {
                // Verify content
                let read_slice = &read_buf[..bytes_read as usize];
                if read_slice == test_data {
                    io::print("[CHILD]   PASS: Read correct data from inherited pipe FD\n");
                    tests_passed += 1;
                } else {
                    io::print("[CHILD]   FAIL: Data mismatch on inherited FD\n");
                }
            } else {
                io::print("[CHILD]   FAIL: Could not read from inherited pipe (bytes=");
                print_number(bytes_read as u64);
                io::print(")\n");
            }

            // Test 2: Signal handler inheritance - send SIGUSR1 to self
            io::print("\n[CHILD] Test 2: Signal handler inheritance\n");
            if signal::kill(child_pid as i32, signal::SIGUSR1).is_ok() {
                // Yield to allow signal delivery
                for _ in 0..20 {
                    process::yield_now();
                    if HANDLER_CALLED {
                        break;
                    }
                }
                if HANDLER_CALLED {
                    io::print("[CHILD]   PASS: Inherited signal handler was called\n");
                    tests_passed += 1;
                } else {
                    io::print("[CHILD]   FAIL: Inherited signal handler was NOT called\n");
                }
            } else {
                io::print("[CHILD]   FAIL: kill() failed\n");
            }

            // Test 3: PGID inheritance
            io::print("\n[CHILD] Test 3: Process group ID inheritance\n");
            let child_pgid = process::getpgid(0);
            io::print("[CHILD]   Parent PGID=");
            print_number(parent_pgid as u64);
            io::print(", Child PGID=");
            print_number(child_pgid as u64);
            io::print("\n");
            if child_pgid == parent_pgid {
                io::print("[CHILD]   PASS: Child inherited parent's PGID\n");
                tests_passed += 1;
            } else {
                io::print("[CHILD]   FAIL: PGID mismatch\n");
            }

            // Test 4: Session ID inheritance
            io::print("\n[CHILD] Test 4: Session ID inheritance\n");
            let child_sid = process::getsid(0);
            io::print("[CHILD]   Parent SID=");
            print_number(parent_sid as u64);
            io::print(", Child SID=");
            print_number(child_sid as u64);
            io::print("\n");
            if child_sid == parent_sid {
                io::print("[CHILD]   PASS: Child inherited parent's SID\n");
                tests_passed += 1;
            } else {
                io::print("[CHILD]   FAIL: SID mismatch\n");
            }

            // Close child's pipe FDs
            io::close(read_fd);
            io::close(write_fd);

            // Summary
            io::print("\n[CHILD] Tests passed: ");
            print_number(tests_passed);
            io::print("/");
            print_number(total_tests);
            io::print("\n");

            if tests_passed == total_tests {
                io::print("[CHILD] All tests PASSED!\n");
                process::exit(0);
            } else {
                io::print("[CHILD] Some tests FAILED!\n");
                process::exit(1);
            }
        } else {
            // =============================================
            // PARENT PROCESS
            // =============================================
            io::print("[PARENT] Forked child PID: ");
            print_number(fork_result as u64);
            io::print("\n");

            // Close parent's pipe FDs (child has them)
            io::close(read_fd);
            io::close(write_fd);

            // Wait for child to complete
            io::print("[PARENT] Waiting for child...\n");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(fork_result as i32, &mut status, 0);

            if wait_result != fork_result {
                io::print("[PARENT] waitpid returned wrong PID\n");
                io::print("FORK_STATE_COPY_FAILED\n");
                process::exit(1);
            }

            // Check if child exited normally with code 0
            if process::wifexited(status) {
                let exit_code = process::wexitstatus(status);
                if exit_code == 0 {
                    io::print("\n=== All fork state copy tests passed! ===\n");
                    io::print("FORK_STATE_COPY_PASSED\n");
                    process::exit(0);
                } else {
                    io::print("[PARENT] Child exited with error code ");
                    print_number(exit_code as u64);
                    io::print("\n");
                    io::print("FORK_STATE_COPY_FAILED\n");
                    process::exit(1);
                }
            } else {
                io::print("[PARENT] Child did not exit normally\n");
                io::print("FORK_STATE_COPY_FAILED\n");
                process::exit(1);
            }
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in fork_state_test!\n");
    io::print("FORK_STATE_COPY_FAILED\n");
    process::exit(255);
}
