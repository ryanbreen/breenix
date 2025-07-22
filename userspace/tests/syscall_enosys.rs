#![no_std]
#![no_main]

use core::panic::PanicInfo;

const SYS_EXIT: u64     = 0;
const SYS_WRITE: u64    = 1;
const SYS_UNKNOWN: u64  = 999;         // guaranteed unimplemented
const FD_STDOUT: u64    = 1;
const ENOSYS_U64: u64   = (!38u64) + 1; // -38 wrapped to u64

#[inline(always)]
unsafe fn syscall0(num: u64) -> u64 {
    let ret;
    core::arch::asm!(
        "int 0x80",
        in("rax") num,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(num: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let ret;
    core::arch::asm!(
        "int 0x80",
        in("rax") num, in("rdi") a1, in("rsi") a2, in("rdx") a3,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let rv = unsafe { syscall0(SYS_UNKNOWN) };
    if rv == ENOSYS_U64 {
        write_str("ENOSYS OK\n");
    } else {
        write_str("ENOSYS FAIL\n");
    }
    unsafe { syscall3(SYS_EXIT, 0, 0, 0); }
    loop {}
}

fn write_str(s: &str) {
    unsafe { syscall3(SYS_WRITE, FD_STDOUT, s.as_ptr() as u64, s.len() as u64); }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! { loop {} }