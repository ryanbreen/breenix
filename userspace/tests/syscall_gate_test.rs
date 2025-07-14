#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;

// Test syscall number for Phase 4A
const TEST_SYSCALL: u64 = 0x1234;

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

// Test syscall - just call int $0x80 with EAX=0x1234
#[inline(always)]
unsafe fn test_syscall() -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") TEST_SYSCALL,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Call the test syscall with EAX=0x1234
    unsafe {
        let result = test_syscall();
        // The kernel should handle this and return some value
        // For now, we just need to test that the syscall gate works
    }
    
    // Exit with code 0 (success)
    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    
    // Should never reach here - but add explicit halt to prevent runaway execution
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