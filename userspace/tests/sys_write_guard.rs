#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::arch::asm;

// Syscall constants
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 0;
const SYS_GET_TIME: u64 = 4;

// File descriptors
const STDOUT: u64 = 1;

#[inline(always)]
unsafe fn syscall0(num: u64) -> u64 {
    let ret: u64;
    asm!(
        "int 0x80",
        in("rax") num,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    asm!(
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

#[inline(always)]
unsafe fn syscall1(num: u64, arg1: u64) -> u64 {
    let ret: u64;
    asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, STDOUT, s.as_ptr() as u64, s.len() as u64);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Ensure at least 5 timer ticks elapse (anti-cheat requirement)
    let start_time = unsafe { syscall0(SYS_GET_TIME) };
    
    // Print the required sentinel text
    write_str("WRITE_OK\n");
    
    // Wait until at least 5 timer ticks have elapsed
    loop {
        let current_time = unsafe { syscall0(SYS_GET_TIME) };
        if current_time >= start_time + 5 {
            break;
        }
        // Small delay to avoid busy-waiting too aggressively
        for _ in 0..1000 {
            unsafe { core::arch::asm!("nop") };
        }
    }
    
    // Exit cleanly
    unsafe {
        syscall1(SYS_EXIT, 0);
    }
    
    // Should never reach here
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in sys_write_guard\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}