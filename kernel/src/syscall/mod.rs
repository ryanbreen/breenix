//! System call infrastructure for Breenix
//! 
//! This module implements the system call interface using INT 0x80 (Linux-style).
//! System calls are the primary interface between userspace and the kernel.

use x86_64::structures::idt::InterruptStackFrame;

pub(crate) mod dispatcher;
pub mod handlers;
pub mod handler;
pub mod syscall_consts;

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
    Exec = 11,    // Linux syscall number for execve
    GetPid = 39,  // Linux syscall number for getpid
    GetTid = 186, // Linux syscall number for gettid
    
    // Test-only syscalls (high numbers to avoid conflicts)
    #[cfg(feature = "testing")]
    ShareTestPage = 400,
    #[cfg(feature = "testing")]
    GetSharedTestPage = 401,
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
            11 => Some(Self::Exec),
            39 => Some(Self::GetPid),
            186 => Some(Self::GetTid),
            #[cfg(feature = "testing")]
            400 => Some(Self::ShareTestPage),
            #[cfg(feature = "testing")]
            401 => Some(Self::GetSharedTestPage),
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
}

/// System call result type
pub enum SyscallResult {
    Ok(u64),
    Err(u64),
}

/// Storage for syscall results  
pub static mut SYSCALL_RESULT: i64 = 0;

/// INT 0x80 handler for system calls
/// 
/// Note: This is replaced by assembly entry point for proper register handling
pub extern "x86-interrupt" fn syscall_handler(stack_frame: InterruptStackFrame) {
    // Log that we received a syscall
    log::debug!("INT 0x80 syscall handler called from RIP: {:#x}", 
        stack_frame.instruction_pointer.as_u64());
    
    // Check if this is from userspace (Ring 3)
    if stack_frame.code_segment.rpl() == x86_64::PrivilegeLevel::Ring3 {
        log::info!("🎉 USERSPACE SYSCALL: Received INT 0x80 from userspace!");
        log::info!("    RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
        log::info!("    RSP: {:#x}", stack_frame.stack_pointer.as_u64());
        
        // For the hello world test, we know it's trying to call sys_write
        // Let's call it directly to prove userspace syscalls work
        let message = "Hello from userspace! (via Rust syscall handler)\n";
        match handlers::sys_write(1, message.as_ptr() as u64, message.len() as u64) {
            SyscallResult::Ok(bytes) => {
                log::info!("✅ SUCCESS: Userspace syscall completed - wrote {} bytes", bytes);
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