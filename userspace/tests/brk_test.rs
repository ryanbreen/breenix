//! brk syscall test program
//!
//! Tests the POSIX-compliant brk() syscall for heap management.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::ptr::{write_volatile, read_volatile};

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_BRK: u64 = 12;

// Simple syscall wrappers
// NOTE: INT 0x80 may clobber argument registers - use inlateout to force the
// compiler to actually emit MOV instructions and not assume register values
// persist across syscalls.
unsafe fn syscall1(n: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
    );
    ret
}

unsafe fn syscall3(n: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
        inlateout("rdx") arg3 => _,
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
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';

    let hex_chars = b"0123456789abcdef";
    for i in 0..16 {
        let shift = 60 - (i * 4);
        let digit = ((n >> shift) & 0xf) as usize;
        buf[i + 2] = hex_chars[digit];
    }

    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    write_str(s);
}

// Helper to exit with error message
fn fail(msg: &str) -> ! {
    write_str("USERSPACE BRK: FAIL - ");
    write_str(msg);
    write_str("\n");
    unsafe { syscall1(SYS_EXIT, 1); }
    loop {}
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== brk Test Program ===\n");

    // Phase 1: Query current program break
    write_str("Phase 1: Querying initial program break with brk(0)...\n");
    let initial_brk = unsafe { syscall1(SYS_BRK, 0) };

    write_str("  Initial break: ");
    write_hex(initial_brk);
    write_str("\n");

    // Validate initial break is in expected range (0x40000000-0x80000000)
    if initial_brk < 0x40000000 || initial_brk > 0x80000000 {
        fail("Initial break outside expected range");
    }
    write_str("  Initial break is valid\n");

    // Phase 2: Expand heap by 4KB
    write_str("Phase 2: Expanding heap by 4KB...\n");
    let new_brk_requested = initial_brk + 4096;
    write_str("  Requesting break at: ");
    write_hex(new_brk_requested);
    write_str("\n");

    let new_brk = unsafe { syscall1(SYS_BRK, new_brk_requested) };
    write_str("  Returned break: ");
    write_hex(new_brk);
    write_str("\n");

    // Kernel may page-align, so check >= requested
    if new_brk < new_brk_requested {
        fail("Heap expansion failed");
    }
    write_str("  Heap expanded successfully\n");

    // Phase 3: Write to allocated memory
    write_str("Phase 3: Writing test pattern to allocated memory...\n");
    let test_addr = initial_brk as *mut u64;
    let test_pattern: u64 = 0xdeadbeef_cafebabe;

    unsafe {
        write_volatile(test_addr, test_pattern);
    }
    write_str("  Written pattern: ");
    write_hex(test_pattern);
    write_str("\n");

    // Phase 4: Read back and verify
    write_str("Phase 4: Reading back and verifying...\n");
    let read_value = unsafe { read_volatile(test_addr) };
    write_str("  Read value: ");
    write_hex(read_value);
    write_str("\n");

    if read_value != test_pattern {
        fail("Memory verification failed - read value doesn't match");
    }
    write_str("  Memory verification successful\n");

    // Phase 5: Expand again and test second region
    write_str("Phase 5: Expanding by another 4KB and testing...\n");
    let second_brk_requested = new_brk + 4096;
    let second_brk = unsafe { syscall1(SYS_BRK, second_brk_requested) };

    if second_brk < second_brk_requested {
        fail("Second heap expansion failed");
    }

    // Write to second region
    let second_test_addr = new_brk as *mut u64;
    let second_pattern: u64 = 0x12345678_9abcdef0;

    unsafe {
        write_volatile(second_test_addr, second_pattern);
    }

    let second_read = unsafe { read_volatile(second_test_addr) };
    if second_read != second_pattern {
        fail("Second region memory verification failed");
    }
    write_str("  Second region verified successfully\n");

    // All tests passed
    write_str("USERSPACE BRK: ALL TESTS PASSED\n");
    unsafe { syscall1(SYS_EXIT, 0); }
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in brk test!\n");
    unsafe { syscall1(SYS_EXIT, 1); }
    loop {}
}
