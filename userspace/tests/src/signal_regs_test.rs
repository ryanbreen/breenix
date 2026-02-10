//! Signal register preservation test (std version)
//!
//! Tests that all general-purpose registers are correctly preserved across
//! signal delivery and sigreturn. This is CRITICAL - if any register is
//! corrupted, userspace will malfunction.
//!
//! NOTE: This test uses x86_64 inline assembly and is x86_64-only.

use std::sync::atomic::{AtomicBool, Ordering};

static HANDLER_RAN: AtomicBool = AtomicBool::new(false);

const SIGUSR1: i32 = 10;
const SA_RESTORER: u64 = 0x04000000;

#[repr(C)]
struct KernelSigaction {
    handler: u64,
    mask: u64,
    flags: u64,
    restorer: u64,
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn getpid() -> i32;
    fn sched_yield() -> i32;
}

#[cfg(target_arch = "x86_64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") 13u64,
        in("rdi") sig as u64,
        in("rsi") act as u64,
        in("rdx") oldact as u64,
        in("r10") 8u64,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64
}

#[cfg(target_arch = "aarch64")]
unsafe fn raw_sigaction(sig: i32, act: *const KernelSigaction, oldact: *mut KernelSigaction) -> i64 {
    let ret: u64;
    std::arch::asm!(
        "svc #0",
        in("x8") 13u64,
        inlateout("x0") sig as u64 => ret,
        in("x1") act as u64,
        in("x2") oldact as u64,
        in("x3") 8u64,
        options(nostack),
    );
    ret as i64
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov rax, 15",
        "int 0x80",
        "ud2",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
extern "C" fn __restore_rt() -> ! {
    std::arch::naked_asm!(
        "mov x8, 15",
        "svc #0",
        "brk #1",
    )
}

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
        let action = KernelSigaction {
            handler: handler as u64,
            mask: 0,
            flags: SA_RESTORER,
            restorer: __restore_rt as u64,
        };

        let ret = unsafe { raw_sigaction(SIGUSR1, &action, std::ptr::null_mut()) };
        if ret < 0 {
            println!("  FAIL: sigaction failed with error {}", -ret);
            std::process::exit(1);
        }
        println!("  Handler registered successfully");

        // Send signal to self
        println!("\nStep 3: Sending SIGUSR1 to self");
        let my_pid = unsafe { getpid() };
        let ret = unsafe { kill(my_pid, SIGUSR1) };
        if ret != 0 {
            println!("  FAIL: kill failed");
            std::process::exit(1);
        }
        println!("  Signal sent successfully");

        // Yield to allow signal delivery
        println!("\nStep 4: Yielding to allow signal delivery");
        for i in 0..100 {
            unsafe { sched_yield(); }
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

        println!("Step 1: Setting callee-saved registers (x20-x23) to known values");

        unsafe {
            std::arch::asm!(
                "mov x20, {0}",
                "mov x21, {1}",
                "mov x22, {2}",
                "mov x23, {3}",
                in(reg) x20_expected,
                in(reg) x21_expected,
                in(reg) x22_expected,
                in(reg) x23_expected,
            );
        }

        println!("  X20 = {:#018x}", x20_expected);
        println!("  X21 = {:#018x}", x21_expected);
        println!("  X22 = {:#018x}", x22_expected);
        println!("  X23 = {:#018x}", x23_expected);

        println!("\nStep 2: Registering signal handler for SIGUSR1");
        let action = KernelSigaction {
            handler: handler as u64,
            mask: 0,
            flags: SA_RESTORER,
            restorer: __restore_rt as u64,
        };

        let ret = unsafe { raw_sigaction(SIGUSR1, &action, std::ptr::null_mut()) };
        if ret < 0 {
            println!("  FAIL: sigaction failed with error {}", -ret);
            std::process::exit(1);
        }
        println!("  Handler registered successfully");

        println!("\nStep 3: Sending SIGUSR1 to self");
        let my_pid = unsafe { getpid() };
        let ret = unsafe { kill(my_pid, SIGUSR1) };
        if ret != 0 {
            println!("  FAIL: kill failed");
            std::process::exit(1);
        }
        println!("  Signal sent successfully");

        println!("\nStep 4: Yielding to allow signal delivery");
        for i in 0..100 {
            unsafe { sched_yield(); }
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

        println!("\nStep 5: Checking register values after signal return");

        let x20_actual: u64;
        let x21_actual: u64;
        let x22_actual: u64;
        let x23_actual: u64;

        unsafe {
            std::arch::asm!(
                "mov {0}, x20",
                "mov {1}, x21",
                "mov {2}, x22",
                "mov {3}, x23",
                out(reg) x20_actual,
                out(reg) x21_actual,
                out(reg) x22_actual,
                out(reg) x23_actual,
            );
        }

        println!("  X20 = {:#018x}", x20_actual);
        println!("  X21 = {:#018x}", x21_actual);
        println!("  X22 = {:#018x}", x22_actual);
        println!("  X23 = {:#018x}", x23_actual);

        let mut all_match = true;
        let mut errors = 0u64;

        if x20_actual != x20_expected { println!("  FAIL: X20 mismatch"); all_match = false; errors += 1; }
        if x21_actual != x21_expected { println!("  FAIL: X21 mismatch"); all_match = false; errors += 1; }
        if x22_actual != x22_expected { println!("  FAIL: X22 mismatch"); all_match = false; errors += 1; }
        if x23_actual != x23_expected { println!("  FAIL: X23 mismatch"); all_match = false; errors += 1; }

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
