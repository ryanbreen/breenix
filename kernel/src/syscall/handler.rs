use super::{SyscallNumber, SyscallResult};

#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    // General purpose registers (pushed by syscall_entry)
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,  // Syscall number
    
    // Interrupt frame (pushed by CPU)
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl SyscallFrame {
    /// Check if this syscall came from userspace
    pub fn is_from_userspace(&self) -> bool {
        // Check CS register - if RPL (bits 0-1) is 3, it's from userspace
        (self.cs & 0x3) == 3
    }
    
    /// Get syscall number
    pub fn syscall_number(&self) -> u64 {
        self.rax
    }
    
    /// Get syscall arguments (following System V ABI)
    pub fn args(&self) -> (u64, u64, u64, u64, u64, u64) {
        (self.rdi, self.rsi, self.rdx, self.r10, self.r8, self.r9)
    }
    
    /// Set return value
    pub fn set_return_value(&mut self, value: u64) {
        self.rax = value;
    }
}

/// Main syscall handler called from assembly
#[no_mangle]
pub extern "C" fn rust_syscall_handler(frame: &mut SyscallFrame) {
    // Log syscall entry
    let from_userspace = frame.is_from_userspace();
    // Commented out to reduce noise - uncomment for debugging
    // log::debug!("Syscall entry: from_userspace={}, CS={:#x}, SS={:#x}", 
    //     from_userspace, frame.cs, frame.ss);
    
    // Verify this came from userspace (security check)
    if !from_userspace {
        log::warn!("Syscall from kernel mode - this shouldn't happen!");
        frame.set_return_value(u64::MAX); // Error
        return;
    }
    
    let syscall_num = frame.syscall_number();
    let args = frame.args();
    
    // Only log non-write syscalls to reduce noise
    if syscall_num != 1 {  // 1 is sys_write
        log::trace!(
            "Syscall {} from userspace: RIP={:#x}, args=({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})",
            syscall_num,
            frame.rip,
            args.0, args.1, args.2, args.3, args.4, args.5
        );
    }
    
    // Dispatch to the appropriate syscall handler
    let result = match SyscallNumber::from_u64(syscall_num) {
        Some(SyscallNumber::Exit) => super::handlers::sys_exit(args.0 as i32),
        Some(SyscallNumber::Write) => super::handlers::sys_write(args.0, args.1, args.2),
        Some(SyscallNumber::Read) => super::handlers::sys_read(args.0, args.1, args.2),
        Some(SyscallNumber::Yield) => super::handlers::sys_yield(),
        Some(SyscallNumber::GetTime) => super::handlers::sys_get_time(),
        None => {
            log::warn!("Unknown syscall number: {}", syscall_num);
            SyscallResult::Err(u64::MAX)
        }
    };
    
    // Set return value in RAX
    match result {
        SyscallResult::Ok(val) => frame.set_return_value(val),
        SyscallResult::Err(errno) => {
            // Return -errno in RAX for errors (Linux convention)
            frame.set_return_value((-(errno as i64)) as u64);
        }
    }
    
    // Note: Context switches after sys_yield happen on the next timer interrupt
}

// Assembly functions defined in entry.s
extern "C" {
    pub fn syscall_entry();
    pub fn syscall_return_to_userspace(user_rip: u64, user_rsp: u64, user_rflags: u64) -> !;
}