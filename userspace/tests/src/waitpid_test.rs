//! Waitpid syscall test program (std version)
//!
//! Tests that waitpid() correctly waits for a child process:
//! - Fork creates a child process
//! - Child exits with a specific exit code (42)
//! - Parent calls waitpid() to wait for child
//! - Verify the returned PID matches the child PID
//! - Verify the exit status is correct (wexitstatus == 42)
//! - Test WNOHANG with no children returns ECHILD

const WNOHANG: i32 = 1;

extern "C" {
    fn fork() -> i32;
    fn getpid() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
}

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

fn fail(msg: &str) -> ! {
    println!("WAITPID_TEST: FAIL - {}", msg);
    std::process::exit(1);
}

fn main() {
    println!("=== Waitpid Syscall Test ===");

    // Phase 1: Fork to create child process
    println!("Phase 1: Forking process...");
    let fork_result = unsafe { fork() };

    if fork_result < 0 {
        println!("  fork() failed with error: {}", fork_result);
        fail("fork failed");
    }

    if fork_result == 0 {
        // ========== CHILD PROCESS ==========
        println!("[CHILD] Process started");
        println!("[CHILD] PID: {}", unsafe { getpid() });

        // Exit with a specific code that the parent will verify
        println!("[CHILD] Exiting with code 42");
        std::process::exit(42);
    } else {
        // ========== PARENT PROCESS ==========
        println!("[PARENT] Process continuing");
        println!("[PARENT] PID: {}", unsafe { getpid() });
        println!("[PARENT] Child PID: {}", fork_result);

        // Phase 2: Wait for child process
        println!("[PARENT] Phase 2: Calling waitpid()...");
        let mut status: i32 = 0;
        let result = unsafe { waitpid(fork_result, &mut status, 0) };

        println!("[PARENT] waitpid returned: {}", result);
        println!("[PARENT] status value: {}", status);

        // Verify waitpid returned the child PID
        if result != fork_result {
            println!("[PARENT] ERROR: waitpid returned wrong PID");
            println!("  Expected: {}", fork_result);
            println!("  Got: {}", result);
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
        println!("[PARENT] Phase 3: Testing WNOHANG with no children...");
        let mut status2: i32 = 0;
        let wnohang_result = unsafe { waitpid(-1, &mut status2, WNOHANG) };

        // With no children, waitpid(-1, ..., WNOHANG) MUST return -ECHILD (errno 10)
        // POSIX requires ECHILD when there are no child processes to wait for.
        // Returning 0 here would be incorrect - 0 means "children exist but none exited yet"
        println!("[PARENT] waitpid(-1, WNOHANG) returned: {}", wnohang_result);

        if wnohang_result == -10 {
            println!("[PARENT] Correctly returned ECHILD for no children");
        } else {
            println!("[PARENT] ERROR: Expected -10 (ECHILD) but got different value");
            fail("waitpid with no children must return ECHILD");
        }

        // All tests passed!
        println!("\n=== All waitpid tests passed! ===");
        println!("WAITPID_TEST_PASSED");
        std::process::exit(0);
    }
}
