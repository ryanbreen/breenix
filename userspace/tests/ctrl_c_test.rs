//! Ctrl-C signal handling test
//!
//! This test validates that:
//! 1. A child process can be forked
//! 2. SIGINT can be sent to the child via kill()
//! 3. The child is terminated by the signal
//! 4. waitpid() correctly reports WIFSIGNALED and WTERMSIG == SIGINT
//!
//! This tests the core signal delivery mechanism that would be triggered
//! by Ctrl-C from the keyboard (the TTY->SIGINT path is tested separately
//! in TTY unit tests).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to string and print it
unsafe fn print_number(prefix: &str, num: u64) {
    io::print(prefix);

    let mut n = num;
    let mut i = 0;

    if n == 0 {
        BUFFER[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            BUFFER[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        for j in 0..i / 2 {
            let tmp = BUFFER[j];
            BUFFER[j] = BUFFER[i - j - 1];
            BUFFER[i - j - 1] = tmp;
        }
    }

    io::write(libbreenix::types::fd::STDOUT, &BUFFER[..i]);
    io::print("\n");
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Ctrl-C Signal Test ===\n");

        let my_pid = process::getpid();
        print_number("Parent PID: ", my_pid);

        // Fork a child process
        io::print("Forking child process...\n");
        let fork_result = process::fork();

        if fork_result == 0 {
            // Child process - loop forever, waiting for signal
            io::print("  CHILD: Started, waiting for SIGINT...\n");
            let child_pid = process::getpid();
            print_number("  CHILD: My PID is ", child_pid);

            // Busy loop - should be killed by SIGINT from parent
            let mut counter = 0u64;
            loop {
                counter = counter.wrapping_add(1);
                if counter % 10_000_000 == 0 {
                    io::print("  CHILD: Still alive...\n");
                }
                // Yield to let parent run
                if counter % 100_000 == 0 {
                    process::yield_now();
                }
            }
        } else if fork_result > 0 {
            // Parent process
            let child_pid = fork_result;
            print_number("  PARENT: Forked child with PID ", child_pid as u64);

            // Small delay to let child start
            io::print("  PARENT: Waiting for child to start...\n");
            for i in 0..5 {
                io::print("  PARENT: yield ");
                print_number("", i as u64);
                process::yield_now();
            }
            io::print("  PARENT: Done waiting, about to send SIGINT\n");

            // Send SIGINT to child (simulating Ctrl-C)
            io::print("  PARENT: Sending SIGINT (Ctrl-C) to child\n");
            match signal::kill(child_pid as i32, signal::SIGINT) {
                Ok(()) => {
                    io::print("  PARENT: kill(SIGINT) syscall succeeded\n");
                }
                Err(e) => {
                    io::print("  PARENT: kill(SIGINT) failed with error ");
                    print_number("", e as u64);
                    process::exit(1);
                }
            }

            // Wait for child to actually terminate using waitpid
            io::print("  PARENT: Waiting for child to terminate...\n");
            let mut status: i32 = 0;
            let result = process::waitpid(child_pid as i32, &mut status, 0);

            if result == child_pid {
                print_number("  PARENT: waitpid returned, status = ", status as u64);

                // Check if child was terminated by signal using POSIX macros
                if process::wifsignaled(status) {
                    let termsig = process::wtermsig(status);
                    print_number("  PARENT: Child terminated by signal ", termsig as u64);

                    if termsig == signal::SIGINT {
                        io::print("  PARENT: Child correctly terminated by SIGINT!\n");
                        io::print("CTRL_C_TEST_PASSED\n");
                        process::exit(0);
                    } else {
                        io::print("  PARENT: FAIL - Child terminated by wrong signal\n");
                        io::print("  Expected SIGINT (2), got: ");
                        print_number("", termsig as u64);
                        process::exit(2);
                    }
                } else if process::wifexited(status) {
                    // Child exited normally (not by signal)
                    let exit_code = process::wexitstatus(status);
                    io::print("  PARENT: FAIL - Child exited normally, not by signal\n");
                    io::print("  Exit code: ");
                    print_number("", exit_code as u64);
                    process::exit(3);
                } else if process::wifstopped(status) {
                    let stopsig = process::wstopsig(status);
                    io::print("  PARENT: FAIL - Child was stopped, not terminated\n");
                    io::print("  Stop signal: ");
                    print_number("", stopsig as u64);
                    process::exit(4);
                } else {
                    io::print("  PARENT: FAIL - Unknown wait status\n");
                    process::exit(5);
                }
            } else if result < 0 {
                io::print("  PARENT: waitpid failed with error ");
                print_number("", (-result) as u64);
                process::exit(6);
            } else {
                io::print("  PARENT: waitpid returned unexpected PID ");
                print_number("", result as u64);
                process::exit(7);
            }
        } else {
            // Fork failed
            io::print("  PARENT: fork() failed with error ");
            print_number("", (-fork_result) as u64);
            process::exit(8);
        }
    }

}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in ctrl_c_test!\n");
    process::exit(255);
}
