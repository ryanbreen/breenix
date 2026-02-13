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
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;
    pub const GET_TIME: u64 = 4;
    pub const FORK: u64 = 5;
    pub const CLOSE: u64 = 6;        // Custom number (not Linux standard)
    pub const POLL: u64 = 7;          // Linux x86_64 poll
    pub const MMAP: u64 = 9;         // Linux x86_64 mmap
    pub const MPROTECT: u64 = 10;    // Linux x86_64 mprotect
    pub const MUNMAP: u64 = 11;      // Linux x86_64 munmap
    pub const BRK: u64 = 12;
    pub const SIGACTION: u64 = 13;   // Linux x86_64 rt_sigaction
    pub const SIGPROCMASK: u64 = 14; // Linux x86_64 rt_sigprocmask
    pub const SIGRETURN: u64 = 15;   // Linux x86_64 rt_sigreturn
    pub const IOCTL: u64 = 16;       // Linux x86_64 ioctl
    pub const ACCESS: u64 = 21;      // Linux x86_64 access
    pub const PIPE: u64 = 22;        // Linux x86_64 pipe
    pub const GETCWD: u64 = 79;      // Linux x86_64 getcwd
    pub const CHDIR: u64 = 80;       // Linux x86_64 chdir
    pub const SELECT: u64 = 23;       // Linux x86_64 select
    pub const PIPE2: u64 = 293;       // Linux x86_64 pipe2
    pub const DUP: u64 = 32;          // Linux x86_64 dup
    pub const DUP2: u64 = 33;         // Linux x86_64 dup2
    pub const PAUSE: u64 = 34;        // Linux x86_64 pause
    pub const NANOSLEEP: u64 = 35;    // Linux x86_64 nanosleep
    pub const GETITIMER: u64 = 36;    // Linux x86_64 getitimer
    pub const ALARM: u64 = 37;        // Linux x86_64 alarm
    pub const SETITIMER: u64 = 38;    // Linux x86_64 setitimer
    pub const GETPID: u64 = 39;
    pub const FCNTL: u64 = 72;        // Linux x86_64 fcntl
    pub const SOCKET: u64 = 41;
    pub const CONNECT: u64 = 42;
    pub const ACCEPT: u64 = 43;
    pub const SENDTO: u64 = 44;
    pub const RECVFROM: u64 = 45;
    pub const SHUTDOWN: u64 = 48;
    pub const BIND: u64 = 49;
    pub const LISTEN: u64 = 50;
    pub const GETSOCKNAME: u64 = 51; // Linux x86_64 getsockname
    pub const GETPEERNAME: u64 = 52; // Linux x86_64 getpeername
    pub const SOCKETPAIR: u64 = 53;  // Linux x86_64 socketpair
    pub const SETSOCKOPT: u64 = 54;  // Linux x86_64 setsockopt
    pub const GETSOCKOPT: u64 = 55;  // Linux x86_64 getsockopt
    pub const EXEC: u64 = 59;        // Linux x86_64 execve
    pub const WAIT4: u64 = 61;       // Linux x86_64 wait4/waitpid
    pub const KILL: u64 = 62;        // Linux x86_64 kill
    pub const SETPGID: u64 = 109;    // Linux x86_64 setpgid
    pub const GETPPID: u64 = 110;    // Linux x86_64 getppid
    pub const SETSID: u64 = 112;     // Linux x86_64 setsid
    pub const GETPGID: u64 = 121;    // Linux x86_64 getpgid
    pub const GETSID: u64 = 124;     // Linux x86_64 getsid
    pub const SIGPENDING: u64 = 127; // Linux x86_64 rt_sigpending
    pub const SIGSUSPEND: u64 = 130; // Linux x86_64 rt_sigsuspend
    pub const SIGALTSTACK: u64 = 131; // Linux x86_64 sigaltstack
    pub const RENAME: u64 = 82;       // Linux x86_64 rename
    pub const MKDIR: u64 = 83;        // Linux x86_64 mkdir
    pub const RMDIR: u64 = 84;        // Linux x86_64 rmdir
    pub const LINK: u64 = 86;         // Linux x86_64 link (hard links)
    pub const UNLINK: u64 = 87;       // Linux x86_64 unlink
    pub const SYMLINK: u64 = 88;      // Linux x86_64 symlink
    pub const READLINK: u64 = 89;     // Linux x86_64 readlink
    pub const MKNOD: u64 = 133;       // Linux x86_64 mknod (used for mkfifo)
    pub const GETTID: u64 = 186;
    pub const SET_TID_ADDRESS: u64 = 218; // Linux x86_64 set_tid_address
    pub const CLOCK_GETTIME: u64 = 228;
    pub const EXIT_GROUP: u64 = 231; // Linux x86_64 exit_group
    pub const OPEN: u64 = 257;        // Breenix: filesystem open syscall
    pub const LSEEK: u64 = 258;       // Breenix: filesystem lseek syscall
    pub const FSTAT: u64 = 259;       // Breenix: filesystem fstat syscall
    pub const GETDENTS64: u64 = 260;  // Breenix: directory listing syscall
    // Graphics syscalls (Breenix-specific)
    pub const FBINFO: u64 = 410;       // Breenix: get framebuffer info
    pub const FBDRAW: u64 = 411;       // Breenix: draw to framebuffer
    pub const FBMMAP: u64 = 412;       // Breenix: mmap framebuffer into userspace
    pub const GET_MOUSE_POS: u64 = 413; // Breenix: get mouse cursor position
    // Audio syscalls (Breenix-specific)
    pub const AUDIO_INIT: u64 = 420;   // Breenix: initialize audio stream
    pub const AUDIO_WRITE: u64 = 421;  // Breenix: write PCM data to audio device
    pub const CLONE: u64 = 56;           // Linux x86_64 clone
    pub const FUTEX: u64 = 202;          // Linux x86_64 futex
    pub const GETRANDOM: u64 = 318;    // Linux x86_64 getrandom
    // Display takeover (Breenix-specific)
    pub const TAKE_OVER_DISPLAY: u64 = 431; // Breenix: userspace takes over display
    // Testing syscalls (Breenix-specific)
    pub const COW_STATS: u64 = 500;   // Breenix: get CoW statistics (for testing)
    pub const SIMULATE_OOM: u64 = 501; // Breenix: enable/disable OOM simulation (for testing)
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
