//! Signal register preservation test (std version)
//!
//! Tests that all general-purpose registers are correctly preserved across
//! signal delivery and sigreturn. This is CRITICAL - if any register is
//! corrupted, userspace will malfunction.
//!
//! NOTE: This test uses x86_64 inline assembly and is x86_64-only.

use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::signal::SIGUSR1;
use libbreenix::{kill, sigaction, Sigaction};
use libbreenix::process::{getpid, yield_now};

static HANDLER_RAN: AtomicBool = AtomicBool::new(false);

/// Signal handler - intentionally clobbers callee-saved registers
extern "C" fn handler(_sig: i32) {
    HANDLER_RAN.store(true, Ordering::SeqCst);
    println!("  HANDLER: Received signal, clobbering registers...");

    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!(
            "mov r12, 0xDEADBEEFDEADBEEF",
            "mov r13, 0xCAFEBABECAFEBABE",
            "mov r14, 0x1111111111111111",
            "mov r15, 0x2222222222222222",
            out("r12") _,
            out("r13") _,
            out("r14") _,
            out("r15") _,
        );
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Use x9-x12 (caller-saved) to clobber in signal handler.
        // x19 is reserved by LLVM and cannot be used in inline asm.
        std::arch::asm!(
            "mov x9, 0xDEAD",
            "mov x10, 0xCAFE",
            "mov x11, 0x1111",
            "mov x12, 0x2222",
            out("x9") _,
            out("x10") _,
            out("x11") _,
            out("x12") _,
        );
    }

    println!("  HANDLER: Registers clobbered, returning...");
}

fn main() {
    println!("=== Signal Register Preservation Test ===");

    #[cfg(target_arch = "x86_64")]
    {
        let r12_expected: u64 = 0xAAAA_BBBB_CCCC_DDDD;
        let r13_expected: u64 = 0x1111_2222_3333_4444;
        let r14_expected: u64 = 0x5555_6666_7777_8888;
        let r15_expected: u64 = 0x9999_AAAA_BBBB_CCCC;

        println!("Step 1: Setting callee-saved registers (r12-r15) to known values");

        unsafe {
            std::arch::asm!(
                "mov r12, {0}",
                "mov r13, {1}",
                "mov r14, {2}",
                "mov r15, {3}",
                in(reg) r12_expected,
                in(reg) r13_expected,
                in(reg) r14_expected,
                in(reg) r15_expected,
            );
        }

        println!("  R12 = {:#018x}", r12_expected);
        println!("  R13 = {:#018x}", r13_expected);
        println!("  R14 = {:#018x}", r14_expected);
        println!("  R15 = {:#018x}", r15_expected);

        // Register signal handler
        println!("\nStep 2: Registering signal handler for SIGUSR1");
        let action = Sigaction::new(handler);

        if sigaction(SIGUSR1, Some(&action), None).is_err() {
            println!("  FAIL: sigaction failed");
            std::process::exit(1);
        }
        println!("  Handler registered successfully");

        // Send signal to self
        println!("\nStep 3: Sending SIGUSR1 to self");
        let my_pid = getpid().unwrap().raw() as i32;
        if kill(my_pid, SIGUSR1).is_err() {
            println!("  FAIL: kill failed");
            std::process::exit(1);
        }
        println!("  Signal sent successfully");

        // Yield to allow signal delivery
        println!("\nStep 4: Yielding to allow signal delivery");
        for i in 0..100 {
            let _ = yield_now();
            if HANDLER_RAN.load(Ordering::SeqCst) && i > 10 {
                break;
            }
        }

        if !HANDLER_RAN.load(Ordering::SeqCst) {
            println!("  FAIL: Handler never ran");
            println!("SIGNAL_REGS_CORRUPTED");
            std::process::exit(1);
        }

        println!("  Handler executed and returned");

        // Read back register values after signal handling
        println!("\nStep 5: Checking register values after signal return");

        let r12_actual: u64;
        let r13_actual: u64;
        let r14_actual: u64;
        let r15_actual: u64;

        unsafe {
            std::arch::asm!(
                "mov {0}, r12",
                "mov {1}, r13",
                "mov {2}, r14",
                "mov {3}, r15",
                out(reg) r12_actual,
                out(reg) r13_actual,
                out(reg) r14_actual,
                out(reg) r15_actual,
            );
        }

        println!("  R12 = {:#018x}", r12_actual);
        println!("  R13 = {:#018x}", r13_actual);
        println!("  R14 = {:#018x}", r14_actual);
        println!("  R15 = {:#018x}", r15_actual);

        let mut all_match = true;
        let mut errors = 0u64;

        if r12_actual != r12_expected {
            println!("  FAIL: R12 mismatch - expected {:#018x} but got {:#018x}", r12_expected, r12_actual);
            all_match = false;
            errors += 1;
        }
        if r13_actual != r13_expected {
            println!("  FAIL: R13 mismatch - expected {:#018x} but got {:#018x}", r13_expected, r13_actual);
            all_match = false;
            errors += 1;
        }
        if r14_actual != r14_expected {
            println!("  FAIL: R14 mismatch - expected {:#018x} but got {:#018x}", r14_expected, r14_actual);
            all_match = false;
            errors += 1;
        }
        if r15_actual != r15_expected {
            println!("  FAIL: R15 mismatch - expected {:#018x} but got {:#018x}", r15_expected, r15_actual);
            all_match = false;
            errors += 1;
        }

        println!("\n=== TEST RESULT ===");
        if all_match {
            println!("PASS: All callee-saved registers preserved across signal delivery");
            println!("SIGNAL_REGS_PRESERVED");
            std::process::exit(0);
        } else {
            println!("FAIL: {} registers were corrupted", errors);
            println!("SIGNAL_REGS_CORRUPTED");
            std::process::exit(1);
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // ARM64 version uses x20-x23 as callee-saved registers.
        // x19 is reserved by LLVM and cannot be used in inline asm.
        let x20_expected: u64 = 0xAAAA_BBBB_CCCC_DDDD;
        let x21_expected: u64 = 0x1111_2222_3333_4444;
        let x22_expected: u64 = 0x5555_6666_7777_8888;
        let x23_expected: u64 = 0x9999_AAAA_BBBB_CCCC;

        println!("Step 1: Will set callee-saved registers (x20-x23) to known values");
        println!("  X20 = {:#018x}", x20_expected);
        println!("  X21 = {:#018x}", x21_expected);
        println!("  X22 = {:#018x}", x22_expected);
        println!("  X23 = {:#018x}", x23_expected);

        println!("\nStep 2: Registering signal handler for SIGUSR1");
        let action = Sigaction::new(handler);

        if sigaction(SIGUSR1, Some(&action), None).is_err() {
            println!("  FAIL: sigaction failed");
            std::process::exit(1);
        }
        println!("  Handler registered successfully");

        let my_pid = getpid().unwrap().raw() as i32;

        // Use a single asm block for the critical path: set registers, send
        // signal via syscall, yield to allow delivery, then read registers back.
        // This prevents the compiler from using x20-x23 between set and read.
        // Breenix syscall numbers: kill=62, sched_yield=3
        println!("\nStep 3: Setting registers, sending signal, yielding (all in asm)");

        let x20_actual: u64;
        let x21_actual: u64;
        let x22_actual: u64;
        let x23_actual: u64;

        unsafe {
            std::arch::asm!(
                // Set callee-saved registers to known values
                "mov x20, {e20}",
                "mov x21, {e21}",
                "mov x22, {e22}",
                "mov x23, {e23}",
                // kill(my_pid, SIGUSR1) - syscall 62
                "mov x8, 62",
                "mov x0, {pid}",
                "mov x1, 10",
                "svc #0",
                // Yield loop to allow signal delivery (100 iterations)
                // sched_yield = syscall 3
                "mov x9, 100",
                "2:",
                "mov x8, 3",
                "svc #0",
                "sub x9, x9, 1",
                "cbnz x9, 2b",
                // Read back callee-saved registers
                "mov {a20}, x20",
                "mov {a21}, x21",
                "mov {a22}, x22",
                "mov {a23}, x23",
                e20 = in(reg) x20_expected,
                e21 = in(reg) x21_expected,
                e22 = in(reg) x22_expected,
                e23 = in(reg) x23_expected,
                pid = in(reg) my_pid as u64,
                a20 = out(reg) x20_actual,
                a21 = out(reg) x21_actual,
                a22 = out(reg) x22_actual,
                a23 = out(reg) x23_actual,
                out("x0") _,
                out("x1") _,
                out("x8") _,
                out("x9") _,
                out("x20") _,
                out("x21") _,
                out("x22") _,
                out("x23") _,
            );
        }

        println!("  Signal sent and yields completed");

        // Verify handler ran (check after asm block is safe)
        println!("\nStep 4: Checking if handler ran");
        if !HANDLER_RAN.load(Ordering::SeqCst) {
            println!("  FAIL: Handler never ran");
            println!("SIGNAL_REGS_CORRUPTED");
            std::process::exit(1);
        }
        println!("  Handler executed and returned");

        println!("\nStep 5: Checking register values after signal return");
        println!("  X20 = {:#018x}", x20_actual);
        println!("  X21 = {:#018x}", x21_actual);
        println!("  X22 = {:#018x}", x22_actual);
        println!("  X23 = {:#018x}", x23_actual);

        let mut all_match = true;
        let mut errors = 0u64;

        if x20_actual != x20_expected { println!("  FAIL: X20 mismatch: expected {:#018x} got {:#018x}", x20_expected, x20_actual); all_match = false; errors += 1; }
        if x21_actual != x21_expected { println!("  FAIL: X21 mismatch: expected {:#018x} got {:#018x}", x21_expected, x21_actual); all_match = false; errors += 1; }
        if x22_actual != x22_expected { println!("  FAIL: X22 mismatch: expected {:#018x} got {:#018x}", x22_expected, x22_actual); all_match = false; errors += 1; }
        if x23_actual != x23_expected { println!("  FAIL: X23 mismatch: expected {:#018x} got {:#018x}", x23_expected, x23_actual); all_match = false; errors += 1; }

        println!("\n=== TEST RESULT ===");
        if all_match {
            println!("PASS: All callee-saved registers preserved across signal delivery");
            println!("SIGNAL_REGS_PRESERVED");
            std::process::exit(0);
        } else {
            println!("FAIL: {} registers were corrupted", errors);
            println!("SIGNAL_REGS_CORRUPTED");
            std::process::exit(1);
        }
    }
}
