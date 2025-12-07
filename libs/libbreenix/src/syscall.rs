//! Raw syscall primitives for Breenix
//!
//! This module provides the low-level syscall interface using INT 0x80.
//! All syscalls follow the Linux AMD64 calling convention:
//! - Syscall number in RAX
//! - Arguments in RDI, RSI, RDX, R10, R8, R9
//! - Return value in RAX

use core::arch::asm;

/// Syscall numbers matching kernel/src/syscall/mod.rs
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;
    pub const GET_TIME: u64 = 4;
    pub const FORK: u64 = 5;
    pub const MMAP: u64 = 9;       // Linux x86_64 mmap
    pub const MUNMAP: u64 = 11;    // Linux x86_64 munmap
    pub const BRK: u64 = 12;
    pub const EXEC: u64 = 59;      // Linux x86_64 execve
    pub const GETPID: u64 = 39;
    pub const GETTID: u64 = 186;
    pub const CLOCK_GETTIME: u64 = 228;
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
