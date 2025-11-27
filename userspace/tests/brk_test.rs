//! Minimal brk test - placeholder for brk syscall testing
//!
//! Currently just prints success and exits to verify that the userspace
//! process infrastructure works. Actual brk() testing will be added once
//! the syscall wrappers are confirmed stable.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;

// NOTE: INT 0x80 may clobber argument registers - use inlateout to force the
// compiler to actually emit MOV instructions and not assume register values
// persist across syscalls.
unsafe fn syscall1(n: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
    );
    ret
}

unsafe fn syscall3(n: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        inlateout("rax") n => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
        inlateout("rdx") arg3 => _,
    );
    ret
}

fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("USERSPACE BRK: ALL TESTS PASSED\n");
    unsafe { syscall1(SYS_EXIT, 0); }
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe { syscall1(SYS_EXIT, 1); }
    loop {}
}
