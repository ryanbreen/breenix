//! Raw syscall primitives for Breenix
//!
//! This module provides the low-level syscall interface.
//!
//! x86_64: Uses INT 0x80 with Linux AMD64 calling convention:
//! - Syscall number in RAX
//! - Arguments in RDI, RSI, RDX, R10, R8, R9
//! - Return value in RAX
//!
//! ARM64: Uses SVC #0 with Linux ARM64 calling convention:
//! - Syscall number in X8
//! - Arguments in X0-X5
//! - Return value in X0

use core::arch::asm;

/// Syscall numbers matching kernel/src/syscall/mod.rs
///
/// x86_64: Uses Linux x86_64 ABI numbers for musl libc compatibility.
/// ARM64: Uses legacy Breenix numbers (unchanged).
#[cfg(target_arch = "x86_64")]
pub mod nr {
    // Linux x86_64 ABI numbers
    pub const READ: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const CLOSE: u64 = 3;
    pub const FSTAT: u64 = 5;
    pub const POLL: u64 = 7;
    pub const LSEEK: u64 = 8;
    pub const MMAP: u64 = 9;
    pub const MPROTECT: u64 = 10;
    pub const MUNMAP: u64 = 11;
    pub const BRK: u64 = 12;
    pub const SIGACTION: u64 = 13;
    pub const SIGPROCMASK: u64 = 14;
    pub const SIGRETURN: u64 = 15;
    pub const IOCTL: u64 = 16;
    pub const READV: u64 = 19;
    pub const WRITEV: u64 = 20;
    pub const ACCESS: u64 = 21;
    pub const PIPE: u64 = 22;
    pub const SELECT: u64 = 23;
    pub const YIELD: u64 = 24;
    pub const MREMAP: u64 = 25;
    pub const MADVISE: u64 = 28;
    pub const DUP: u64 = 32;
    pub const DUP2: u64 = 33;
    pub const PAUSE: u64 = 34;
    pub const NANOSLEEP: u64 = 35;
    pub const GETITIMER: u64 = 36;
    pub const ALARM: u64 = 37;
    pub const SETITIMER: u64 = 38;
    pub const GETPID: u64 = 39;
    pub const SOCKET: u64 = 41;
    pub const CONNECT: u64 = 42;
    pub const ACCEPT: u64 = 43;
    pub const SENDTO: u64 = 44;
    pub const RECVFROM: u64 = 45;
    pub const SHUTDOWN: u64 = 48;
    pub const BIND: u64 = 49;
    pub const LISTEN: u64 = 50;
    pub const GETSOCKNAME: u64 = 51;
    pub const GETPEERNAME: u64 = 52;
    pub const SOCKETPAIR: u64 = 53;
    pub const SETSOCKOPT: u64 = 54;
    pub const GETSOCKOPT: u64 = 55;
    pub const CLONE: u64 = 56;
    pub const FORK: u64 = 57;
    pub const EXEC: u64 = 59;
    pub const EXIT: u64 = 60;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const FCNTL: u64 = 72;
    pub const GETCWD: u64 = 79;
    pub const CHDIR: u64 = 80;
    pub const RENAME: u64 = 82;
    pub const MKDIR: u64 = 83;
    pub const RMDIR: u64 = 84;
    pub const LINK: u64 = 86;
    pub const UNLINK: u64 = 87;
    pub const SYMLINK: u64 = 88;
    pub const READLINK: u64 = 89;
    pub const SETPGID: u64 = 109;
    pub const GETPPID: u64 = 110;
    pub const SETSID: u64 = 112;
    pub const GETPGID: u64 = 121;
    pub const GETSID: u64 = 124;
    pub const SIGPENDING: u64 = 127;
    pub const SIGSUSPEND: u64 = 130;
    pub const SIGALTSTACK: u64 = 131;
    pub const MKNOD: u64 = 133;
    pub const ARCH_PRCTL: u64 = 158;
    pub const GETTID: u64 = 186;
    pub const FUTEX: u64 = 202;
    pub const GETDENTS64: u64 = 217;
    pub const SET_TID_ADDRESS: u64 = 218;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const EXIT_GROUP: u64 = 231;
    pub const OPEN: u64 = 257;
    pub const NEWFSTATAT: u64 = 262;
    pub const PPOLL: u64 = 271;
    pub const SET_ROBUST_LIST: u64 = 273;
    pub const PIPE2: u64 = 293;
    pub const GETRANDOM: u64 = 318;
    // PTY syscalls (Breenix-specific, same on both architectures)
    pub const POSIX_OPENPT: u64 = 400;
    pub const GRANTPT: u64 = 401;
    pub const UNLOCKPT: u64 = 402;
    pub const PTSNAME: u64 = 403;
    // Graphics syscalls (Breenix-specific)
    pub const FBINFO: u64 = 410;
    pub const FBDRAW: u64 = 411;
    pub const FBMMAP: u64 = 412;
    pub const GET_MOUSE_POS: u64 = 413;
    // Audio syscalls (Breenix-specific)
    pub const AUDIO_INIT: u64 = 420;
    pub const AUDIO_WRITE: u64 = 421;
    // Display takeover (Breenix-specific)
    pub const TAKE_OVER_DISPLAY: u64 = 431;
    pub const GIVE_BACK_DISPLAY: u64 = 432;
    // Testing syscalls (Breenix-specific)
    pub const COW_STATS: u64 = 500;
    pub const SIMULATE_OOM: u64 = 501;
}

#[cfg(target_arch = "aarch64")]
pub mod nr {
    // Legacy Breenix numbers (ARM64 Linux renumbering is a separate future effort)
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;
    pub const GET_TIME: u64 = 4;
    pub const FORK: u64 = 5;
    pub const CLOSE: u64 = 6;
    pub const POLL: u64 = 7;
    pub const MMAP: u64 = 9;
    pub const MPROTECT: u64 = 10;
    pub const MUNMAP: u64 = 11;
    pub const BRK: u64 = 12;
    pub const SIGACTION: u64 = 13;
    pub const SIGPROCMASK: u64 = 14;
    pub const SIGRETURN: u64 = 15;
    pub const IOCTL: u64 = 16;
    pub const READV: u64 = 19;
    pub const WRITEV: u64 = 20;
    pub const ACCESS: u64 = 21;
    pub const PIPE: u64 = 22;
    pub const SELECT: u64 = 23;
    pub const MREMAP: u64 = 25;
    pub const MADVISE: u64 = 28;
    pub const DUP: u64 = 32;
    pub const DUP2: u64 = 33;
    pub const PAUSE: u64 = 34;
    pub const NANOSLEEP: u64 = 35;
    pub const GETITIMER: u64 = 36;
    pub const ALARM: u64 = 37;
    pub const SETITIMER: u64 = 38;
    pub const GETPID: u64 = 39;
    pub const SOCKET: u64 = 41;
    pub const CONNECT: u64 = 42;
    pub const ACCEPT: u64 = 43;
    pub const SENDTO: u64 = 44;
    pub const RECVFROM: u64 = 45;
    pub const SHUTDOWN: u64 = 48;
    pub const BIND: u64 = 49;
    pub const LISTEN: u64 = 50;
    pub const GETSOCKNAME: u64 = 51;
    pub const GETPEERNAME: u64 = 52;
    pub const SOCKETPAIR: u64 = 53;
    pub const SETSOCKOPT: u64 = 54;
    pub const GETSOCKOPT: u64 = 55;
    pub const CLONE: u64 = 56;
    pub const EXEC: u64 = 59;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const FCNTL: u64 = 72;
    pub const GETCWD: u64 = 79;
    pub const CHDIR: u64 = 80;
    pub const RENAME: u64 = 82;
    pub const MKDIR: u64 = 83;
    pub const RMDIR: u64 = 84;
    pub const LINK: u64 = 86;
    pub const UNLINK: u64 = 87;
    pub const SYMLINK: u64 = 88;
    pub const READLINK: u64 = 89;
    pub const SETPGID: u64 = 109;
    pub const GETPPID: u64 = 110;
    pub const SETSID: u64 = 112;
    pub const GETPGID: u64 = 121;
    pub const GETSID: u64 = 124;
    pub const SIGPENDING: u64 = 127;
    pub const SIGSUSPEND: u64 = 130;
    pub const SIGALTSTACK: u64 = 131;
    pub const MKNOD: u64 = 133;
    pub const GETTID: u64 = 186;
    pub const FUTEX: u64 = 202;
    pub const SET_TID_ADDRESS: u64 = 218;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const EXIT_GROUP: u64 = 231;
    pub const OPEN: u64 = 257;
    pub const LSEEK: u64 = 258;
    pub const FSTAT: u64 = 259;
    pub const GETDENTS64: u64 = 260;
    pub const NEWFSTATAT: u64 = 262;
    pub const PPOLL: u64 = 271;
    pub const SET_ROBUST_LIST: u64 = 273;
    pub const PIPE2: u64 = 293;
    pub const GETRANDOM: u64 = 318;
    // PTY syscalls (Breenix-specific)
    pub const POSIX_OPENPT: u64 = 400;
    pub const GRANTPT: u64 = 401;
    pub const UNLOCKPT: u64 = 402;
    pub const PTSNAME: u64 = 403;
    // Graphics syscalls (Breenix-specific)
    pub const FBINFO: u64 = 410;
    pub const FBDRAW: u64 = 411;
    pub const FBMMAP: u64 = 412;
    pub const GET_MOUSE_POS: u64 = 413;
    // Audio syscalls (Breenix-specific)
    pub const AUDIO_INIT: u64 = 420;
    pub const AUDIO_WRITE: u64 = 421;
    // Display takeover (Breenix-specific)
    pub const TAKE_OVER_DISPLAY: u64 = 431;
    pub const GIVE_BACK_DISPLAY: u64 = 432;
    // Testing syscalls (Breenix-specific)
    pub const COW_STATS: u64 = 500;
    pub const SIMULATE_OOM: u64 = 501;
}

/// Raw syscall functions - use higher-level wrappers when possible
#[cfg(target_arch = "x86_64")]
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

/// ARM64 raw syscall functions
/// Uses SVC #0 with syscall number in X8, args in X0-X5, return in X0
#[cfg(target_arch = "aarch64")]
pub mod raw {
    use super::*;

    #[inline(always)]
    pub unsafe fn syscall0(num: u64) -> u64 {
        let ret: u64;
        asm!(
            "svc #0",
            in("x8") num,
            lateout("x0") ret,
            options(nostack),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall1(num: u64, arg1: u64) -> u64 {
        let ret: u64;
        asm!(
            "svc #0",
            in("x8") num,
            inlateout("x0") arg1 => ret,
            options(nostack),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall2(num: u64, arg1: u64, arg2: u64) -> u64 {
        let ret: u64;
        asm!(
            "svc #0",
            in("x8") num,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            options(nostack),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
        let ret: u64;
        asm!(
            "svc #0",
            in("x8") num,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            in("x2") arg3,
            options(nostack),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall4(num: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> u64 {
        let ret: u64;
        asm!(
            "svc #0",
            in("x8") num,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            in("x2") arg3,
            in("x3") arg4,
            options(nostack),
        );
        ret
    }

    #[inline(always)]
    pub unsafe fn syscall5(num: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> u64 {
        let ret: u64;
        asm!(
            "svc #0",
            in("x8") num,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            in("x2") arg3,
            in("x3") arg4,
            in("x4") arg5,
            options(nostack),
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
            "svc #0",
            in("x8") num,
            inlateout("x0") arg1 => ret,
            in("x1") arg2,
            in("x2") arg3,
            in("x3") arg4,
            in("x4") arg5,
            in("x5") arg6,
            options(nostack),
        );
        ret
    }
}
