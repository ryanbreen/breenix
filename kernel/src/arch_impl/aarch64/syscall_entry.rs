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
    // Fork needs special handling because it requires access to the frame
    let result = if syscall_num == syscall_nums::FORK {
        sys_fork_aarch64(frame)
    } else {
        dispatch_syscall(syscall_num, arg1, arg2, arg3, arg4, arg5, arg6)
    };

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
// Syscall dispatch (Breenix ABI - same as x86_64 for consistency)
// =============================================================================

/// Syscall numbers (Breenix ABI - matches libbreenix/src/syscall.rs)
/// We use the same syscall numbers across architectures for simplicity.
mod syscall_nums {
    // Core syscalls
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;
    pub const GET_TIME: u64 = 4;
    pub const FORK: u64 = 5;
    pub const CLOSE: u64 = 6;
    pub const POLL: u64 = 7;
    pub const MMAP: u64 = 9;
    pub const MPROTECT: u64 = 10;
    pub const MUNMAP: u64 = 11;
    pub const BRK: u64 = 12;
    // Signal syscalls
    pub const SIGACTION: u64 = 13;
    pub const SIGPROCMASK: u64 = 14;
    pub const SIGRETURN: u64 = 15;
    pub const IOCTL: u64 = 16;
    pub const ACCESS: u64 = 21;
    pub const PIPE: u64 = 22;
    pub const SELECT: u64 = 23;
    pub const DUP: u64 = 32;
    pub const DUP2: u64 = 33;
    pub const PAUSE: u64 = 34;
    pub const GETITIMER: u64 = 36;
    pub const ALARM: u64 = 37;
    pub const SETITIMER: u64 = 38;
    pub const GETPID: u64 = 39;
    pub const SOCKET: u64 = 41;
    pub const CONNECT: u64 = 42;
    pub const ACCEPT: u64 = 43;
    pub const SENDTO: u64 = 44;
    pub const RECVFROM: u64 = 45;
    pub const SHUTDOWN: u64 = 48;
    pub const BIND: u64 = 49;
    pub const LISTEN: u64 = 50;
    pub const SOCKETPAIR: u64 = 53;
    pub const EXEC: u64 = 59;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const FCNTL: u64 = 72;
    pub const GETCWD: u64 = 79;
    pub const CHDIR: u64 = 80;
    pub const RENAME: u64 = 82;
    pub const MKDIR: u64 = 83;
    pub const RMDIR: u64 = 84;
    pub const LINK: u64 = 86;
    pub const UNLINK: u64 = 87;
    pub const SYMLINK: u64 = 88;
    pub const READLINK: u64 = 89;
    pub const SETPGID: u64 = 109;
    pub const SETSID: u64 = 112;
    pub const GETPGID: u64 = 121;
    pub const GETSID: u64 = 124;
    pub const SIGPENDING: u64 = 127;
    pub const SIGSUSPEND: u64 = 130;
    pub const SIGALTSTACK: u64 = 131;
    pub const MKNOD: u64 = 133;
    pub const GETTID: u64 = 186;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const OPEN: u64 = 257;
    pub const LSEEK: u64 = 258;
    pub const FSTAT: u64 = 259;
    pub const GETDENTS64: u64 = 260;
    pub const PIPE2: u64 = 293;
    // PTY syscalls (Breenix-specific)
    pub const POSIX_OPENPT: u64 = 400;
    pub const GRANTPT: u64 = 401;
    pub const UNLOCKPT: u64 = 402;
    pub const PTSNAME: u64 = 403;
    // Graphics syscalls (Breenix-specific)
    pub const FBINFO: u64 = 410;
    pub const FBDRAW: u64 = 411;
    // Testing syscalls (Breenix-specific)
    pub const COW_STATS: u64 = 500;
    pub const SIMULATE_OOM: u64 = 501;

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
    arg4: u64,
    arg5: u64,
    arg6: u64,
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
            // Stub: not implemented yet
            (-38_i64) as u64 // -ENOSYS
        }

        syscall_nums::CLOSE => {
            // Stub: close - just succeed for now
            0
        }

        syscall_nums::BRK => {
            // Use the shared brk implementation
            match crate::syscall::memory::sys_brk(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        // Memory mapping syscalls (use shared implementations)
        syscall_nums::MMAP => {
            match crate::syscall::mmap::sys_mmap(arg1, arg2, arg3 as u32, arg4 as u32, arg5 as i64, arg6) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::MUNMAP => {
            match crate::syscall::mmap::sys_munmap(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::MPROTECT => {
            match crate::syscall::mmap::sys_mprotect(arg1, arg2, arg3 as u32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        // Signal syscalls (stubs)
        syscall_nums::SIGACTION | syscall_nums::SIGPROCMASK | syscall_nums::SIGPENDING |
        syscall_nums::SIGALTSTACK | syscall_nums::KILL | syscall_nums::ALARM |
        syscall_nums::GETITIMER | syscall_nums::SETITIMER => {
            (-38_i64) as u64 // -ENOSYS
        }

        // Pipe and I/O syscalls (stubs)
        syscall_nums::PIPE | syscall_nums::PIPE2 | syscall_nums::DUP | syscall_nums::DUP2 |
        syscall_nums::FCNTL | syscall_nums::POLL | syscall_nums::SELECT | syscall_nums::IOCTL => {
            (-38_i64) as u64 // -ENOSYS
        }

        // Process syscalls (stubs)
        syscall_nums::EXEC | syscall_nums::WAIT4 => {
            (-38_i64) as u64 // -ENOSYS
        }

        // Socket syscalls - use shared implementations
        // Note: TCP (AF_INET) is not supported on ARM64, but Unix domain (AF_UNIX) and UDP work
        syscall_nums::SOCKET => {
            match crate::syscall::socket::sys_socket(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::CONNECT => {
            match crate::syscall::socket::sys_connect(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::ACCEPT => {
            match crate::syscall::socket::sys_accept(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SENDTO => {
            match crate::syscall::socket::sys_sendto(arg1, arg2, arg3, arg4, arg5, arg6) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::RECVFROM => {
            match crate::syscall::socket::sys_recvfrom(arg1, arg2, arg3, arg4, arg5, arg6) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::BIND => {
            match crate::syscall::socket::sys_bind(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::LISTEN => {
            match crate::syscall::socket::sys_listen(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SHUTDOWN => {
            match crate::syscall::socket::sys_shutdown(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SOCKETPAIR => {
            match crate::syscall::socket::sys_socketpair(arg1, arg2, arg3, arg4) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        // Filesystem syscalls (stubs)
        syscall_nums::OPEN | syscall_nums::LSEEK | syscall_nums::FSTAT |
        syscall_nums::GETDENTS64 | syscall_nums::ACCESS | syscall_nums::GETCWD |
        syscall_nums::CHDIR | syscall_nums::RENAME | syscall_nums::MKDIR |
        syscall_nums::RMDIR | syscall_nums::LINK | syscall_nums::UNLINK |
        syscall_nums::SYMLINK | syscall_nums::READLINK | syscall_nums::MKNOD => {
            (-38_i64) as u64 // -ENOSYS
        }

        // Session syscalls (stubs)
        syscall_nums::SETPGID | syscall_nums::SETSID | syscall_nums::GETPGID |
        syscall_nums::GETSID => {
            (-38_i64) as u64 // -ENOSYS
        }

        // PTY syscalls (stubs)
        syscall_nums::POSIX_OPENPT | syscall_nums::GRANTPT |
        syscall_nums::UNLOCKPT | syscall_nums::PTSNAME => {
            (-38_i64) as u64 // -ENOSYS
        }

        // Graphics syscalls (stubs)
        syscall_nums::FBINFO | syscall_nums::FBDRAW => {
            (-38_i64) as u64 // -ENOSYS
        }

        // Testing syscalls (stubs)
        syscall_nums::COW_STATS | syscall_nums::SIMULATE_OOM => {
            (-38_i64) as u64 // -ENOSYS
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
