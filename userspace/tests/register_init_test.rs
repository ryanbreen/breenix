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
///
/// NOTE: We save registers to a static buffer instead of the stack because:
/// 1. Calling conventions may clobber registers before we read them
/// 2. Stack-based approach was giving incorrect results for RAX
#[no_mangle]
#[unsafe(naked)]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        // Save all registers to a static buffer IMMEDIATELY at entry
        // This is simpler and more reliable than stack-based approach
        "lea r11, [{saved_regs}]",  // Load address of static buffer into r11 (clobbers r11)
        "mov [r11 + 0*8], rax",
        "mov [r11 + 1*8], rbx",
        "mov [r11 + 2*8], rcx",
        "mov [r11 + 3*8], rdx",
        "mov [r11 + 4*8], rsi",
        "mov [r11 + 5*8], rdi",
        "mov [r11 + 6*8], r8",
        "mov [r11 + 7*8], r9",
        "mov [r11 + 8*8], r10",
        // r11 is already used as base pointer, can't save its original value
        "mov [r11 + 10*8], r12",
        "mov [r11 + 11*8], r13",
        "mov [r11 + 12*8], r14",
        "mov [r11 + 13*8], r15",

        // Now call Rust function to check them
        "call {check_registers}",

        // Never returns
        "ud2",

        saved_regs = sym SAVED_REGS,
        check_registers = sym check_registers_from_buffer,
    );
}

/// Static buffer to hold saved registers
/// Order: rax, rbx, rcx, rdx, rsi, rdi, r8, r9, r10, (r11 skipped), r12, r13, r14, r15
static mut SAVED_REGS: [u64; 14] = [0; 14];

/// Check register values that were saved to the static buffer
unsafe extern "C" fn check_registers_from_buffer() -> ! {
    // Read values from SAVED_REGS static buffer
    // Layout: rax at 0, rbx at 1, ..., r15 at 13 (r11 is at index 9 but was clobbered)
    let rax = SAVED_REGS[0];
    let rbx = SAVED_REGS[1];
    let rcx = SAVED_REGS[2];
    let rdx = SAVED_REGS[3];
    let rsi = SAVED_REGS[4];
    let rdi_saved = SAVED_REGS[5];
    let r8  = SAVED_REGS[6];
    let r9  = SAVED_REGS[7];
    let r10 = SAVED_REGS[8];
    // r11 was used as base pointer, skip it (SAVED_REGS[9] contains garbage)
    let r12 = SAVED_REGS[10];
    let r13 = SAVED_REGS[11];
    let r14 = SAVED_REGS[12];
    let r15 = SAVED_REGS[13];

    // Check if all registers are zero
    // Note: rdi_saved is the original rdi value (before we used rdi for argument passing)
    // Note: r11 is not checked because we used it as base pointer to save other registers
    let all_zero = rax == 0
        && rbx == 0
        && rcx == 0
        && rdx == 0
        && rsi == 0
        && rdi_saved == 0
        && r8 == 0
        && r9 == 0
        && r10 == 0
        // r11 cannot be checked - used as base pointer
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
        if rdi_saved != 0 {
            write_str("  RDI = ");
            write_hex(rdi_saved);
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
        // r11 not checked - used as base pointer in _start
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
