#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_GET_TIME: u64 = 4;

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

// Convert number to string (simple implementation)
fn num_to_str(mut num: u64, buf: &mut [u8]) -> &str {
    if num == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap();
    }
    
    let mut i = 0;
    let mut digits = [0u8; 20]; // enough for u64
    
    while num > 0 {
        digits[i] = b'0' + (num % 10) as u8;
        num /= 10;
        i += 1;
    }
    
    // Reverse the digits
    for j in 0..i {
        buf[j] = digits[i - 1 - j];
    }
    
    core::str::from_utf8(&buf[..i]).unwrap()
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Get current time
    let ticks = unsafe { syscall0(SYS_GET_TIME) };
    
    // Print greeting
    write_str("Hello from userspace! Current time: ");
    
    // Convert ticks to string and print
    let mut buf = [0u8; 20];
    let time_str = num_to_str(ticks, &mut buf);
    write_str(time_str);
    
    write_str(" ticks\n");
    
    // Exit cleanly with code 0
    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    
    // Should never reach here
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("Userspace panic!\n");
    
    // Exit with error code 1
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    
    // Should never reach here
    loop {}
}