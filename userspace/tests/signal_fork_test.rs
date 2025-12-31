//! Signal handler fork inheritance test
//!
//! Tests that signal handlers are inherited across fork():
//! 1. Parent registers a signal handler for SIGUSR1
//! 2. Parent forks
//! 3. Child sends SIGUSR1 to itself
//! 4. Child's handler is called (inherited from parent)
//!
//! POSIX requires that signal handlers are inherited by the child process.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Static flag to track if handler was called
static mut HANDLER_CALLED: bool = false;

/// Signal handler for SIGUSR1
extern "C" fn sigusr1_handler(_sig: i32) {
    unsafe {
        HANDLER_CALLED = true;
        io::print("  HANDLER: SIGUSR1 received!\n");
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
        io::print("=== Signal Fork Inheritance Test ===\n");

        // Step 1: Register signal handler in parent
        io::print("\nStep 1: Register SIGUSR1 handler in parent\n");
        let action = signal::Sigaction::new(sigusr1_handler);

        match signal::sigaction(signal::SIGUSR1, Some(&action), None) {
            Ok(()) => io::print("  PASS: sigaction registered handler\n"),
            Err(e) => {
                io::print("  FAIL: sigaction returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGNAL_FORK_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Step 2: Fork
        io::print("\nStep 2: Forking process...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            io::print("  FAIL: fork() failed with error ");
            print_signed(fork_result);
            io::print("\n");
            io::print("SIGNAL_FORK_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Process started\n");

            let my_pid = process::getpid();
            io::print("[CHILD] PID: ");
            print_number(my_pid);
            io::print("\n");

            // Step 3: Send SIGUSR1 to self
            io::print("[CHILD] Step 3: Sending SIGUSR1 to self...\n");
            match signal::kill(my_pid as i32, signal::SIGUSR1) {
                Ok(()) => io::print("[CHILD]   kill() succeeded\n"),
                Err(e) => {
                    io::print("[CHILD]   FAIL: kill() returned error ");
                    print_number(e as u64);
                    io::print("\n");
                    io::print("SIGNAL_FORK_TEST_FAILED\n");
                    process::exit(1);
                }
            }

            // Step 4: Yield to allow signal delivery
            io::print("[CHILD] Step 4: Yielding for signal delivery...\n");
            for i in 0..10 {
                process::yield_now();
                if HANDLER_CALLED {
                    io::print("[CHILD]   Handler called after ");
                    print_number(i + 1);
                    io::print(" yields\n");
                    break;
                }
            }

            // Step 5: Verify handler was called
            io::print("[CHILD] Step 5: Verify handler execution\n");
            if HANDLER_CALLED {
                io::print("[CHILD]   PASS: Inherited handler was called!\n");
                io::print("[CHILD] Exiting with success\n");
                process::exit(0);
            } else {
                io::print("[CHILD]   FAIL: Inherited handler was NOT called\n");
                io::print("SIGNAL_FORK_TEST_FAILED\n");
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
                io::print("SIGNAL_FORK_TEST_FAILED\n");
                process::exit(1);
            }

            // Check if child exited normally with code 0
            if process::wifexited(status) {
                let exit_code = process::wexitstatus(status);
                if exit_code == 0 {
                    io::print("[PARENT] Child exited successfully (code 0)\n");
                    io::print("\n=== All signal fork inheritance tests passed! ===\n");
                    io::print("SIGNAL_FORK_TEST_PASSED\n");
                    process::exit(0);
                } else {
                    io::print("[PARENT] Child exited with non-zero code: ");
                    print_number(exit_code as u64);
                    io::print("\n");
                    io::print("SIGNAL_FORK_TEST_FAILED\n");
                    process::exit(1);
                }
            } else {
                io::print("[PARENT] Child did not exit normally\n");
                io::print("SIGNAL_FORK_TEST_FAILED\n");
                process::exit(1);
            }
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in signal fork test!\n");
    io::print("SIGNAL_FORK_TEST_FAILED\n");
    process::exit(255);
}
