//! Signal handler fork inheritance test (std version)
//!
//! Tests that signal handlers are inherited across fork():
//! 1. Parent registers a signal handler for SIGUSR1
//! 2. Parent forks
//! 3. Child sends SIGUSR1 to itself
//! 4. Child's handler is called (inherited from parent)
//!
//! POSIX requires that signal handlers are inherited by the child process.

use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::signal::SIGUSR1;
use libbreenix::{kill, sigaction, Sigaction};
use libbreenix::process::{self, ForkResult, getpid, yield_now, wifexited, wexitstatus};

/// Static flag to track if handler was called
static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);

/// Signal handler for SIGUSR1
extern "C" fn sigusr1_handler(_sig: i32) {
    HANDLER_CALLED.store(true, Ordering::SeqCst);
    println!("  HANDLER: SIGUSR1 received!");
}

fn main() {
    println!("=== Signal Fork Inheritance Test ===");

    // Step 1: Register signal handler in parent
    println!("\nStep 1: Register SIGUSR1 handler in parent");
    let action = Sigaction::new(sigusr1_handler);

    if sigaction(SIGUSR1, Some(&action), None).is_err() {
        println!("  FAIL: sigaction returned error");
        println!("SIGNAL_FORK_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered handler");

    // Step 2: Fork
    println!("\nStep 2: Forking process...");
    match process::fork() {
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("SIGNAL_FORK_TEST_FAILED");
            std::process::exit(1);
        }
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Process started");

            let my_pid = getpid().unwrap().raw() as i32;
            println!("[CHILD] PID: {}", my_pid);

            // Step 3: Send SIGUSR1 to self
            println!("[CHILD] Step 3: Sending SIGUSR1 to self...");
            if kill(my_pid, SIGUSR1).is_ok() {
                println!("[CHILD]   kill() succeeded");
            } else {
                println!("[CHILD]   FAIL: kill() returned error");
                println!("SIGNAL_FORK_TEST_FAILED");
                std::process::exit(1);
            }

            // Step 4: Yield to allow signal delivery
            println!("[CHILD] Step 4: Yielding for signal delivery...");
            for i in 0..10 {
                let _ = yield_now();
                if HANDLER_CALLED.load(Ordering::SeqCst) {
                    println!("[CHILD]   Handler called after {} yields", i + 1);
                    break;
                }
            }

            // Step 5: Verify handler was called
            println!("[CHILD] Step 5: Verify handler execution");
            if HANDLER_CALLED.load(Ordering::SeqCst) {
                println!("[CHILD]   PASS: Inherited handler was called!");
                println!("[CHILD] Exiting with success");
                std::process::exit(0);
            } else {
                println!("[CHILD]   FAIL: Inherited handler was NOT called");
                println!("SIGNAL_FORK_TEST_FAILED");
                std::process::exit(1);
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            let child_pid_raw = child_pid.raw() as i32;
            println!("[PARENT] Forked child PID: {}", child_pid_raw);

            // Wait for child to complete
            println!("[PARENT] Waiting for child...");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(child_pid_raw, &mut status, 0);

            match wait_result {
                Ok(waited_pid) if waited_pid.raw() as i32 == child_pid_raw => {
                    // Check if child exited normally with code 0
                    if wifexited(status) {
                        let exit_code = wexitstatus(status);
                        if exit_code == 0 {
                            println!("[PARENT] Child exited successfully (code 0)");
                            println!("\n=== All signal fork inheritance tests passed! ===");
                            println!("SIGNAL_FORK_TEST_PASSED");
                            std::process::exit(0);
                        } else {
                            println!("[PARENT] Child exited with non-zero code: {}", exit_code);
                            println!("SIGNAL_FORK_TEST_FAILED");
                            std::process::exit(1);
                        }
                    } else {
                        println!("[PARENT] Child did not exit normally");
                        println!("SIGNAL_FORK_TEST_FAILED");
                        std::process::exit(1);
                    }
                }
                _ => {
                    println!("[PARENT] FAIL: waitpid returned wrong PID");
                    println!("SIGNAL_FORK_TEST_FAILED");
                    std::process::exit(1);
                }
            }
        }
    }
}
