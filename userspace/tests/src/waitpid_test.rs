//! Waitpid syscall test program (std version)
//!
//! Tests that waitpid() correctly waits for a child process:
//! - Fork creates a child process
//! - Child exits with a specific exit code (42)
//! - Parent calls waitpid() to wait for child
//! - Verify the returned PID matches the child PID
//! - Verify the exit status is correct (wexitstatus == 42)
//! - Test WNOHANG with no children returns ECHILD

use libbreenix::process::{fork, getpid, waitpid, wifexited, wexitstatus, ForkResult, WNOHANG};
use libbreenix::raw;
use libbreenix::syscall::nr;

fn fail(msg: &str) -> ! {
    println!("WAITPID_TEST: FAIL - {}", msg);
    std::process::exit(1);
}

fn main() {
    println!("=== Waitpid Syscall Test ===");

    // Phase 1: Fork to create child process
    println!("Phase 1: Forking process...");

    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Process started");
            println!("[CHILD] PID: {}", getpid().unwrap().raw() as i32);

            // Exit with a specific code that the parent will verify
            println!("[CHILD] Exiting with code 42");
            std::process::exit(42);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            let child_pid_i32 = child_pid.raw() as i32;
            println!("[PARENT] Process continuing");
            println!("[PARENT] PID: {}", getpid().unwrap().raw() as i32);
            println!("[PARENT] Child PID: {}", child_pid_i32);

            // Phase 2: Wait for child process
            println!("[PARENT] Phase 2: Calling waitpid()...");
            let mut status: i32 = 0;
            let result = waitpid(child_pid_i32, &mut status, 0).unwrap();

            println!("[PARENT] waitpid returned: {}", result.raw() as i32);
            println!("[PARENT] status value: {}", status);

            // Verify waitpid returned the child PID
            if result.raw() as i32 != child_pid_i32 {
                println!("[PARENT] ERROR: waitpid returned wrong PID");
                println!("  Expected: {}", child_pid_i32);
                println!("  Got: {}", result.raw() as i32);
                fail("waitpid returned wrong PID");
            }

            println!("[PARENT] waitpid returned correct child PID");

            // Verify child exited normally
            if !wifexited(status) {
                println!("[PARENT] ERROR: child did not exit normally");
                println!("  status: {}", status);
                fail("child did not exit normally");
            }

            println!("[PARENT] Child exited normally (WIFEXITED=true)");

            // Verify exit code
            let exit_code = wexitstatus(status);
            println!("[PARENT] Child exit code (WEXITSTATUS): {}", exit_code);

            if exit_code != 42 {
                println!("[PARENT] ERROR: child exit code wrong");
                println!("  Expected: 42");
                println!("  Got: {}", exit_code);
                fail("child exit code wrong");
            }

            println!("[PARENT] Child exit code verified: 42");

            // Phase 3: Test WNOHANG with no more children
            // Use raw syscall to get the kernel return value directly (-errno)
            // instead of the C library convention (-1 + set errno).
            println!("[PARENT] Phase 3: Testing WNOHANG with no children...");
            let mut status2: i32 = 0;
            let wnohang_result = unsafe {
                raw::syscall3(
                    nr::WAIT4,
                    (-1i32) as u64,
                    &mut status2 as *mut i32 as u64,
                    WNOHANG as u64,
                ) as i64
            };

            // With no children, waitpid(-1, ..., WNOHANG) MUST return -ECHILD (-10)
            // POSIX requires ECHILD when there are no child processes to wait for.
            // Returning 0 here would be incorrect - 0 means "children exist but none exited yet"
            println!("[PARENT] raw_waitpid(-1, WNOHANG) returned: {}", wnohang_result);

            if wnohang_result == -10 {
                println!("[PARENT] Correctly returned ECHILD for no children");
            } else {
                println!("[PARENT] ERROR: Expected -10 (ECHILD) but got {}", wnohang_result);
                fail("waitpid with no children must return ECHILD");
            }

            // All tests passed!
            println!("\n=== All waitpid tests passed! ===");
            println!("WAITPID_TEST_PASSED");
            std::process::exit(0);
        }
        Err(_) => {
            fail("fork failed");
        }
    }
}
