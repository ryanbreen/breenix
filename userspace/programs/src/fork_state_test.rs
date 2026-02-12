//! Fork state inheritance test (std version)
//!
//! Tests that copy_process_state correctly copies all inherited state:
//! 1. File descriptors - pipe FD works across fork (shared file position)
//! 2. Signal handlers - child inherits parent's handler
//! 3. Process group ID - child inherits parent's pgid
//! 4. Session ID - child inherits parent's sid
//!
//! POSIX requires all of these to be inherited by the child process.

use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::process::{fork, getpid, getpgid, getsid, waitpid, yield_now, wifexited, wexitstatus, ForkResult};
use libbreenix::signal::{SIGUSR1, kill};
use libbreenix::{sigaction, Sigaction};
use libbreenix::io::{pipe, read, write, close};

/// Static flag to track if handler was called
static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);

/// Signal handler for SIGUSR1
extern "C" fn sigusr1_handler(_sig: i32) {
    HANDLER_CALLED.store(true, Ordering::SeqCst);
}

fn fail(msg: &str) -> ! {
    println!("FORK_STATE_TEST FAIL: {}", msg);
    println!("FORK_STATE_COPY_FAILED");
    std::process::exit(1);
}

fn main() {
    println!("=== Fork State Copy Test ===\n");

    // =============================================
    // STEP 1: Set up parent state before fork
    // =============================================
    println!("Step 1: Setting up parent state...");

    // 1a. Create a pipe (FD inheritance test)
    println!("  1a. Creating pipe for FD inheritance test");
    let (read_fd, write_fd) = pipe().unwrap_or_else(|_| fail("pipe creation failed"));
    println!("      Pipe created: read_fd={}, write_fd={}", read_fd.raw(), write_fd.raw());

    // 1b. Register signal handler (signal inheritance test)
    println!("  1b. Registering SIGUSR1 handler");
    let action = Sigaction::new(sigusr1_handler);

    sigaction(SIGUSR1, Some(&action), None).unwrap_or_else(|_| fail("sigaction failed"));
    println!("      SIGUSR1 handler registered");

    // 1c. Get parent's pgid and sid
    println!("  1c. Getting parent pgid and sid");
    let parent_pid = getpid().unwrap().raw() as i32;
    let parent_pgid = getpgid(0).unwrap().raw() as i32;
    let parent_sid = getsid(0).unwrap().raw() as i32;
    println!("      Parent PID={}, PGID={}, SID={}", parent_pid, parent_pgid, parent_sid);

    // Write test data to pipe before fork
    println!("  1d. Writing test data to pipe");
    let test_data = b"FORK_TEST_DATA";
    let written = write(write_fd, test_data).unwrap();
    if written != test_data.len() {
        fail("pipe write failed");
    }
    println!("      Wrote {} bytes to pipe", written);

    // =============================================
    // STEP 2: Fork
    // =============================================
    println!("\nStep 2: Forking process...");

    match fork() {
        Ok(ForkResult::Child) => {
            // =============================================
            // CHILD PROCESS
            // =============================================
            let child_pid = getpid().unwrap().raw() as i32;
            println!("\n[CHILD] Started (fork returned 0)");
            println!("[CHILD] PID={}", child_pid);

            let mut tests_passed = 0;
            let total_tests = 4;

            // Test 1: FD inheritance - read from inherited pipe
            println!("\n[CHILD] Test 1: File descriptor inheritance");
            let mut read_buf = [0u8; 32];
            // Retry on EAGAIN
            let mut bytes_read: isize = -11;
            let mut retries = 0;
            while bytes_read == -11 && retries < 50 {
                match read(read_fd, &mut read_buf) {
                    Ok(n) => bytes_read = n as isize,
                    Err(_) => bytes_read = -11,
                }
                if bytes_read == -11 {
                    let _ = yield_now();
                    retries += 1;
                }
            }
            if bytes_read == test_data.len() as isize {
                // Verify content
                let read_slice = &read_buf[..bytes_read as usize];
                if read_slice == test_data {
                    println!("[CHILD]   PASS: Read correct data from inherited pipe FD");
                    tests_passed += 1;
                } else {
                    println!("[CHILD]   FAIL: Data mismatch on inherited FD");
                }
            } else {
                println!("[CHILD]   FAIL: Could not read from inherited pipe (bytes={})", bytes_read);
            }

            // Test 2: Signal handler inheritance - send SIGUSR1 to self
            println!("\n[CHILD] Test 2: Signal handler inheritance");
            let kill_ret = kill(child_pid, SIGUSR1);
            if kill_ret.is_ok() {
                // Yield to allow signal delivery
                for _ in 0..20 {
                    let _ = yield_now();
                    if HANDLER_CALLED.load(Ordering::SeqCst) {
                        break;
                    }
                }
                if HANDLER_CALLED.load(Ordering::SeqCst) {
                    println!("[CHILD]   PASS: Inherited signal handler was called");
                    tests_passed += 1;
                } else {
                    println!("[CHILD]   FAIL: Inherited signal handler was NOT called");
                }
            } else {
                println!("[CHILD]   FAIL: kill() failed");
            }

            // Test 3: PGID inheritance
            println!("\n[CHILD] Test 3: Process group ID inheritance");
            let child_pgid = getpgid(0).unwrap().raw() as i32;
            println!("[CHILD]   Parent PGID={}, Child PGID={}", parent_pgid, child_pgid);
            if child_pgid == parent_pgid {
                println!("[CHILD]   PASS: Child inherited parent's PGID");
                tests_passed += 1;
            } else {
                println!("[CHILD]   FAIL: PGID mismatch");
            }

            // Test 4: Session ID inheritance
            println!("\n[CHILD] Test 4: Session ID inheritance");
            let child_sid = getsid(0).unwrap().raw() as i32;
            println!("[CHILD]   Parent SID={}, Child SID={}", parent_sid, child_sid);
            if child_sid == parent_sid {
                println!("[CHILD]   PASS: Child inherited parent's SID");
                tests_passed += 1;
            } else {
                println!("[CHILD]   FAIL: SID mismatch");
            }

            // Close child's pipe FDs
            let _ = close(read_fd);
            let _ = close(write_fd);

            // Summary
            println!("\n[CHILD] Tests passed: {}/{}", tests_passed, total_tests);

            if tests_passed == total_tests {
                println!("[CHILD] All tests PASSED!");
                std::process::exit(0);
            } else {
                println!("[CHILD] Some tests FAILED!");
                std::process::exit(1);
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // =============================================
            // PARENT PROCESS
            // =============================================
            println!("[PARENT] Forked child PID: {}", child_pid.raw());

            // Close parent's pipe FDs (child has them)
            let _ = close(read_fd);
            let _ = close(write_fd);

            // Wait for child to complete
            println!("[PARENT] Waiting for child...");
            let mut status: i32 = 0;
            let wait_result = waitpid(child_pid.raw() as i32, &mut status, 0).unwrap();

            if wait_result.raw() as i32 != child_pid.raw() as i32 {
                println!("[PARENT] waitpid returned wrong PID");
                println!("FORK_STATE_COPY_FAILED");
                std::process::exit(1);
            }

            // Check if child exited normally with code 0
            if wifexited(status) {
                let exit_code = wexitstatus(status);
                if exit_code == 0 {
                    println!("\n=== All fork state copy tests passed! ===");
                    println!("FORK_STATE_COPY_PASSED");
                    std::process::exit(0);
                } else {
                    println!("[PARENT] Child exited with error code {}", exit_code);
                    println!("FORK_STATE_COPY_FAILED");
                    std::process::exit(1);
                }
            } else {
                println!("[PARENT] Child did not exit normally");
                println!("FORK_STATE_COPY_FAILED");
                std::process::exit(1);
            }
        }
        Err(_) => {
            fail("fork failed");
        }
    }
}
