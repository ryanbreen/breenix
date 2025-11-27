//! brk() syscall test program
//!
//! Tests heap allocation using the brk() syscall.
//! This validates that userspace programs can dynamically allocate memory.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_BRK: u64 = 12;

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
    let hex_digits = b"0123456789abcdef";
    write_str("0x");

    let mut buf = [0u8; 16];
    for i in 0..16 {
        let digit = ((n >> (60 - i * 4)) & 0xf) as usize;
        buf[i] = hex_digits[digit];
    }

    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    write_str(s);
}

// Helper to exit
fn exit(code: i32) -> ! {
    unsafe {
        syscall1(SYS_EXIT, code as u64);
    }
    loop {}
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== brk() Test Program ===\n");

    // Test 1: Query current program break
    write_str("Test 1: Query current program break\n");
    let initial_brk = unsafe { syscall1(SYS_BRK, 0) };
    write_str("  Initial program break: ");
    write_hex(initial_brk);
    write_str("\n");

    if initial_brk == 0 {
        write_str("  FAIL: brk(0) returned 0\n");
        exit(1);
    }
    write_str("  PASS: Got valid program break\n");

    // Test 2: Allocate one page (4KB)
    write_str("\nTest 2: Allocate one page (4096 bytes)\n");
    let new_brk = initial_brk + 4096;
    write_str("  Requesting brk at: ");
    write_hex(new_brk);
    write_str("\n");

    let result = unsafe { syscall1(SYS_BRK, new_brk) };
    write_str("  Result: ");
    write_hex(result);
    write_str("\n");

    if result < new_brk {
        write_str("  FAIL: brk() did not allocate requested memory\n");
        exit(1);
    }
    write_str("  PASS: Successfully allocated memory\n");

    // Test 3: Write to allocated memory
    write_str("\nTest 3: Write to allocated memory\n");
    unsafe {
        let ptr = initial_brk as *mut u64;
        write_str("  Writing pattern to heap...\n");

        // Write a pattern to verify memory is accessible
        for i in 0..512 {
            // 512 * 8 = 4096 bytes
            let addr = ptr.add(i);
            core::ptr::write_volatile(addr, 0xDEADBEEF_C0FFEE00 + i as u64);
        }

        write_str("  Verifying pattern...\n");

        // Read back and verify
        for i in 0..512 {
            let addr = ptr.add(i);
            let value = core::ptr::read_volatile(addr);
            let expected = 0xDEADBEEF_C0FFEE00 + i as u64;
            if value != expected {
                write_str("  FAIL: Memory verification failed at offset ");
                write_hex(i as u64);
                write_str("\n");
                write_str("    Expected: ");
                write_hex(expected);
                write_str("\n");
                write_str("    Got: ");
                write_hex(value);
                write_str("\n");
                exit(1);
            }
        }
    }
    write_str("  PASS: Memory is readable and writable\n");

    // Test 4: Allocate more memory
    write_str("\nTest 4: Allocate another page\n");
    let new_brk2 = result + 4096;
    write_str("  Requesting brk at: ");
    write_hex(new_brk2);
    write_str("\n");

    let result2 = unsafe { syscall1(SYS_BRK, new_brk2) };
    write_str("  Result: ");
    write_hex(result2);
    write_str("\n");

    if result2 < new_brk2 {
        write_str("  FAIL: Second allocation failed\n");
        exit(1);
    }
    write_str("  PASS: Successfully allocated more memory\n");

    // Test 5: Contract heap
    write_str("\nTest 5: Contract heap back to initial size\n");
    let result3 = unsafe { syscall1(SYS_BRK, initial_brk) };
    write_str("  Result: ");
    write_hex(result3);
    write_str("\n");

    if result3 != initial_brk {
        write_str("  FAIL: Heap contraction failed\n");
        exit(1);
    }
    write_str("  PASS: Successfully contracted heap\n");

    // Success!
    write_str("\n==============================\n");
    write_str("USERSPACE BRK: ALL TESTS PASSED\n");
    write_str("==============================\n");

    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in brk test!\n");
    exit(1);
}
