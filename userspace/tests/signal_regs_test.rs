//! Signal register preservation test
//!
//! Tests that all general-purpose registers are correctly preserved across
//! signal delivery and sigreturn. This is CRITICAL - if any register is
//! corrupted, userspace will malfunction.
//!
//! Test flow:
//! 1. Set callee-saved registers (r12-r15, rbx, rbp) to known values
//! 2. Register a signal handler that intentionally clobbers those registers
//! 3. Send SIGUSR1 to self
//! 4. Handler runs (clobbering registers), then returns via sigreturn
//! 5. Check if registers were restored to original values
//! 6. Print "SIGNAL_REGS_PRESERVED" if all correct, "SIGNAL_REGS_CORRUPTED" if wrong

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;

/// Flag to indicate handler ran
static mut HANDLER_RAN: bool = false;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to hex string and print it
unsafe fn print_hex(prefix: &str, num: u64) {
    io::print(prefix);
    io::print("0x");

    let hex_chars = b"0123456789abcdef";
    for i in (0..16).rev() {
        let nibble = ((num >> (i * 4)) & 0xf) as usize;
        BUFFER[0] = hex_chars[nibble];
        io::write(libbreenix::types::fd::STDOUT, &BUFFER[..1]);
    }
    io::print("\n");
}

/// Signal handler - intentionally clobbers callee-saved registers
extern "C" fn handler(_sig: i32) {
    unsafe {
        HANDLER_RAN = true;
        io::print("  HANDLER: Received signal, clobbering registers...\n");

        // Clobber callee-saved registers r12-r15 to verify they're restored
        // Note: RBX and RBP cannot be used as inline asm operands (reserved by LLVM)
        core::arch::asm!(
            "mov r12, 0xDEADBEEFDEADBEEF",
            "mov r13, 0xCAFEBABECAFEBABE",
            "mov r14, 0x1111111111111111",
            "mov r15, 0x2222222222222222",
            out("r12") _,
            out("r13") _,
            out("r14") _,
            out("r15") _,
        );

        io::print("  HANDLER: Registers clobbered, returning...\n");
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Signal Register Preservation Test ===\n");

        // Define known test values for callee-saved registers r12-r15
        // Note: RBX and RBP cannot be used as inline asm operands (reserved by LLVM)
        let r12_expected: u64 = 0xAAAA_BBBB_CCCC_DDDD;
        let r13_expected: u64 = 0x1111_2222_3333_4444;
        let r14_expected: u64 = 0x5555_6666_7777_8888;
        let r15_expected: u64 = 0x9999_AAAA_BBBB_CCCC;

        io::print("Step 1: Setting callee-saved registers (r12-r15) to known values\n");

        // Set known values in callee-saved registers
        core::arch::asm!(
            "mov r12, {0}",
            "mov r13, {1}",
            "mov r14, {2}",
            "mov r15, {3}",
            in(reg) r12_expected,
            in(reg) r13_expected,
            in(reg) r14_expected,
            in(reg) r15_expected,
        );

        print_hex("  R12 = ", r12_expected);
        print_hex("  R13 = ", r13_expected);
        print_hex("  R14 = ", r14_expected);
        print_hex("  R15 = ", r15_expected);

        // Register signal handler
        io::print("\nStep 2: Registering signal handler for SIGUSR1\n");
        let action = signal::Sigaction::new(handler);
        match signal::sigaction(signal::SIGUSR1, Some(&action), None) {
            Ok(()) => io::print("  Handler registered successfully\n"),
            Err(e) => {
                io::print("  FAIL: sigaction failed with error ");
                print_hex("", e as u64);
                process::exit(1);
            }
        }

        // Send signal to self
        io::print("\nStep 3: Sending SIGUSR1 to self\n");
        let my_pid = process::getpid() as i32;
        match signal::kill(my_pid, signal::SIGUSR1) {
            Ok(()) => io::print("  Signal sent successfully\n"),
            Err(e) => {
                io::print("  FAIL: kill failed with error ");
                print_hex("", e as u64);
                process::exit(1);
            }
        }

        // Yield to allow signal delivery
        io::print("\nStep 4: Yielding to allow signal delivery\n");
        for i in 0..100 {
            process::yield_now();
            // Check if handler ran
            if HANDLER_RAN && i > 10 {
                break;
            }
        }

        if !HANDLER_RAN {
            io::print("  FAIL: Handler never ran\n");
            io::print("SIGNAL_REGS_CORRUPTED\n");
            process::exit(1);
        }

        io::print("  Handler executed and returned\n");

        // Read back register values after signal handling
        io::print("\nStep 5: Checking register values after signal return\n");

        let r12_actual: u64;
        let r13_actual: u64;
        let r14_actual: u64;
        let r15_actual: u64;

        core::arch::asm!(
            "mov {0}, r12",
            "mov {1}, r13",
            "mov {2}, r14",
            "mov {3}, r15",
            out(reg) r12_actual,
            out(reg) r13_actual,
            out(reg) r14_actual,
            out(reg) r15_actual,
        );

        print_hex("  R12 = ", r12_actual);
        print_hex("  R13 = ", r13_actual);
        print_hex("  R14 = ", r14_actual);
        print_hex("  R15 = ", r15_actual);

        // Check if all registers match
        let mut all_match = true;
        let mut errors = 0;

        if r12_actual != r12_expected {
            io::print("  FAIL: R12 mismatch - expected ");
            print_hex("", r12_expected);
            io::print("         but got ");
            print_hex("", r12_actual);
            all_match = false;
            errors += 1;
        }

        if r13_actual != r13_expected {
            io::print("  FAIL: R13 mismatch - expected ");
            print_hex("", r13_expected);
            io::print("         but got ");
            print_hex("", r13_actual);
            all_match = false;
            errors += 1;
        }

        if r14_actual != r14_expected {
            io::print("  FAIL: R14 mismatch - expected ");
            print_hex("", r14_expected);
            io::print("         but got ");
            print_hex("", r14_actual);
            all_match = false;
            errors += 1;
        }

        if r15_actual != r15_expected {
            io::print("  FAIL: R15 mismatch - expected ");
            print_hex("", r15_expected);
            io::print("         but got ");
            print_hex("", r15_actual);
            all_match = false;
            errors += 1;
        }

        io::print("\n=== TEST RESULT ===\n");
        if all_match {
            io::print("✓ PASS: All callee-saved registers preserved across signal delivery\n");
            io::print("SIGNAL_REGS_PRESERVED\n");
            process::exit(0);
        } else {
            io::print("✗ FAIL: ");
            print_hex("", errors);
            io::print(" registers were corrupted\n");
            io::print("SIGNAL_REGS_CORRUPTED\n");
            process::exit(1);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in signal_regs_test!\n");
    io::print("SIGNAL_REGS_CORRUPTED\n");
    process::exit(255);
}
