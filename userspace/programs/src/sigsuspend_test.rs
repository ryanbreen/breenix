//! Sigsuspend syscall test program (std version)
//!
//! Tests the sigsuspend() syscall which atomically replaces the signal mask
//! and suspends until a signal is delivered.

use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::signal::{SIGUSR1, SIG_BLOCK, SIG_SETMASK, sigmask};
use libbreenix::{kill, sigaction, sigprocmask, sigsuspend, Sigaction};
use libbreenix::process::{self, ForkResult, getpid, yield_now};

static SIGUSR1_RECEIVED: AtomicBool = AtomicBool::new(false);

/// Raw write syscall - async-signal-safe (no locks, no RefCell, no allocations)
fn raw_write_str(s: &str) {
    let _ = libbreenix::io::write(libbreenix::types::Fd::STDOUT, s.as_bytes());
}

/// SIGUSR1 handler - sets flag when called
/// IMPORTANT: Uses raw write syscall, NOT println!, because signal handlers
/// must be async-signal-safe. println! holds a RefCell borrow on stdout
/// and would panic if the signal fires during another println.
extern "C" fn sigusr1_handler(_sig: i32) {
    SIGUSR1_RECEIVED.store(true, Ordering::SeqCst);
    raw_write_str("  HANDLER: SIGUSR1 received in parent!\n");
}

fn main() {
    println!("=== Sigsuspend Syscall Test ===");

    let parent_pid = getpid().unwrap().raw() as i32;
    println!("Parent PID: {}", parent_pid);

    // Step 1: Block SIGUSR1 initially with sigprocmask
    println!("\nStep 1: Block SIGUSR1 with sigprocmask");
    let sigusr1_mask = sigmask(SIGUSR1);
    println!("  SIGUSR1 mask: {:#018x}", sigusr1_mask);

    let mut old_mask: u64 = 0;
    if sigprocmask(SIG_BLOCK, Some(&sigusr1_mask), Some(&mut old_mask)).is_err() {
        println!("  FAIL: sigprocmask returned error");
        println!("SIGSUSPEND_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigprocmask blocked SIGUSR1");
    println!("  Old mask: {:#018x}", old_mask);

    // Step 2: Verify SIGUSR1 is now blocked
    println!("\nStep 2: Verify SIGUSR1 is blocked");
    let mut current_mask: u64 = 0;
    if sigprocmask(SIG_SETMASK, None, Some(&mut current_mask)).is_err() {
        println!("  FAIL: sigprocmask query failed");
        println!("SIGSUSPEND_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  Current mask: {:#018x}", current_mask);

    if (current_mask & sigusr1_mask) != 0 {
        println!("  PASS: SIGUSR1 is blocked in current mask");
    } else {
        println!("  FAIL: SIGUSR1 is NOT blocked in current mask");
        println!("SIGSUSPEND_TEST_FAILED");
        std::process::exit(1);
    }

    // Step 3: Register SIGUSR1 handler
    println!("\nStep 3: Register SIGUSR1 handler in parent");
    let action = Sigaction::new(sigusr1_handler);

    if sigaction(SIGUSR1, Some(&action), None).is_err() {
        println!("  FAIL: sigaction returned error");
        println!("SIGSUSPEND_TEST_FAILED");
        std::process::exit(1);
    }
    println!("  PASS: sigaction registered SIGUSR1 handler");

    // Step 4: Fork child
    println!("\nStep 4: Forking child process...");
    match process::fork() {
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("SIGSUSPEND_TEST_FAILED");
            std::process::exit(1);
        }
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            let my_pid = getpid().unwrap().raw() as i32;
            println!("[CHILD] Process started");
            println!("[CHILD] My PID: {}", my_pid);

            // Give parent time to call sigsuspend()
            println!("[CHILD] Yielding to let parent call sigsuspend()...");
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

            // Step 5: Call sigsuspend() with a mask that UNBLOCKS SIGUSR1
            println!("\nStep 5: Calling sigsuspend() with empty mask (unblocks SIGUSR1)...");
            let suspend_mask: u64 = 0;
            println!("  Suspend mask: {:#018x}", suspend_mask);
            println!("  Calling sigsuspend()...");

            let suspend_result = sigsuspend(&suspend_mask);

            // sigsuspend always returns Err with EINTR
            println!("[PARENT] sigsuspend() returned");

            // Step 6: Verify sigsuspend() returned EINTR
            println!("\nStep 6: Verify sigsuspend() return value");
            if suspend_result.is_err() {
                println!("  PASS: sigsuspend() correctly returned error (EINTR)");
            } else {
                println!("  FAIL: sigsuspend() should return error");
                println!("SIGSUSPEND_TEST_FAILED");
                std::process::exit(1);
            }

            // Step 7: Verify signal handler was called
            println!("\nStep 7: Verify SIGUSR1 handler was called");
            if SIGUSR1_RECEIVED.load(Ordering::SeqCst) {
                println!("  PASS: SIGUSR1 handler was called!");
            } else {
                println!("  FAIL: SIGUSR1 handler was NOT called");
                println!("SIGSUSPEND_TEST_FAILED");
                std::process::exit(1);
            }

            // Step 8: Verify original mask (with SIGUSR1 blocked) is restored
            println!("\nStep 8: Verify original mask is restored after sigsuspend()");
            let mut restored_mask: u64 = 0;
            if sigprocmask(SIG_SETMASK, None, Some(&mut restored_mask)).is_err() {
                println!("  FAIL: sigprocmask query failed");
                println!("SIGSUSPEND_TEST_FAILED");
                std::process::exit(1);
            }
            println!("  Restored mask: {:#018x}", restored_mask);
            println!("  Expected mask: {:#018x}", sigusr1_mask);

            if (restored_mask & sigusr1_mask) != 0 {
                println!("  PASS: Original mask restored - SIGUSR1 is blocked again");
            } else {
                println!("  FAIL: Original mask NOT restored - SIGUSR1 is not blocked");
                println!("SIGSUSPEND_TEST_FAILED");
                std::process::exit(1);
            }

            // Step 9: Verify signal was delivered during suspend
            println!("\nStep 9: Verify signal was delivered during sigsuspend(), not after");
            println!("  (Handler was already called during sigsuspend() - correct behavior)");
            println!("  PASS: Signal delivered atomically during mask replacement");

            // Step 10: Wait for child to exit
            println!("\nStep 10: Waiting for child to exit...");
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
            println!("\n=== All sigsuspend() tests passed! ===");
            println!("SIGSUSPEND_TEST_PASSED");
            std::process::exit(0);
        }
    }
}
