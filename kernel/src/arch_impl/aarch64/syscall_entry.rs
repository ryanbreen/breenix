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

use alloc::boxed::Box;
use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, Ordering};

use super::cpu::without_interrupts;
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
    // Some syscalls need special handling because they require access to the frame
    let result = match syscall_num {
        syscall_nums::FORK => sys_fork_aarch64(frame),
        syscall_nums::SIGRETURN => sys_sigreturn_aarch64(frame),
        syscall_nums::PAUSE => sys_pause_aarch64(frame),
        syscall_nums::SIGSUSPEND => sys_sigsuspend_aarch64(frame, arg1, arg2),
        _ => dispatch_syscall(syscall_num, arg1, arg2, arg3, arg4, arg5, arg6),
    };

    // Set return value in X0
    frame.set_return_value(result);

    // Check for pending signals before returning to userspace
    // This is required for POSIX compliance - signals must be delivered on syscall return
    check_and_deliver_signals_on_syscall_return_aarch64(frame);

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
// Syscall dispatch (Breenix ABI - same as x86_64 for consistency)
// =============================================================================

/// Syscall numbers (Breenix ABI - matches libbreenix/src/syscall.rs)
/// We use the same syscall numbers across architectures for simplicity.
mod syscall_nums {
    // Breenix syscall numbers (same as x86_64 for consistency)
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;
    pub const GET_TIME: u64 = 4;
    pub const FORK: u64 = 5;
    pub const CLOSE: u64 = 6;
    pub const BRK: u64 = 12;
    pub const SIGACTION: u64 = 13;
    pub const SIGPROCMASK: u64 = 14;
    pub const SIGRETURN: u64 = 15;
    pub const PAUSE: u64 = 34;
    pub const GETITIMER: u64 = 36;
    pub const ALARM: u64 = 37;
    pub const SETITIMER: u64 = 38;
    pub const GETPID: u64 = 39;
    pub const KILL: u64 = 62;
    pub const SIGPENDING: u64 = 127;
    pub const SIGSUSPEND: u64 = 130;
    pub const SIGALTSTACK: u64 = 131;
    pub const GETTID: u64 = 186;
    pub const CLOCK_GETTIME: u64 = 228;

    // Also accept Linux ARM64 syscall numbers for compatibility
    pub const ARM64_EXIT: u64 = 93;
    pub const ARM64_EXIT_GROUP: u64 = 94;
    pub const ARM64_WRITE: u64 = 64;
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
        syscall_nums::EXIT | syscall_nums::ARM64_EXIT | syscall_nums::ARM64_EXIT_GROUP => {
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

        syscall_nums::WRITE | syscall_nums::ARM64_WRITE => sys_write(arg1, arg2, arg3),

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

        syscall_nums::YIELD => {
            // Yield does nothing for single-process kernel
            0
        }

        syscall_nums::GET_TIME => {
            // Legacy GET_TIME: returns ticks directly in x0
            sys_get_time()
        }

        syscall_nums::CLOCK_GETTIME => {
            // clock_gettime: writes to timespec pointer in arg2
            sys_clock_gettime(arg1 as u32, arg2 as *mut Timespec)
        }

        // Signal syscalls
        syscall_nums::KILL => {
            match crate::syscall::signal::sys_kill(arg1 as i64, arg2 as i32) {
                crate::syscall::SyscallResult::Ok(v) => v,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::SIGACTION => {
            match crate::syscall::signal::sys_sigaction(arg1 as i32, arg2, arg3, arg4) {
                crate::syscall::SyscallResult::Ok(v) => v,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::SIGPROCMASK => {
            match crate::syscall::signal::sys_sigprocmask(arg1 as i32, arg2, arg3, arg4) {
                crate::syscall::SyscallResult::Ok(v) => v,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::SIGPENDING => {
            match crate::syscall::signal::sys_sigpending(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(v) => v,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::SIGALTSTACK => {
            match crate::syscall::signal::sys_sigaltstack(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(v) => v,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::ALARM => {
            match crate::syscall::signal::sys_alarm(arg1) {
                crate::syscall::SyscallResult::Ok(v) => v,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::GETITIMER => {
            match crate::syscall::signal::sys_getitimer(arg1 as i32, arg2) {
                crate::syscall::SyscallResult::Ok(v) => v,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::SETITIMER => {
            match crate::syscall::signal::sys_setitimer(arg1 as i32, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(v) => v,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        // Note: SIGRETURN, SIGSUSPEND, and PAUSE require frame access
        // They are handled separately with the frame passed in
        syscall_nums::SIGRETURN | syscall_nums::SIGSUSPEND | syscall_nums::PAUSE => {
            // These should not reach here - they need frame access
            log::warn!("[syscall] {} requires frame access - use rust_syscall_handler_aarch64", num);
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

/// sys_get_time implementation - returns ticks directly
fn sys_get_time() -> u64 {
    // Return monotonic nanoseconds as ticks
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    secs as u64 * 1_000_000_000 + nanos as u64
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
// Fork syscall implementation for ARM64
// =============================================================================

/// sys_fork for ARM64 - creates a child process with Copy-on-Write memory
///
/// This function captures the parent's full register state from the exception frame
/// and creates a child process that will resume from the same point.
///
/// Returns:
/// - To parent: child PID (positive)
/// - To child: 0
/// - On error: negative errno
fn sys_fork_aarch64(frame: &Aarch64ExceptionFrame) -> u64 {
    // Read SP_EL0 (user stack pointer) which isn't in the exception frame
    let user_sp: u64;
    unsafe {
        core::arch::asm!("mrs {}, sp_el0", out(reg) user_sp, options(nomem, nostack));
    }

    // Create a CpuContext from the exception frame
    let parent_context = crate::task::thread::CpuContext::from_aarch64_frame(frame, user_sp);

    log::info!(
        "sys_fork_aarch64: userspace SP = {:#x}, return PC (ELR) = {:#x}",
        user_sp,
        frame.elr
    );

    log::debug!(
        "sys_fork_aarch64: x19={:#x}, x20={:#x}, x29={:#x}, x30={:#x}",
        frame.x19, frame.x20, frame.x29, frame.x30
    );

    // Disable interrupts for the entire fork operation to ensure atomicity
    without_interrupts(|| {
        // Get current thread ID from scheduler
        let scheduler_thread_id = crate::task::scheduler::current_thread_id();
        let current_thread_id = match scheduler_thread_id {
            Some(id) => id,
            None => {
                log::error!("sys_fork_aarch64: No current thread in scheduler");
                return (-22_i64) as u64; // -EINVAL
            }
        };

        if current_thread_id == 0 {
            log::error!("sys_fork_aarch64: Cannot fork from idle thread");
            return (-22_i64) as u64; // -EINVAL
        }

        // Find the current process by thread ID
        let manager_guard = crate::process::manager();
        let process_info = if let Some(ref manager) = *manager_guard {
            manager.find_process_by_thread(current_thread_id)
        } else {
            log::error!("sys_fork_aarch64: Process manager not available");
            return (-12_i64) as u64; // -ENOMEM
        };

        let (parent_pid, parent_process) = match process_info {
            Some((pid, process)) => (pid, process),
            None => {
                log::error!(
                    "sys_fork_aarch64: Current thread {} not found in any process",
                    current_thread_id
                );
                return (-3_i64) as u64; // -ESRCH
            }
        };

        log::info!(
            "sys_fork_aarch64: Found parent process {} (PID {})",
            parent_process.name,
            parent_pid.as_u64()
        );

        // Drop the lock before creating page table to avoid deadlock
        drop(manager_guard);

        // Create the child page table BEFORE re-acquiring the lock
        log::info!("sys_fork_aarch64: Creating page table for child process");
        let child_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
            Ok(pt) => Box::new(pt),
            Err(e) => {
                log::error!("sys_fork_aarch64: Failed to create child page table: {}", e);
                return (-12_i64) as u64; // -ENOMEM
            }
        };
        log::info!("sys_fork_aarch64: Child page table created successfully");

        // Now re-acquire the lock and complete the fork
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            match manager.fork_process_aarch64(parent_pid, parent_context, child_page_table) {
                Ok(child_pid) => {
                    // Get the child's thread ID to add to scheduler
                    if let Some(child_process) = manager.get_process(child_pid) {
                        if let Some(child_thread) = &child_process.main_thread {
                            let child_thread_id = child_thread.id;
                            let child_thread_clone = child_thread.clone();

                            // Drop the lock before spawning to avoid issues
                            drop(manager_guard);

                            // Add the child thread to the scheduler
                            log::info!(
                                "sys_fork_aarch64: Spawning child thread {} to scheduler",
                                child_thread_id
                            );
                            crate::task::scheduler::spawn(Box::new(child_thread_clone));
                            log::info!("sys_fork_aarch64: Child thread spawned successfully");

                            log::info!(
                                "sys_fork_aarch64: Fork successful - parent {} gets child PID {}, thread {}",
                                parent_pid.as_u64(), child_pid.as_u64(), child_thread_id
                            );

                            // Return the child PID to the parent
                            child_pid.as_u64()
                        } else {
                            log::error!("sys_fork_aarch64: Child process has no main thread");
                            (-12_i64) as u64 // -ENOMEM
                        }
                    } else {
                        log::error!("sys_fork_aarch64: Failed to find newly created child process");
                        (-12_i64) as u64 // -ENOMEM
                    }
                }
                Err(e) => {
                    log::error!("sys_fork_aarch64: Failed to fork process: {}", e);
                    (-12_i64) as u64 // -ENOMEM
                }
            }
        } else {
            log::error!("sys_fork_aarch64: Process manager not available");
            (-12_i64) as u64 // -ENOMEM
        }
    })
}

// =============================================================================
// Signal-related syscalls for ARM64
// =============================================================================

/// Userspace address limit - addresses must be below this to be valid userspace
const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

/// sys_sigreturn for ARM64 - Return from signal handler
///
/// This syscall is called by the signal trampoline after a signal handler returns.
/// It restores the pre-signal execution context from the SignalFrame pushed to
/// the user stack when the signal was delivered.
fn sys_sigreturn_aarch64(frame: &mut Aarch64ExceptionFrame) -> u64 {
    use crate::signal::types::SignalFrame;

    // Read SP_EL0 to find the user stack
    let user_sp: u64;
    unsafe {
        core::arch::asm!("mrs {}, sp_el0", out(reg) user_sp, options(nomem, nostack));
    }

    // The signal frame is at SP - 8 (signal handler's 'ret' popped the return address)
    let signal_frame_ptr = (user_sp - 8) as *const SignalFrame;

    // Read the signal frame from userspace
    let signal_frame = unsafe { *signal_frame_ptr };

    // Verify magic number
    if signal_frame.magic != SignalFrame::MAGIC {
        log::error!(
            "sys_sigreturn_aarch64: invalid magic {:#x} (expected {:#x})",
            signal_frame.magic,
            SignalFrame::MAGIC
        );
        return (-14_i64) as u64; // -EFAULT
    }

    // Validate saved_pc is in userspace
    if signal_frame.saved_pc >= USER_SPACE_END {
        log::error!(
            "sys_sigreturn_aarch64: saved_pc {:#x} is not in userspace",
            signal_frame.saved_pc
        );
        return (-14_i64) as u64; // -EFAULT
    }

    // Validate saved_sp is in userspace
    if signal_frame.saved_sp >= USER_SPACE_END {
        log::error!(
            "sys_sigreturn_aarch64: saved_sp {:#x} is not in userspace",
            signal_frame.saved_sp
        );
        return (-14_i64) as u64; // -EFAULT
    }

    log::debug!(
        "sigreturn_aarch64: restoring context from frame at {:#x}, saved_pc={:#x}",
        user_sp,
        signal_frame.saved_pc
    );

    // Restore the execution context
    frame.elr = signal_frame.saved_pc;
    frame.spsr = signal_frame.saved_pstate;

    // Restore SP_EL0
    unsafe {
        core::arch::asm!("msr sp_el0, {}", in(reg) signal_frame.saved_sp, options(nomem, nostack));
    }

    // Restore general-purpose registers (x0-x30)
    frame.x0 = signal_frame.saved_x[0];
    frame.x1 = signal_frame.saved_x[1];
    frame.x2 = signal_frame.saved_x[2];
    frame.x3 = signal_frame.saved_x[3];
    frame.x4 = signal_frame.saved_x[4];
    frame.x5 = signal_frame.saved_x[5];
    frame.x6 = signal_frame.saved_x[6];
    frame.x7 = signal_frame.saved_x[7];
    frame.x8 = signal_frame.saved_x[8];
    frame.x9 = signal_frame.saved_x[9];
    frame.x10 = signal_frame.saved_x[10];
    frame.x11 = signal_frame.saved_x[11];
    frame.x12 = signal_frame.saved_x[12];
    frame.x13 = signal_frame.saved_x[13];
    frame.x14 = signal_frame.saved_x[14];
    frame.x15 = signal_frame.saved_x[15];
    frame.x16 = signal_frame.saved_x[16];
    frame.x17 = signal_frame.saved_x[17];
    frame.x18 = signal_frame.saved_x[18];
    frame.x19 = signal_frame.saved_x[19];
    frame.x20 = signal_frame.saved_x[20];
    frame.x21 = signal_frame.saved_x[21];
    frame.x22 = signal_frame.saved_x[22];
    frame.x23 = signal_frame.saved_x[23];
    frame.x24 = signal_frame.saved_x[24];
    frame.x25 = signal_frame.saved_x[25];
    frame.x26 = signal_frame.saved_x[26];
    frame.x27 = signal_frame.saved_x[27];
    frame.x28 = signal_frame.saved_x[28];
    frame.x29 = signal_frame.saved_x[29];
    frame.x30 = signal_frame.saved_x[30];

    // Restore the signal mask
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return (-3_i64) as u64, // -ESRCH
    };

    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread_mut(current_thread_id) {
                // Check if we're returning from a signal that interrupted sigsuspend
                if let Some(saved_mask) = process.signals.sigsuspend_saved_mask.take() {
                    process.signals.set_blocked(saved_mask);
                    log::info!(
                        "sigreturn_aarch64: restored sigsuspend saved mask to {:#x}",
                        saved_mask
                    );
                } else {
                    process.signals.set_blocked(signal_frame.saved_blocked);
                    log::debug!(
                        "sigreturn_aarch64: restored signal mask to {:#x}",
                        signal_frame.saved_blocked
                    );
                }

                // Clear on_stack flag if we were on alt stack
                if process.signals.alt_stack.on_stack {
                    process.signals.alt_stack.on_stack = false;
                }
            }
        }
    }

    log::info!(
        "sigreturn_aarch64: restored context, returning to PC={:#x} SP={:#x}",
        signal_frame.saved_pc,
        signal_frame.saved_sp
    );

    0 // Return value is ignored - original x0 was restored above
}

/// sys_pause for ARM64 - Wait until a signal is delivered
fn sys_pause_aarch64(frame: &Aarch64ExceptionFrame) -> u64 {
    let thread_id = crate::task::scheduler::current_thread_id().unwrap_or(0);
    log::info!("sys_pause_aarch64: Thread {} blocking until signal arrives", thread_id);

    // Read SP_EL0 for context
    let user_sp: u64;
    unsafe {
        core::arch::asm!("mrs {}, sp_el0", out(reg) user_sp, options(nomem, nostack));
    }

    // Save userspace context
    let userspace_context = crate::task::thread::CpuContext::from_aarch64_frame(frame, user_sp);

    // Save to process
    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
                if let Some(ref mut thread) = process.main_thread {
                    thread.saved_userspace_context = Some(userspace_context.clone());
                }
            }
        }
    }

    // Block until signal
    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_signal_with_context(Some(userspace_context));
    });

    // Re-enable preemption for HLT loop
    Aarch64PerCpu::preempt_enable();

    // Wait loop
    loop {
        crate::task::scheduler::yield_current();
        unsafe { core::arch::asm!("wfi"); }

        let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.state == crate::task::thread::ThreadState::BlockedOnSignal
            } else {
                false
            }
        }).unwrap_or(false);

        if !still_blocked {
            break;
        }
    }

    // Clear blocked state
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
            thread.saved_userspace_context = None;
        }
    });

    // Re-disable preemption
    Aarch64PerCpu::preempt_disable();

    (-4_i64) as u64 // -EINTR
}

/// sys_sigsuspend for ARM64 - Atomically set signal mask and wait
fn sys_sigsuspend_aarch64(frame: &Aarch64ExceptionFrame, mask_ptr: u64, sigsetsize: u64) -> u64 {
    use crate::signal::constants::UNCATCHABLE_SIGNALS;

    // Validate sigsetsize
    if sigsetsize != 8 {
        log::warn!("sys_sigsuspend_aarch64: invalid sigsetsize {}", sigsetsize);
        return (-22_i64) as u64; // -EINVAL
    }

    // Read mask from userspace
    let new_mask: u64 = if mask_ptr != 0 {
        unsafe { *(mask_ptr as *const u64) }
    } else {
        return (-14_i64) as u64; // -EFAULT
    };

    let thread_id = crate::task::scheduler::current_thread_id().unwrap_or(0);
    log::info!(
        "sys_sigsuspend_aarch64: Thread {} suspending with mask {:#x}",
        thread_id, new_mask
    );

    // Read SP_EL0 for context
    let user_sp: u64;
    unsafe {
        core::arch::asm!("mrs {}, sp_el0", out(reg) user_sp, options(nomem, nostack));
    }

    let userspace_context = crate::task::thread::CpuContext::from_aarch64_frame(frame, user_sp);

    // Save mask and context atomically
    {
        if let Some(mut manager_guard) = crate::process::try_manager() {
            if let Some(ref mut manager) = *manager_guard {
                if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
                    let saved_mask = process.signals.blocked;
                    let sanitized_mask = new_mask & !UNCATCHABLE_SIGNALS;
                    process.signals.set_blocked(sanitized_mask);
                    process.signals.sigsuspend_saved_mask = Some(saved_mask);

                    if let Some(ref mut thread) = process.main_thread {
                        thread.saved_userspace_context = Some(userspace_context.clone());
                    }

                    log::info!(
                        "sys_sigsuspend_aarch64: Thread {} saved mask {:#x}, set temp mask {:#x}",
                        thread_id, saved_mask, sanitized_mask
                    );
                } else {
                    return (-3_i64) as u64; // -ESRCH
                }
            } else {
                return (-3_i64) as u64; // -ESRCH
            }
        } else {
            return (-3_i64) as u64; // -ESRCH
        }
    }

    // Block until signal
    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_signal_with_context(Some(userspace_context));
    });

    // Re-enable preemption for wait loop
    Aarch64PerCpu::preempt_enable();

    // Wait loop
    loop {
        crate::task::scheduler::yield_current();
        unsafe { core::arch::asm!("wfi"); }

        let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.state == crate::task::thread::ThreadState::BlockedOnSignal
            } else {
                false
            }
        }).unwrap_or(false);

        if !still_blocked {
            break;
        }
    }

    // Clear blocked state
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
            thread.saved_userspace_context = None;
        }
    });

    // Re-disable preemption
    Aarch64PerCpu::preempt_disable();

    (-4_i64) as u64 // -EINTR
}

// =============================================================================
// Signal delivery on syscall return (ARM64)
// =============================================================================

/// Check for pending signals before returning to userspace (ARM64)
///
/// This is critical for POSIX compliance - signals must be delivered on syscall return.
fn check_and_deliver_signals_on_syscall_return_aarch64(frame: &mut Aarch64ExceptionFrame) {
    use crate::signal::constants::*;
    use crate::signal::types::SignalFrame;

    // Get current thread ID
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };

    // Thread 0 is idle - no signals
    if current_thread_id == 0 {
        return;
    }

    // Try to acquire process manager lock (non-blocking)
    let mut manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => return, // Lock held, skip - will happen on next timer interrupt
    };

    if let Some(ref mut manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread_mut(current_thread_id) {
            // Check interval timers
            crate::signal::delivery::check_and_fire_alarm(process);
            crate::signal::delivery::check_and_fire_itimer_real(process, 5000);

            // Check if there are any deliverable signals
            if !crate::signal::delivery::has_deliverable_signals(process) {
                return;
            }

            // Get next deliverable signal
            let sig = match process.signals.next_deliverable_signal() {
                Some(s) => s,
                None => return,
            };

            // Clear pending flag
            process.signals.clear_pending(sig);

            // Get the handler
            let action = *process.signals.get_handler(sig);

            match action.handler {
                SIG_DFL => {
                    // Default action - re-queue for timer interrupt to handle
                    process.signals.set_pending(sig);
                    return;
                }
                SIG_IGN => {
                    // Signal ignored
                    return;
                }
                handler_addr => {
                    // User-defined handler - set up signal frame
                    deliver_signal_to_user_handler_aarch64(
                        process,
                        frame,
                        sig,
                        handler_addr,
                        &action,
                    );
                }
            }
        }
    }
}

/// Deliver a signal to a user-defined handler (ARM64)
fn deliver_signal_to_user_handler_aarch64(
    process: &mut crate::process::Process,
    frame: &mut Aarch64ExceptionFrame,
    sig: u32,
    handler_addr: u64,
    action: &crate::signal::types::SignalAction,
) {
    use crate::signal::constants::*;
    use crate::signal::types::SignalFrame;

    // Read current SP_EL0
    let current_sp: u64;
    unsafe {
        core::arch::asm!("mrs {}, sp_el0", out(reg) current_sp, options(nomem, nostack));
    }

    // Check if we should use alternate stack
    let use_alt_stack = (action.flags & SA_ONSTACK) != 0
        && (process.signals.alt_stack.flags & SS_DISABLE) == 0
        && process.signals.alt_stack.size > 0
        && !process.signals.alt_stack.on_stack;

    let user_sp = if use_alt_stack {
        let alt_top = process.signals.alt_stack.base + process.signals.alt_stack.size as u64;
        process.signals.alt_stack.on_stack = true;
        alt_top
    } else {
        current_sp
    };

    // Build signal frame
    let mut signal_frame = SignalFrame {
        trampoline_addr: action.restorer,
        magic: SignalFrame::MAGIC,
        signal: sig as u64,
        siginfo_ptr: 0,
        ucontext_ptr: 0,
        saved_pc: frame.elr,
        saved_sp: current_sp,
        saved_pstate: frame.spsr,
        saved_x: [0u64; 31],
        saved_blocked: process.signals.blocked,
    };

    // Save x0-x30
    signal_frame.saved_x[0] = frame.x0;
    signal_frame.saved_x[1] = frame.x1;
    signal_frame.saved_x[2] = frame.x2;
    signal_frame.saved_x[3] = frame.x3;
    signal_frame.saved_x[4] = frame.x4;
    signal_frame.saved_x[5] = frame.x5;
    signal_frame.saved_x[6] = frame.x6;
    signal_frame.saved_x[7] = frame.x7;
    signal_frame.saved_x[8] = frame.x8;
    signal_frame.saved_x[9] = frame.x9;
    signal_frame.saved_x[10] = frame.x10;
    signal_frame.saved_x[11] = frame.x11;
    signal_frame.saved_x[12] = frame.x12;
    signal_frame.saved_x[13] = frame.x13;
    signal_frame.saved_x[14] = frame.x14;
    signal_frame.saved_x[15] = frame.x15;
    signal_frame.saved_x[16] = frame.x16;
    signal_frame.saved_x[17] = frame.x17;
    signal_frame.saved_x[18] = frame.x18;
    signal_frame.saved_x[19] = frame.x19;
    signal_frame.saved_x[20] = frame.x20;
    signal_frame.saved_x[21] = frame.x21;
    signal_frame.saved_x[22] = frame.x22;
    signal_frame.saved_x[23] = frame.x23;
    signal_frame.saved_x[24] = frame.x24;
    signal_frame.saved_x[25] = frame.x25;
    signal_frame.saved_x[26] = frame.x26;
    signal_frame.saved_x[27] = frame.x27;
    signal_frame.saved_x[28] = frame.x28;
    signal_frame.saved_x[29] = frame.x29;
    signal_frame.saved_x[30] = frame.x30;

    // Align stack and make room for signal frame
    let new_sp = (user_sp - SignalFrame::SIZE as u64) & !0xF; // 16-byte align

    // Write signal frame to user stack
    let frame_ptr = new_sp as *mut SignalFrame;
    unsafe {
        *frame_ptr = signal_frame;
    }

    // Block signals during handler (including the signal being handled)
    let blocked_during_handler = process.signals.blocked | action.mask | sig_mask(sig);
    process.signals.set_blocked(blocked_during_handler & !UNCATCHABLE_SIGNALS);

    // Set up registers for signal handler call:
    // x0 = signal number
    // x30 (lr) = restorer address (trampoline)
    // elr = handler address
    // sp_el0 = new stack pointer with signal frame
    frame.x0 = sig as u64;
    frame.x30 = action.restorer;
    frame.elr = handler_addr;

    // Set new stack pointer
    unsafe {
        core::arch::asm!("msr sp_el0, {}", in(reg) new_sp, options(nomem, nostack));
    }

    log::info!(
        "signal_aarch64: delivering signal {} to handler {:#x}, restorer={:#x}, sp={:#x}",
        sig, handler_addr, action.restorer, new_sp
    );
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
