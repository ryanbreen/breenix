//! SIGCHLD delivery test
//!
//! Tests that SIGCHLD is delivered to parent when child exits:
//! 1. Parent registers SIGCHLD handler
//! 2. Parent forks child
//! 3. Child exits
//! 4. Parent's SIGCHLD handler is called
//!
//! POSIX requires that the parent receive SIGCHLD when a child terminates.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Static flag to track if SIGCHLD handler was called
static mut SIGCHLD_RECEIVED: bool = false;

/// SIGCHLD handler
extern "C" fn sigchld_handler(_sig: i32) {
    unsafe {
        SIGCHLD_RECEIVED = true;
        io::print("  SIGCHLD_HANDLER: Child termination signal received!\n");
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
        io::print("=== SIGCHLD Delivery Test ===\n");

        // Step 1: Register SIGCHLD handler
        io::print("\nStep 1: Register SIGCHLD handler in parent\n");
        let action = signal::Sigaction::new(sigchld_handler);

        match signal::sigaction(signal::SIGCHLD, Some(&action), None) {
            Ok(()) => io::print("  PASS: sigaction registered SIGCHLD handler\n"),
            Err(e) => {
                io::print("  FAIL: sigaction returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGCHLD_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Step 2: Fork child
        io::print("\nStep 2: Forking child process...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            io::print("  FAIL: fork() failed with error ");
            print_signed(fork_result);
            io::print("\n");
            io::print("SIGCHLD_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Process started, exiting immediately with code 42\n");
            process::exit(42);
        } else {
            // ========== PARENT PROCESS ==========
            io::print("[PARENT] Forked child PID: ");
            print_number(fork_result as u64);
            io::print("\n");

            // Step 3: Wait for child with waitpid - this GUARANTEES child has exited
            // When waitpid returns, SIGCHLD must have been set as pending because
            // the kernel sets it when the child exits. Signal delivery happens on
            // syscall return boundary (when returning from waitpid).
            io::print("\nStep 3: Waiting for child with waitpid (blocking)...\n");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

            if wait_result != fork_result {
                io::print("[PARENT] FAIL: waitpid returned wrong PID: ");
                print_signed(wait_result);
                io::print("\n");
                io::print("SIGCHLD_TEST_FAILED\n");
                process::exit(1);
            }

            // Verify child exit code
            if process::wifexited(status) {
                let exit_code = process::wexitstatus(status);
                io::print("[PARENT] Child exited with code: ");
                print_number(exit_code as u64);
                io::print("\n");
            }

            // Step 4: Check if SIGCHLD was already delivered
            // After waitpid returns, SIGCHLD should have been delivered because:
            // 1. Child exit sets SIGCHLD pending on parent
            // 2. Signal delivery happens when returning from syscall to userspace
            io::print("\nStep 4: Verify SIGCHLD was delivered\n");

            // If not delivered yet, yield once to give signal delivery a chance
            // Signal delivery can happen during context switch (timer interrupt)
            if !SIGCHLD_RECEIVED {
                io::print("  SIGCHLD not yet received, yielding once...\n");
                process::yield_now();
            }

            // Final check
            if SIGCHLD_RECEIVED {
                io::print("  PASS: SIGCHLD handler was called!\n");
                io::print("\n=== All SIGCHLD delivery tests passed! ===\n");
                io::print("SIGCHLD_TEST_PASSED\n");
                process::exit(0);
            } else {
                io::print("  FAIL: SIGCHLD handler was NOT called\n");
                io::print("  (Note: This may indicate the kernel doesn't send SIGCHLD on child exit)\n");
                io::print("SIGCHLD_TEST_FAILED\n");
                process::exit(1);
            }
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in SIGCHLD test!\n");
    io::print("SIGCHLD_TEST_FAILED\n");
    process::exit(255);
}
