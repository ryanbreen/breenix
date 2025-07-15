// Test to verify Phase 4B syscalls are working
// This should be run from userspace and call sys_get_time

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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Test 1: Call sys_write
    let msg1 = b"Phase 4B Test: Testing sys_write...\n";
    unsafe {
        syscall3(SYS_WRITE, STDOUT, msg1.as_ptr() as u64, msg1.len() as u64);
    }
    
    // Test 2: Call sys_get_time
    let msg2 = b"Phase 4B Test: Calling sys_get_time...\n";
    unsafe {
        syscall3(SYS_WRITE, STDOUT, msg2.as_ptr() as u64, msg2.len() as u64);
    }
    
    let ticks = unsafe { syscall0(SYS_GET_TIME) };
    
    // Test 3: Report result
    let msg3 = b"Phase 4B Test: sys_get_time returned!\n";
    unsafe {
        syscall3(SYS_WRITE, STDOUT, msg3.as_ptr() as u64, msg3.len() as u64);
    }
    
    // If ticks is non-zero, Phase 4B is working!
    if ticks > 0 {
        let msg_success = b"SUCCESS: Phase 4B sys_get_time works! Got non-zero ticks\n";
        unsafe {
            syscall3(SYS_WRITE, STDOUT, msg_success.as_ptr() as u64, msg_success.len() as u64);
        }
    } else {
        let msg_fail = b"FAIL: sys_get_time returned 0\n";
        unsafe {
            syscall3(SYS_WRITE, STDOUT, msg_fail.as_ptr() as u64, msg_fail.len() as u64);
        }
    }
    
    // Exit cleanly
    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}