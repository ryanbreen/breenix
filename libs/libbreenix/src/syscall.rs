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
/// ARM64: Uses Linux ARM64 (asm-generic/unistd.h) numbers for musl libc compatibility.
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
    pub const OPEN: u64 = 2;           // Linux x86_64 open
    pub const NEWFSTATAT: u64 = 262;
    pub const OPENAT: u64 = 257;
    pub const MKDIRAT: u64 = 258;
    pub const MKNODAT: u64 = 259;
    pub const UNLINKAT: u64 = 263;
    pub const RENAMEAT: u64 = 264;
    pub const LINKAT: u64 = 265;
    pub const SYMLINKAT: u64 = 266;
    pub const READLINKAT: u64 = 267;
    pub const FACCESSAT: u64 = 269;
    pub const PSELECT6: u64 = 270;
    pub const PPOLL: u64 = 271;
    pub const SET_ROBUST_LIST: u64 = 273;
    pub const DUP3: u64 = 292;
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
    // Resource limits and system info
    pub const GETRLIMIT: u64 = 97;
    pub const PRLIMIT64: u64 = 302;
    pub const UNAME: u64 = 63;
    // epoll
    pub const EPOLL_WAIT: u64 = 232;
    pub const EPOLL_CTL: u64 = 233;
    pub const EPOLL_PWAIT: u64 = 281;
    pub const EPOLL_CREATE1: u64 = 291;
    // Testing syscalls (Breenix-specific)
    pub const COW_STATS: u64 = 500;
    pub const SIMULATE_OOM: u64 = 501;
}

#[cfg(target_arch = "aarch64")]
pub mod nr {
    // Linux ARM64 ABI numbers (asm-generic/unistd.h)
    // ARM64 Linux has NO legacy syscalls: use *at variants instead of
    // open/mkdir/rmdir/link/unlink/symlink/readlink/mknod/rename/access.
    // Use dup3 instead of dup2, pipe2 instead of pipe, clone instead of fork.

    // I/O
    pub const GETCWD: u64 = 17;
    pub const DUP: u64 = 23;
    pub const DUP3: u64 = 24;
    pub const FCNTL: u64 = 25;
    pub const IOCTL: u64 = 29;

    // Filesystem *at variants
    pub const MKNODAT: u64 = 33;
    pub const MKDIRAT: u64 = 34;
    pub const UNLINKAT: u64 = 35;
    pub const SYMLINKAT: u64 = 36;
    pub const LINKAT: u64 = 37;
    pub const RENAMEAT: u64 = 38;
    pub const FACCESSAT: u64 = 48;
    pub const CHDIR: u64 = 49;
    pub const OPENAT: u64 = 56;
    pub const CLOSE: u64 = 57;
    pub const PIPE2: u64 = 59;
    pub const GETDENTS64: u64 = 61;
    pub const LSEEK: u64 = 62;
    pub const READ: u64 = 63;
    pub const WRITE: u64 = 64;
    pub const READV: u64 = 65;
    pub const WRITEV: u64 = 66;

    // I/O multiplexing
    pub const PSELECT6: u64 = 72;
    pub const PPOLL: u64 = 73;
    pub const READLINKAT: u64 = 78;
    pub const NEWFSTATAT: u64 = 79;
    pub const FSTAT: u64 = 80;

    // Process management
    pub const EXIT: u64 = 93;
    pub const EXIT_GROUP: u64 = 94;
    pub const SET_TID_ADDRESS: u64 = 96;
    pub const FUTEX: u64 = 98;
    pub const SET_ROBUST_LIST: u64 = 99;

    // Timers
    pub const NANOSLEEP: u64 = 101;
    pub const GETITIMER: u64 = 102;
    pub const SETITIMER: u64 = 103;
    pub const CLOCK_GETTIME: u64 = 113;

    // Scheduling
    pub const YIELD: u64 = 124;

    // Signals
    pub const KILL: u64 = 129;
    pub const SIGALTSTACK: u64 = 132;
    pub const SIGSUSPEND: u64 = 133;
    pub const SIGACTION: u64 = 134;
    pub const SIGPROCMASK: u64 = 135;
    pub const SIGPENDING: u64 = 136;
    pub const SIGRETURN: u64 = 139;

    // Session/process group
    pub const SETPGID: u64 = 154;
    pub const GETPGID: u64 = 155;
    pub const GETSID: u64 = 156;
    pub const SETSID: u64 = 157;

    // Process info
    pub const GETPID: u64 = 172;
    pub const GETPPID: u64 = 173;
    pub const GETTID: u64 = 178;

    // Socket
    pub const SOCKET: u64 = 198;
    pub const SOCKETPAIR: u64 = 199;
    pub const BIND: u64 = 200;
    pub const LISTEN: u64 = 201;
    pub const ACCEPT: u64 = 202;
    pub const CONNECT: u64 = 203;
    pub const GETSOCKNAME: u64 = 204;
    pub const GETPEERNAME: u64 = 205;
    pub const SENDTO: u64 = 206;
    pub const RECVFROM: u64 = 207;
    pub const SETSOCKOPT: u64 = 208;
    pub const GETSOCKOPT: u64 = 209;
    pub const SHUTDOWN: u64 = 210;

    // Memory
    pub const BRK: u64 = 214;
    pub const MUNMAP: u64 = 215;
    pub const MREMAP: u64 = 216;
    pub const CLONE: u64 = 220;
    pub const EXEC: u64 = 221;
    pub const MMAP: u64 = 222;
    pub const MPROTECT: u64 = 226;
    pub const MADVISE: u64 = 233;

    // Wait
    pub const WAIT4: u64 = 260;

    // Random
    pub const GETRANDOM: u64 = 278;

    // NOTE: ARM64 Linux has NO legacy syscalls: open, dup2, pipe, fork,
    // access, rename, mkdir, rmdir, link, unlink, symlink, readlink,
    // mknod, select, poll, alarm, pause. Callers must use the *at variants
    // (openat, mkdirat, etc.) or modern replacements (dup3, pipe2, clone)
    // with the correct argument counts. See libbreenix/src/fs.rs for examples.

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
    // Resource limits and system info (ARM64 has no getrlimit; use prlimit64)
    pub const PRLIMIT64: u64 = 261;
    pub const UNAME: u64 = 160;
    // epoll
    pub const EPOLL_CREATE1: u64 = 20;
    pub const EPOLL_CTL: u64 = 21;
    pub const EPOLL_PWAIT: u64 = 22;
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
