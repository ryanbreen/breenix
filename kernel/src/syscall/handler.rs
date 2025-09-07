use super::{SyscallNumber, SyscallResult};

#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    // General purpose registers (in memory order after all pushes)
    // Stack grows down, so last pushed is at lowest address (where RSP points)
    // Assembly pushes in reverse order: r15 first, rax last
    pub rax: u64, // Syscall number - pushed last, so at RSP+0
    pub rcx: u64, // at RSP+8
    pub rdx: u64, // at RSP+16
    pub rbx: u64, // at RSP+24
    pub rbp: u64, // at RSP+32
    pub rsi: u64, // at RSP+40
    pub rdi: u64, // at RSP+48
    pub r8: u64,  // at RSP+56
    pub r9: u64,  // at RSP+64
    pub r10: u64, // at RSP+72
    pub r11: u64, // at RSP+80
    pub r12: u64, // at RSP+88
    pub r13: u64, // at RSP+96
    pub r14: u64, // at RSP+104
    pub r15: u64, // pushed first, so at RSP+112

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
    // Increment preempt count on syscall entry (prevents scheduling during syscall)
    crate::per_cpu::preempt_disable();
    
    // Enhanced syscall entry logging per Cursor requirements
    let from_userspace = frame.is_from_userspace();
    
    // Get current CR3 for logging
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
    }
    
    // Log syscall entry with full frame info for first few syscalls
    use core::sync::atomic::{AtomicU32, Ordering};
    static SYSCALL_ENTRY_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
    
    let entry_count = SYSCALL_ENTRY_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if entry_count < 5 {
        log::info!("R3-SYSCALL ENTRY #{}: CS={:#x} (RPL={}), RIP={:#x}, RSP={:#x}, SS={:#x}, CR3={:#x}",
            entry_count + 1, frame.cs, frame.cs & 3, frame.rip, frame.rsp, frame.ss, cr3);
        
        if from_userspace {
            log::info!("  ✓ Syscall from Ring 3 confirmed (CPL=3)");
        } else {
            log::error!("  ⚠ WARNING: Syscall from Ring {}!", frame.cs & 3);
        }
        
        // Log syscall number and arguments
        log::info!("  Syscall: num={}, args=({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})",
            frame.syscall_number(), frame.args().0, frame.args().1, frame.args().2,
            frame.args().3, frame.args().4, frame.args().5);
    }

    // Verify this came from userspace (security check)
    if !from_userspace {
        log::warn!("Syscall from kernel mode - this shouldn't happen!");
        frame.set_return_value(u64::MAX); // Error
        return;
    }

    let syscall_num = frame.syscall_number();
    let args = frame.args();

    // Only log non-write syscalls to reduce noise
    if syscall_num != 1 {
        // 1 is sys_write
        log::trace!(
            "Syscall {} from userspace: RIP={:#x}, args=({:#x}, {:#x}, {:#x}, {:#x}, {:#x}, {:#x})",
            syscall_num,
            frame.rip,
            args.0,
            args.1,
            args.2,
            args.3,
            args.4,
            args.5
        );

        // Debug: Log critical frame values
        log::debug!(
            "Syscall frame before: RIP={:#x}, CS={:#x}, RSP={:#x}, SS={:#x}, RAX={:#x}",
            frame.rip,
            frame.cs,
            frame.rsp,
            frame.ss,
            frame.rax
        );
    }
    
    // Log first few syscalls from userspace with full frame validation
    static SYSCALL_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
    
    if (frame.cs & 3) == 3 {
        let count = SYSCALL_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
        if count < 5 {  // Log first 5 syscalls for verification
            log::info!("Syscall #{} from userspace - full frame validation:", count + 1);
            log::info!("  RIP: {:#x}", frame.rip);
            log::info!("  CS: {:#x} (RPL={})", frame.cs, frame.cs & 3);
            log::info!("  SS: {:#x} (RPL={})", frame.ss, frame.ss & 3);
            log::info!("  RSP: {:#x} (user stack)", frame.rsp);
            log::info!("  RFLAGS: {:#x} (IF={})", frame.rflags, if frame.rflags & 0x200 != 0 { "1" } else { "0" });
            
            // Validate invariants
            if (frame.cs & 3) != 3 {
                log::error!("  ERROR: CS RPL is not 3!");
            }
            if (frame.ss & 3) != 3 {
                log::error!("  ERROR: SS RPL is not 3!");
            }
            if frame.rsp < 0x10000000 || frame.rsp > 0x20000000 {
                log::warn!("  WARNING: RSP {:#x} may be outside expected user range", frame.rsp);
            }
            
            // Get current CR3
            let cr3: u64;
            unsafe {
                core::arch::asm!("mov {}, cr3", out(reg) cr3);
            }
            log::info!("  CR3: {:#x} (current page table)", cr3);
            
            // Try to read the previous 2 bytes (should be 0xcd 0x80 for int 0x80)
            if frame.rip >= 2 {
                unsafe {
                    let int_addr = (frame.rip - 2) as *const u8;
                    // Use volatile read to prevent optimization
                    let byte1 = core::ptr::read_volatile(int_addr);
                    let byte2 = core::ptr::read_volatile(int_addr.offset(1));
                    log::info!("  Previous 2 bytes at RIP-2: {:#02x} {:#02x}", byte1, byte2);
                    if byte1 == 0xcd && byte2 == 0x80 {
                        log::info!("  ✓ Confirmed: int 0x80 instruction detected");
                    } else {
                        log::warn!("  ⚠ Expected int 0x80 (0xcd 0x80) but found {:#02x} {:#02x}", byte1, byte2);
                    }
                }
            }
        }
    }

    // Dispatch to the appropriate syscall handler
    let result = match SyscallNumber::from_u64(syscall_num) {
        Some(SyscallNumber::Exit) => super::handlers::sys_exit(args.0 as i32),
        Some(SyscallNumber::Write) => super::handlers::sys_write(args.0, args.1, args.2),
        Some(SyscallNumber::Read) => super::handlers::sys_read(args.0, args.1, args.2),
        Some(SyscallNumber::Yield) => super::handlers::sys_yield(),
        Some(SyscallNumber::GetTime) => super::handlers::sys_get_time(),
        Some(SyscallNumber::Fork) => super::handlers::sys_fork_with_frame(frame),
        Some(SyscallNumber::Exec) => super::handlers::sys_exec(args.0, args.1),
        Some(SyscallNumber::GetPid) => super::handlers::sys_getpid(),
        Some(SyscallNumber::GetTid) => super::handlers::sys_gettid(),
        Some(SyscallNumber::ClockGetTime) => {
            let clock_id = args.0 as u32;
            let user_timespec_ptr = args.1 as *mut super::time::Timespec;
            super::time::sys_clock_gettime(clock_id, user_timespec_ptr)
        }
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

    // Debug: Log frame after handling
    if syscall_num != 1 {
        // 1 is sys_write
        log::debug!(
            "Syscall frame after: RIP={:#x}, CS={:#x}, RSP={:#x}, SS={:#x}, RAX={:#x} (return)",
            frame.rip,
            frame.cs,
            frame.rsp,
            frame.ss,
            frame.rax
        );
    }

    // Note: Context switches after sys_yield happen on the next timer interrupt
    
    // Flush any pending IRQ logs before returning to userspace
    crate::irq_log::flush_local_try();
    
    // Decrement preempt count on syscall exit
    crate::per_cpu::preempt_enable();
}

// Assembly functions defined in entry.s
extern "C" {
    pub fn syscall_entry();
    pub fn syscall_return_to_userspace(user_rip: u64, user_rsp: u64, user_rflags: u64) -> !;
}

/// Enhanced trace function that logs full IRETQ frame before returning to Ring 3
/// Called from assembly with pointer to IRETQ frame on stack
#[no_mangle]
pub extern "C" fn trace_iretq_to_ring3(frame_ptr: *const u64) {
    use core::sync::atomic::{AtomicU32, Ordering};
    static IRETQ_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
    
    let count = IRETQ_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if count < 3 {  // Log first 3 transitions for verification
        unsafe {
            // IRETQ frame layout: RIP, CS, RFLAGS, RSP, SS
            let rip = *frame_ptr;
            let cs = *frame_ptr.offset(1);
            let rflags = *frame_ptr.offset(2);
            let rsp = *frame_ptr.offset(3);
            let ss = *frame_ptr.offset(4);
            
            // Get current CR3
            let cr3: u64;
            core::arch::asm!("mov {}, cr3", out(reg) cr3);
            
            log::info!("R3-IRET #{}: rip={:#x}, cs={:#x} (RPL={}), ss={:#x} (RPL={}), rflags={:#x}, rsp={:#x}, cr3={:#x}",
                count + 1, rip, cs, cs & 3, ss, ss & 3, rflags, rsp, cr3);
            
            // Verify we're returning to Ring 3
            if (cs & 3) == 3 && (ss & 3) == 3 {
                log::info!("  ✓ Confirmed: Returning to Ring 3 (CPL=3)");
            } else {
                log::error!("  ⚠ WARNING: Not returning to Ring 3! CS RPL={}, SS RPL={}", cs & 3, ss & 3);
            }
        }
    }
}
