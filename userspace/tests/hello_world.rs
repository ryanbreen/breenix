#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;

// File descriptors
const STDOUT: u64 = 1;

// Inline assembly for INT 0x80 syscalls
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
    write_str("Hello from second process!\n");
    write_str("This process will exit with code 42\n");
    
    // Exit with code 42
    unsafe {
        syscall1(SYS_EXIT, 42);
    }
    
    // Should never reach here
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("Second process panic!\n");
    
    // Exit with error code 2
    unsafe {
        syscall1(SYS_EXIT, 2);
    }
    
    // Should never reach here
    loop {}
}