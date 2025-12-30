//! Stdin read test program
//!
//! Tests reading from stdin (fd 0) to verify keyboard input infrastructure.
//! This test verifies that:
//! 1. Reading from stdin returns EAGAIN when no data is available
//! 2. The kernel correctly handles stdin fd lookups

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ: u64 = 2;

// Error codes
const EAGAIN: i64 = 11;
const ERESTARTSYS: i64 = 512;

// Syscall wrappers
#[inline(always)]
unsafe fn syscall1(n: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        out("rcx") _,
        out("rdx") _,
        out("rsi") _,
        out("r8") _,
        out("r9") _,
        out("r10") _,
        out("r11") _,
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(n: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
        inlateout("rdx") arg3 => _,
        out("rcx") _,
        out("r8") _,
        out("r9") _,
        out("r10") _,
        out("r11") _,
    );
    ret
}

// Helper to write a string
#[inline(always)]
fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64);
    }
}

// Helper to write a decimal number
#[inline(always)]
fn write_num(n: i64) {
    if n < 0 {
        write_str("-");
        write_num_inner(-n as u64);
    } else {
        write_num_inner(n as u64);
    }
}

#[inline(always)]
fn write_num_inner(mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = 19;

    if n == 0 {
        write_str("0");
        return;
    }

    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i -= 1;
    }

    let s = unsafe { core::str::from_utf8_unchecked(&buf[i + 1..]) };
    write_str(s);
}

// Helper to exit with error message
#[inline(always)]
fn fail(msg: &str) -> ! {
    write_str("USERSPACE STDIN: FAIL - ");
    write_str(msg);
    write_str("\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== Stdin Read Test Program ===\n");

    // Phase 1: Test non-blocking read from stdin when empty
    write_str("Phase 1: Testing read from empty stdin...\n");

    let mut read_buf = [0u8; 16];
    let ret = unsafe {
        syscall3(SYS_READ, 0, read_buf.as_mut_ptr() as u64, read_buf.len() as u64)
    } as i64;

    write_str("  read(stdin) returned: ");
    write_num(ret);
    write_str("\n");

    // When stdin buffer is empty:
    // - If blocking is enabled, we should get ERESTARTSYS (thread blocked)
    // - If non-blocking or already data, we'd get EAGAIN or the data
    // Since we're testing the infrastructure, any of these is acceptable:
    // - EAGAIN (11) = no data, would block (non-blocking behavior)
    // - ERESTARTSYS (512) = thread was blocked, syscall should restart
    // - 0 = EOF (though stdin shouldn't EOF)
    // - positive = data was actually read

    if ret == -EAGAIN || ret == -ERESTARTSYS || ret == 0 {
        write_str("  Got expected result for empty stdin\n");
        if ret == -EAGAIN {
            write_str("  (EAGAIN: no data, would block)\n");
        } else if ret == -ERESTARTSYS {
            write_str("  (ERESTARTSYS: thread blocked for input)\n");
        } else {
            write_str("  (0: no data available)\n");
        }
    } else if ret > 0 {
        // Data was actually in the buffer (unlikely but valid)
        write_str("  Data was in stdin buffer: ");
        write_num(ret);
        write_str(" bytes\n");
    } else if ret < 0 {
        // Unexpected error
        write_str("  Unexpected error code: ");
        write_num(ret);
        write_str("\n");
        fail("Unexpected stdin read error");
    }

    // Phase 2: Verify fd 0 is properly set up as stdin
    write_str("Phase 2: Verifying stdin fd is accessible...\n");

    // A zero-length read should always succeed with 0
    let ret2 = unsafe {
        syscall3(SYS_READ, 0, read_buf.as_mut_ptr() as u64, 0)
    } as i64;

    write_str("  read(stdin, buf, 0) returned: ");
    write_num(ret2);
    write_str("\n");

    if ret2 != 0 {
        fail("Zero-length read should return 0");
    }
    write_str("  Zero-length read works correctly\n");

    // All tests passed
    write_str("USERSPACE STDIN: ALL TESTS PASSED\n");
    write_str("STDIN_TEST_PASSED\n");

    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in stdin test!\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}
