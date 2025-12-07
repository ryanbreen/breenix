// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║                         🚨 CRITICAL HOT PATH 🚨                               ║
// ║                                                                              ║
// ║  THIS FILE IS ON THE PROHIBITED MODIFICATIONS LIST.                          ║
// ║                                                                              ║
// ║  DO NOT ADD:                                                                 ║
// ║    - log::*, serial_println!, or ANY serial output                           ║
// ║    - Raw assembly that writes to port 0x3F8 (serial)                         ║
// ║    - Heap allocations (Box, Vec, String, format!)                            ║
// ║    - Locks that might contend (use try_lock with fallback only)              ║
// ║    - Page table walks or memory mapping operations                           ║
// ║    - Any code that takes more than ~100 cycles                               ║
// ║                                                                              ║
// ║  Timer interrupts fire every 1ms. Serial output takes 10,000+ cycles.        ║
// ║  Adding a single log statement here will cause:                              ║
// ║    - clock_gettime precision tests to fail (need sub-ms timing)              ║
// ║    - Userspace to never execute (timer fires before IRETQ completes)         ║
// ║    - Infinite kernel loops and stack overflows                               ║
// ║                                                                              ║
// ║  To debug syscalls, use GDB: See CLAUDE.md "GDB-Only Kernel Debugging"       ║
// ║                                                                              ║
// ║  If you believe you need to modify this file, you MUST:                      ║
// ║    1. Explain why GDB debugging is insufficient                              ║
// ║    2. Get explicit user approval                                             ║
// ║    3. Remove any added logging before committing                             ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

use super::{SyscallNumber, SyscallResult};
use core::sync::atomic::{AtomicBool, Ordering};

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
///
/// CRITICAL: This is a hot path. NO logging, NO serial output, NO allocations.
/// See CLAUDE.md "Interrupt and Syscall Development - CRITICAL PATH REQUIREMENTS"
#[no_mangle]
pub extern "C" fn rust_syscall_handler(frame: &mut SyscallFrame) {
    // CRITICAL MARKER: Emit RING3_CONFIRMED marker on FIRST Ring 3 syscall only
    // This proves userspace executed and triggered INT 0x80
    if (frame.cs & 3) == 3 && !RING3_CONFIRMED.swap(true, Ordering::SeqCst) {
        log::info!("🎯 RING3_CONFIRMED: First syscall received from Ring 3 (CS={:#x}, RPL=3)", frame.cs);
        crate::serial_println!("🎯 RING3_CONFIRMED: First syscall received from Ring 3 (CS={:#x}, RPL=3)", frame.cs);
    }

    // Increment preempt count on syscall entry (prevents scheduling during syscall)
    crate::per_cpu::preempt_disable();

    // Verify this came from userspace (security check)
    if !frame.is_from_userspace() {
        log::warn!("Syscall from kernel mode - this shouldn't happen!");
        frame.set_return_value(u64::MAX); // Error
        crate::per_cpu::preempt_enable();
        return;
    }

    let syscall_num = frame.syscall_number();
    let args = frame.args();

    // Dispatch to the appropriate syscall handler
    // NOTE: No logging here! This is the hot path.
    let result = match SyscallNumber::from_u64(syscall_num) {
        Some(SyscallNumber::Exit) => super::handlers::sys_exit(args.0 as i32),
        Some(SyscallNumber::Write) => super::handlers::sys_write(args.0, args.1, args.2),
        Some(SyscallNumber::Read) => super::handlers::sys_read(args.0, args.1, args.2),
        Some(SyscallNumber::Yield) => super::handlers::sys_yield(),
        Some(SyscallNumber::GetTime) => super::handlers::sys_get_time(),
        Some(SyscallNumber::Fork) => super::handlers::sys_fork_with_frame(frame),
        Some(SyscallNumber::Mmap) => {
            let addr = args.0;
            let length = args.1;
            let prot = args.2 as u32;
            let flags = args.3 as u32;
            let fd = args.4 as i64;
            let offset = args.5;
            super::mmap::sys_mmap(addr, length, prot, flags, fd, offset)
        }
        Some(SyscallNumber::Mprotect) => {
            let addr = args.0;
            let length = args.1;
            let prot = args.2 as u32;
            super::mmap::sys_mprotect(addr, length, prot)
        }
        Some(SyscallNumber::Munmap) => {
            let addr = args.0;
            let length = args.1;
            super::mmap::sys_munmap(addr, length)
        }
        Some(SyscallNumber::Exec) => super::handlers::sys_exec(args.0, args.1),
        Some(SyscallNumber::GetPid) => super::handlers::sys_getpid(),
        Some(SyscallNumber::GetTid) => super::handlers::sys_gettid(),
        Some(SyscallNumber::ClockGetTime) => {
            // NOTE: No logging here! Serial I/O takes thousands of cycles
            // and would cause the sub-millisecond precision test to fail.
            let clock_id = args.0 as u32;
            let user_timespec_ptr = args.1 as *mut super::time::Timespec;
            super::time::sys_clock_gettime(clock_id, user_timespec_ptr)
        }
        Some(SyscallNumber::Brk) => super::memory::sys_brk(args.0),
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

    // CRITICAL FIX: Update TSS.RSP0 before returning to userspace
    // When userspace triggers an interrupt (like int3), the CPU switches to kernel
    // mode and uses TSS.RSP0 as the kernel stack. This must be set correctly!
    let kernel_stack_top = crate::per_cpu::kernel_stack_top();
    if kernel_stack_top.as_u64() != 0 {
        crate::gdt::set_tss_rsp0(kernel_stack_top);
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
