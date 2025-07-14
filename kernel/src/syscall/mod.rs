//! System call infrastructure for Breenix
//! 
//! This module implements the system call interface using INT 0x80 (Linux-style).
//! System calls are the primary interface between userspace and the kernel.


pub(crate) mod dispatcher;
pub mod handlers;
pub mod handler;
pub mod table;
pub mod exec;

/// System call numbers following Linux conventions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
#[allow(dead_code)]
pub enum SyscallNumber {
    Exit = 0,
    Write = 1,
    Read = 2,
    Yield = 3,
    GetTime = 4,
    Fork = 5,
    Wait = 7,     // Linux syscall number for waitpid (wait4 is deprecated)
    Exec = 11,    // Linux syscall number for execve
    GetPid = 39,  // Linux syscall number for getpid
    Spawn = 57,   // Using clone() syscall number for spawn
    Waitpid = 61, // Linux syscall number for wait4/waitpid
    GetTid = 186, // Linux syscall number for gettid
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
            7 => Some(Self::Wait),
            11 => Some(Self::Exec),
            39 => Some(Self::GetPid),
            57 => Some(Self::Spawn),
            61 => Some(Self::Waitpid),
            186 => Some(Self::GetTid),
            _ => None,
        }
    }
}

/// System call error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
#[allow(dead_code)]
pub enum SyscallError {
    /// Invalid system call number
    NoSys = -38,
    /// Invalid argument
    InvalidArgument = -22,
    /// Operation not permitted
    PermissionDenied = -1,
    /// I/O error
    IoError = -5,
    /// No child processes
    NoChild = -10,
    /// Interrupted system call
    Interrupted = -4,
}

/// System call result type
pub enum SyscallResult {
    Ok(u64),
    Err(u64),
}


/// Initialize the system call infrastructure
pub fn init() {
    log::info!("Initializing system call infrastructure");
    
    // Register INT 0x80 handler in IDT (done in interrupts module)
    // The actual registration happens in interrupts::init_idt()
    
    log::info!("System call infrastructure initialized");
}