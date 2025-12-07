//! Signal handler return test
//!
//! This test proves the signal trampoline works end-to-end:
//! 1. Register a handler for SIGUSR1
//! 2. Send SIGUSR1 to self
//! 3. Handler executes and returns
//! 4. Trampoline calls sigreturn to restore pre-signal context
//! 5. Execution resumes where it was interrupted
//!
//! If all variables are set correctly, the trampoline worked.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Flags to track execution flow
static mut BEFORE_SIGNAL: bool = false;
static mut HANDLER_RAN: bool = false;
static mut AFTER_SIGNAL: bool = false;

/// Simple signal handler that just sets a flag and returns
/// The return will jump to the trampoline, which calls sigreturn
extern "C" fn sigusr1_handler(_sig: i32) {
    unsafe {
        HANDLER_RAN = true;
    }
    // Handler returns normally - this tests the trampoline mechanism
    // The return address points to trampoline code on the stack
    // Trampoline executes: mov rax, 15; int 0x80; ud2
    // This calls SYS_SIGRETURN which restores pre-signal context
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Signal Return Test ===\n");
        io::print("Testing signal handler return via trampoline\n\n");

        // Get our PID for sending signal to self
        let my_pid = process::getpid();

        // Register handler for SIGUSR1
        io::print("Step 1: Registering SIGUSR1 handler\n");
        let action = signal::Sigaction::new(sigusr1_handler);
        match signal::sigaction(signal::SIGUSR1, Some(&action), None) {
            Ok(()) => io::print("  Handler registered successfully\n"),
            Err(_) => {
                io::print("  FAIL: sigaction returned error\n");
                process::exit(1);
            }
        }

        // Set flag before sending signal
        io::print("\nStep 2: Setting BEFORE_SIGNAL flag\n");
        BEFORE_SIGNAL = true;
        io::print("  BEFORE_SIGNAL = true\n");

        // Send signal to self
        io::print("\nStep 3: Sending SIGUSR1 to self\n");
        match signal::kill(my_pid as i32, signal::SIGUSR1) {
            Ok(()) => io::print("  Signal sent successfully\n"),
            Err(_) => {
                io::print("  FAIL: kill returned error\n");
                process::exit(1);
            }
        }

        // Yield to allow signal delivery
        // The scheduler will deliver the signal when this process is next scheduled
        io::print("\nStep 4: Yielding to allow signal delivery\n");
        for i in 0..100 {
            process::yield_now();

            // Check if handler ran (signal was delivered)
            if HANDLER_RAN {
                io::print("  Signal delivered and handler executed\n");
                break;
            }

            // Periodic status update
            if i % 20 == 19 {
                io::print("  Still waiting for signal delivery...\n");
            }
        }

        // If we reach here, the handler MUST have returned successfully
        // because the trampoline restored our execution context
        io::print("\nStep 5: Execution resumed after handler\n");
        AFTER_SIGNAL = true;
        io::print("  AFTER_SIGNAL = true\n");

        // Verify all flags are set correctly
        io::print("\n=== Verification ===\n");
        io::print("BEFORE_SIGNAL: ");
        if BEFORE_SIGNAL {
            io::print("✓ true\n");
        } else {
            io::print("✗ false (ERROR)\n");
        }

        io::print("HANDLER_RAN:   ");
        if HANDLER_RAN {
            io::print("✓ true\n");
        } else {
            io::print("✗ false (handler never executed)\n");
        }

        io::print("AFTER_SIGNAL:  ");
        if AFTER_SIGNAL {
            io::print("✓ true\n");
        } else {
            io::print("✗ false (execution didn't resume after handler)\n");
        }

        // Final verdict
        io::print("\n=== Result ===\n");
        if BEFORE_SIGNAL && HANDLER_RAN && AFTER_SIGNAL {
            io::print("SIGNAL_RETURN_WORKS\n");
            io::print("\nThe signal trampoline successfully:\n");
            io::print("  1. Delivered the signal\n");
            io::print("  2. Executed the handler\n");
            io::print("  3. Called sigreturn via trampoline\n");
            io::print("  4. Restored pre-signal execution context\n");
            io::print("  5. Resumed execution after the signal\n");
            process::exit(0);
        } else {
            io::print("SIGNAL_RETURN_FAILED\n");
            io::print("\nThe trampoline did not work correctly:\n");
            if !HANDLER_RAN {
                io::print("  - Handler never executed (signal not delivered)\n");
            }
            if !AFTER_SIGNAL {
                io::print("  - Execution didn't resume (sigreturn failed)\n");
            }
            process::exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in signal return test!\n");
    process::exit(255);
}
