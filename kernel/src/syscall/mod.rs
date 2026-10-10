//! System call infrastructure for Breenix
//!
//! This module implements the system call interface:
//! - x86_64: Uses SYSCALL (Linux AMD64 ABI), with INT 0x80 compatibility
//! - ARM64: Uses SVC instruction
//!
//! Architecture-independent syscall implementations are shared between both
//! architectures, with only the entry/exit code being architecture-specific.

// Architecture-independent modules (compile for both x86_64 and ARM64)
pub mod errno;
pub(crate) mod exec;
pub mod memory;
pub mod memory_advice;
pub mod memory_common;
pub mod mmap;
pub mod time;
pub mod clocks;
pub mod sleep;
pub mod userptr;
// Syscall handler - the main dispatcher
// x86_64: Full handler with signal delivery and process management
// ARM64: Handler is in arch_impl/aarch64/syscall_entry.rs
#[cfg(target_arch = "x86_64")]
pub mod handler;

// Syscall implementations
// - dispatcher is x86_64-only (ARM64 dispatch is in arch_impl/aarch64/syscall_entry.rs)
// - handlers is shared across architectures (arch-specific parts are cfg-gated internally)
pub mod affinity;
pub mod audio;
pub(crate) mod blocking_io;
#[cfg(feature = "boot_tests")]
pub mod blocking_io_oracle;
pub mod clone;
#[cfg(target_arch = "x86_64")]
pub(crate) mod dispatcher;
pub mod epoll;
pub mod fifo;
pub mod fs;
pub mod futex;
#[cfg(feature = "boot_tests")]
pub mod futex_oracle;
pub mod futex_timeout_record;
pub mod graphics;
pub mod handlers;
pub mod resource;
pub mod ioctl;
pub mod iovec;
pub mod metadata;
mod multiplex;
pub mod pipe;
pub mod priority;
pub mod pty;
pub mod random;
pub mod rusage;
pub mod session;
pub mod signal;
pub mod socket;
pub mod timers;
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
    GetTime, // Legacy: ARM64 only (x86_64 uses ClockGetTime)
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
    Readv,  // Vectored read (musl stdio)
    Writev, // Vectored write (musl stdio)
    Pipe,
    Select,
    Mremap,  // Stub: returns -ENOMEM
    Madvise, // Stub: returns 0 (advisory)
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
    Waitid,
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
    #[cfg(target_arch = "aarch64")]
    Sigtimedwait,
    Sigsuspend,
    Sigaltstack,
    ArchPrctl, // x86_64 TLS setup (FS/GS base)
    GetTid,
    Futex,
    SetTidAddress,
    ClockGetTime,
    ClockSetTime,
    ClockGetRes,
    ClockNanosleep,
    Gettimeofday,
    Time,
    ExitGroup,
    Ppoll,         // Stub: returns -ENOSYS
    SetRobustList, // Stub: returns 0
    Pipe2,
    GetRandom,
    // Filesystem syscalls
    Access,
    Getcwd,
    Chdir,
    Fchdir,
    Rename,
    Mkdir,
    Rmdir,
    Link,
    Unlink,
    Symlink,
    Readlink,
    Mknod,
    Chmod,
    Fchmod,
    Fchmodat,
    Chown,
    Fchown,
    Lchown,
    Fchownat,
    Open,
    Lseek,
    Fstat,
    Fstatfs,
    Getdents64,
    Newfstatat, // Path-based file stat (AT_FDCWD support)
    Fsync,
    Fdatasync,
    Sync,
    Statfs,
    Truncate,
    Ftruncate,
    // *at variants (Linux ARM64 has these instead of legacy syscalls)
    Openat,     // openat(dirfd, path, flags, mode) - replacement for open
    Dup3,       // dup3(oldfd, newfd, flags) - replacement for dup2
    Faccessat,  // faccessat(dirfd, path, mode, flags)
    Mkdirat,    // mkdirat(dirfd, path, mode)
    Mknodat,    // mknodat(dirfd, path, mode, dev)
    Unlinkat,   // unlinkat(dirfd, path, flags) - replaces unlink + rmdir
    Symlinkat,  // symlinkat(target, dirfd, linkpath)
    Linkat,     // linkat(olddirfd, oldpath, newdirfd, newpath, flags)
    Renameat,   // renameat(olddirfd, oldpath, newdirfd, newpath)
    Readlinkat, // readlinkat(dirfd, path, buf, bufsiz)
    Pselect6,   // pselect6(nfds, readfds, writefds, exceptfds, timeout, sigmask)
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
    // Process identity
    Getuid,
    Geteuid,
    Getgid,
    Getegid,
    Setuid,
    Setgid,
    Setreuid,
    Setregid,
    Setgroups,
    Getgroups,
    // Priorities and CPU usage
    Getpriority,
    Setpriority,
    Getrusage,
    Times,
    // File creation mask
    Umask,
    // Timestamps
    Utimensat,
    // Positional I/O
    Pread64,
    Pwrite64,
    // Process spawning (Breenix-specific) — avoids fork+exec overhead
    Spawn,
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
            0 => Some(Self::Read), // was Breenix Exit=0
            1 => Some(Self::Write),
            2 => Some(Self::Open),  // Linux x86_64 open
            3 => Some(Self::Close), // was Breenix Yield=3
            5 => Some(Self::Fstat), // was Breenix Fork=5
            7 => Some(Self::Poll),
            8 => Some(Self::Lseek), // was Breenix 258
            9 => Some(Self::Mmap),
            10 => Some(Self::Mprotect),
            11 => Some(Self::Munmap),
            12 => Some(Self::Brk),
            13 => Some(Self::Sigaction),
            14 => Some(Self::Sigprocmask),
            15 => Some(Self::Sigreturn),
            16 => Some(Self::Ioctl),
            19 => Some(Self::Readv),  // NEW
            20 => Some(Self::Writev), // NEW
            21 => Some(Self::Access),
            22 => Some(Self::Pipe),
            23 => Some(Self::Select),
            24 => Some(Self::Yield),   // was Breenix 3
            25 => Some(Self::Mremap),  // NEW stub
            28 => Some(Self::Madvise), // NEW stub
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
            57 => Some(Self::Fork), // was Breenix 5
            59 => Some(Self::Exec),
            60 => Some(Self::Exit), // was Breenix 0
            61 => Some(Self::Wait4),
            247 => Some(Self::Waitid),
            62 => Some(Self::Kill),
            63 => Some(Self::Uname),
            72 => Some(Self::Fcntl),
            162 => Some(Self::Sync),
            137 => Some(Self::Statfs),
            74 => Some(Self::Fsync),
            75 => Some(Self::Fdatasync),
            76 => Some(Self::Truncate),
            77 => Some(Self::Ftruncate),
            79 => Some(Self::Getcwd),
            80 => Some(Self::Chdir),
            81 => Some(Self::Fchdir),
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
            138 => Some(Self::Fstatfs),
            158 => Some(Self::ArchPrctl), // NEW
            186 => Some(Self::GetTid),
            202 => Some(Self::Futex),
            217 => Some(Self::Getdents64), // was Breenix 260
            218 => Some(Self::SetTidAddress),
            227 => Some(Self::ClockSetTime),
            228 => Some(Self::ClockGetTime),
            229 => Some(Self::ClockGetRes),
            230 => Some(Self::ClockNanosleep),
            96 => Some(Self::Gettimeofday),
            201 => Some(Self::Time),
            231 => Some(Self::ExitGroup),
            257 => Some(Self::Openat), // Linux x86_64 openat (was Breenix Open)
            258 => Some(Self::Mkdirat),
            259 => Some(Self::Mknodat),
            262 => Some(Self::Newfstatat), // NEW
            263 => Some(Self::Unlinkat),
            264 => Some(Self::Renameat),
            265 => Some(Self::Linkat),
            266 => Some(Self::Symlinkat),
            267 => Some(Self::Readlinkat),
            268 => Some(Self::Fchmodat),
            260 => Some(Self::Fchownat),
            90 => Some(Self::Chmod),
            91 => Some(Self::Fchmod),
            92 => Some(Self::Chown),
            93 => Some(Self::Fchown),
            94 => Some(Self::Lchown),
            116 => Some(Self::Setgroups),
            115 => Some(Self::Getgroups),
            269 => Some(Self::Faccessat),
            270 => Some(Self::Pselect6),
            271 => Some(Self::Ppoll),         // NEW stub
            273 => Some(Self::SetRobustList), // NEW stub
            292 => Some(Self::Dup3),
            293 => Some(Self::Pipe2),
            232 => Some(Self::EpollWait),
            233 => Some(Self::EpollCtl),
            281 => Some(Self::EpollPwait),
            291 => Some(Self::EpollCreate1),
            302 => Some(Self::Prlimit64),
            95 => Some(Self::Umask),
            102 => Some(Self::Getuid),
            104 => Some(Self::Getgid),
            105 => Some(Self::Setuid),
            106 => Some(Self::Setgid),
            113 => Some(Self::Setreuid),
            114 => Some(Self::Setregid),
            98 => Some(Self::Getrusage),
            100 => Some(Self::Times),
            140 => Some(Self::Getpriority),
            141 => Some(Self::Setpriority),
            107 => Some(Self::Geteuid),
            108 => Some(Self::Getegid),
            17 => Some(Self::Pread64),
            18 => Some(Self::Pwrite64),
            280 => Some(Self::Utimensat),
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
            // Process spawning (Breenix-specific)
            440 => Some(Self::Spawn),
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
            44 => Some(Self::Fstatfs),
            45 => Some(Self::Truncate),
            46 => Some(Self::Ftruncate),
            48 => Some(Self::Faccessat),
            52 => Some(Self::Fchmod),
            53 => Some(Self::Fchmodat),
            54 => Some(Self::Fchownat),
            55 => Some(Self::Fchown),
            159 => Some(Self::Setgroups),
            158 => Some(Self::Getgroups),
            49 => Some(Self::Chdir),
            50 => Some(Self::Fchdir),
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
            81 => Some(Self::Sync),
            43 => Some(Self::Statfs),
            82 => Some(Self::Fsync),
            83 => Some(Self::Fdatasync),
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
            112 => Some(Self::ClockSetTime),
            113 => Some(Self::ClockGetTime),
            114 => Some(Self::ClockGetRes),
            115 => Some(Self::ClockNanosleep),
            169 => Some(Self::Gettimeofday),
            // Scheduling
            124 => Some(Self::Yield),
            // Signals
            129 => Some(Self::Kill),
            132 => Some(Self::Sigaltstack),
            133 => Some(Self::Sigsuspend),
            134 => Some(Self::Sigaction),
            135 => Some(Self::Sigprocmask),
            136 => Some(Self::Sigpending),
            137 => Some(Self::Sigtimedwait),
            139 => Some(Self::Sigreturn),
            // Session/process group
            154 => Some(Self::SetPgid),
            155 => Some(Self::GetPgid),
            156 => Some(Self::GetSid),
            157 => Some(Self::SetSid),
            160 => Some(Self::Uname),
            163 => Some(Self::Getrlimit),
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
            95 => Some(Self::Waitid),
            261 => Some(Self::Prlimit64),
            // Positional I/O
            67 => Some(Self::Pread64),
            68 => Some(Self::Pwrite64),
            // Timestamps
            88 => Some(Self::Utimensat),
            // Process identity
            143 => Some(Self::Setregid),
            144 => Some(Self::Setgid),
            145 => Some(Self::Setreuid),
            146 => Some(Self::Setuid),
            140 => Some(Self::Setpriority),
            141 => Some(Self::Getpriority),
            153 => Some(Self::Times),
            165 => Some(Self::Getrusage),
            166 => Some(Self::Umask),
            174 => Some(Self::Getuid),
            175 => Some(Self::Geteuid),
            176 => Some(Self::Getgid),
            177 => Some(Self::Getegid),
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
            // Process spawning (Breenix-specific)
            440 => Some(Self::Spawn),
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
    /// Resource temporarily unavailable
    TryAgain = 11, // EAGAIN
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

/// A quiet wait has no signal, stop or CPU-limit work to serialize with
/// PROCESS_MANAGER. Publishers set the return-to-user admission hint. Some
/// waits reopen preemption, so protect the current-thread read briefly.
fn signal_wait_thread() -> Option<u64> {
    crate::arch_without_interrupts(|| {
        #[cfg(target_arch = "x86_64")]
        let thread = crate::per_cpu::current_thread();
        #[cfg(target_arch = "aarch64")]
        let thread = crate::per_cpu_aarch64::current_thread();
        let thread = thread?;
        thread.needs_user_return_check().then_some(thread.id)
    })
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
    let thread_id = signal_wait_thread()?;

    let manager_guard = crate::process::manager();
    let mut interrupted = false;
    if let Some(ref manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            if crate::signal::delivery::has_interrupting_signals(process) {
                interrupted = true;
            }
        }
    }
    drop(manager_guard);
    if interrupted {
        return Some(errno::EINTR);
    }
    None
}

/// Handle job-control stops without completing an interruptible wait. Only
/// caught or fatal signals end the wait; SIGCONT resumes its existing deadline
/// and temporary mask. Called with syscall preemption disabled and no PM guard.
pub fn check_signals_for_wait() -> Option<i32> {
    let tid = signal_wait_thread()?;
    loop {
        let mut guard = crate::process::manager();
        let (_, p) = guard.as_mut()?.find_process_by_thread_mut(tid)?;
        p.signals.collect_timer_signals(&p.itimers);
        if crate::signal::delivery::stop_pending_or_in_force(p) {
            drop(guard);
            crate::signal::delivery::hold_stopped_thread_on_syscall_return();
            continue;
        }
        let sig = p.signals.next_deliverable_signal()?;
        let action = p.signals.get_handler(sig);
        if action.is_handler() || crate::signal::delivery::fatal_exit_code(sig).is_some() {
            return Some(errno::EINTR);
        }
        p.signals.take(sig);
    }
}

/// `check_signals_for_eintr` for a wait that SA_RESTART resumes: blocking
/// read and write on pipes, FIFOs, sockets and terminals, wait4, accept, a
/// blocking FIFO open and F_SETLKW. Returns Some(EINTR) when the interrupting
/// signal will run a handler installed without SA_RESTART, and
/// Some(ERESTARTSYS) otherwise, which the syscall return path turns into a
/// re-execution of the syscall once any handler has run.
pub fn check_signals_for_restartable_wait() -> Option<i32> {
    let thread_id = signal_wait_thread()?;

    let manager_guard = crate::process::manager();
    let restarts = manager_guard.as_ref().and_then(|manager| {
        let (_pid, process) = manager.find_process_by_thread(thread_id)?;
        if crate::signal::delivery::has_interrupting_signals(process) {
            Some(process.signals.interruption_restarts())
        } else {
            None
        }
    });
    drop(manager_guard);
    match restarts? {
        true => Some(errno::ERESTARTSYS),
        false => Some(errno::EINTR),
    }
}

/// Match the IDT entry alias used when the kernel is linked in low memory.
#[cfg(target_arch = "x86_64")]
pub(crate) fn entry_address(address: u64) -> u64 {
    if (0x100000..=0x40000000).contains(&address) {
        crate::memory::layout::high_alias_from_low(address)
    } else {
        address
    }
}

/// Initialize the system call infrastructure.
#[cfg(target_arch = "x86_64")]
pub fn init() {
    log::info!("Initializing system call infrastructure");

    // MSRs are CPU-local. Every online x86 CPU must run this initialization
    // before entering userspace (currently x86 brings up only the BSP).
    use x86_64::registers::{
        model_specific::{Efer, EferFlags, LStar, SFMask, Star},
        rflags::RFlags,
    };
    extern "C" {
        fn syscall_instruction_entry();
    }
    unsafe {
        // GDT: kernel CS/SS = 0x08/0x10, user SS/CS = 0x2b/0x33.
        Star::write_raw(0x23, 0x08);
        LStar::write(x86_64::VirtAddr::new(entry_address(
            syscall_instruction_entry as u64,
        )));
        SFMask::write(
            RFlags::INTERRUPT_FLAG
                | RFlags::DIRECTION_FLAG
                | RFlags::TRAP_FLAG
                | RFlags::ALIGNMENT_CHECK
                | RFlags::NESTED_TASK,
        );
        Efer::update(|flags| flags.insert(EferFlags::SYSTEM_CALL_EXTENSIONS));
    }

    log::info!("System call infrastructure initialized");
}

/// msync has a guarded dispatch arm rather than an enum variant, allowing
/// the x86 Tier 1 dispatcher change to be committed independently.
#[cfg(target_arch = "x86_64")]
pub const MSYNC_SYSCALL_NUMBER: u64 = 26;
#[cfg(target_arch = "aarch64")]
pub const MSYNC_SYSCALL_NUMBER: u64 = 227;

/// Native Linux setrlimit number, dispatched alongside msync without an enum variant.
#[cfg(target_arch = "x86_64")]
pub const SETRLIMIT_SYSCALL_NUMBER: u64 = 160;
#[cfg(target_arch = "aarch64")]
pub const SETRLIMIT_SYSCALL_NUMBER: u64 = 164;

/// Native Linux rt_sigqueueinfo, tkill and tgkill numbers, dispatched like
/// msync without enum variants.
#[cfg(target_arch = "x86_64")]
pub const RT_SIGQUEUEINFO_SYSCALL_NUMBER: u64 = 129;
#[cfg(target_arch = "aarch64")]
pub const RT_SIGQUEUEINFO_SYSCALL_NUMBER: u64 = 138;
#[cfg(target_arch = "x86_64")]
pub const TKILL_SYSCALL_NUMBER: u64 = 200;
#[cfg(target_arch = "aarch64")]
pub const TKILL_SYSCALL_NUMBER: u64 = 130;
#[cfg(target_arch = "x86_64")]
pub const TGKILL_SYSCALL_NUMBER: u64 = 234;
#[cfg(target_arch = "aarch64")]
pub const TGKILL_SYSCALL_NUMBER: u64 = 131;

/// Native Linux numbers of the POSIX timer calls, sched_setaffinity,
/// sched_getaffinity and getcpu, dispatched like msync without enum variants.
#[cfg(target_arch = "x86_64")]
pub const TIMER_CREATE_SYSCALL_NUMBER: u64 = 222;
#[cfg(target_arch = "aarch64")]
pub const TIMER_CREATE_SYSCALL_NUMBER: u64 = 107;
#[cfg(target_arch = "x86_64")]
pub const TIMER_SETTIME_SYSCALL_NUMBER: u64 = 223;
#[cfg(target_arch = "aarch64")]
pub const TIMER_SETTIME_SYSCALL_NUMBER: u64 = 110;
#[cfg(target_arch = "x86_64")]
pub const TIMER_GETTIME_SYSCALL_NUMBER: u64 = 224;
#[cfg(target_arch = "aarch64")]
pub const TIMER_GETTIME_SYSCALL_NUMBER: u64 = 108;
#[cfg(target_arch = "x86_64")]
pub const TIMER_GETOVERRUN_SYSCALL_NUMBER: u64 = 225;
#[cfg(target_arch = "aarch64")]
pub const TIMER_GETOVERRUN_SYSCALL_NUMBER: u64 = 109;
#[cfg(target_arch = "x86_64")]
pub const TIMER_DELETE_SYSCALL_NUMBER: u64 = 226;
#[cfg(target_arch = "aarch64")]
pub const TIMER_DELETE_SYSCALL_NUMBER: u64 = 111;
#[cfg(target_arch = "x86_64")]
pub const SCHED_SETAFFINITY_SYSCALL_NUMBER: u64 = 203;
#[cfg(target_arch = "aarch64")]
pub const SCHED_SETAFFINITY_SYSCALL_NUMBER: u64 = 122;
#[cfg(target_arch = "x86_64")]
pub const SCHED_GETAFFINITY_SYSCALL_NUMBER: u64 = 204;
#[cfg(target_arch = "aarch64")]
pub const SCHED_GETAFFINITY_SYSCALL_NUMBER: u64 = 123;
#[cfg(target_arch = "x86_64")]
pub const GETCPU_SYSCALL_NUMBER: u64 = 309;
#[cfg(target_arch = "aarch64")]
pub const GETCPU_SYSCALL_NUMBER: u64 = 168;

#[cfg(target_arch = "x86_64")]
pub const MLOCK_SYSCALL_NUMBER: u64 = 149;
#[cfg(target_arch = "aarch64")]
pub const MLOCK_SYSCALL_NUMBER: u64 = 228;
#[cfg(target_arch = "x86_64")]
pub const MUNLOCK_SYSCALL_NUMBER: u64 = 150;
#[cfg(target_arch = "aarch64")]
pub const MUNLOCK_SYSCALL_NUMBER: u64 = 229;
#[cfg(target_arch = "x86_64")]
pub const MLOCKALL_SYSCALL_NUMBER: u64 = 151;
#[cfg(target_arch = "aarch64")]
pub const MLOCKALL_SYSCALL_NUMBER: u64 = 230;
#[cfg(target_arch = "x86_64")]
pub const MUNLOCKALL_SYSCALL_NUMBER: u64 = 152;
#[cfg(target_arch = "aarch64")]
pub const MUNLOCKALL_SYSCALL_NUMBER: u64 = 231;
#[cfg(target_arch = "x86_64")]
pub const MINCORE_SYSCALL_NUMBER: u64 = 27;
#[cfg(target_arch = "aarch64")]
pub const MINCORE_SYSCALL_NUMBER: u64 = 232;

/// The calls dispatched by number above that are not dispatched by enum, on
/// both architectures: None for any other number.
pub fn dispatch_numbered(number: u64, a: [u64; 4]) -> Option<SyscallResult> {
    Some(match number {
        MLOCK_SYSCALL_NUMBER => memory_advice::sys_mlock(a[0], a[1]),
        MUNLOCK_SYSCALL_NUMBER => memory_advice::sys_munlock(a[0], a[1]),
        MLOCKALL_SYSCALL_NUMBER => memory_advice::sys_mlockall(a[0]),
        MUNLOCKALL_SYSCALL_NUMBER => memory_advice::sys_munlockall(),
        MINCORE_SYSCALL_NUMBER => memory_advice::sys_mincore(a[0], a[1], a[2]),
        TIMER_CREATE_SYSCALL_NUMBER => timers::sys_timer_create(a[0] as i32, a[1], a[2]),
        TIMER_SETTIME_SYSCALL_NUMBER => timers::sys_timer_settime(a[0] as i32, a[1], a[2], a[3]),
        TIMER_GETTIME_SYSCALL_NUMBER => timers::sys_timer_gettime(a[0] as i32, a[1]),
        TIMER_GETOVERRUN_SYSCALL_NUMBER => timers::sys_timer_getoverrun(a[0] as i32),
        TIMER_DELETE_SYSCALL_NUMBER => timers::sys_timer_delete(a[0] as i32),
        SCHED_SETAFFINITY_SYSCALL_NUMBER => affinity::sys_sched_setaffinity(a[0] as i32 as i64, a[1], a[2]),
        SCHED_GETAFFINITY_SYSCALL_NUMBER => affinity::sys_sched_getaffinity(a[0] as i32 as i64, a[1], a[2]),
        GETCPU_SYSCALL_NUMBER => affinity::sys_getcpu(a[0], a[1], a[2]),
        _ => return None,
    })
}
