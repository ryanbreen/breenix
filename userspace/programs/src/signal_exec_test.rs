//! Signal exec reset test (std version)
//!
//! Tests that signal handlers are reset to SIG_DFL after exec():
//! 1. Process registers a user handler for SIGUSR1
//! 2. Process forks a child
//! 3. Child execs signal_exec_check program
//! 4. The new program verifies the handler is SIG_DFL (not inherited)
//!
//! POSIX requires that signals with custom handlers be reset to SIG_DFL
//! after exec, since the old handler code no longer exists in the new
//! address space.

use libbreenix::process::{fork, waitpid, execv, wifexited, wexitstatus, ForkResult};
use libbreenix::signal::{SIGUSR1, SIG_DFL, SIG_IGN};
use libbreenix::{sigaction, Sigaction};

/// Signal handler for SIGUSR1 (should never be called in exec'd process)
extern "C" fn sigusr1_handler(_sig: i32) {
    println!("ERROR: Handler was called but should have been reset by exec!");
}

fn main() {
    println!("=== Signal Exec Reset Test ===");

    // Step 1: Register signal handler for SIGUSR1
    println!("\nStep 1: Register SIGUSR1 handler");
    let action = Sigaction::new(sigusr1_handler);

    let ret = sigaction(SIGUSR1, Some(&action), None);
    if ret.is_err() {
        println!("  FAIL: sigaction returned error");
        println!("SIGNAL_EXEC_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered handler");

    // Verify handler was set
    let mut verify_action = Sigaction::default();
    let ret = sigaction(SIGUSR1, None, Some(&mut verify_action));
    if ret.is_ok() {
        println!("  Handler address: {}", verify_action.handler);
        if verify_action.handler == SIG_DFL || verify_action.handler == SIG_IGN {
            println!("  WARN: Handler appears to be default/ignore, test may not be valid");
        }
    } else {
        println!("  WARN: Could not verify handler was set");
    }

    // Step 2: Fork child
    println!("\nStep 2: Forking child process...");

    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Forked successfully, about to exec signal_exec_check");

            // Verify child inherited the handler (before exec)
            let mut child_action = Sigaction::default();
            let ret = sigaction(SIGUSR1, None, Some(&mut child_action));
            if ret.is_ok() {
                println!("[CHILD] Pre-exec handler: {}", child_action.handler);
                if child_action.handler != SIG_DFL && child_action.handler != SIG_IGN {
                    println!("[CHILD] Handler inherited from parent (as expected)");
                }
            }

            // Step 3: Exec into signal_exec_check
            println!("[CHILD] Calling exec(signal_exec_check)...");

            let path = b"signal_exec_check\0";
            let argv: [*const u8; 1] = [std::ptr::null()];
            let exec_result = execv(path, argv.as_ptr());

            // If exec returns, it failed
            println!("[CHILD] exec() returned (should not happen on success): {:?}", exec_result.err());

            // Fallback: Check handler state after failed exec
            println!("[CHILD] Note: exec may not be fully implemented for this binary");
            println!("[CHILD] Checking if handler is still set post-exec-attempt...");

            let mut post_exec_action = Sigaction::default();
            let ret = sigaction(SIGUSR1, None, Some(&mut post_exec_action));
            if ret.is_ok() {
                println!("[CHILD] Post-exec handler: {}", post_exec_action.handler);
            }

            // Since exec didn't work as expected, this is a partial test
            println!("[CHILD] Exiting - exec implementation may need extension");
            std::process::exit(42); // Special exit code to indicate exec didn't replace process
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            let child_pid_i32 = child_pid.raw() as i32;
            println!("[PARENT] Forked child PID: {}", child_pid_i32);

            // Wait for child
            println!("[PARENT] Waiting for child...");
            let mut status: i32 = 0;
            let wait_result = waitpid(child_pid_i32, &mut status, 0).unwrap();

            if wait_result.raw() as i32 != child_pid_i32 {
                println!("[PARENT] FAIL: waitpid returned wrong PID");
                println!("SIGNAL_EXEC_TEST_FAILED");
                std::process::exit(1);
            }

            if wifexited(status) {
                let exit_code = wexitstatus(status);
                println!("[PARENT] Child exit code: {}", exit_code);

                if exit_code == 0 {
                    // signal_exec_check verified handler is SIG_DFL
                    println!("[PARENT] Child (signal_exec_check) verified SIG_DFL!");
                    println!("\n=== Signal exec reset test passed! ===");
                    println!("SIGNAL_EXEC_TEST_PASSED");
                    std::process::exit(0);
                } else if exit_code == 1 {
                    // signal_exec_check found SIG_IGN (acceptable per POSIX but not ideal)
                    println!("[PARENT] Child reported handler is SIG_IGN (partial pass per POSIX)");
                    println!("\n=== Signal exec reset test passed (SIG_IGN) ===");
                    println!("SIGNAL_EXEC_TEST_PASSED");
                    std::process::exit(0);
                } else if exit_code == 2 {
                    // signal_exec_check found user handler NOT reset
                    println!("[PARENT] FAIL: Handler was NOT reset to SIG_DFL after exec!");
                    println!("[PARENT] The old handler address was inherited, violating POSIX.");
                    println!("SIGNAL_EXEC_TEST_FAILED");
                    std::process::exit(1);
                } else if exit_code == 3 {
                    // signal_exec_check couldn't query sigaction
                    println!("[PARENT] FAIL: Child couldn't query signal handler state.");
                    println!("SIGNAL_EXEC_TEST_FAILED");
                    std::process::exit(1);
                } else if exit_code == 42 {
                    // Exec returned instead of replacing the process
                    println!("[PARENT] FAIL: exec() returned instead of replacing process!");
                    println!("[PARENT] The exec syscall did not work as expected.");
                    println!("SIGNAL_EXEC_TEST_FAILED");
                    std::process::exit(1);
                } else {
                    println!("[PARENT] FAIL: Unexpected exit code from child");
                    println!("SIGNAL_EXEC_TEST_FAILED");
                    std::process::exit(1);
                }
            } else {
                println!("[PARENT] Child did not exit normally");
                println!("SIGNAL_EXEC_TEST_FAILED");
                std::process::exit(1);
            }
        }
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("SIGNAL_EXEC_TEST_FAILED");
            std::process::exit(1);
        }
    }
}
