//! Signal handler test program
//!
//! Tests that signal handlers actually execute:
//! 1. Register a signal handler using sigaction
//! 2. Send a signal to self using kill
//! 3. Verify the handler was called
//! 4. Print boot stage marker for validation

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Static flag to track if handler was called
/// We use a static mutable because signal handlers can't capture closures
static mut HANDLER_CALLED: bool = false;

/// Signal handler for SIGUSR1
/// This must be a simple extern "C" function
extern "C" fn sigusr1_handler(_sig: i32) {
    unsafe {
        HANDLER_CALLED = true;
        io::print("  HANDLER: SIGUSR1 received and executed!\n");
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Signal Handler Test ===\n");

        let my_pid = process::getpid();
        io::print("My PID: ");
        print_number(my_pid);
        io::print("\n");

        // Test 1: Register signal handler using sigaction
        io::print("\nTest 1: Register SIGUSR1 handler\n");
        let action = signal::Sigaction::new(sigusr1_handler);

        match signal::sigaction(signal::SIGUSR1, Some(&action), None) {
            Ok(()) => io::print("  PASS: sigaction registered handler\n"),
            Err(e) => {
                io::print("  FAIL: sigaction returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGNAL_HANDLER_NOT_EXECUTED\n");
                process::exit(1);
            }
        }

        // Test 2: Send SIGUSR1 to self
        io::print("\nTest 2: Send SIGUSR1 to self using kill\n");
        match signal::kill(my_pid as i32, signal::SIGUSR1) {
            Ok(()) => io::print("  PASS: kill() succeeded\n"),
            Err(e) => {
                io::print("  FAIL: kill() returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("SIGNAL_HANDLER_NOT_EXECUTED\n");
                process::exit(1);
            }
        }

        // Test 3: Yield to allow signal delivery
        io::print("\nTest 3: Yielding to allow signal delivery...\n");
        for i in 0..10 {
            process::yield_now();

            // Check if handler was called
            if HANDLER_CALLED {
                io::print("  Handler called after ");
                print_number(i + 1);
                io::print(" yields\n");
                break;
            }
        }

        // Test 4: Verify handler was called
        io::print("\nTest 4: Verify handler execution\n");
        if HANDLER_CALLED {
            io::print("  PASS: Handler was called!\n");
            io::print("\n");
            io::print("SIGNAL_HANDLER_EXECUTED\n");
            process::exit(0);
        } else {
            io::print("  FAIL: Handler was NOT called after 10 yields\n");
            io::print("\n");
            io::print("SIGNAL_HANDLER_NOT_EXECUTED\n");
            process::exit(1);
        }
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

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in signal handler test!\n");
    io::print("SIGNAL_HANDLER_NOT_EXECUTED\n");
    process::exit(255);
}
