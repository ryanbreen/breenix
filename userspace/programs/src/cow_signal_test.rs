//! Copy-on-Write Signal Delivery Test
//!
//! This test specifically verifies that signal delivery works correctly
//! when the user stack is a CoW-shared page. This was the root cause of
//! a deadlock bug where:
//!
//! 1. Signal delivery acquires PROCESS_MANAGER lock
//! 2. Signal delivery writes to user stack (signal frame + trampoline)
//! 3. User stack is a CoW page (shared with parent after fork)
//! 4. CoW page fault handler needs PROCESS_MANAGER lock
//! 5. DEADLOCK - spinning forever waiting for a lock we already hold
//!
//! The fix uses `try_manager()` and falls back to direct page table
//! manipulation via CR3 when the lock is already held.
//!
//! Test markers:
//! - COW_SIGNAL_TEST_PASSED: All tests passed
//! - COW_SIGNAL_TEST_FAILED: A test failed

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::ptr;

use libbreenix::process::{fork, waitpid, getpid, yield_now, wifexited, wexitstatus, ForkResult};
use libbreenix::signal::{self, Sigaction, SIGUSR1, SA_RESTORER, __restore_rt};
use libbreenix::kill;

/// Static flag to track if handler was called
static HANDLER_CALLED: AtomicBool = AtomicBool::new(false);

/// Static variable that handler will modify (on CoW page)
static HANDLER_MODIFIED_VALUE: AtomicU64 = AtomicU64::new(0);

/// Signal handler for SIGUSR1
/// This handler writes to stack and static memory - both may be CoW pages
extern "C" fn sigusr1_handler(_sig: i32) {
    // This write happens while signal delivery context is active
    // If CoW handling deadlocks, we'll never reach here
    HANDLER_CALLED.store(true, Ordering::SeqCst);
    HANDLER_MODIFIED_VALUE.store(0xCAFEBABE, Ordering::SeqCst);

    // Write to stack (local variable) - this is a CoW page write
    // during signal handler execution
    let mut stack_var: u64 = 0xDEADBEEF;
    // Prevent optimization
    unsafe { ptr::write_volatile(&mut stack_var, 0x12345678) };

    println!("  HANDLER: Signal received, wrote to stack!");
}

fn main() {
    println!("=== CoW Signal Delivery Test ===");
    println!("Tests signal delivery when user stack is CoW-shared\n");

    // Step 1: Touch the stack to ensure the page is present before fork
    // This ensures the page will be CoW-shared (not demand-paged)
    let mut stack_marker: u64 = 0xDEADBEEF;
    unsafe { ptr::write_volatile(&mut stack_marker, 0xDEADBEEF) };
    println!("Step 1: Touched stack page before fork");

    // Step 2: Fork - child inherits parent's address space with CoW
    println!("Step 2: Forking process...");
    match fork() {
        Ok(ForkResult::Child) => {
            // ========== CHILD PROCESS ==========
            println!("[CHILD] Process started");

            let my_pid = getpid().unwrap();
            println!("[CHILD] PID: {}", my_pid.raw());

            // Step 3: Register signal handler
            println!("[CHILD] Step 3: Register SIGUSR1 handler");
            let action = Sigaction {
                handler: sigusr1_handler as u64,
                mask: 0,
                flags: SA_RESTORER,
                restorer: __restore_rt as u64,
            };

            match signal::sigaction(SIGUSR1, Some(&action), None) {
                Ok(()) => println!("[CHILD]   sigaction registered handler"),
                Err(_) => {
                    println!("[CHILD]   FAIL: sigaction returned error");
                    println!("COW_SIGNAL_TEST_FAILED");
                    std::process::exit(1);
                }
            }

            // Step 4: Send SIGUSR1 to self
            // This triggers the critical path:
            // - Signal delivery holds PROCESS_MANAGER lock
            // - Signal delivery writes to user stack (signal frame)
            // - User stack is CoW page (shared with parent)
            // - CoW fault must be handled WITHOUT deadlocking
            println!("[CHILD] Step 4: Sending SIGUSR1 to self (triggers CoW on stack)...");
            match kill(my_pid.raw() as i32, SIGUSR1) {
                Ok(()) => println!("[CHILD]   kill() succeeded"),
                Err(_) => {
                    println!("[CHILD]   FAIL: kill() returned error");
                    println!("COW_SIGNAL_TEST_FAILED");
                    std::process::exit(1);
                }
            }

            // Step 5: Yield to allow signal delivery
            println!("[CHILD] Step 5: Yielding for signal delivery...");
            for i in 0..20 {
                let _ = yield_now();
                if HANDLER_CALLED.load(Ordering::SeqCst) {
                    println!("[CHILD]   Handler called after {} yields", i + 1);
                    break;
                }
            }

            // Step 6: Verify handler was called
            println!("[CHILD] Step 6: Verify handler execution");
            if HANDLER_CALLED.load(Ordering::SeqCst)
                && HANDLER_MODIFIED_VALUE.load(Ordering::SeqCst) == 0xCAFEBABE
            {
                println!("[CHILD]   PASS: Handler executed and modified memory!");
                println!("[CHILD]   CoW fault during signal delivery was handled correctly");
                std::process::exit(0);
            } else if !HANDLER_CALLED.load(Ordering::SeqCst) {
                println!("[CHILD]   FAIL: Handler was NOT called");
                println!("[CHILD]   This could indicate deadlock in CoW fault handling");
                println!("COW_SIGNAL_TEST_FAILED");
                std::process::exit(1);
            } else {
                println!("[CHILD]   FAIL: Handler called but didn't modify value");
                println!("COW_SIGNAL_TEST_FAILED");
                std::process::exit(1);
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // ========== PARENT PROCESS ==========
            println!("[PARENT] Forked child PID: {}", child_pid.raw());

            // Wait for child to complete
            println!("[PARENT] Waiting for child...");
            let mut status: i32 = 0;
            let wait_result = waitpid(child_pid.raw() as i32, &mut status, 0);

            match wait_result {
                Ok(pid) if pid.raw() as i32 == child_pid.raw() as i32 => {}
                _ => {
                    println!("[PARENT] FAIL: waitpid returned wrong PID");
                    println!("COW_SIGNAL_TEST_FAILED");
                    std::process::exit(1);
                }
            }

            // Check if child exited normally with code 0
            if wifexited(status) {
                let exit_code = wexitstatus(status);
                if exit_code == 0 {
                    println!("[PARENT] Child exited successfully");
                    println!("\n=== CoW Signal Delivery Test PASSED ===");
                    println!("COW_SIGNAL_TEST_PASSED");
                    std::process::exit(0);
                } else {
                    println!("[PARENT] Child exited with non-zero code: {}", exit_code);
                    println!("COW_SIGNAL_TEST_FAILED");
                    std::process::exit(1);
                }
            } else {
                println!("[PARENT] Child did not exit normally");
                println!("COW_SIGNAL_TEST_FAILED");
                std::process::exit(1);
            }
        }
        Err(_) => {
            println!("  FAIL: fork() failed");
            println!("COW_SIGNAL_TEST_FAILED");
            std::process::exit(1);
        }
    }
}
