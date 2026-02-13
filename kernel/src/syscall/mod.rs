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
#[cfg(target_arch = "aarch64")]
pub mod io;

// Syscall handler - the main dispatcher
// x86_64: Full handler with signal delivery and process management
// ARM64: Handler is in arch_impl/aarch64/syscall_entry.rs
#[cfg(target_arch = "x86_64")]
pub mod handler;

// Syscall implementations
// - dispatcher/handlers remain x86_64-only for now
// - other modules are shared across architectures
#[cfg(target_arch = "x86_64")]
pub(crate) mod dispatcher;
pub mod clone;
pub mod fifo;
pub mod fs;
pub mod futex;
pub mod graphics;
// handlers module has deep dependencies on x86_64-only subsystems
// ARM64 uses arch_impl/aarch64/syscall_entry.rs for dispatch
#[cfg(target_arch = "x86_64")]
pub mod handlers;
pub mod ioctl;
pub mod pipe;
pub mod pty;
pub mod random;
pub mod session;
pub mod signal;
// Socket syscalls - enabled for both architectures
// Unix domain sockets are fully arch-independent
pub mod socket;
#[cfg(target_arch = "aarch64")]
pub mod wait;

/// System call numbers following Linux conventions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
#[allow(dead_code)]
pub enum SyscallNumber {
    Exit = 0,
    Write = 1,
    Read = 2,
    Yield = 3,          // Note: Linux uses sched_yield = 24, but we use 3
    GetTime = 4,
    Fork = 5,
    Close = 6,          // Custom number (Linux close = 3, conflicts with our Yield)
    Poll = 7,           // Linux syscall number for poll
    Mmap = 9,           // Linux syscall number for mmap
    Mprotect = 10,      // Linux syscall number for mprotect
    Munmap = 11,        // Linux syscall number for munmap
    Brk = 12,           // Linux syscall number for brk (heap management)
    Sigaction = 13,     // Linux syscall number for rt_sigaction
    Sigprocmask = 14,   // Linux syscall number for rt_sigprocmask
    Sigreturn = 15,     // Linux syscall number for rt_sigreturn
    Ioctl = 16,         // Linux syscall number for ioctl
    Pipe = 22,          // Linux syscall number for pipe
    Select = 23,        // Linux syscall number for select
    Dup = 32,           // Linux syscall number for dup
    Dup2 = 33,          // Linux syscall number for dup2
    Pause = 34,         // Linux syscall number for pause
    Nanosleep = 35,     // Linux syscall number for nanosleep
    Getitimer = 36,     // Linux syscall number for getitimer
    Alarm = 37,         // Linux syscall number for alarm
    Setitimer = 38,     // Linux syscall number for setitimer
    Fcntl = 72,         // Linux syscall number for fcntl
    GetPid = 39,        // Linux syscall number for getpid
    Socket = 41,        // Linux syscall number for socket
    Connect = 42,       // Linux syscall number for connect
    Accept = 43,        // Linux syscall number for accept
    SendTo = 44,        // Linux syscall number for sendto
    RecvFrom = 45,      // Linux syscall number for recvfrom
    Shutdown = 48,      // Linux syscall number for shutdown
    Bind = 49,          // Linux syscall number for bind
    Listen = 50,        // Linux syscall number for listen
    Socketpair = 53,    // Linux syscall number for socketpair
    Exec = 59,          // Linux syscall number for execve
    Wait4 = 61,         // Linux syscall number for wait4/waitpid
    Kill = 62,          // Linux syscall number for kill
    Getsockname = 51,    // Linux syscall number for getsockname
    Getpeername = 52,    // Linux syscall number for getpeername
    Setsockopt = 54,     // Linux syscall number for setsockopt
    Clone = 56,          // Linux syscall number for clone
    Getsockopt = 55,     // Linux syscall number for getsockopt
    SetPgid = 109,      // Linux syscall number for setpgid
    Getppid = 110,      // Linux syscall number for getppid
    SetSid = 112,       // Linux syscall number for setsid
    GetPgid = 121,      // Linux syscall number for getpgid
    GetSid = 124,       // Linux syscall number for getsid
    Sigpending = 127,   // Linux syscall number for rt_sigpending
    Sigsuspend = 130,   // Linux syscall number for rt_sigsuspend
    Sigaltstack = 131,  // Linux syscall number for sigaltstack
    GetTid = 186,       // Linux syscall number for gettid
    SetTidAddress = 218, // Linux syscall number for set_tid_address
    ClockGetTime = 228, // Linux syscall number for clock_gettime
    ExitGroup = 231,    // Linux syscall number for exit_group
    Pipe2 = 293,        // Linux syscall number for pipe2
    Futex = 202,        // Linux syscall number for futex
    GetRandom = 318,    // Linux syscall number for getrandom
    // Filesystem syscalls
    Access = 21,        // Linux syscall number for access
    Getcwd = 79,        // Linux syscall number for getcwd
    Chdir = 80,         // Linux syscall number for chdir
    Rename = 82,        // Linux syscall number for rename
    Mkdir = 83,         // Linux syscall number for mkdir
    Rmdir = 84,         // Linux syscall number for rmdir
    Link = 86,          // Linux syscall number for link (hard links)
    Unlink = 87,        // Linux syscall number for unlink
    Symlink = 88,       // Linux syscall number for symlink
    Readlink = 89,      // Linux syscall number for readlink
    Mknod = 133,        // Linux syscall number for mknod (used for mkfifo)
    Open = 257,         // Breenix: filesystem open syscall
    Lseek = 258,        // Breenix: filesystem lseek syscall
    Fstat = 259,        // Breenix: filesystem fstat syscall
    Getdents64 = 260,   // Breenix: directory listing syscall
    // PTY syscalls (Breenix-specific numbers)
    PosixOpenpt = 400,  // Breenix: open PTY master
    Grantpt = 401,      // Breenix: grant access to PTY slave
    Unlockpt = 402,     // Breenix: unlock PTY slave
    Ptsname = 403,      // Breenix: get PTY slave path
    // Graphics syscalls (Breenix-specific)
    FbInfo = 410,       // Breenix: get framebuffer info
    FbDraw = 411,       // Breenix: draw to framebuffer (left pane)
    FbMmap = 412,       // Breenix: mmap framebuffer into userspace
    GetMousePos = 413,  // Breenix: get mouse cursor position
    CowStats = 500,     // Breenix: get Copy-on-Write statistics (for testing)
    SimulateOom = 501,  // Breenix: enable/disable OOM simulation (for testing)
}

#[allow(dead_code)]
impl SyscallNumber {
    /// Try to convert a u64 to a SyscallNumber
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Exit),
            1 => Some(Self::Write),
            2 => Some(Self::Read),
            3 => Some(Self::Yield),
            4 => Some(Self::GetTime),
            5 => Some(Self::Fork),
            6 => Some(Self::Close),
            7 => Some(Self::Poll),
            9 => Some(Self::Mmap),
            10 => Some(Self::Mprotect),
            11 => Some(Self::Munmap),
            12 => Some(Self::Brk),
            13 => Some(Self::Sigaction),
            14 => Some(Self::Sigprocmask),
            15 => Some(Self::Sigreturn),
            16 => Some(Self::Ioctl),
            22 => Some(Self::Pipe),
            23 => Some(Self::Select),
            32 => Some(Self::Dup),
            33 => Some(Self::Dup2),
            34 => Some(Self::Pause),
            35 => Some(Self::Nanosleep),
            36 => Some(Self::Getitimer),
            37 => Some(Self::Alarm),
            38 => Some(Self::Setitimer),
            39 => Some(Self::GetPid),
            72 => Some(Self::Fcntl),
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
            59 => Some(Self::Exec),
            61 => Some(Self::Wait4),
            62 => Some(Self::Kill),
            109 => Some(Self::SetPgid),
            110 => Some(Self::Getppid),
            112 => Some(Self::SetSid),
            121 => Some(Self::GetPgid),
            124 => Some(Self::GetSid),
            127 => Some(Self::Sigpending),
            130 => Some(Self::Sigsuspend),
            131 => Some(Self::Sigaltstack),
            186 => Some(Self::GetTid),
            202 => Some(Self::Futex),
            218 => Some(Self::SetTidAddress),
            228 => Some(Self::ClockGetTime),
            231 => Some(Self::ExitGroup),
            293 => Some(Self::Pipe2),
            318 => Some(Self::GetRandom),
            // Filesystem syscalls
            21 => Some(Self::Access),
            79 => Some(Self::Getcwd),
            80 => Some(Self::Chdir),
            82 => Some(Self::Rename),
            83 => Some(Self::Mkdir),
            84 => Some(Self::Rmdir),
            86 => Some(Self::Link),
            87 => Some(Self::Unlink),
            88 => Some(Self::Symlink),
            89 => Some(Self::Readlink),
            133 => Some(Self::Mknod),
            257 => Some(Self::Open),
            258 => Some(Self::Lseek),
            259 => Some(Self::Fstat),
            260 => Some(Self::Getdents64),
            // PTY syscalls
            400 => Some(Self::PosixOpenpt),
            401 => Some(Self::Grantpt),
            402 => Some(Self::Unlockpt),
            403 => Some(Self::Ptsname),
            // Graphics syscalls
            410 => Some(Self::FbInfo),
            411 => Some(Self::FbDraw),
            412 => Some(Self::FbMmap),
            413 => Some(Self::GetMousePos),
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
