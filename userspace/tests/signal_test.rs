//! Signal test program
//!
//! Tests basic signal functionality:
//! 1. kill() syscall to send SIGTERM to child
//! 2. Default signal handler (terminate)

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
        io::print("=== Signal Test ===\n");

        let my_pid = process::getpid();
        print_number("My PID: ", my_pid);

        // Test 1: Check if process exists using kill(pid, 0)
        io::print("\nTest 1: Check process exists with kill(pid, 0)\n");
        match signal::kill(my_pid as i32, 0) {
            Ok(()) => io::print("  PASS: Process exists\n"),
            Err(e) => {
                io::print("  FAIL: kill returned error ");
                print_number("", e as u64);
            }
        }

        // Test 2: Fork and send SIGTERM to child
        io::print("\nTest 2: Fork and send SIGTERM to child\n");
        let fork_result = process::fork();

        if fork_result == 0 {
            // Child process - loop forever, waiting for signal
            io::print("  CHILD: Started, waiting for signal...\n");
            let child_pid = process::getpid();
            print_number("  CHILD: My PID is ", child_pid);

            // Busy loop - should be killed by parent
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
        } else {
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
            io::print("  PARENT: Done waiting, about to send signal\n");

            // Send SIGTERM to child
            io::print("  PARENT: Sending SIGTERM to child\n");
            match signal::kill(child_pid as i32, signal::SIGTERM) {
                Ok(()) => {
                    io::print("  PARENT: kill() syscall succeeded\n");
                }
                Err(e) => {
                    io::print("  PARENT: kill() failed with error ");
                    print_number("", e as u64);
                    process::exit(1);
                }
            }

            // Wait for child to actually terminate using waitpid
            io::print("  PARENT: Waiting for child to terminate...\n");
            let mut status: i32 = 0;
            let result = process::waitpid(child_pid as i32, &mut status, 0);

            if result == child_pid {
                // Check if child was terminated by signal
                // WIFSIGNALED: (status & 0x7f) != 0
                // WTERMSIG: status & 0x7f
                let termsig = status & 0x7f;
                if termsig == signal::SIGTERM as i32 {
                    io::print("  PARENT: Child terminated by SIGTERM!\n");
                    io::print("SIGNAL_KILL_TEST_PASSED\n");
                } else if termsig != 0 {
                    io::print("  PARENT: Child terminated by wrong signal: ");
                    print_number("", termsig as u64);
                    process::exit(2);
                } else {
                    // Child exited normally (WIFEXITED)
                    io::print("  PARENT: Child exited normally (not by signal), exit code: ");
                    print_number("", ((status >> 8) & 0xff) as u64);
                    process::exit(3);
                }
            } else {
                io::print("  PARENT: waitpid returned unexpected value: ");
                print_number("", result as u64);
                process::exit(4);
            }

            io::print("  PARENT: Test complete, exiting\n");
            process::exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in signal test!\n");
    process::exit(255);
}
