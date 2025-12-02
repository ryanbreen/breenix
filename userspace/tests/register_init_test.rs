//! Register initialization test program
//!
//! Tests that all general-purpose registers are initialized to zero
//! when entering userspace for the first time.
//!
//! CRITICAL: This test must capture register values BEFORE any Rust code runs,
//! because Rust function calls will corrupt registers via calling conventions.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;

// Simple syscall wrappers
unsafe fn syscall1(n: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") n,
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack, preserves_flags)
    );
    ret
}

unsafe fn syscall3(n: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") n,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") ret,
        options(nostack, preserves_flags)
    );
    ret
}

// Helper to write a string
fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64);
    }
}

// Helper to write a hex number
fn write_hex(n: u64) {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';

    for i in 0..16 {
        let nibble = ((n >> ((15 - i) * 4)) & 0xf) as usize;
        buf[i + 2] = HEX_CHARS[nibble];
    }

    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    write_str(s);
}

/// Entry point - captures registers in pure assembly BEFORE any Rust code runs
#[no_mangle]
#[unsafe(naked)]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        // Save all registers to stack IMMEDIATELY at entry
        // Stack layout after this: [rax][rbx][rcx][rdx][rsi][rdi][r8][r9][r10][r11][r12][r13][r14][r15]
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // Now call Rust function to check them (stack pointer points to saved regs)
        "call {check_registers}",

        // Never returns
        "ud2",

        check_registers = sym check_registers_from_stack,
    );
}

/// Check register values that were saved on the stack
unsafe extern "C" fn check_registers_from_stack() -> ! {
    // Read values from stack
    // Stack layout: [r15][r14][r13][r12][r11][r10][r9][r8][rdi][rsi][rdx][rcx][rbx][rax][return_addr]
    //                                                                                        ^RSP
    // So we need to skip return address (+8) and read backwards
    let regs_ptr: *const u64;
    core::arch::asm!("lea {}, [rsp + 8]", out(reg) regs_ptr);

    // Read in reverse order (last pushed = first in memory going up)
    let r15 = *regs_ptr.offset(0);
    let r14 = *regs_ptr.offset(1);
    let r13 = *regs_ptr.offset(2);
    let r12 = *regs_ptr.offset(3);
    let r11 = *regs_ptr.offset(4);
    let r10 = *regs_ptr.offset(5);
    let r9  = *regs_ptr.offset(6);
    let r8  = *regs_ptr.offset(7);
    let rdi = *regs_ptr.offset(8);
    let rsi = *regs_ptr.offset(9);
    let rdx = *regs_ptr.offset(10);
    let rcx = *regs_ptr.offset(11);
    let rbx = *regs_ptr.offset(12);
    let rax = *regs_ptr.offset(13);

    // Check if all registers are zero
    let all_zero = rax == 0
        && rbx == 0
        && rcx == 0
        && rdx == 0
        && rsi == 0
        && rdi == 0
        && r8 == 0
        && r9 == 0
        && r10 == 0
        && r11 == 0
        && r12 == 0
        && r13 == 0
        && r14 == 0
        && r15 == 0;

    if all_zero {
        write_str("✓ PASS: All registers initialized to zero\n");
    } else {
        write_str("✗ FAIL: Some registers not initialized to zero:\n");
        if rax != 0 {
            write_str("  RAX = ");
            write_hex(rax);
            write_str(" (expected 0)\n");
        }
        if rbx != 0 {
            write_str("  RBX = ");
            write_hex(rbx);
            write_str(" (expected 0)\n");
        }
        if rcx != 0 {
            write_str("  RCX = ");
            write_hex(rcx);
            write_str(" (expected 0)\n");
        }
        if rdx != 0 {
            write_str("  RDX = ");
            write_hex(rdx);
            write_str(" (expected 0)\n");
        }
        if rsi != 0 {
            write_str("  RSI = ");
            write_hex(rsi);
            write_str(" (expected 0)\n");
        }
        if rdi != 0 {
            write_str("  RDI = ");
            write_hex(rdi);
            write_str(" (expected 0)\n");
        }
        if r8 != 0 {
            write_str("  R8  = ");
            write_hex(r8);
            write_str(" (expected 0)\n");
        }
        if r9 != 0 {
            write_str("  R9  = ");
            write_hex(r9);
            write_str(" (expected 0)\n");
        }
        if r10 != 0 {
            write_str("  R10 = ");
            write_hex(r10);
            write_str(" (expected 0)\n");
        }
        if r11 != 0 {
            write_str("  R11 = ");
            write_hex(r11);
            write_str(" (expected 0)\n");
        }
        if r12 != 0 {
            write_str("  R12 = ");
            write_hex(r12);
            write_str(" (expected 0)\n");
        }
        if r13 != 0 {
            write_str("  R13 = ");
            write_hex(r13);
            write_str(" (expected 0)\n");
        }
        if r14 != 0 {
            write_str("  R14 = ");
            write_hex(r14);
            write_str(" (expected 0)\n");
        }
        if r15 != 0 {
            write_str("  R15 = ");
            write_hex(r15);
            write_str(" (expected 0)\n");
        }
    }

    syscall1(SYS_EXIT, if all_zero { 0 } else { 1 });

    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in register_init_test!\n");
    unsafe { syscall1(SYS_EXIT, 1); }
    loop {}
}
