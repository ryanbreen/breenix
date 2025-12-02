use super::{SyscallNumber, SyscallResult};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    // General purpose registers (in memory order after all pushes)
    // Stack grows down, so last pushed is at lowest address (where RSP points)
    // Assembly pushes: rax first, then rcx, rdx, rbx, rbp, rsi, rdi, r8-r15
    // So r15 (pushed last) is at RSP+0, and rax (pushed first) is at RSP+112
    pub r15: u64, // pushed last, at RSP+0
    pub r14: u64, // at RSP+8
    pub r13: u64, // at RSP+16
    pub r12: u64, // at RSP+24
    pub r11: u64, // at RSP+32
    pub r10: u64, // at RSP+40
    pub r9: u64,  // at RSP+48
    pub r8: u64,  // at RSP+56
    pub rdi: u64, // at RSP+64
    pub rsi: u64, // at RSP+72
    pub rbp: u64, // at RSP+80
    pub rbx: u64, // at RSP+88
    pub rdx: u64, // at RSP+96
    pub rcx: u64, // at RSP+104
    pub rax: u64, // Syscall number - pushed first, at RSP+112

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

// Static flag to track first Ring 3 syscall
static RING3_CONFIRMED: AtomicBool = AtomicBool::new(false);

/// Main syscall handler called from assembly
#[no_mangle]
pub extern "C" fn rust_syscall_handler(frame: &mut SyscallFrame) {
    // CRITICAL MARKER: Emit RING3_CONFIRMED marker on FIRST Ring 3 syscall
    // This proves that:
    // 1. IRETQ succeeded (CPU transitioned to Ring 3)
    // 2. Userspace code actually executed
    // 3. INT 0x80 was triggered from userspace
    // 4. We received a syscall with CS RPL == 3
    if (frame.cs & 3) == 3 && !RING3_CONFIRMED.swap(true, Ordering::SeqCst) {
        log::info!("🎯 RING3_CONFIRMED: First syscall received from Ring 3 (CS={:#x}, RPL=3)", frame.cs);
        crate::serial_println!("🎯 RING3_CONFIRMED: First syscall received from Ring 3 (CS={:#x}, RPL=3)", frame.cs);
    }

    // Raw serial output to detect if syscall handler is called
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x53",      // 'S' for Syscall
            "out dx, al",
            "mov al, 0x43",      // 'C'
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
    }

    // Increment preempt count on syscall entry (prevents scheduling during syscall)
    crate::per_cpu::preempt_disable();

    // Raw serial output to detect if we got past preempt_disable
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x50",      // 'P' for Past preempt_disable
            "out dx, al",
            "mov al, 0x44",      // 'D'
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
    }

    // Enhanced syscall entry logging per Cursor requirements
    let from_userspace = frame.is_from_userspace();

    // Raw serial output to detect if we got past is_from_userspace check
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x46",      // 'F' for Frame check passed
            "out dx, al",
            "mov al, 0x55",      // 'U' for Userspace
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
    }

    // Raw serial output to detect if we got past CR3 read
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x43",      // 'C' for CR3 read
            "out dx, al",
            "mov al, 0x33",      // '3'
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
    }

    // Log syscall entry with full frame info for first few syscalls
    static SYSCALL_ENTRY_LOG_COUNT: AtomicU32 = AtomicU32::new(0);

    let _entry_count = SYSCALL_ENTRY_LOG_COUNT.fetch_add(1, Ordering::Relaxed);

    // Raw serial output before log
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x4C",      // 'L' for Log
            "out dx, al",
            "mov al, 0x42",      // 'B' for Before
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
    }

    // Try to acquire serial lock - if it fails, we have a deadlock
    use crate::serial::SERIAL1;
    let serial_locked = SERIAL1.try_lock().is_some();

    // Raw serial output to show if serial lock is available
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x53",      // 'S' for Serial lock test
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
        if serial_locked {
            core::arch::asm!(
                "mov dx, 0x3F8",
                "mov al, 0x59",      // 'Y' for Yes (lock available)
                "out dx, al",
                options(nostack, nomem, preserves_flags)
            );
        } else {
            core::arch::asm!(
                "mov dx, 0x3F8",
                "mov al, 0x4E",      // 'N' for No (lock held by someone else)
                "out dx, al",
                options(nostack, nomem, preserves_flags)
            );
        }
    }

    // COMPLETELY SKIP all lock-based logging for the second syscall onwards
    // The first syscall (entry_count == 0) worked fine
    // Something is holding a lock when the second syscall starts
    // For now, bypass ALL logging to see if the syscall itself works

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

            // NOTE: Cannot safely read userspace memory at RIP-2 from kernel context
            // without proper page table validation. This diagnostic code was causing
            // page faults when attempting to access userspace addresses.
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
            log::debug!("clock_gettime syscall (228) received: frame.rdi={:#x}, frame.rsi={:#x}", frame.rdi, frame.rsi);
            log::debug!("clock_gettime syscall (228) received: args.0={:#x}, args.1={:#x}", args.0, args.1);
            log::debug!("clock_gettime syscall (228) received: clock_id={}, user_ptr={:#x}",
                clock_id, user_timespec_ptr as u64);
            super::time::sys_clock_gettime(clock_id, user_timespec_ptr)
        }
        None => {
            log::warn!("Unknown syscall number: {} - returning ENOSYS", syscall_num);
            SyscallResult::Err(super::ErrorCode::NoSys as u64)
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
    
    // CRITICAL FIX: Update TSS.RSP0 before returning to userspace
    // When userspace triggers an interrupt (like int3), the CPU switches to kernel
    // mode and uses TSS.RSP0 as the kernel stack. This must be set correctly!
    let kernel_stack_top = crate::per_cpu::kernel_stack_top();
    if kernel_stack_top.as_u64() != 0 {
        crate::gdt::set_tss_rsp0(kernel_stack_top);
        log::trace!("Updated TSS.RSP0 to {:#x} for userspace return", kernel_stack_top.as_u64());
    } else {
        log::error!("CRITICAL: Cannot set TSS.RSP0 - kernel_stack_top is 0!");
    }
    
    // Flush any pending IRQ logs before returning to userspace
    crate::irq_log::flush_local_try();
    
    // Decrement preempt count on syscall exit
    crate::per_cpu::preempt_enable();
}

// Assembly functions defined in entry.s
extern "C" {
    #[allow(dead_code)]
    pub fn syscall_entry();
    #[allow(dead_code)]
    pub fn syscall_return_to_userspace(user_rip: u64, user_rsp: u64, user_rflags: u64) -> !;
}

/// Trace function called before IRETQ to Ring 3
///
/// IMPORTANT: This function must be MINIMAL to avoid slowing down the iretq path.
/// Heavy diagnostics here cause the timer interrupt to fire before userspace
/// executes even a single instruction, creating an infinite loop.
///
/// The full page table verification code has been removed. If you need to debug
/// Ring 3 transition issues, temporarily re-enable diagnostics but be aware
/// this will prevent userspace from running.
#[no_mangle]
pub extern "C" fn trace_iretq_to_ring3(_frame_ptr: *const u64) {
    // Intentionally empty - diagnostics were causing timer to preempt before
    // userspace could execute. See commit history for the original diagnostic code.
}
