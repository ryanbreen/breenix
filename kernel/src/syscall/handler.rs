use super::table;

/// Number of bytes pushed for GP registers in syscall_entry.asm
/// 15 registers * 8 bytes each = 120 bytes
const GP_SAVE_BYTES: usize = 15 * 8;

/// Hardware interrupt frame pushed by CPU on INT 0x80
#[repr(C)]
#[derive(Debug)]
pub struct InterruptFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// Get mutable reference to the actual IRET frame
/// The frame parameter points to the GP save area, so we need to skip past it
unsafe fn get_iret_frame_mut(gp_save_ptr: *mut SyscallFrame) -> *mut InterruptFrame {
    (gp_save_ptr as *mut u8).add(GP_SAVE_BYTES) as *mut InterruptFrame
}

#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    // General purpose registers (in memory order after all pushes)
    // Stack grows down, so last pushed is at lowest address (where RSP points)
    // Assembly pushes in reverse order: r15 first, rax last
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
    // Critical debug: Print immediately when syscall is received
    crate::serial_println!("SYSCALL_ENTRY: Received syscall from userspace! RAX={:#x}", frame.rax);
    crate::serial_println!("STEP6-BREADCRUMB: INT 0x80 fired successfully from userspace!");
    
    // DEBUG: Verify frame pointer location
    let current_rsp: u64;
    unsafe { core::arch::asm!("mov {}, rsp", out(reg) current_rsp); }
    crate::serial_println!("DEBUG: frame ptr = {:#x}, rsp in ASM = {:#x}",
                          frame as *const _ as u64, current_rsp);
    
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
    
    // Dispatch using the new table-driven approach
    let result = table::dispatch(syscall_num as usize, frame);
    
    // Set return value in RAX (result is already isize, following Linux ABI)
    frame.set_return_value(result as u64);
    
    // Check if exec() just happened and we need to update the interrupt frame
    crate::serial_println!("EXEC_DEBUG: Checking exec pending flag");
    if crate::syscall::exec::check_and_clear_exec_pending() {
        crate::serial_println!("EXEC_DEBUG: Exec pending detected - updating IRET frame");
        log::info!("Exec pending detected - updating IRET frame with new context");
        
        // Get current thread info and update the REAL interrupt frame
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            crate::task::scheduler::with_thread_mut(thread_id, |thread| {
                // Log thread context values first
                crate::serial_println!("EXEC_DEBUG: Thread context: RIP={:#x}, RSP={:#x}", 
                                     thread.context.rip, thread.context.rsp);
                
                // Get the actual IRET frame (not the GP save area)
                let iret_frame_ptr = unsafe { get_iret_frame_mut(frame) };
                crate::serial_println!("EXEC_DEBUG: GP save area at {:#x}, IRET frame at {:#x}", 
                                     frame as *const _ as u64, iret_frame_ptr as u64);
                
                let iret_frame = unsafe { &mut *iret_frame_ptr };
                
                // Log old values
                crate::serial_println!("EXEC_DEBUG: Old IRET frame: RIP={:#x}, RSP={:#x}", 
                                     iret_frame.rip, iret_frame.rsp);
                
                // Update the IRET frame with the new context
                iret_frame.rip = thread.context.rip;
                iret_frame.cs = thread.context.cs;
                iret_frame.rflags = thread.context.rflags;
                iret_frame.rsp = thread.context.rsp;
                iret_frame.ss = thread.context.ss;
                
                // Log new values
                crate::serial_println!("EXEC_DEBUG: New IRET frame: RIP={:#x}, RSP={:#x}", 
                                     iret_frame.rip, iret_frame.rsp);
                log::info!("Updated IRET frame for exec: RIP={:#x}, RSP={:#x}", 
                          iret_frame.rip, iret_frame.rsp);
            });
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

