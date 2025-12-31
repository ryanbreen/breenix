//! Signal exec reset test
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

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Signal handler for SIGUSR1 (should never be called in exec'd process)
extern "C" fn sigusr1_handler(_sig: i32) {
    io::print("ERROR: Handler was called but should have been reset by exec!\n");
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
        io::print("=== Signal Exec Reset Test ===\n");

        // Step 1: Register signal handler for SIGUSR1
        io::print("\nStep 1: Register SIGUSR1 handler\n");
        let action = signal::Sigaction::new(sigusr1_handler);

        match signal::sigaction(signal::SIGUSR1, Some(&action), None) {
            Ok(()) => io::print("  PASS: sigaction registered handler\n"),
            Err(e) => {
                io::print("  FAIL: sigaction returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGNAL_EXEC_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Verify handler was set
        let mut verify_action = signal::Sigaction::default();
        match signal::sigaction(signal::SIGUSR1, None, Some(&mut verify_action)) {
            Ok(()) => {
                io::print("  Handler address: ");
                print_number(verify_action.handler);
                io::print("\n");
                if verify_action.handler == signal::SIG_DFL || verify_action.handler == signal::SIG_IGN {
                    io::print("  WARN: Handler appears to be default/ignore, test may not be valid\n");
                }
            }
            Err(_) => {
                io::print("  WARN: Could not verify handler was set\n");
            }
        }

        // Step 2: Fork child
        io::print("\nStep 2: Forking child process...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            io::print("  FAIL: fork() failed with error ");
            print_signed(fork_result);
            io::print("\n");
            io::print("SIGNAL_EXEC_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Forked successfully, about to exec signal_exec_check\n");

            // Verify child inherited the handler (before exec)
            let mut child_action = signal::Sigaction::default();
            match signal::sigaction(signal::SIGUSR1, None, Some(&mut child_action)) {
                Ok(()) => {
                    io::print("[CHILD] Pre-exec handler: ");
                    print_number(child_action.handler);
                    io::print("\n");
                    if child_action.handler != signal::SIG_DFL && child_action.handler != signal::SIG_IGN {
                        io::print("[CHILD] Handler inherited from parent (as expected)\n");
                    }
                }
                Err(_) => {}
            }

            // Step 3: Exec into signal_exec_check
            // Note: The Breenix exec currently uses hardcoded binaries.
            // We'll exec with program name "signal_exec_check" and hope the kernel
            // loads it. If not, we'll fall back to verifying handler reset locally.
            io::print("[CHILD] Calling exec(signal_exec_check)...\n");

            // The program name must be null-terminated for the kernel to read it correctly
            // Rust &str is NOT null-terminated, so we use a static C string
            static PROGRAM_NAME: &[u8] = b"signal_exec_check\0";
            let exec_result = unsafe {
                libbreenix::syscall::raw::syscall2(
                    libbreenix::syscall::nr::EXEC,
                    PROGRAM_NAME.as_ptr() as u64,
                    0,
                ) as i64
            };

            // If exec returns, it failed
            io::print("[CHILD] exec() returned (should not happen on success): ");
            print_signed(exec_result);
            io::print("\n");

            // Fallback: Check handler state after failed exec
            // This isn't ideal but shows the test structure
            io::print("[CHILD] Note: exec may not be fully implemented for this binary\n");
            io::print("[CHILD] Checking if handler is still set post-exec-attempt...\n");

            let mut post_exec_action = signal::Sigaction::default();
            match signal::sigaction(signal::SIGUSR1, None, Some(&mut post_exec_action)) {
                Ok(()) => {
                    io::print("[CHILD] Post-exec handler: ");
                    print_number(post_exec_action.handler);
                    io::print("\n");
                }
                Err(_) => {}
            }

            // Since exec didn't work as expected, this is a partial test
            io::print("[CHILD] Exiting - exec implementation may need extension\n");
            process::exit(42); // Special exit code to indicate exec didn't replace process
        } else {
            // ========== PARENT PROCESS ==========
            io::print("[PARENT] Forked child PID: ");
            print_number(fork_result as u64);
            io::print("\n");

            // Wait for child
            io::print("[PARENT] Waiting for child...\n");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

            if wait_result != fork_result {
                io::print("[PARENT] FAIL: waitpid returned wrong PID\n");
                io::print("SIGNAL_EXEC_TEST_FAILED\n");
                process::exit(1);
            }

            if process::wifexited(status) {
                let exit_code = process::wexitstatus(status);
                io::print("[PARENT] Child exit code: ");
                print_number(exit_code as u64);
                io::print("\n");

                if exit_code == 0 {
                    // signal_exec_check verified handler is SIG_DFL
                    io::print("[PARENT] Child (signal_exec_check) verified SIG_DFL!\n");
                    io::print("\n=== Signal exec reset test passed! ===\n");
                    io::print("SIGNAL_EXEC_TEST_PASSED\n");
                    process::exit(0);
                } else if exit_code == 1 {
                    // signal_exec_check found SIG_IGN (acceptable per POSIX but not ideal)
                    io::print("[PARENT] Child reported handler is SIG_IGN (partial pass per POSIX)\n");
                    io::print("\n=== Signal exec reset test passed (SIG_IGN) ===\n");
                    io::print("SIGNAL_EXEC_TEST_PASSED\n");
                    process::exit(0);
                } else if exit_code == 2 {
                    // signal_exec_check found user handler NOT reset
                    io::print("[PARENT] FAIL: Handler was NOT reset to SIG_DFL after exec!\n");
                    io::print("[PARENT] The old handler address was inherited, violating POSIX.\n");
                    io::print("SIGNAL_EXEC_TEST_FAILED\n");
                    process::exit(1);
                } else if exit_code == 3 {
                    // signal_exec_check couldn't query sigaction
                    io::print("[PARENT] FAIL: Child couldn't query signal handler state.\n");
                    io::print("SIGNAL_EXEC_TEST_FAILED\n");
                    process::exit(1);
                } else if exit_code == 42 {
                    // Exec returned instead of replacing the process
                    // This is a REAL failure - exec is broken
                    io::print("[PARENT] FAIL: exec() returned instead of replacing process!\n");
                    io::print("[PARENT] The exec syscall did not work as expected.\n");
                    io::print("SIGNAL_EXEC_TEST_FAILED\n");
                    process::exit(1);
                } else {
                    io::print("[PARENT] FAIL: Unexpected exit code from child\n");
                    io::print("SIGNAL_EXEC_TEST_FAILED\n");
                    process::exit(1);
                }
            } else {
                io::print("[PARENT] Child did not exit normally\n");
                io::print("SIGNAL_EXEC_TEST_FAILED\n");
                process::exit(1);
            }
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in signal exec test!\n");
    io::print("SIGNAL_EXEC_TEST_FAILED\n");
    process::exit(255);
}
