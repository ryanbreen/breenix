#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::arch::asm;

// Syscall constants
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 0;

// File descriptors
const STDOUT: u64 = 1;

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
    // Exit with status 7 as required
    // The kernel should print EXIT_OK when it sees this exit code
    unsafe {
        syscall1(SYS_EXIT, 7);
    }
    
    // Should never reach here
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in sys_exit_guard\n");
    unsafe {
        syscall1(SYS_EXIT, 1);
    }
    loop {}
}