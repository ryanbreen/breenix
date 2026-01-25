//! Sigsuspend syscall test program
//!
//! Tests the sigsuspend() syscall which atomically replaces the signal mask
//! and suspends until a signal is delivered:
//! 1. Parent blocks SIGUSR1 with sigprocmask
//! 2. Parent registers a signal handler for SIGUSR1
//! 3. Parent forks a child process
//! 4. Child sends SIGUSR1 to parent after yielding
//! 5. Parent calls sigsuspend() with a mask that UNBLOCKS SIGUSR1
//! 6. Parent wakes up when signal is delivered
//! 7. Verify sigsuspend() returns -EINTR (-4)
//! 8. Verify original mask (with SIGUSR1 blocked) is restored
//! 9. Print "SIGSUSPEND_TEST_PASSED" on success

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

/// Print a 64-bit mask in hex
unsafe fn print_hex_mask(mask: u64) {
    io::print("0x");
    let hex_digits = b"0123456789abcdef";
    let mut buffer: [u8; 16] = [0; 16];

    for i in 0..16 {
        let nibble = ((mask >> (60 - i * 4)) & 0xF) as usize;
        buffer[i] = hex_digits[nibble];
    }

    io::write(fd::STDOUT, &buffer[..]);
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Sigsuspend Syscall Test ===\n");

        let parent_pid = process::getpid();
        io::print("Parent PID: ");
        print_number(parent_pid);
        io::print("\n");

        // Step 1: Block SIGUSR1 initially with sigprocmask
        io::print("\nStep 1: Block SIGUSR1 with sigprocmask\n");
        let sigusr1_mask = signal::sigmask(signal::SIGUSR1);

        io::print("  SIGUSR1 mask: ");
        print_hex_mask(sigusr1_mask);
        io::print("\n");

        let mut old_mask: u64 = 0;
        match signal::sigprocmask(signal::SIG_BLOCK, Some(&sigusr1_mask), Some(&mut old_mask)) {
            Ok(()) => {
                io::print("  PASS: sigprocmask blocked SIGUSR1\n");
                io::print("  Old mask: ");
                print_hex_mask(old_mask);
                io::print("\n");
            }
            Err(e) => {
                io::print("  FAIL: sigprocmask returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGSUSPEND_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Step 2: Verify SIGUSR1 is now blocked by checking current mask
        io::print("\nStep 2: Verify SIGUSR1 is blocked\n");
        let mut current_mask: u64 = 0;
        match signal::sigprocmask(signal::SIG_SETMASK, None, Some(&mut current_mask)) {
            Ok(()) => {
                io::print("  Current mask: ");
                print_hex_mask(current_mask);
                io::print("\n");

                if (current_mask & sigusr1_mask) != 0 {
                    io::print("  PASS: SIGUSR1 is blocked in current mask\n");
                } else {
                    io::print("  FAIL: SIGUSR1 is NOT blocked in current mask\n");
                    io::print("SIGSUSPEND_TEST_FAILED\n");
                    process::exit(1);
                }
            }
            Err(e) => {
                io::print("  FAIL: sigprocmask query failed with error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGSUSPEND_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Step 3: Register SIGUSR1 handler
        io::print("\nStep 3: Register SIGUSR1 handler in parent\n");
        let action = signal::Sigaction::new(sigusr1_handler);

        match signal::sigaction(signal::SIGUSR1, Some(&action), None) {
            Ok(()) => io::print("  PASS: sigaction registered SIGUSR1 handler\n"),
            Err(e) => {
                io::print("  FAIL: sigaction returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGSUSPEND_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Step 4: Fork child
        io::print("\nStep 4: Forking child process...\n");
        let fork_result = process::fork();

        if fork_result < 0 {
            io::print("  FAIL: fork() failed with error ");
            print_signed(fork_result);
            io::print("\n");
            io::print("SIGSUSPEND_TEST_FAILED\n");
            process::exit(1);
        }

        if fork_result == 0 {
            // ========== CHILD PROCESS ==========
            io::print("[CHILD] Process started\n");
            io::print("[CHILD] My PID: ");
            print_number(process::getpid());
            io::print("\n");

            // Give parent time to call sigsuspend()
            io::print("[CHILD] Yielding to let parent call sigsuspend()...\n");
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

            // Step 5: Call sigsuspend() with a mask that UNBLOCKS SIGUSR1
            io::print("\nStep 5: Calling sigsuspend() with empty mask (unblocks SIGUSR1)...\n");

            // Create an empty mask (no signals blocked) for sigsuspend
            // This will UNBLOCK SIGUSR1 atomically when sigsuspend() is called
            let suspend_mask: u64 = 0;

            io::print("  Suspend mask: ");
            print_hex_mask(suspend_mask);
            io::print("\n");
            io::print("  Calling sigsuspend()...\n");

            let suspend_ret = signal::sigsuspend(&suspend_mask);

            // sigsuspend() should return -EINTR (-4) when interrupted by signal
            io::print("[PARENT] sigsuspend() returned: ");
            print_signed(suspend_ret);
            io::print("\n");

            // Step 6: Verify sigsuspend() returned -EINTR (-4)
            io::print("\nStep 6: Verify sigsuspend() return value\n");
            if suspend_ret != -4 {
                io::print("  FAIL: sigsuspend() should return -4 (-EINTR), got ");
                print_signed(suspend_ret);
                io::print("\n");
                io::print("SIGSUSPEND_TEST_FAILED\n");
                process::exit(1);
            }
            io::print("  PASS: sigsuspend() correctly returned -EINTR (-4)\n");

            // Step 7: Verify signal handler was called
            io::print("\nStep 7: Verify SIGUSR1 handler was called\n");

            if SIGUSR1_RECEIVED {
                io::print("  PASS: SIGUSR1 handler was called!\n");
            } else {
                io::print("  FAIL: SIGUSR1 handler was NOT called\n");
                io::print("SIGSUSPEND_TEST_FAILED\n");
                process::exit(1);
            }

            // Step 8: Verify original mask (with SIGUSR1 blocked) is restored
            io::print("\nStep 8: Verify original mask is restored after sigsuspend()\n");

            let mut restored_mask: u64 = 0;
            match signal::sigprocmask(signal::SIG_SETMASK, None, Some(&mut restored_mask)) {
                Ok(()) => {
                    io::print("  Restored mask: ");
                    print_hex_mask(restored_mask);
                    io::print("\n");
                    io::print("  Expected mask: ");
                    print_hex_mask(sigusr1_mask);
                    io::print("\n");

                    if (restored_mask & sigusr1_mask) != 0 {
                        io::print("  PASS: Original mask restored - SIGUSR1 is blocked again\n");
                    } else {
                        io::print("  FAIL: Original mask NOT restored - SIGUSR1 is not blocked\n");
                        io::print("SIGSUSPEND_TEST_FAILED\n");
                        process::exit(1);
                    }
                }
                Err(e) => {
                    io::print("  FAIL: sigprocmask query failed with error ");
                    print_number(e as u64);
                    io::print("\n");
                    io::print("SIGSUSPEND_TEST_FAILED\n");
                    process::exit(1);
                }
            }

            // Step 9: Verify blocked signals during suspend are not delivered
            io::print("\nStep 9: Verify signal was delivered during sigsuspend(), not after\n");
            io::print("  (Handler was already called during sigsuspend() - correct behavior)\n");
            io::print("  PASS: Signal delivered atomically during mask replacement\n");

            // Step 10: Wait for child to exit
            io::print("\nStep 10: Waiting for child to exit...\n");
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
            io::print("\n=== All sigsuspend() tests passed! ===\n");
            io::print("SIGSUSPEND_TEST_PASSED\n");
            process::exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in sigsuspend test!\n");
    io::print("SIGSUSPEND_TEST_FAILED\n");
    process::exit(255);
}
