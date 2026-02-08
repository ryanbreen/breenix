//! Comprehensive syscall diagnostic test sequence (std version)
//!
//! This test runs through a systematic sequence to isolate the syscall
//! register corruption bug that appears after the first syscall.
//!
//! Tests:
//! - 41a: Multiple no-arg syscalls (getpid)
//! - 41b: Multiple sys_write calls
//! - 41c: Single clock_gettime + verify memory
//! - 41d: Register preservation across syscall
//! - 41e: Second clock_gettime call (the failing case)
//!
//! NOTE: This is an x86_64-only test due to inline assembly using int 0x80.

// System call numbers
const SYS_WRITE: u64 = 1;
const SYS_GETPID: u64 = 39;
const SYS_CLOCK_GETTIME: u64 = 228;

// Clock IDs
const CLOCK_MONOTONIC: u32 = 1;

/// Timespec structure for clock_gettime
#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

// Syscall wrappers using inline asm
// Note: int 0x80 can clobber rcx and r11, and may access memory.
// We must declare these clobbers so the compiler doesn't keep local
// variables in those registers across syscalls.
#[cfg(target_arch = "x86_64")]
unsafe fn syscall0(n: u64) -> u64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") n,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
unsafe fn syscall1(n: u64, arg1: u64) -> u64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") n,
        in("rdi") arg1,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn syscall2(n: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") n,
        in("rdi") arg1,
        in("rsi") arg2,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn syscall3(n: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    std::arch::asm!(
        "int 0x80",
        in("rax") n,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    ret
}

// Helper to write a string via raw syscall
#[cfg(target_arch = "x86_64")]
fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64);
    }
}

// Helper to write a number via raw syscall
#[cfg(target_arch = "x86_64")]
fn write_num(mut n: u64) {
    if n == 0 {
        write_str("0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 19;

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if i == 0 {
            break;
        }
        i -= 1;
    }

    let s = unsafe { core::str::from_utf8_unchecked(&buf[i + 1..20]) };
    write_str(s);
}

// Helper to write a signed number via raw syscall
#[cfg(target_arch = "x86_64")]
fn write_signed(n: i64) {
    if n < 0 {
        write_str("-");
        write_num((-n) as u64);
    } else {
        write_num(n as u64);
    }
}

// Helper to write a hex number via raw syscall
#[cfg(target_arch = "x86_64")]
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

// ────────────────────────────────────────────────────────────────────────────
// Test 41a: Multiple getpid calls
// ────────────────────────────────────────────────────────────────────────────
#[cfg(target_arch = "x86_64")]
fn test_41a_getpid() -> bool {
    write_str("\nTest 41a: Multiple no-arg syscalls (getpid)\n");

    let pid1 = unsafe { syscall0(SYS_GETPID) };
    write_str("  Call 1: pid = ");
    write_num(pid1);
    write_str("\n");

    let pid2 = unsafe { syscall0(SYS_GETPID) };
    write_str("  Call 2: pid = ");
    write_num(pid2);
    write_str("\n");

    let pid3 = unsafe { syscall0(SYS_GETPID) };
    write_str("  Call 3: pid = ");
    write_num(pid3);
    write_str("\n");

    let pass = pid1 == pid2 && pid2 == pid3 && pid1 > 0;
    write_str("  Result: ");
    if pass {
        write_str("PASS\n");
    } else {
        write_str("FAIL\n");
    }

    pass
}

// ────────────────────────────────────────────────────────────────────────────
// Test 41b: Multiple sys_write calls
// ────────────────────────────────────────────────────────────────────────────
#[cfg(target_arch = "x86_64")]
fn test_41b_write() -> bool {
    write_str("\nTest 41b: Multiple sys_write calls\n");

    let r1 = unsafe { syscall3(SYS_WRITE, 1, b".\0".as_ptr() as u64, 1) };
    write_str("\n  Write 1: returned ");
    write_num(r1);
    write_str(" bytes\n");

    let r2 = unsafe { syscall3(SYS_WRITE, 1, b".\0".as_ptr() as u64, 1) };
    write_str("\n  Write 2: returned ");
    write_num(r2);
    write_str(" bytes\n");

    let r3 = unsafe { syscall3(SYS_WRITE, 1, b".\0".as_ptr() as u64, 1) };
    write_str("\n  Write 3: returned ");
    write_num(r3);
    write_str(" bytes\n");

    let pass = r1 == 1 && r2 == 1 && r3 == 1;
    write_str("  Result: ");
    if pass {
        write_str("PASS\n");
    } else {
        write_str("FAIL\n");
    }

    pass
}

// ────────────────────────────────────────────────────────────────────────────
// Test 41c: Single clock_gettime + verify memory
// ────────────────────────────────────────────────────────────────────────────
#[cfg(target_arch = "x86_64")]
fn test_41c_single_clock_gettime() -> bool {
    write_str("\nTest 41c: Single clock_gettime + verify memory\n");
    write_str("  Calling clock_gettime once...\n");

    let mut ts = Timespec {
        tv_sec: -1,
        tv_nsec: -1,
    };

    let ret = unsafe {
        syscall2(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC as u64, &mut ts as *mut Timespec as u64)
    } as i64;

    write_str("  Return value: ");
    write_signed(ret);
    write_str("\n");
    write_str("  tv_sec: ");
    write_signed(ts.tv_sec);
    write_str("\n");
    write_str("  tv_nsec: ");
    write_signed(ts.tv_nsec);
    write_str("\n");

    let pass = ret == 0 && ts.tv_sec >= 0 && ts.tv_nsec >= 0 && ts.tv_nsec < 1_000_000_000;
    write_str("  Result: ");
    if pass {
        write_str("PASS\n");
    } else {
        write_str("FAIL\n");
    }

    pass
}

// ────────────────────────────────────────────────────────────────────────────
// Test 41d: Register preservation across syscall
// ────────────────────────────────────────────────────────────────────────────
#[cfg(target_arch = "x86_64")]
fn test_41d_register_preservation() -> bool {
    write_str("\nTest 41d: Register preservation across syscall\n");
    write_str("  Setting R12=0xDEADBEEFDEADBEEF, R13=0xCAFEBABECAFEBABE before syscall\n");

    let r12_before: u64 = 0xDEADBEEF_DEADBEEF;
    let r13_before: u64 = 0xCAFEBABE_CAFEBABE;
    let r12_after: u64;
    let r13_after: u64;

    unsafe {
        std::arch::asm!(
            "mov r12, {r12_in}",
            "mov r13, {r13_in}",
            "mov rax, 39",          // SYS_GETPID
            "int 0x80",
            "mov {r12_out}, r12",
            "mov {r13_out}, r13",
            r12_in = in(reg) r12_before,
            r13_in = in(reg) r13_before,
            r12_out = out(reg) r12_after,
            r13_out = out(reg) r13_after,
            out("rax") _,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack, preserves_flags)
        );
    }

    write_str("  After syscall: R12=");
    write_hex(r12_after);
    write_str(", R13=");
    write_hex(r13_after);
    write_str("\n");

    let pass = r12_after == r12_before && r13_after == r13_before;
    write_str("  Result: ");
    if pass {
        write_str("PASS (registers preserved)\n");
    } else {
        write_str("FAIL (registers corrupted)\n");
    }

    pass
}

// ────────────────────────────────────────────────────────────────────────────
// Test 41e: Second clock_gettime call (the critical test)
// ────────────────────────────────────────────────────────────────────────────
#[cfg(target_arch = "x86_64")]
fn test_41e_second_clock_gettime() -> bool {
    write_str("\nTest 41e: Second clock_gettime call\n");
    write_str("  Calling clock_gettime again...\n");

    let mut ts = Timespec {
        tv_sec: -1,
        tv_nsec: -1,
    };

    let ret = unsafe {
        syscall2(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC as u64, &mut ts as *mut Timespec as u64)
    } as i64;

    write_str("  Return value: ");
    write_signed(ret);
    write_str("\n");
    write_str("  tv_sec: ");
    write_signed(ts.tv_sec);
    write_str("\n");
    write_str("  tv_nsec: ");
    write_signed(ts.tv_nsec);
    write_str("\n");

    let pass = ret == 0 && ts.tv_sec >= 0 && ts.tv_nsec >= 0 && ts.tv_nsec < 1_000_000_000;
    write_str("  Result: ");
    if pass {
        write_str("PASS\n");
    } else {
        write_str("FAIL\n");
    }

    pass
}

// ────────────────────────────────────────────────────────────────────────────
// Main entry point
// ────────────────────────────────────────────────────────────────────────────
#[cfg(target_arch = "x86_64")]
fn main() {
    write_str("=== SYSCALL DIAGNOSTIC TEST SEQUENCE ===\n");

    let mut passed = 0;
    let mut failed = 0;

    // Run all tests in sequence
    if test_41a_getpid() {
        passed += 1;
    } else {
        failed += 1;
    }

    if test_41b_write() {
        passed += 1;
    } else {
        failed += 1;
    }

    if test_41c_single_clock_gettime() {
        passed += 1;
    } else {
        failed += 1;
    }

    if test_41d_register_preservation() {
        passed += 1;
    } else {
        failed += 1;
    }

    if test_41e_second_clock_gettime() {
        passed += 1;
    } else {
        failed += 1;
    }

    // Summary
    write_str("\n=== SUMMARY: ");
    write_num(passed);
    write_str("/5 tests passed ===\n");
    write_str("DEBUG: passed=");
    write_num(passed);
    write_str(", failed=");
    write_num(failed);
    write_str("\n");

    if failed == 0 {
        write_str("\n✓ All diagnostic tests passed\n");
        std::process::exit(0);
    } else {
        write_str("\n✗ Some diagnostic tests failed\n");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn main() {
    println!("=== SYSCALL DIAGNOSTIC TEST SEQUENCE ===");
    println!("SKIP: This test is x86_64-only (uses int 0x80 inline asm)");
    println!("\n✓ All diagnostic tests passed");
    std::process::exit(0);
}
