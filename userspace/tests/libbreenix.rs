//! Breenix userspace system call library

use core::arch::asm;

// Include shared syscall constants from kernel
include!("../../kernel/src/syscall/syscall_consts.rs");

// Legacy syscall numbers still used by some code - use aliases to avoid conflicts
const SYS_EXIT_LEGACY: u64 = 0;
const SYS_EXEC_LEGACY: u64 = 11;
const SYS_GETPID_LEGACY: u64 = 39;

// Test syscalls - define them here since we can't use feature gates in userspace
const SYS_SHARE_TEST_PAGE: u64 = 400;
const SYS_GET_SHARED_TEST_PAGE: u64 = 401;

// Inline assembly for INT 0x80 syscalls
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

#[inline(always)]
unsafe fn syscall2(num: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        in("rsi") arg2,
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

// System call wrappers
pub unsafe fn sys_exit(code: i32) -> ! {
    syscall1(SYS_EXIT, code as u64);
    unreachable!("exit should not return");
}

pub unsafe fn sys_write(fd: u64, buf: &[u8]) -> u64 {
    syscall3(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64)
}

pub unsafe fn sys_write_const(buf: &[u8]) -> u64 {
    syscall3(SYS_WRITE, 1, buf.as_ptr() as u64, buf.len() as u64)
}

pub unsafe fn sys_read(fd: u64, buf: &mut [u8]) -> u64 {
    syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64)
}

pub unsafe fn sys_yield() -> u64 {
    syscall0(SYS_YIELD)
}

pub unsafe fn sys_get_time() -> u64 {
    syscall0(SYS_GET_TIME)
}

pub unsafe fn sys_fork() -> u64 {
    syscall0(SYS_FORK)
}

pub unsafe fn sys_exec(path: &str, args: &str) -> u64 {
    syscall2(SYS_EXEC, path.as_ptr() as u64, args.as_ptr() as u64)
}

pub unsafe fn sys_getpid() -> u64 {
    syscall0(SYS_GETPID)
}

pub unsafe fn sys_share_test_page(addr: u64) -> u64 {
    syscall1(SYS_SHARE_TEST_PAGE, addr)
}

pub unsafe fn sys_get_shared_test_page() -> u64 {
    syscall0(SYS_GET_SHARED_TEST_PAGE)
}