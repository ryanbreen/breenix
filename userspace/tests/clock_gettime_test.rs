//! clock_gettime syscall test program
//!
//! Tests the POSIX-compliant clock_gettime syscall with CLOCK_MONOTONIC.
//! Validates that TSC-based high-resolution timing works correctly from userspace.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_CLOCK_GETTIME: u64 = 228;

// Clock IDs (Linux conventions)
const CLOCK_MONOTONIC: u32 = 1;

/// Timespec structure for clock_gettime
#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

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

unsafe fn syscall2(n: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") n,
        in("rdi") arg1,
        in("rsi") arg2,
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

// Helper to write a number
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
        i -= 1;
    }

    let s = unsafe { core::str::from_utf8_unchecked(&buf[i + 1..20]) };
    write_str(s);
}

// Helper to write a signed number
fn write_signed(n: i64) {
    if n < 0 {
        write_str("-");
        write_num((-n) as u64);
    } else {
        write_num(n as u64);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== clock_gettime Userspace Test ===\n");

    let mut passed = 0;
    let mut failed = 0;

    // ── Test 1: Basic syscall functionality ───────────────────────
    write_str("\nTest 1: Basic syscall functionality\n");

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
    write_str("  tv_sec:  ");
    write_signed(ts.tv_sec);
    write_str("\n");
    write_str("  tv_nsec: ");
    write_signed(ts.tv_nsec);
    write_str("\n");

    if ret == 0 && ts.tv_sec >= 0 && ts.tv_nsec >= 0 && ts.tv_nsec < 1_000_000_000 {
        write_str("  PASS: Syscall returned valid time\n");
        passed += 1;
    } else {
        write_str("  FAIL: Invalid return or out-of-range values\n");
        failed += 1;
    }

    // ── Test 2: Time advances between calls ───────────────────────
    write_str("\nTest 2: Time advances between calls\n");

    let mut t1 = Timespec { tv_sec: 0, tv_nsec: 0 };
    let mut t2 = Timespec { tv_sec: 0, tv_nsec: 0 };

    unsafe {
        syscall2(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC as u64, &mut t1 as *mut Timespec as u64);
        syscall2(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC as u64, &mut t2 as *mut Timespec as u64);
    }

    let t1_ns = t1.tv_sec * 1_000_000_000 + t1.tv_nsec;
    let t2_ns = t2.tv_sec * 1_000_000_000 + t2.tv_nsec;

    write_str("  First call:  ");
    write_signed(t1.tv_sec);
    write_str(" s, ");
    write_signed(t1.tv_nsec);
    write_str(" ns\n");
    write_str("  Second call: ");
    write_signed(t2.tv_sec);
    write_str(" s, ");
    write_signed(t2.tv_nsec);
    write_str(" ns\n");

    if t2_ns >= t1_ns {
        write_str("  PASS: Time did not go backwards\n");
        passed += 1;
    } else {
        write_str("  FAIL: Time went backwards!\n");
        failed += 1;
    }

    // ── Test 3: Sub-millisecond precision (TSC vs PIT) ────────────
    write_str("\nTest 3: Sub-millisecond precision\n");

    let elapsed_ns = t2_ns - t1_ns;
    write_str("  Elapsed: ");
    write_num(elapsed_ns as u64);
    write_str(" ns\n");

    // With TSC, rapid calls should show < 1ms elapsed time
    // With PIT fallback, would be 0 or >= 1ms
    if elapsed_ns < 1_000_000 {
        write_str("  PASS: Sub-millisecond precision (TSC active)\n");
        passed += 1;
    } else {
        write_str("  FAIL: Elapsed time >= 1ms (possible PIT fallback)\n");
        failed += 1;
    }

    // ── Test 4: Nanoseconds not suspiciously aligned ──────────────
    write_str("\nTest 4: Nanosecond precision (not millisecond-aligned)\n");

    // Collect 10 samples
    let mut aligned_count = 0;
    let mut i = 0;
    while i < 10 {
        let mut ts_sample = Timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe {
            syscall2(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC as u64, &mut ts_sample as *mut Timespec as u64);
        }
        if ts_sample.tv_nsec % 1_000_000 == 0 {
            aligned_count += 1;
        }
        i += 1;
    }

    write_str("  Millisecond-aligned samples: ");
    write_num(aligned_count);
    write_str("/10\n");

    // If TSC works, should have mostly non-aligned values
    // PIT fallback would have ALL aligned (10/10)
    if aligned_count < 8 {
        write_str("  PASS: Nanosecond precision confirmed\n");
        passed += 1;
    } else {
        write_str("  FAIL: Too many aligned values (possible PIT fallback)\n");
        failed += 1;
    }

    // ── Test 5: Multiple calls maintain monotonicity ──────────────
    write_str("\nTest 5: Monotonicity over multiple calls\n");

    let mut prev_ns = t2_ns;
    let mut monotonic = true;
    let mut call_count = 0;

    while call_count < 10 {
        let mut ts_check = Timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe {
            syscall2(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC as u64, &mut ts_check as *mut Timespec as u64);
        }
        let now_ns = ts_check.tv_sec * 1_000_000_000 + ts_check.tv_nsec;

        if now_ns < prev_ns {
            monotonic = false;
            break;
        }
        prev_ns = now_ns;
        call_count += 1;
    }

    if monotonic {
        write_str("  PASS: 10 calls maintained monotonicity\n");
        passed += 1;
    } else {
        write_str("  FAIL: Time went backwards during calls\n");
        failed += 1;
    }

    // ── Summary ────────────────────────────────────────────────────
    write_str("\n=== Test Summary ===\n");
    write_str("Passed: ");
    write_num(passed);
    write_str("/5\n");
    write_str("Failed: ");
    write_num(failed);
    write_str("/5\n");

    if failed == 0 {
        write_str("\nUSERSPACE CLOCK_GETTIME: OK\n");
        write_str("TSC-based high-resolution timing validated from userspace\n");
        unsafe { syscall1(SYS_EXIT, 0); }
    } else {
        write_str("\nUSERSPACE CLOCK_GETTIME: FAIL\n");
        unsafe { syscall1(SYS_EXIT, 1); }
    }

    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in clock_gettime test!\n");
    unsafe { syscall1(SYS_EXIT, 1); }
    loop {}
}
