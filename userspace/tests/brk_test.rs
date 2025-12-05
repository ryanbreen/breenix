//! brk syscall test program
//!
//! Tests the POSIX-compliant brk() syscall for heap management.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_BRK: u64 = 12;

// Simple syscall wrappers
// NOTE: INT 0x80 may clobber argument registers - use inlateout to force the
// compiler to actually emit MOV instructions and not assume register values
// persist across syscalls.
// IMPORTANT: Must be #[inline(always)] to prevent LLVM from placing them before
// _start in the .text section, which would cause the ELF entry point to not match
// the actual start of the code.
#[inline(always)]
unsafe fn syscall1(n: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        // Mark all caller-saved registers as clobbered
        // Even though kernel should preserve them, a timer interrupt during
        // syscall handling might corrupt them during context switch operations
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
        // Mark remaining caller-saved registers as clobbered
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

// Helper to write a hex number
#[inline(always)]
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
#[inline(always)]
fn fail(msg: &str) -> ! {
    write_str("USERSPACE BRK: FAIL - ");
    write_str(msg);
    write_str("\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
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

    // Phase 3: Write full page with unique patterns (512 u64 values = 4096 bytes)
    // Using unique pattern per offset catches indexing bugs and validates full page mapping
    write_str("Phase 3: Writing 512 unique patterns (4KB) to allocated memory...\n");
    let base_addr = initial_brk as *mut u64;
    const NUM_VALUES: usize = 512; // 512 * 8 = 4096 bytes = 1 page

    for i in 0..NUM_VALUES {
        let pattern: u64 = 0xDEADBEEF_C0FFEE00 + (i as u64);
        unsafe {
            write_volatile(base_addr.add(i), pattern);
        }
    }
    write_str("  Written 512 unique patterns\n");

    // Phase 4: Read back and verify all patterns
    write_str("Phase 4: Verifying all 512 patterns...\n");
    let mut errors = 0u64;
    for i in 0..NUM_VALUES {
        let expected: u64 = 0xDEADBEEF_C0FFEE00 + (i as u64);
        let actual = unsafe { read_volatile(base_addr.add(i)) };
        if actual != expected {
            errors += 1;
            if errors <= 3 {
                // Only report first few errors to avoid flooding output
                write_str("  ERROR at offset ");
                write_hex(i as u64);
                write_str(": expected ");
                write_hex(expected);
                write_str(", got ");
                write_hex(actual);
                write_str("\n");
            }
        }
    }

    if errors > 0 {
        write_str("  FAIL: ");
        write_hex(errors);
        write_str(" verification errors\n");
        fail("Full page memory verification failed");
    }
    write_str("  All 512 patterns verified successfully\n");

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

    // Phase 6: Contract heap back to initial size
    // This tests the heap shrink functionality which is implemented but
    // must be exercised to ensure proper page unmapping and memory cleanup
    write_str("Phase 6: Contracting heap back to initial size...\n");
    write_str("  Current break: ");
    write_hex(second_brk);
    write_str("\n  Requesting: ");
    write_hex(initial_brk);
    write_str("\n");

    let contracted_brk = unsafe { syscall1(SYS_BRK, initial_brk) };
    write_str("  Returned break: ");
    write_hex(contracted_brk);
    write_str("\n");

    if contracted_brk != initial_brk {
        write_str("  FAIL: Expected break at ");
        write_hex(initial_brk);
        write_str(" but got ");
        write_hex(contracted_brk);
        write_str("\n");
        fail("Heap contraction failed");
    }
    write_str("  Heap contracted successfully to initial size\n");

    // Phase 7: Verify we can expand again after contraction
    // This validates that heap management state is consistent after shrink
    write_str("Phase 7: Re-expanding heap after contraction...\n");
    let reexpand_brk = unsafe { syscall1(SYS_BRK, initial_brk + 4096) };

    if reexpand_brk < initial_brk + 4096 {
        fail("Re-expansion after contraction failed");
    }

    // Write and verify a pattern to the re-allocated region
    write_str("  Re-expand brk returned: ");
    write_hex(reexpand_brk);
    write_str("\n  Writing to addr: ");
    write_hex(initial_brk);
    write_str("\n");

    let reexpand_addr = initial_brk as *mut u64;
    let reexpand_pattern: u64 = 0xCAFEBABE_DEADBEEF;

    unsafe {
        write_volatile(reexpand_addr, reexpand_pattern);
    }
    write_str("  Pattern written\n");

    let reexpand_read = unsafe { read_volatile(reexpand_addr) };
    write_str("  Read back: ");
    write_hex(reexpand_read);
    write_str("\n  Expected: ");
    write_hex(reexpand_pattern);
    write_str("\n");

    if reexpand_read != reexpand_pattern {
        write_str("  MISMATCH!\n");
        fail("Re-expanded region memory verification failed");
    }
    write_str("  Re-expansion verified successfully\n");

    // All tests passed
    write_str("USERSPACE BRK: ALL TESTS PASSED\n");
    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in brk test!\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}
