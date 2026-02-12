//! Pause syscall test program (std version)
//!
//! Tests the pause() syscall which blocks until a signal is delivered.

use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::signal::SIGUSR1;
use libbreenix::{kill, sigaction, Sigaction};
use libbreenix::process::{self, ForkResult, getpid, yield_now};

static SIGUSR1_RECEIVED: AtomicBool = AtomicBool::new(false);

/// Raw write syscall - async-signal-safe (no locks, no RefCell, no allocations)
fn raw_write_str(s: &str) {
    let _ = libbreenix::io::write(libbreenix::types::Fd::STDOUT, s.as_bytes());
}

/// SIGUSR1 handler - sets flag when called
/// IMPORTANT: Uses raw write syscall, NOT println!, because signal handlers
/// can fire during another println (which holds a RefCell borrow on stdout).
/// Using println here would panic with "RefCell already borrowed".
extern "C" fn sigusr1_handler(_sig: i32) {
    SIGUSR1_RECEIVED.store(true, Ordering::SeqCst);
    raw_write_str("  HANDLER: SIGUSR1 received in parent!\n");
}

fn main() {
    println!("=== Pause Syscall Test ===");

    let parent_pid = getpid().unwrap().raw() as i32;
    println!("Parent PID: {}", parent_pid);

    // Step 1: Register SIGUSR1 handler
    println!("\nStep 1: Register SIGUSR1 handler in parent");
    let action = Sigaction::new(sigusr1_handler);

    if sigaction(SIGUSR1, Some(&action), None).is_err() {
        println!("  FAIL: sigaction returned error");
        println!("PAUSE_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered SIGUSR1 handler");

    // Step 2: Fork child
    println!("\nStep 2: Forking child process...");
    match process::fork() {
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("PAUSE_TEST_FAILED");
            std::process::exit(1);
        }
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            let my_pid = getpid().unwrap().raw() as i32;
            println!("[CHILD] Process started");
            println!("[CHILD] My PID: {}", my_pid);

            // Give parent time to call pause()
            println!("[CHILD] Yielding to let parent call pause()...");
            for _ in 0..5 {
                let _ = yield_now();
            }

            // Send SIGUSR1 to parent
            println!("[CHILD] Sending SIGUSR1 to parent (PID {})...", parent_pid);
            if kill(parent_pid, SIGUSR1).is_ok() {
                println!("[CHILD] kill() succeeded");
            } else {
                println!("[CHILD] kill() failed");
            }

            println!("[CHILD] Exiting with code 0");
            std::process::exit(0);
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            let child_pid_raw = child_pid.raw() as i32;
            println!("[PARENT] Forked child PID: {}", child_pid_raw);

            // Step 3: Call pause() to wait for signal
            // NOTE: On ARM64, sched_yield doesn't cause an immediate context switch
            // (it sets need_resched but the SVC return path has PREEMPT_ACTIVE set).
            // The child may complete all its yields and send the signal before the
            // parent gets a timer-driven context switch. In that case, the signal
            // is delivered on a syscall return BEFORE we reach pause().
            // We handle both paths: signal-before-pause and signal-during-pause.
            if SIGUSR1_RECEIVED.load(Ordering::SeqCst) {
                println!("\nStep 3: Signal already received before pause() (race-safe path)");
                println!("  PASS: SIGUSR1 was delivered before pause() was called");
            } else {
                println!("\nStep 3: Calling pause() to wait for signal...");
                let pause_result = libbreenix::signal::pause();

                // pause() always returns Err with EINTR when a signal is caught
                println!("[PARENT] pause() returned");

                if pause_result.is_err() {
                    println!("  PASS: pause() correctly returned after signal");
                } else {
                    println!("  FAIL: pause() should return error (EINTR)");
                    println!("PAUSE_TEST_FAILED");
                    std::process::exit(1);
                }
            }

            // Step 4: Verify signal handler was called
            println!("\nStep 4: Verify SIGUSR1 handler was called");
            if SIGUSR1_RECEIVED.load(Ordering::SeqCst) {
                println!("  PASS: SIGUSR1 handler was called!");
            } else {
                println!("  FAIL: SIGUSR1 handler was NOT called");
                println!("PAUSE_TEST_FAILED");
                std::process::exit(1);
            }

            // Step 5: Wait for child to exit
            println!("\nStep 5: Waiting for child to exit...");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(child_pid_raw, &mut status, 0);

            match wait_result {
                Ok(pid) if pid.raw() as i32 == child_pid_raw => {
                    println!("  Child reaped successfully");
                }
                _ => {
                    println!("  Warning: waitpid returned unexpected result");
                }
            }

            // All tests passed
            println!("\n=== All pause() tests passed! ===");
            println!("PAUSE_TEST_PASSED");
            std::process::exit(0);
        }
    }
}
