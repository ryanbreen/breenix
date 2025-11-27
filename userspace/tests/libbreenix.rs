//! Breenix userspace system call library

use core::arch::asm;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_READ: u64 = 2;
const SYS_YIELD: u64 = 3;
const SYS_GET_TIME: u64 = 4;
const SYS_FORK: u64 = 5;
const SYS_EXEC: u64 = 11;
const SYS_GETPID: u64 = 39;
const SYS_CLOCK_GETTIME: u64 = 228;

// Clock IDs (Linux conventions)
pub const CLOCK_REALTIME: u32 = 0;
pub const CLOCK_MONOTONIC: u32 = 1;

/// Timespec structure for clock_gettime
#[repr(C)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

// Inline assembly for INT 0x80 syscalls
// NOTE: INT 0x80 may clobber argument registers - use inlateout to force the
// compiler to actually emit MOV instructions and not assume register values
// persist across syscalls.
#[inline(always)]
unsafe fn syscall0(num: u64) -> u64 {
    let ret: u64;
    asm!(
        "int 0x80",
        inlateout("rax") num => ret,
    );
    ret
}

#[inline(always)]
unsafe fn syscall1(num: u64, arg1: u64) -> u64 {
    let ret: u64;
    asm!(
        "int 0x80",
        inlateout("rax") num => ret,
        inlateout("rdi") arg1 => _,
    );
    ret
}

#[inline(always)]
unsafe fn syscall2(num: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    asm!(
        "int 0x80",
        inlateout("rax") num => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    asm!(
        "int 0x80",
        inlateout("rax") num => ret,
        inlateout("rdi") arg1 => _,
        inlateout("rsi") arg2 => _,
        inlateout("rdx") arg3 => _,
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

/// Get clock time (clock_gettime syscall)
/// Returns 0 on success, negative error code on failure
pub unsafe fn sys_clock_gettime(clock_id: u32, ts: &mut Timespec) -> i64 {
    syscall2(SYS_CLOCK_GETTIME, clock_id as u64, ts as *mut Timespec as u64) as i64
}