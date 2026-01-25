//! ARM64 syscall entry and exit handling.
//!
//! This module provides the Rust-side handling for ARM64 syscalls.
//! The assembly entry point in `syscall_entry.S` saves registers and calls
//! `rust_syscall_handler_aarch64`, which dispatches to the appropriate syscall.
//!
//! ARM64 syscall convention (Linux compatible):
//!   - X8 = syscall number
//!   - X0-X5 = arguments (6 args max)
//!   - X0 = return value (or negative errno on error)
//!
//! Key differences from x86_64:
//!   - SVC #0 instead of INT 0x80 / SYSCALL
//!   - No SWAPGS - use TPIDR_EL1 directly for per-CPU data
//!   - ERET instead of IRETQ/SYSRET
//!   - TTBR0_EL1/TTBR1_EL1 instead of CR3

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, Ordering};

use super::exception_frame::Aarch64ExceptionFrame;
use super::percpu::Aarch64PerCpu;
use crate::arch_impl::traits::{PerCpuOps, SyscallFrame};

// Include the syscall entry assembly
global_asm!(include_str!("syscall_entry.S"));

// Static flag to track first EL0 syscall (mirrors x86_64's RING3_CONFIRMED)
static EL0_CONFIRMED: AtomicBool = AtomicBool::new(false);

/// Returns true if userspace has started (first EL0 syscall received).
/// Used by scheduler to determine if idle thread should use idle_loop or
/// restore saved context from boot.
pub fn is_el0_confirmed() -> bool {
    EL0_CONFIRMED.load(Ordering::Relaxed)
}

/// Main syscall handler called from assembly.
///
/// This is the ARM64 equivalent of `rust_syscall_handler` for x86_64.
/// It dispatches syscalls and handles signal delivery on return.
///
/// # Safety
///
/// This function is called from assembly with a valid frame pointer.
/// The frame must be properly aligned and contain saved register state.
#[no_mangle]
pub extern "C" fn rust_syscall_handler_aarch64(frame: &mut Aarch64ExceptionFrame) {
    // Check if this is from EL0 (userspace) by examining SPSR
    let from_el0 = (frame.spsr & 0xF) == 0; // M[3:0] = 0 means EL0

    // Emit EL0_CONFIRMED marker on FIRST EL0 syscall only
    if from_el0 && !EL0_CONFIRMED.swap(true, Ordering::SeqCst) {
        log::info!(
            "EL0_CONFIRMED: First syscall received from EL0 (SPSR={:#x})",
            frame.spsr
        );
        crate::serial_println!(
            "EL0_CONFIRMED: First syscall received from EL0 (SPSR={:#x})",
            frame.spsr
        );
    }

    // Increment preempt count on syscall entry
    Aarch64PerCpu::preempt_disable();

    // Verify this came from userspace (security check)
    if !from_el0 {
        log::warn!("Syscall from kernel mode (EL1) - this shouldn't happen!");
        frame.set_return_value(u64::MAX); // Error
        Aarch64PerCpu::preempt_enable();
        return;
    }

    let syscall_num = frame.syscall_number();
    let arg1 = frame.arg1();
    let arg2 = frame.arg2();
    let arg3 = frame.arg3();
    let arg4 = frame.arg4();
    let arg5 = frame.arg5();
    let arg6 = frame.arg6();

    // Dispatch to syscall handler
    // For now, use the simple ARM64-native handler
    // In the future, this should integrate with the main syscall subsystem
    let result = dispatch_syscall(syscall_num, arg1, arg2, arg3, arg4, arg5, arg6);

    // Set return value in X0
    frame.set_return_value(result);

    // TODO: Check for pending signals before returning to userspace
    // This requires signal infrastructure to be ported to ARM64
    // check_and_deliver_signals_on_syscall_return_aarch64(frame);

    // Decrement preempt count on syscall exit
    Aarch64PerCpu::preempt_enable();
}

/// Check if rescheduling is needed and perform context switch if necessary.
///
/// Called from assembly after syscall handler returns.
/// This is the ARM64 equivalent of `check_need_resched_and_switch`.
#[no_mangle]
pub extern "C" fn check_need_resched_and_switch_aarch64(_frame: &mut Aarch64ExceptionFrame) {
    // Check if we should reschedule
    if !Aarch64PerCpu::need_resched() {
        return;
    }

    // Check preempt count (excluding PREEMPT_ACTIVE bit)
    let preempt_count = Aarch64PerCpu::preempt_count();
    let preempt_value = preempt_count & 0x0FFFFFFF;

    if preempt_value > 0 {
        // Preemption disabled - don't context switch
        return;
    }

    // Check PREEMPT_ACTIVE flag
    let preempt_active = (preempt_count & 0x10000000) != 0;
    if preempt_active {
        // In syscall return path - don't context switch
        return;
    }

    // Clear need_resched and trigger reschedule
    unsafe {
        Aarch64PerCpu::set_need_resched(false);
    }

    // TODO: Implement actual context switch for ARM64
    // This requires the scheduler to be ported to support ARM64
    // crate::task::scheduler::schedule();
}

/// Trace function called before ERET to EL0 (for debugging).
///
/// This is intentionally minimal to avoid slowing down the return path.
#[no_mangle]
pub extern "C" fn trace_eret_to_el0(_elr: u64, _spsr: u64) {
    // Intentionally empty - diagnostics would slow down syscall return
}

// =============================================================================
// Syscall dispatch (ARM64 Linux ABI)
// =============================================================================

/// Syscall numbers (ARM64 Linux ABI)
/// ARM64 uses different syscall numbers than x86_64
mod syscall_nums {
    pub const READ: u64 = 63;
    pub const WRITE: u64 = 64;
    pub const CLOSE: u64 = 57;
    pub const EXIT: u64 = 93;
    pub const EXIT_GROUP: u64 = 94;
    pub const NANOSLEEP: u64 = 101;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const SCHED_YIELD: u64 = 124;
    pub const KILL: u64 = 129;
    pub const SIGACTION: u64 = 134;
    pub const SIGPROCMASK: u64 = 135;
    pub const SIGRETURN: u64 = 139;
    pub const GETPID: u64 = 172;
    pub const GETTID: u64 = 178;
    pub const BRK: u64 = 214;
    pub const MUNMAP: u64 = 215;
    pub const EXECVE: u64 = 221;
    pub const MMAP: u64 = 222;
    pub const MPROTECT: u64 = 226;
    pub const CLONE: u64 = 220;
    pub const CLONE3: u64 = 435;
}

/// Dispatch a syscall to the appropriate handler.
///
/// Returns the syscall result (positive for success, negative errno for error).
fn dispatch_syscall(
    num: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    _arg4: u64,
    _arg5: u64,
    _arg6: u64,
) -> u64 {
    match num {
        syscall_nums::EXIT | syscall_nums::EXIT_GROUP => {
            let exit_code = arg1 as i32;
            crate::serial_println!("[syscall] exit({})", exit_code);
            crate::serial_println!();
            crate::serial_println!("========================================");
            crate::serial_println!("  Userspace Test Complete!");
            crate::serial_println!("  Exit code: {}", exit_code);
            crate::serial_println!("========================================");
            crate::serial_println!();

            // For now, halt - real implementation would terminate process
            loop {
                unsafe {
                    core::arch::asm!("wfi");
                }
            }
        }

        syscall_nums::WRITE => sys_write(arg1, arg2, arg3),

        syscall_nums::READ => {
            // Not implemented yet
            (-38_i64) as u64 // -ENOSYS
        }

        syscall_nums::CLOSE => {
            // Close syscall - no file descriptors yet, just succeed
            0
        }

        syscall_nums::BRK => {
            // brk syscall - return same address (no-op)
            arg1
        }

        syscall_nums::GETPID => {
            // Return fixed PID for now
            1
        }

        syscall_nums::GETTID => {
            // Return fixed TID for now
            1
        }

        syscall_nums::SCHED_YIELD => {
            // Yield does nothing for single-process kernel
            0
        }

        syscall_nums::CLOCK_GETTIME => {
            // Use the architecture-independent time module
            sys_clock_gettime(arg1 as u32, arg2 as *mut Timespec)
        }

        syscall_nums::MMAP | syscall_nums::MUNMAP | syscall_nums::MPROTECT => {
            // Memory management not implemented yet
            (-38_i64) as u64 // -ENOSYS
        }

        syscall_nums::KILL | syscall_nums::SIGACTION | syscall_nums::SIGPROCMASK | syscall_nums::SIGRETURN => {
            // Signals not implemented yet
            (-38_i64) as u64 // -ENOSYS
        }

        syscall_nums::NANOSLEEP => {
            // Not implemented yet
            (-38_i64) as u64 // -ENOSYS
        }

        syscall_nums::CLONE | syscall_nums::CLONE3 | syscall_nums::EXECVE => {
            // Process management not implemented yet
            (-38_i64) as u64 // -ENOSYS
        }

        _ => {
            crate::serial_println!("[syscall] Unknown ARM64 syscall {} - returning ENOSYS", num);
            (-38_i64) as u64 // -ENOSYS
        }
    }
}

/// Timespec structure for clock_gettime (matches POSIX/Linux ABI)
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// sys_write implementation
fn sys_write(fd: u64, buf: u64, count: u64) -> u64 {
    // Only support stdout (1) and stderr (2)
    if fd != 1 && fd != 2 {
        return (-9_i64) as u64; // -EBADF
    }

    // Validate buffer pointer
    if buf == 0 {
        return (-14_i64) as u64; // -EFAULT
    }

    // Write each byte to serial
    for i in 0..count {
        let byte = unsafe { *((buf + i) as *const u8) };
        crate::serial_print!("{}", byte as char);
    }

    count
}

/// sys_clock_gettime implementation - uses architecture-independent time module
fn sys_clock_gettime(clock_id: u32, user_timespec_ptr: *mut Timespec) -> u64 {
    // Validate pointer
    if user_timespec_ptr.is_null() {
        return (-14_i64) as u64; // -EFAULT
    }

    // Get time from arch-agnostic time module
    let (tv_sec, tv_nsec) = match clock_id {
        0 => {
            // CLOCK_REALTIME
            crate::time::get_real_time_ns()
        }
        1 => {
            // CLOCK_MONOTONIC
            let (secs, nanos) = crate::time::get_monotonic_time_ns();
            (secs as i64, nanos as i64)
        }
        _ => {
            return (-22_i64) as u64; // -EINVAL
        }
    };

    // Write to userspace
    unsafe {
        (*user_timespec_ptr).tv_sec = tv_sec;
        (*user_timespec_ptr).tv_nsec = tv_nsec;
    }

    0
}

// =============================================================================
// Assembly function declarations
// =============================================================================

extern "C" {
    /// Entry point for syscalls from EL0 (defined in syscall_entry.S).
    /// Not called directly from Rust - used by exception vector.
    #[allow(dead_code)]
    pub fn syscall_entry_from_el0();

    /// Return to userspace for new thread start (defined in syscall_entry.S).
    /// Arguments:
    ///   - entry_point: user entry address (ELR_EL1)
    ///   - stack_ptr: user stack pointer (SP_EL0)
    ///   - pstate: user PSTATE (SPSR_EL1, typically 0 for EL0t)
    #[allow(dead_code)]
    pub fn syscall_return_to_userspace_aarch64(
        entry_point: u64,
        stack_ptr: u64,
        pstate: u64,
    ) -> !;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_el0_confirmed_initial_state() {
        // EL0_CONFIRMED starts as false in test context
        // (may be true if other tests ran first)
        let initial = EL0_CONFIRMED.load(Ordering::Relaxed);
        if !initial {
            assert!(!is_el0_confirmed());
        }
    }

    #[test]
    fn test_el0_confirmed_swap_behavior() {
        let was_confirmed = EL0_CONFIRMED.load(Ordering::Relaxed);
        let prev = EL0_CONFIRMED.swap(true, Ordering::SeqCst);
        assert_eq!(prev, was_confirmed);
        assert!(is_el0_confirmed());
    }
}
