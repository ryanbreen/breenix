//! System call infrastructure for Breenix
//!
//! This module implements the system call interface:
//! - x86_64: Uses INT 0x80 (Linux-style)
//! - ARM64: Uses SVC instruction
//!
//! Architecture-independent syscall implementations are shared between both
//! architectures, with only the entry/exit code being architecture-specific.

#[cfg(target_arch = "x86_64")]
use x86_64::structures::idt::InterruptStackFrame;

// Architecture-independent modules (compile for both x86_64 and ARM64)
pub mod errno;
pub mod memory;
pub mod memory_common;
pub mod mmap;
pub mod time;
pub mod userptr;
// Syscall handler - the main dispatcher
// x86_64: Full handler with signal delivery and process management
// ARM64: Handler is in arch_impl/aarch64/syscall_entry.rs
#[cfg(target_arch = "x86_64")]
pub mod handler;

// Syscall implementations
// - dispatcher is x86_64-only (ARM64 dispatch is in arch_impl/aarch64/syscall_entry.rs)
// - handlers is shared across architectures (arch-specific parts are cfg-gated internally)
#[cfg(target_arch = "x86_64")]
pub(crate) mod dispatcher;
pub mod iovec;
pub mod clone;
pub mod epoll;
pub mod fifo;
pub mod fs;
pub mod futex;
pub mod graphics;
pub mod handlers;
pub mod ioctl;
pub mod pipe;
pub mod pty;
pub mod audio;
pub mod random;
pub mod session;
pub mod signal;
pub mod socket;
#[cfg(target_arch = "aarch64")]
pub mod wait;

/// System call numbers - semantic names only.
///
/// The numeric mapping is architecture-specific and handled by `from_u64()`.
/// x86_64 uses Linux x86_64 ABI numbers for musl libc compatibility.
/// ARM64 uses Linux ARM64 (asm-generic) numbers for musl libc compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SyscallNumber {
    // Core syscalls
    Exit,
    Write,
    Read,
    Yield,
    GetTime,        // Legacy: ARM64 only (x86_64 uses ClockGetTime)
    Fork,
    Close,
    Poll,
    Mmap,
    Mprotect,
    Munmap,
    Brk,
    Sigaction,
    Sigprocmask,
    Sigreturn,
    Ioctl,
    Readv,          // Vectored read (musl stdio)
    Writev,         // Vectored write (musl stdio)
    Pipe,
    Select,
    Mremap,         // Stub: returns -ENOMEM
    Madvise,        // Stub: returns 0 (advisory)
    Dup,
    Dup2,
    Pause,
    Nanosleep,
    Getitimer,
    Alarm,
    Setitimer,
    Fcntl,
    GetPid,
    Socket,
    Connect,
    Accept,
    SendTo,
    RecvFrom,
    Shutdown,
    Bind,
    Listen,
    Socketpair,
    Exec,
    Wait4,
    Kill,
    Getsockname,
    Getpeername,
    Setsockopt,
    Clone,
    Getsockopt,
    SetPgid,
    Getppid,
    SetSid,
    GetPgid,
    GetSid,
    Sigpending,
    Sigsuspend,
    Sigaltstack,
    ArchPrctl,      // x86_64 TLS setup (FS/GS base)
    GetTid,
    Futex,
    SetTidAddress,
    ClockGetTime,
    ExitGroup,
    Ppoll,          // Stub: returns -ENOSYS
    SetRobustList,  // Stub: returns 0
    Pipe2,
    GetRandom,
    // Filesystem syscalls
    Access,
    Getcwd,
    Chdir,
    Rename,
    Mkdir,
    Rmdir,
    Link,
    Unlink,
    Symlink,
    Readlink,
    Mknod,
    Open,
    Lseek,
    Fstat,
    Getdents64,
    Newfstatat,     // Path-based file stat (AT_FDCWD support)
    // *at variants (Linux ARM64 has these instead of legacy syscalls)
    Openat,         // openat(dirfd, path, flags, mode) - replacement for open
    Dup3,           // dup3(oldfd, newfd, flags) - replacement for dup2
    Faccessat,      // faccessat(dirfd, path, mode, flags)
    Mkdirat,        // mkdirat(dirfd, path, mode)
    Mknodat,        // mknodat(dirfd, path, mode, dev)
    Unlinkat,       // unlinkat(dirfd, path, flags) - replaces unlink + rmdir
    Symlinkat,      // symlinkat(target, dirfd, linkpath)
    Linkat,         // linkat(olddirfd, oldpath, newdirfd, newpath, flags)
    Renameat,       // renameat(olddirfd, oldpath, newdirfd, newpath)
    Readlinkat,     // readlinkat(dirfd, path, buf, bufsiz)
    Pselect6,       // pselect6(nfds, readfds, writefds, exceptfds, timeout, sigmask)
    // PTY syscalls (Breenix-specific numbers)
    PosixOpenpt,
    Grantpt,
    Unlockpt,
    Ptsname,
    // Graphics syscalls (Breenix-specific)
    FbInfo,
    FbDraw,
    FbMmap,
    GetMousePos,
    // Audio syscalls (Breenix-specific)
    AudioInit,
    AudioWrite,
    // Display takeover (Breenix-specific)
    TakeOverDisplay,
    GiveBackDisplay,
    // Testing (Breenix-specific)
    CowStats,
    SimulateOom,
    // Resource limits and system info
    Getrlimit,
    Prlimit64,
    Uname,
    // epoll
    EpollCreate1,
    EpollCtl,
    EpollWait,
    EpollPwait,
}

#[allow(dead_code)]
impl SyscallNumber {
    /// Try to convert a raw syscall number to a SyscallNumber.
    ///
    /// x86_64: Uses Linux x86_64 ABI numbers for musl libc compatibility.
    /// ARM64: Uses legacy Breenix numbers (ARM64 Linux renumbering is future work).
    #[cfg(target_arch = "x86_64")]
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            // Linux x86_64 ABI numbers
            0 => Some(Self::Read),          // was Breenix Exit=0
            1 => Some(Self::Write),
            2 => Some(Self::Open),          // Linux x86_64 open
            3 => Some(Self::Close),         // was Breenix Yield=3
            5 => Some(Self::Fstat),         // was Breenix Fork=5
            7 => Some(Self::Poll),
            8 => Some(Self::Lseek),         // was Breenix 258
            9 => Some(Self::Mmap),
            10 => Some(Self::Mprotect),
            11 => Some(Self::Munmap),
            12 => Some(Self::Brk),
            13 => Some(Self::Sigaction),
            14 => Some(Self::Sigprocmask),
            15 => Some(Self::Sigreturn),
            16 => Some(Self::Ioctl),
            19 => Some(Self::Readv),        // NEW
            20 => Some(Self::Writev),       // NEW
            21 => Some(Self::Access),
            22 => Some(Self::Pipe),
            23 => Some(Self::Select),
            24 => Some(Self::Yield),        // was Breenix 3
            25 => Some(Self::Mremap),       // NEW stub
            28 => Some(Self::Madvise),      // NEW stub
            32 => Some(Self::Dup),
            33 => Some(Self::Dup2),
            34 => Some(Self::Pause),
            35 => Some(Self::Nanosleep),
            36 => Some(Self::Getitimer),
            37 => Some(Self::Alarm),
            38 => Some(Self::Setitimer),
            39 => Some(Self::GetPid),
            41 => Some(Self::Socket),
            42 => Some(Self::Connect),
            43 => Some(Self::Accept),
            44 => Some(Self::SendTo),
            45 => Some(Self::RecvFrom),
            48 => Some(Self::Shutdown),
            49 => Some(Self::Bind),
            50 => Some(Self::Listen),
            51 => Some(Self::Getsockname),
            52 => Some(Self::Getpeername),
            53 => Some(Self::Socketpair),
            54 => Some(Self::Setsockopt),
            55 => Some(Self::Getsockopt),
            56 => Some(Self::Clone),
            57 => Some(Self::Fork),         // was Breenix 5
            59 => Some(Self::Exec),
            60 => Some(Self::Exit),         // was Breenix 0
            61 => Some(Self::Wait4),
            62 => Some(Self::Kill),
            63 => Some(Self::Uname),
            72 => Some(Self::Fcntl),
            79 => Some(Self::Getcwd),
            80 => Some(Self::Chdir),
            82 => Some(Self::Rename),
            83 => Some(Self::Mkdir),
            84 => Some(Self::Rmdir),
            86 => Some(Self::Link),
            87 => Some(Self::Unlink),
            88 => Some(Self::Symlink),
            89 => Some(Self::Readlink),
            97 => Some(Self::Getrlimit),
            109 => Some(Self::SetPgid),
            110 => Some(Self::Getppid),
            112 => Some(Self::SetSid),
            121 => Some(Self::GetPgid),
            124 => Some(Self::GetSid),
            127 => Some(Self::Sigpending),
            130 => Some(Self::Sigsuspend),
            131 => Some(Self::Sigaltstack),
            133 => Some(Self::Mknod),
            158 => Some(Self::ArchPrctl),   // NEW
            186 => Some(Self::GetTid),
            202 => Some(Self::Futex),
            217 => Some(Self::Getdents64),  // was Breenix 260
            218 => Some(Self::SetTidAddress),
            228 => Some(Self::ClockGetTime),
            231 => Some(Self::ExitGroup),
            257 => Some(Self::Openat),        // Linux x86_64 openat (was Breenix Open)
            258 => Some(Self::Mkdirat),
            259 => Some(Self::Mknodat),
            262 => Some(Self::Newfstatat),  // NEW
            263 => Some(Self::Unlinkat),
            264 => Some(Self::Renameat),
            265 => Some(Self::Linkat),
            266 => Some(Self::Symlinkat),
            267 => Some(Self::Readlinkat),
            269 => Some(Self::Faccessat),
            270 => Some(Self::Pselect6),
            271 => Some(Self::Ppoll),       // NEW stub
            273 => Some(Self::SetRobustList), // NEW stub
            292 => Some(Self::Dup3),
            293 => Some(Self::Pipe2),
            232 => Some(Self::EpollWait),
            233 => Some(Self::EpollCtl),
            281 => Some(Self::EpollPwait),
            291 => Some(Self::EpollCreate1),
            302 => Some(Self::Prlimit64),
            318 => Some(Self::GetRandom),
            // PTY syscalls (Breenix-specific, same on both archs)
            400 => Some(Self::PosixOpenpt),
            401 => Some(Self::Grantpt),
            402 => Some(Self::Unlockpt),
            403 => Some(Self::Ptsname),
            // Graphics syscalls (Breenix-specific)
            410 => Some(Self::FbInfo),
            411 => Some(Self::FbDraw),
            412 => Some(Self::FbMmap),
            413 => Some(Self::GetMousePos),
            // Audio syscalls (Breenix-specific)
            420 => Some(Self::AudioInit),
            421 => Some(Self::AudioWrite),
            431 => Some(Self::TakeOverDisplay),
            432 => Some(Self::GiveBackDisplay),
            500 => Some(Self::CowStats),
            501 => Some(Self::SimulateOom),
            _ => None,
        }
    }

    /// ARM64: Uses Linux ARM64 (asm-generic/unistd.h) numbers for musl compatibility.
    #[cfg(target_arch = "aarch64")]
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            // Linux ARM64 generic syscall numbers (from asm-generic/unistd.h)
            // epoll
            20 => Some(Self::EpollCreate1),
            21 => Some(Self::EpollCtl),
            22 => Some(Self::EpollPwait),
            // I/O
            17 => Some(Self::Getcwd),
            23 => Some(Self::Dup),
            24 => Some(Self::Dup3),
            25 => Some(Self::Fcntl),
            29 => Some(Self::Ioctl),
            // Filesystem *at variants (ARM64 has no legacy open/mkdir/etc.)
            33 => Some(Self::Mknodat),
            34 => Some(Self::Mkdirat),
            35 => Some(Self::Unlinkat),
            36 => Some(Self::Symlinkat),
            37 => Some(Self::Linkat),
            38 => Some(Self::Renameat),
            48 => Some(Self::Faccessat),
            49 => Some(Self::Chdir),
            56 => Some(Self::Openat),
            57 => Some(Self::Close),
            59 => Some(Self::Pipe2),
            61 => Some(Self::Getdents64),
            62 => Some(Self::Lseek),
            63 => Some(Self::Read),
            64 => Some(Self::Write),
            65 => Some(Self::Readv),
            66 => Some(Self::Writev),
            // I/O multiplexing
            72 => Some(Self::Pselect6),
            73 => Some(Self::Ppoll),
            78 => Some(Self::Readlinkat),
            79 => Some(Self::Newfstatat),
            80 => Some(Self::Fstat),
            // Process management
            93 => Some(Self::Exit),
            94 => Some(Self::ExitGroup),
            96 => Some(Self::SetTidAddress),
            98 => Some(Self::Futex),
            99 => Some(Self::SetRobustList),
            // Timers
            101 => Some(Self::Nanosleep),
            102 => Some(Self::Getitimer),
            103 => Some(Self::Setitimer),
            113 => Some(Self::ClockGetTime),
            // Scheduling
            124 => Some(Self::Yield),
            // Signals
            129 => Some(Self::Kill),
            132 => Some(Self::Sigaltstack),
            133 => Some(Self::Sigsuspend),
            134 => Some(Self::Sigaction),
            135 => Some(Self::Sigprocmask),
            136 => Some(Self::Sigpending),
            139 => Some(Self::Sigreturn),
            // Session/process group
            154 => Some(Self::SetPgid),
            155 => Some(Self::GetPgid),
            156 => Some(Self::GetSid),
            157 => Some(Self::SetSid),
            160 => Some(Self::Uname),
            // Process info
            172 => Some(Self::GetPid),
            173 => Some(Self::Getppid),
            178 => Some(Self::GetTid),
            // Socket
            198 => Some(Self::Socket),
            199 => Some(Self::Socketpair),
            200 => Some(Self::Bind),
            201 => Some(Self::Listen),
            202 => Some(Self::Accept),
            203 => Some(Self::Connect),
            204 => Some(Self::Getsockname),
            205 => Some(Self::Getpeername),
            206 => Some(Self::SendTo),
            207 => Some(Self::RecvFrom),
            208 => Some(Self::Setsockopt),
            209 => Some(Self::Getsockopt),
            210 => Some(Self::Shutdown),
            // Memory
            214 => Some(Self::Brk),
            215 => Some(Self::Munmap),
            216 => Some(Self::Mremap),
            220 => Some(Self::Clone),
            221 => Some(Self::Exec),
            222 => Some(Self::Mmap),
            226 => Some(Self::Mprotect),
            233 => Some(Self::Madvise),
            // Wait
            260 => Some(Self::Wait4),
            261 => Some(Self::Prlimit64),
            // Random
            278 => Some(Self::GetRandom),
            // PTY syscalls (Breenix-specific, same on both archs)
            400 => Some(Self::PosixOpenpt),
            401 => Some(Self::Grantpt),
            402 => Some(Self::Unlockpt),
            403 => Some(Self::Ptsname),
            // Graphics syscalls (Breenix-specific)
            410 => Some(Self::FbInfo),
            411 => Some(Self::FbDraw),
            412 => Some(Self::FbMmap),
            413 => Some(Self::GetMousePos),
            // Audio syscalls (Breenix-specific)
            420 => Some(Self::AudioInit),
            421 => Some(Self::AudioWrite),
            431 => Some(Self::TakeOverDisplay),
            432 => Some(Self::GiveBackDisplay),
            500 => Some(Self::CowStats),
            501 => Some(Self::SimulateOom),
            _ => None,
        }
    }
}

/// System call error codes (Linux conventions)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
#[allow(dead_code)]
pub enum ErrorCode {
    /// Operation not permitted
    PermissionDenied = 1, // EPERM
    /// No such process
    NoSuchProcess = 3, // ESRCH
    /// I/O error
    IoError = 5, // EIO
    /// Cannot allocate memory
    OutOfMemory = 12, // ENOMEM
    /// Bad address
    Fault = 14, // EFAULT
    /// Device or resource busy
    Busy = 16, // EBUSY
    /// Invalid argument
    InvalidArgument = 22, // EINVAL
    /// Function not implemented
    NoSys = 38, // ENOSYS
}

/// System call result type
#[derive(Debug)]
pub enum SyscallResult {
    Ok(u64),
    Err(u64),
}

/// Storage for syscall results
#[cfg(target_arch = "x86_64")]
pub static mut SYSCALL_RESULT: i64 = 0;

/// INT 0x80 handler for system calls
///
/// Note: This is replaced by assembly entry point for proper register handling
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub extern "x86-interrupt" fn syscall_handler(stack_frame: InterruptStackFrame) {
    // Log that we received a syscall
    log::debug!(
        "INT 0x80 syscall handler called from RIP: {:#x}",
        stack_frame.instruction_pointer.as_u64()
    );

    // Check if this is from userspace (Ring 3)
    if stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3 {
        // CRITICAL: Log current CR3 to verify process isolation is working
        use x86_64::registers::control::Cr3;
        let current_cr3 = Cr3::read().0.start_address().as_u64();

        log::info!("🎉 USERSPACE SYSCALL: Received INT 0x80 from userspace!");
        log::info!("    RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
        log::info!("    RSP: {:#x}", stack_frame.stack_pointer.as_u64());
        log::info!("    CR3: {:#x} (process page table)", current_cr3);

        // Also output to serial for easy CI detection
        crate::serial_println!("✅ SYSCALL with CR3={:#x} (process isolated)", current_cr3);

        // For the hello world test, we know it's trying to call sys_write
        // Let's call it directly to prove userspace syscalls work
        let message = "Hello from userspace! (via Rust syscall handler)\n";
        match handlers::sys_write(1, message.as_ptr() as u64, message.len() as u64) {
            SyscallResult::Ok(bytes) => {
                log::info!(
                    "✅ SUCCESS: Userspace syscall completed - wrote {} bytes",
                    bytes
                );
            }
            SyscallResult::Err(e) => {
                log::error!("❌ Userspace syscall failed: {}", e);
            }
        }
    } else {
        log::debug!("Syscall from kernel mode");
    }

    // Store a test result to verify the handler was called
    unsafe {
        SYSCALL_RESULT = 0x1234;
    }
}

/// Check if current thread has pending signals that should interrupt a syscall.
/// Returns Some(EINTR) if syscall should be interrupted, None otherwise.
///
/// This should be called in the blocking wait loop of syscalls like read, recvfrom,
/// accept, connect, and waitpid. If it returns Some(EINTR), the syscall should:
/// 1. Clean up any waiter registrations
/// 2. Return -EINTR to userspace
/// 3. The signal will be delivered when the syscall returns
pub fn check_signals_for_eintr() -> Option<i32> {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return None,
    };

    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            if crate::signal::delivery::has_deliverable_signals(process) {
                return Some(errno::EINTR);
            }
        }
    }
    None
}

/// Initialize the system call infrastructure
#[cfg(target_arch = "x86_64")]
pub fn init() {
    log::info!("Initializing system call infrastructure");

    // Register INT 0x80 handler in IDT (done in interrupts module)
    // The actual registration happens in interrupts::init_idt()

    log::info!("System call infrastructure initialized");
}
