//! Signal exec check program
//!
//! This program is exec'd by signal_exec_test to verify that signal
//! handlers are reset to SIG_DFL after exec().
//!
//! POSIX requires that signals with custom handlers be reset to SIG_DFL
//! after exec, while ignored signals (SIG_IGN) may remain ignored.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

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

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Signal Exec Check (after exec) ===\n");
        io::print("This program was exec'd - checking if signal handlers are reset to SIG_DFL\n\n");

        // Query the current handler for SIGUSR1
        // If exec reset handlers properly, this should return SIG_DFL (0)
        io::print("Querying SIGUSR1 handler state...\n");

        let mut old_action = signal::Sigaction::default();

        // sigaction with act=None queries current handler without changing it
        match signal::sigaction(signal::SIGUSR1, None, Some(&mut old_action)) {
            Ok(()) => {
                io::print("  sigaction query succeeded\n");
                io::print("  Handler value: ");
                print_number(old_action.handler);
                io::print("\n");

                if old_action.handler == signal::SIG_DFL {
                    io::print("  PASS: Handler is SIG_DFL (correctly reset after exec)\n");
                    io::print("\nSIGNAL_EXEC_RESET_VERIFIED\n");
                    process::exit(0);
                } else if old_action.handler == signal::SIG_IGN {
                    io::print("  INFO: Handler is SIG_IGN (may be acceptable per POSIX)\n");
                    // This is technically acceptable for POSIX but we want SIG_DFL
                    io::print("\nSIGNAL_EXEC_RESET_PARTIAL\n");
                    process::exit(1);
                } else {
                    io::print("  FAIL: Handler is NOT SIG_DFL - it was inherited from pre-exec!\n");
                    io::print("\nSIGNAL_EXEC_RESET_FAILED\n");
                    process::exit(2);
                }
            }
            Err(e) => {
                io::print("  FAIL: sigaction query returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("\nSIGNAL_EXEC_RESET_FAILED\n");
                process::exit(3);
            }
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in signal exec check!\n");
    io::print("SIGNAL_EXEC_RESET_FAILED\n");
    process::exit(255);
}
