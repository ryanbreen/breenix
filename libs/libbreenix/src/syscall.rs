//! Raw syscall primitives for Breenix
//!
//! This module provides the low-level syscall interface using INT 0x80.
//! All syscalls follow the Linux AMD64 calling convention:
//! - Syscall number in RAX
//! - Arguments in RDI, RSI, RDX, R10, R8, R9
//! - Return value in RAX

use core::arch::asm;

/// Syscall numbers matching kernel/src/syscall/mod.rs
/// Breenix uses custom numbers for basic syscalls (0-6).
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;
    pub const GET_TIME: u64 = 4;
    pub const FORK: u64 = 5;
    pub const CLOSE: u64 = 6;         // Custom number (not Linux standard)
    pub const POLL: u64 = 7;          // Linux x86_64 poll
    pub const MMAP: u64 = 9;          // Linux x86_64 mmap
    pub const MPROTECT: u64 = 10;     // Linux x86_64 mprotect
    pub const MUNMAP: u64 = 11;       // Linux x86_64 munmap
    pub const BRK: u64 = 12;          // Linux x86_64 brk
    pub const SIGACTION: u64 = 13;    // Linux x86_64 rt_sigaction
    pub const SIGPROCMASK: u64 = 14;  // Linux x86_64 rt_sigprocmask
    pub const SIGRETURN: u64 = 15;    // Linux x86_64 rt_sigreturn
    pub const IOCTL: u64 = 16;        // Linux x86_64 ioctl
    pub const PIPE: u64 = 22;         // Linux x86_64 pipe
    pub const SELECT: u64 = 23;       // Linux x86_64 select
    pub const DUP: u64 = 32;          // Linux x86_64 dup
    pub const DUP2: u64 = 33;         // Linux x86_64 dup2
    pub const PAUSE: u64 = 34;        // Linux x86_64 pause
    pub const GETPID: u64 = 39;       // Linux x86_64 getpid
    pub const SOCKET: u64 = 41;       // Linux x86_64 socket
    pub const SENDTO: u64 = 44;       // Linux x86_64 sendto
    pub const RECVFROM: u64 = 45;     // Linux x86_64 recvfrom
    pub const BIND: u64 = 49;         // Linux x86_64 bind
    pub const EXEC: u64 = 59;         // Linux x86_64 execve
    pub const WAIT4: u64 = 61;        // Linux x86_64 wait4/waitpid
    pub const KILL: u64 = 62;         // Linux x86_64 kill
    pub const FCNTL: u64 = 72;        // Linux x86_64 fcntl
    pub const GETTID: u64 = 186;      // Linux x86_64 gettid
    pub const CLOCK_GETTIME: u64 = 228; // Linux x86_64 clock_gettime
    pub const OPEN: u64 = 257;        // Breenix: new filesystem syscall
    pub const LSEEK: u64 = 258;       // Breenix: new filesystem syscall
    pub const FSTAT: u64 = 259;       // Breenix: new filesystem syscall
    pub const GETDENTS64: u64 = 260;  // Breenix: directory listing syscall
    pub const UNLINK: u64 = 87;       // Linux x86_64 unlink
    pub const PIPE2: u64 = 293;       // Linux x86_64 pipe2
}

/// Raw syscall functions - use higher-level wrappers when possible
pub mod raw {
    use super::*;

    #[inline(always)]
    pub unsafe fn syscall0(num: u64) -> u64 {
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
    pub unsafe fn syscall1(num: u64, arg1: u64) -> u64 {
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
    pub unsafe fn syscall2(num: u64, arg1: u64, arg2: u64) -> u64 {
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
    pub unsafe fn syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
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
    pub unsafe fn syscall4(num: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> u64 {
        let ret: u64;
        asm!(
            "int 0x80",
            in("rax") num,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            lateout("rax") ret,
            options(nostack, preserves_flags),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall5(num: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> u64 {
        let ret: u64;
        asm!(
            "int 0x80",
            in("rax") num,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            lateout("rax") ret,
            options(nostack, preserves_flags),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall6(
        num: u64,
        arg1: u64,
        arg2: u64,
        arg3: u64,
        arg4: u64,
        arg5: u64,
        arg6: u64,
    ) -> u64 {
        let ret: u64;
        asm!(
            "int 0x80",
            in("rax") num,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            in("r9") arg6,
            lateout("rax") ret,
            options(nostack, preserves_flags),
        );
        ret
    }
}
