//! SIGCHLD delivery test (std version)
//!
//! Tests that SIGCHLD is delivered to parent when child exits:
//! 1. Parent registers SIGCHLD handler
//! 2. Parent forks child
//! 3. Child exits
//! 4. Parent's SIGCHLD handler is called
//!
//! POSIX requires that the parent receive SIGCHLD when a child terminates.

use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::signal::SIGCHLD;
use libbreenix::{sigaction, Sigaction};
use libbreenix::process::{self, ForkResult, yield_now, wifexited, wexitstatus};

/// Static flag to track if SIGCHLD handler was called
static SIGCHLD_RECEIVED: AtomicBool = AtomicBool::new(false);

/// SIGCHLD handler
extern "C" fn sigchld_handler(_sig: i32) {
    SIGCHLD_RECEIVED.store(true, Ordering::SeqCst);
    println!("  SIGCHLD_HANDLER: Child termination signal received!");
}

fn main() {
    println!("=== SIGCHLD Delivery Test ===");

    // Step 1: Register SIGCHLD handler
    println!("\nStep 1: Register SIGCHLD handler in parent");
    let action = Sigaction::new(sigchld_handler);

    if sigaction(SIGCHLD, Some(&action), None).is_err() {
        println!("  FAIL: sigaction returned error");
        println!("SIGCHLD_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered SIGCHLD handler");

    // Step 2: Fork child
    println!("\nStep 2: Forking child process...");
    match process::fork() {
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("SIGCHLD_TEST_FAILED");
            std::process::exit(1);
        }
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Process started, exiting immediately with code 42");
            std::process::exit(42);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            let child_pid_raw = child_pid.raw() as i32;
            println!("[PARENT] Forked child PID: {}", child_pid_raw);

            // Step 3: Wait for child with waitpid
            println!("\nStep 3: Waiting for child with waitpid (blocking)...");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(child_pid_raw, &mut status, 0);

            match wait_result {
                Ok(pid) if pid.raw() as i32 == child_pid_raw => {
                    // Verify child exit code
                    if wifexited(status) {
                        let exit_code = wexitstatus(status);
                        println!("[PARENT] Child exited with code: {}", exit_code);
                    }
                }
                _ => {
                    println!("[PARENT] FAIL: waitpid returned wrong PID");
                    println!("SIGCHLD_TEST_FAILED");
                    std::process::exit(1);
                }
            }

            // Step 4: Check if SIGCHLD was already delivered
            println!("\nStep 4: Verify SIGCHLD was delivered");

            if !SIGCHLD_RECEIVED.load(Ordering::SeqCst) {
                println!("  SIGCHLD not yet received, yielding once...");
                let _ = yield_now();
            }

            // Final check
            if SIGCHLD_RECEIVED.load(Ordering::SeqCst) {
                println!("  PASS: SIGCHLD handler was called!");
                println!("\n=== All SIGCHLD delivery tests passed! ===");
                println!("SIGCHLD_TEST_PASSED");
                std::process::exit(0);
            } else {
                println!("  FAIL: SIGCHLD handler was NOT called");
                println!("  (Note: This may indicate the kernel doesn't send SIGCHLD on child exit)");
                println!("SIGCHLD_TEST_FAILED");
                std::process::exit(1);
            }
        }
    }
}
