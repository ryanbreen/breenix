#![no_std]
#![no_main]

//! No-std raw `_start` diagnostic with deliberately padded executable text.

use core::arch::asm;
use core::panic::PanicInfo;

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".section .text.hello_nostd_padded_pad,\"ax\"",
    ".global hello_nostd_padded_pad",
    "hello_nostd_padded_pad:",
    ".rept 24576",
    "nop",
    ".endr",
    "ret",
);

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".section .text.hello_nostd_padded_pad,\"ax\"",
    ".global hello_nostd_padded_pad",
    "hello_nostd_padded_pad:",
    ".rept 24576",
    "nop",
    ".endr",
    "ret",
);

extern "C" {
    fn hello_nostd_padded_pad();
}

const RAW_BEFORE: &[u8] = b"[hello_nostd_padded] raw-before\n";
const RAW_AFTER: &[u8] = b"[hello_nostd_padded] raw-after\n";

#[cfg(target_arch = "aarch64")]
unsafe fn syscall1(num: u64, arg0: u64) -> u64 {
    let ret: u64;
    asm!(
        "svc #0",
        in("x8") num,
        inlateout("x0") arg0 => ret,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "aarch64")]
unsafe fn syscall3(num: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    asm!(
        "svc #0",
        in("x8") num,
        inlateout("x0") arg0 => ret,
        in("x1") arg1,
        in("x2") arg2,
        options(nostack)
    );
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn syscall1(num: u64, arg0: u64) -> u64 {
    let ret: u64;
    asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg0,
        lateout("rax") ret,
        options(nostack, preserves_flags)
    );
    ret
}

#[cfg(target_arch = "x86_64")]
unsafe fn syscall3(num: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg0,
        in("rsi") arg1,
        in("rdx") arg2,
        lateout("rax") ret,
        options(nostack, preserves_flags)
    );
    ret
}

#[cfg(target_arch = "aarch64")]
const SYS_WRITE: u64 = 64;
#[cfg(target_arch = "aarch64")]
const SYS_EXIT: u64 = 93;

#[cfg(target_arch = "x86_64")]
const SYS_WRITE: u64 = 1;
#[cfg(target_arch = "x86_64")]
const SYS_EXIT: u64 = 60;

#[inline(always)]
fn raw_write(buf: &[u8]) {
    unsafe {
        let _ = syscall3(SYS_WRITE, 1, buf.as_ptr() as u64, buf.len() as u64);
    }
}

#[inline(always)]
fn raw_exit(code: i32) -> ! {
    unsafe {
        let _ = syscall1(SYS_EXIT, code as u64);
    }
    loop {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    raw_write(RAW_BEFORE);
    unsafe {
        hello_nostd_padded_pad();
    }
    raw_write(RAW_AFTER);
    raw_exit(42);
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    raw_exit(101);
}
