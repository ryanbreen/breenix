//! clock_gettime syscall test program
//!
//! Tests the POSIX-compliant clock_gettime syscall with CLOCK_MONOTONIC.

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
    write_str("=== clock_gettime Test Program ===\n");

    // Initialize timespec with known values to detect if syscall modifies it
    let mut ts = Timespec {
        tv_sec: -1,
        tv_nsec: -1,
    };

    write_str("Calling clock_gettime(CLOCK_MONOTONIC)...\n");

    // Call clock_gettime with CLOCK_MONOTONIC
    let ret = unsafe {
        syscall2(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC as u64, &mut ts as *mut Timespec as u64)
    } as i64;

    write_str("Return value: ");
    write_signed(ret);
    write_str("\n");

    write_str("tv_sec:  ");
    write_signed(ts.tv_sec);
    write_str("\n");

    write_str("tv_nsec: ");
    write_signed(ts.tv_nsec);
    write_str("\n");

    // Validate results
    let success = ret == 0 && (ts.tv_sec > 0 || ts.tv_nsec > 0);

    if success {
        write_str("USERSPACE CLOCK_GETTIME: OK\n");
        unsafe { syscall1(SYS_EXIT, 0); }
    } else {
        write_str("USERSPACE CLOCK_GETTIME: FAIL\n");
        if ret != 0 {
            write_str("  - Syscall returned error\n");
        }
        if ts.tv_sec <= 0 && ts.tv_nsec <= 0 {
            write_str("  - Time is zero or negative\n");
        }
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
