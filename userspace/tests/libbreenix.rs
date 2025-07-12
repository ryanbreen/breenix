// Breenix userspace system call library

use core::arch::asm;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
#[allow(dead_code)]
const SYS_READ: u64 = 2;
#[allow(dead_code)]
const SYS_YIELD: u64 = 3;
#[allow(dead_code)]
const SYS_GET_TIME: u64 = 4;
const SYS_FORK: u64 = 5;
const SYS_WAIT: u64 = 7;
#[allow(dead_code)]
const SYS_EXEC: u64 = 11;
#[allow(dead_code)]
const SYS_GETPID: u64 = 39;
#[allow(dead_code)]
const SYS_SPAWN: u64 = 57;
const SYS_WAITPID: u64 = 61;

// Inline assembly for INT 0x80 syscalls
#[inline(always)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub unsafe fn sys_exit(code: i32) -> ! {
    syscall1(SYS_EXIT, code as u64);
    unreachable!("exit should not return");
}

#[allow(dead_code)]
pub unsafe fn sys_write(fd: u64, buf: &[u8]) -> u64 {
    syscall3(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64)
}

#[allow(dead_code)]
pub unsafe fn sys_read(fd: u64, buf: &mut [u8]) -> u64 {
    syscall3(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64)
}

#[allow(dead_code)]
pub unsafe fn sys_yield() -> u64 {
    syscall0(SYS_YIELD)
}

#[allow(dead_code)]
pub unsafe fn sys_get_time() -> u64 {
    syscall0(SYS_GET_TIME)
}

#[allow(dead_code)]
pub unsafe fn sys_fork() -> u64 {
    syscall0(SYS_FORK)
}

#[allow(dead_code)]
pub unsafe fn sys_exec(path: &str, args: &str) -> u64 {
    syscall2(SYS_EXEC, path.as_ptr() as u64, args.as_ptr() as u64)
}

#[allow(dead_code)]
pub unsafe fn sys_getpid() -> u64 {
    syscall0(SYS_GETPID)
}

#[allow(dead_code)]
pub unsafe fn sys_spawn(path: &str, args: &str) -> u64 {
    syscall2(SYS_SPAWN, path.as_ptr() as u64, args.as_ptr() as u64)
}

// Wait options
#[allow(dead_code)]
pub const WNOHANG: u32 = 1;

#[allow(dead_code)]
pub unsafe fn sys_wait(status: *mut u32) -> i64 {
    syscall1(SYS_WAIT, status as u64) as i64
}

#[allow(dead_code)]
pub unsafe fn sys_waitpid(pid: i64, status: *mut u32, options: u32) -> i64 {
    syscall3(SYS_WAITPID, pid as u64, status as u64, options as u64) as i64
}

// C-style convenience functions
#[allow(dead_code)]
pub unsafe fn wait(status: *mut i32) -> i64 {
    sys_wait(status as *mut u32)
}

#[allow(dead_code)]
pub unsafe fn waitpid(pid: i64, status: *mut i32, options: i32) -> i64 {
    sys_waitpid(pid, status as *mut u32, options as u32)
}