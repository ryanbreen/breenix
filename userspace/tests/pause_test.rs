//! Pause syscall test program
//!
//! Tests the pause() syscall which blocks until a signal is delivered:
//! 1. Parent registers a signal handler for SIGUSR1
//! 2. Parent forks a child process
//! 3. Parent calls pause() to block
//! 4. Child sends SIGUSR1 to parent using kill()
//! 5. Parent wakes up, verifies signal handler was called
//! 6. Print "PAUSE_TEST_PASSED" on success

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;
use libbreenix::types::fd;

/// Static flag to track if SIGUSR1 handler was called
static mut SIGUSR1_RECEIVED: bool = false;

/// SIGUSR1 handler - sets flag when called
extern "C" fn sigusr1_handler(_sig: i32) {
    unsafe {
        SIGUSR1_RECEIVED = true;
        io::print("  HANDLER: SIGUSR1 received in parent!\n");
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

    io::write(fd::STDOUT, &buffer[..i]);
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
        io::print("=== Pause Syscall Test ===\n");

        let parent_pid = process::getpid();
        io::print("Parent PID: ");
        print_number(parent_pid);
        io::print("\n");

        // Step 1: Register SIGUSR1 handler
        io::print("\nStep 1: Register SIGUSR1 handler in parent\n");
        let action = signal::Sigaction::new(sigusr1_handler);

        match signal::sigaction(signal::SIGUSR1, Some(&action), None) {
            Ok(()) => io::print("  PASS: sigaction registered SIGUSR1 handler\n"),
            Err(e) => {
                io::print("  FAIL: sigaction returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("PAUSE_TEST_FAILED\n");
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
            io::print("PAUSE_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Process started\n");
            io::print("[CHILD] My PID: ");
            print_number(process::getpid());
            io::print("\n");

            // Give parent time to call pause()
            io::print("[CHILD] Yielding to let parent call pause()...\n");
            for _ in 0..5 {
                process::yield_now();
            }

            // Send SIGUSR1 to parent
            io::print("[CHILD] Sending SIGUSR1 to parent (PID ");
            print_number(parent_pid);
            io::print(")...\n");

            match signal::kill(parent_pid as i32, signal::SIGUSR1) {
                Ok(()) => io::print("[CHILD] kill() succeeded\n"),
                Err(e) => {
                    io::print("[CHILD] kill() failed with error ");
                    print_number(e as u64);
                    io::print("\n");
                }
            }

            io::print("[CHILD] Exiting with code 0\n");
            process::exit(0);
        } else {
            // ========== PARENT PROCESS ==========
            io::print("[PARENT] Forked child PID: ");
            print_number(fork_result as u64);
            io::print("\n");

            // Step 3: Call pause() to wait for signal
            io::print("\nStep 3: Calling pause() to wait for signal...\n");
            let pause_ret = signal::pause();

            // pause() should return -EINTR (-4) when interrupted by signal
            io::print("[PARENT] pause() returned: ");
            print_signed(pause_ret);
            io::print("\n");

            // Verify pause() returned -EINTR (-4)
            if pause_ret != -4 {
                io::print("  FAIL: pause() should return -4 (-EINTR), got ");
                print_signed(pause_ret);
                io::print("\n");
                io::print("PAUSE_TEST_FAILED\n");
                process::exit(1);
            }
            io::print("  PASS: pause() correctly returned -EINTR (-4)\n");

            // Step 4: Verify signal handler was called
            io::print("\nStep 4: Verify SIGUSR1 handler was called\n");

            if SIGUSR1_RECEIVED {
                io::print("  PASS: SIGUSR1 handler was called!\n");
            } else {
                io::print("  FAIL: SIGUSR1 handler was NOT called\n");
                io::print("PAUSE_TEST_FAILED\n");
                process::exit(1);
            }

            // Step 5: Wait for child to exit
            io::print("\nStep 5: Waiting for child to exit...\n");
            let mut status: i32 = 0;
            let wait_result = process::waitpid(fork_result as i32, &mut status as *mut i32, 0);

            if wait_result == fork_result {
                io::print("  Child reaped successfully\n");
            } else {
                io::print("  Warning: waitpid returned ");
                print_signed(wait_result);
                io::print(" (expected ");
                print_number(fork_result as u64);
                io::print(")\n");
            }

            // All tests passed
            io::print("\n=== All pause() tests passed! ===\n");
            io::print("PAUSE_TEST_PASSED\n");
            process::exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in pause test!\n");
    io::print("PAUSE_TEST_FAILED\n");
    process::exit(255);
}
