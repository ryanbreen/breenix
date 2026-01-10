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

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Static flag to track if handler was called
static mut HANDLER_CALLED: bool = false;

/// Static variable that handler will modify (on CoW page)
static mut HANDLER_MODIFIED_VALUE: u64 = 0;

/// Signal handler for SIGUSR1
/// This handler writes to stack and static memory - both may be CoW pages
extern "C" fn sigusr1_handler(_sig: i32) {
    unsafe {
        // This write happens while signal delivery context is active
        // If CoW handling deadlocks, we'll never reach here
        HANDLER_CALLED = true;
        HANDLER_MODIFIED_VALUE = 0xCAFEBABE;

        // Write to stack (local variable) - this is a CoW page write
        // during signal handler execution
        let mut stack_var: u64 = 0xDEADBEEF;
        // Prevent optimization
        core::ptr::write_volatile(&mut stack_var, 0x12345678);

        io::print("  HANDLER: Signal received, wrote to stack!\n");
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

    io::write(libbreenix::types::fd::STDOUT, &buffer[..i]);
}

/// Print signed number
unsafe fn print_signed(num: i64) {
    if num < 0 {
        io::print("-");
        print_number((-num) as u64);
    } else {
        print_number(num as u64);
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== CoW Signal Delivery Test ===\n");
        io::print("Tests signal delivery when user stack is CoW-shared\n\n");

        // Step 1: Touch the stack to ensure the page is present before fork
        // This ensures the page will be CoW-shared (not demand-paged)
        let mut stack_marker: u64 = 0xDEADBEEF;
        core::ptr::write_volatile(&mut stack_marker, 0xDEADBEEF);
        io::print("Step 1: Touched stack page before fork\n");

        // Step 2: Fork - child inherits parent's address space with CoW
        io::print("Step 2: Forking process...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            io::print("  FAIL: fork() failed with error ");
            print_signed(fork_result);
            io::print("\n");
            io::print("COW_SIGNAL_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Process started\n");

            let my_pid = process::getpid();
            io::print("[CHILD] PID: ");
            print_number(my_pid);
            io::print("\n");

            // Step 3: Register signal handler
            io::print("[CHILD] Step 3: Register SIGUSR1 handler\n");
            let action = signal::Sigaction::new(sigusr1_handler);

            match signal::sigaction(signal::SIGUSR1, Some(&action), None) {
                Ok(()) => io::print("[CHILD]   sigaction registered handler\n"),
                Err(e) => {
                    io::print("[CHILD]   FAIL: sigaction returned error ");
                    print_number(e as u64);
                    io::print("\n");
                    io::print("COW_SIGNAL_TEST_FAILED\n");
                    process::exit(1);
                }
            }

            // Step 4: Send SIGUSR1 to self
            // This triggers the critical path:
            // - Signal delivery holds PROCESS_MANAGER lock
            // - Signal delivery writes to user stack (signal frame)
            // - User stack is CoW page (shared with parent)
            // - CoW fault must be handled WITHOUT deadlocking
            io::print("[CHILD] Step 4: Sending SIGUSR1 to self (triggers CoW on stack)...\n");
            match signal::kill(my_pid as i32, signal::SIGUSR1) {
                Ok(()) => io::print("[CHILD]   kill() succeeded\n"),
                Err(e) => {
                    io::print("[CHILD]   FAIL: kill() returned error ");
                    print_number(e as u64);
                    io::print("\n");
                    io::print("COW_SIGNAL_TEST_FAILED\n");
                    process::exit(1);
                }
            }

            // Step 5: Yield to allow signal delivery
            io::print("[CHILD] Step 5: Yielding for signal delivery...\n");
            for i in 0..20 {
                process::yield_now();
                if HANDLER_CALLED {
                    io::print("[CHILD]   Handler called after ");
                    print_number(i + 1);
                    io::print(" yields\n");
                    break;
                }
            }

            // Step 6: Verify handler was called
            io::print("[CHILD] Step 6: Verify handler execution\n");
            if HANDLER_CALLED && HANDLER_MODIFIED_VALUE == 0xCAFEBABE {
                io::print("[CHILD]   PASS: Handler executed and modified memory!\n");
                io::print("[CHILD]   CoW fault during signal delivery was handled correctly\n");
                process::exit(0);
            } else if !HANDLER_CALLED {
                io::print("[CHILD]   FAIL: Handler was NOT called\n");
                io::print("[CHILD]   This could indicate deadlock in CoW fault handling\n");
                io::print("COW_SIGNAL_TEST_FAILED\n");
                process::exit(1);
            } else {
                io::print("[CHILD]   FAIL: Handler called but didn't modify value\n");
                io::print("COW_SIGNAL_TEST_FAILED\n");
                process::exit(1);
            }
        } else {
            // ========== PARENT PROCESS ==========
            io::print("[PARENT] Forked child PID: ");
            print_number(fork_result as u64);
            io::print("\n");

            // Wait for child to complete
            io::print("[PARENT] Waiting for child...\n");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

            if wait_result != fork_result {
                io::print("[PARENT] FAIL: waitpid returned wrong PID\n");
                io::print("COW_SIGNAL_TEST_FAILED\n");
                process::exit(1);
            }

            // Check if child exited normally with code 0
            if process::wifexited(status) {
                let exit_code = process::wexitstatus(status);
                if exit_code == 0 {
                    io::print("[PARENT] Child exited successfully\n");
                    io::print("\n=== CoW Signal Delivery Test PASSED ===\n");
                    io::print("COW_SIGNAL_TEST_PASSED\n");
                    process::exit(0);
                } else {
                    io::print("[PARENT] Child exited with non-zero code: ");
                    print_number(exit_code as u64);
                    io::print("\n");
                    io::print("COW_SIGNAL_TEST_FAILED\n");
                    process::exit(1);
                }
            } else {
                io::print("[PARENT] Child did not exit normally\n");
                io::print("COW_SIGNAL_TEST_FAILED\n");
                process::exit(1);
            }
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in cow_signal_test!\n");
    io::print("COW_SIGNAL_TEST_FAILED\n");
    process::exit(255);
}
