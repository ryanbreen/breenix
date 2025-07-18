use super::{SyscallNumber, SyscallResult};
use super::syscall_consts::*;

#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    // General purpose registers (in memory order after all pushes)
    // Stack grows down, so LAST pushed is at LOWEST address (current RSP)
    // Assembly pushes: r15 first, then r14, ..., then rax last
    // So rax (pushed last) is at RSP+0, and r15 (pushed first) is at RSP+112
    pub rax: u64,  // Syscall number - pushed last, so at RSP+0
    pub rcx: u64,  // at RSP+8
    pub rdx: u64,  // at RSP+16
    pub rbx: u64,  // at RSP+24
    pub rbp: u64,  // at RSP+32
    pub rsi: u64,  // at RSP+40
    pub rdi: u64,  // at RSP+48
    pub r8: u64,   // at RSP+56
    pub r9: u64,   // at RSP+64
    pub r10: u64,  // at RSP+72
    pub r11: u64,  // at RSP+80
    pub r12: u64,  // at RSP+88
    pub r13: u64,  // at RSP+96
    pub r14: u64,  // at RSP+104
    pub r15: u64,  // pushed first, so at RSP+112
    
    // Interrupt frame (pushed by CPU before our code)
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
    // Debug: Log raw RAX value
    log::debug!("rust_syscall_handler: Raw frame.rax = {:#x} ({})", frame.rax, frame.rax as i64);
    
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
    
    // Step A-3: Add syscall entry trace logging
    log::info!("SYSCALL entry: rax={}", syscall_num);
    
    // Only log non-write syscalls to reduce noise
    if syscall_num != 1 {  // 1 is sys_write
        log::trace!(
            "Syscall {} from userspace: RIP={:#x}, args=({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})",
            syscall_num,
            frame.rip,
            args.0, args.1, args.2, args.3, args.4, args.5
        );
        
        // Debug: Log critical frame values
        log::debug!(
            "Syscall frame before: RIP={:#x}, CS={:#x}, RSP={:#x}, SS={:#x}, RAX={:#x}",
            frame.rip, frame.cs, frame.rsp, frame.ss, frame.rax
        );
    }
    
    // Dispatch to the appropriate syscall handler using constants
    let result = match syscall_num {
        SYS_EXIT => super::handlers::sys_exit(args.0 as i32),
        SYS_WRITE => super::handlers::sys_write(args.0, args.1, args.2),
        SYS_READ => super::handlers::sys_read(args.0, args.1, args.2),
        SYS_YIELD => super::handlers::sys_yield(),
        SYS_GET_TIME => super::handlers::sys_get_time(),
        SYS_FORK => super::handlers::sys_fork_with_frame(frame),
        SYS_EXEC => super::handlers::sys_exec(args.0, args.1),
        SYS_GETPID => super::handlers::sys_getpid(),
        39 => super::handlers::sys_getpid(), // Legacy SYS_GETPID value
        #[cfg(feature = "testing")]
        SYS_SHARE_TEST_PAGE => super::handlers::sys_share_test_page(args.0),
        #[cfg(feature = "testing")]
        SYS_GET_SHARED_TEST_PAGE => super::handlers::sys_get_shared_test_page(),
        _ => {
            log::warn!("Unknown syscall number: {}", syscall_num);
            SyscallResult::Err(38) // ENOSYS
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
    
    // Debug: Log frame after handling
    if syscall_num != 1 {  // 1 is sys_write
        log::debug!(
            "Syscall frame after: RIP={:#x}, CS={:#x}, RSP={:#x}, SS={:#x}, RAX={:#x} (return)",
            frame.rip, frame.cs, frame.rsp, frame.ss, frame.rax
        );
    }
    
    // Note: Context switches after sys_yield happen on the next timer interrupt
}

// Assembly functions defined in entry.s
extern "C" {
    pub fn syscall_entry();
    pub fn syscall_return_to_userspace(user_rip: u64, user_rsp: u64, user_rflags: u64) -> !;
}