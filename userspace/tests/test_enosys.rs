#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;

// File descriptors
const STDOUT: u64 = 1;

// Invalid syscall number for testing ENOSYS
const SYS_INVALID: u64 = 999;

// Inline assembly for INT 0x80 syscalls
#[inline(always)]
unsafe fn syscall0(num: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        in("rax") num,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
unsafe fn syscall1(num: u64, arg1: u64) -> i64 {
    let ret: i64;
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
unsafe fn syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let ret: i64;
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
    write_str("Testing ENOSYS error handling...\n");
    
    // Call invalid syscall (should return -38 for ENOSYS)
    let result = unsafe { syscall0(SYS_INVALID) };
    
    if result == -38 {
        write_str("SUCCESS: Invalid syscall returned ENOSYS (-38)\n");
        unsafe { syscall1(SYS_EXIT, 0); }
    } else {
        write_str("FAILURE: Invalid syscall did not return ENOSYS\n");
        write_str("Got error code: ");
        
        // Convert number to string (simple implementation for negative numbers)
        let mut buf = [0u8; 20];
        let mut n = if result < 0 { -result } else { result };
        let mut i = 19;
        
        if n == 0 {
            buf[i] = b'0';
            i -= 1;
        } else {
            while n > 0 && i > 0 {
                buf[i] = b'0' + ((n % 10) as u8);
                n /= 10;
                i -= 1;
            }
        }
        
        if result < 0 && i > 0 {
            buf[i] = b'-';
            i -= 1;
        }
        
        let num_str = unsafe { 
            core::str::from_utf8_unchecked(&buf[(i+1)..20])
        };
        write_str(num_str);
        write_str("\n");
        
        unsafe { syscall1(SYS_EXIT, 1); }
    }
    
    // Should never reach here
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("Test panic!\n");
    
    // Exit with error code 2
    unsafe {
        syscall1(SYS_EXIT, 2);
    }
    
    // Should never reach here
    loop {}
}