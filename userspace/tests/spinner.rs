#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_YIELD: u64 = 3;

// File descriptors
const STDOUT: u64 = 1;

// Inline assembly for INT 0x80 syscalls
#[inline(always)]
unsafe fn syscall0(num: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") num,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
unsafe fn syscall1(num: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

// Simple write function
fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, STDOUT, s.as_ptr() as u64, s.len() as u64);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Print greeting
    write_str("Spinner process starting!\n");
    
    // Spinner characters
    let spinner_chars = ['-', '\\', '|', '/'];
    
    // Spin for 20 iterations
    for i in 0..20 {
        // Use carriage return to go back to start of line, then print frame
        // This creates the spinning animation effect on a single line
        write_str("\rSpinner: ");

        // Print spinner character
        let ch = spinner_chars[i % 4];
        let ch_bytes = [ch as u8];
        let ch_str = core::str::from_utf8(&ch_bytes).unwrap();
        write_str(ch_str);

        // Yield to allow other processes to run
        unsafe {
            syscall0(SYS_YIELD);
        }

        // Do some busy work to simulate computation
        let mut sum = 0u64;
        for j in 0..50000 {
            sum = sum.wrapping_add(j);
        }

        // Prevent optimization
        if sum == 0 {
            write_str("Unexpected!\n");
        }
    }

    // Final newline after spinner animation completes
    write_str("\n");
    
    write_str("Spinner process finished!\n");

    // Exit cleanly with code 0
    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    
    // Should never reach here
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("Spinner process panic!\n");
    
    // Exit with error code 4
    unsafe {
        syscall1(SYS_EXIT, 4);
    }
    
    // Should never reach here
    loop {}
}