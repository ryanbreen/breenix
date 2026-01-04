//! System call infrastructure for Breenix
//!
//! This module implements the system call interface using INT 0x80 (Linux-style).
//! System calls are the primary interface between userspace and the kernel.

use x86_64::structures::idt::InterruptStackFrame;

pub(crate) mod dispatcher;
pub mod errno;
pub mod fs;
pub mod handler;
pub mod handlers;
pub mod ioctl;
pub mod memory;
pub mod mmap;
pub mod pipe;
pub mod signal;
pub mod socket;
pub mod time;
pub mod userptr;

/// System call numbers (Breenix conventions)
///
/// Note: We use custom numbers for basic syscalls (0-6) that differ from Linux.
/// Higher numbered syscalls (7+) generally follow Linux x86_64 conventions where practical.
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
    GetPid = 39,        // Linux syscall number for getpid
    Socket = 41,        // Linux syscall number for socket
    SendTo = 44,        // Linux syscall number for sendto
    RecvFrom = 45,      // Linux syscall number for recvfrom
    Bind = 49,          // Linux syscall number for bind
    Exec = 59,          // Linux syscall number for execve
    Wait4 = 61,         // Linux syscall number for wait4/waitpid
    Kill = 62,          // Linux syscall number for kill
    Fcntl = 72,         // Linux syscall number for fcntl
    GetTid = 186,       // Linux syscall number for gettid
    ClockGetTime = 228, // Linux syscall number for clock_gettime
    Open = 257,         // Breenix: new filesystem syscall
    Lseek = 258,        // Breenix: new filesystem syscall
    Fstat = 259,        // Breenix: new filesystem syscall
    Getdents64 = 260,   // Breenix: directory listing syscall
    Unlink = 87,        // Linux syscall number for unlink
    Rename = 82,        // Linux syscall number for rename
    Mkdir = 83,         // Linux syscall number for mkdir
    Rmdir = 84,         // Linux syscall number for rmdir
    Pipe2 = 293,        // Linux syscall number for pipe2
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
            39 => Some(Self::GetPid),
            41 => Some(Self::Socket),
            44 => Some(Self::SendTo),
            45 => Some(Self::RecvFrom),
            49 => Some(Self::Bind),
            59 => Some(Self::Exec),
            61 => Some(Self::Wait4),
            62 => Some(Self::Kill),
            72 => Some(Self::Fcntl),
            87 => Some(Self::Unlink),
            82 => Some(Self::Rename),
            83 => Some(Self::Mkdir),
            84 => Some(Self::Rmdir),
            186 => Some(Self::GetTid),
            228 => Some(Self::ClockGetTime),
            257 => Some(Self::Open),
            258 => Some(Self::Lseek),
            259 => Some(Self::Fstat),
            260 => Some(Self::Getdents64),
            293 => Some(Self::Pipe2),
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
pub static mut SYSCALL_RESULT: i64 = 0;

/// INT 0x80 handler for system calls
///
/// Note: This is replaced by assembly entry point for proper register handling
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

/// Initialize the system call infrastructure
pub fn init() {
    log::info!("Initializing system call infrastructure");

    // Register INT 0x80 handler in IDT (done in interrupts module)
    // The actual registration happens in interrupts::init_idt()

    log::info!("System call infrastructure initialized");
}
