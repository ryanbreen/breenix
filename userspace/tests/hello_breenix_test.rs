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

// Simple write wrapper
unsafe fn write(fd: u64, buf: &[u8]) -> isize {
    let result = syscall3(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64);
    result as i64 as isize  // Convert to signed
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Test sys_write with "Hello, Breenix!"
    let message = b"Hello, Breenix!\n";
    
    unsafe {
        let bytes_written = write(STDOUT, message);
        
        // Check if write was successful
        if bytes_written == message.len() as isize {
            // Success! Exit with code 0
            syscall1(SYS_EXIT, 0);
        } else {
            // Failed - exit with error code
            syscall1(SYS_EXIT, 1);
        }
    }
    
    // Should never reach here
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Exit with error code 1
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    
    // Should never reach here
    loop {}
}