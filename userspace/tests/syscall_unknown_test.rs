#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;

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

// Test unknown syscall - expect -ENOSYS (-38)
#[inline(always)]
unsafe fn test_unknown_syscall() -> i64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") 999u64,  // Invalid syscall number
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret as i64  // Interpret as signed for error codes
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Test unknown syscall
    unsafe {
        let result = test_unknown_syscall();
        
        // Check if we got -ENOSYS (-38)
        if result == -38 {
            // Success! Unknown syscall returned -ENOSYS as expected
            syscall1(SYS_EXIT, 0);  // Exit with success
        } else {
            // Failure - got wrong error code
            syscall1(SYS_EXIT, 1);  // Exit with error
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