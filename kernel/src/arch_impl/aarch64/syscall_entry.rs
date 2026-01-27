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
    let result = if syscall_num == syscall_nums::FORK {
        sys_fork_aarch64(frame)
    } else if syscall_num == syscall_nums::EXEC {
        sys_exec_aarch64(frame, arg1, arg2)
    } else if syscall_num == syscall_nums::SIGRETURN {
        // SIGRETURN restores ALL registers from signal frame - don't overwrite X0 after
        match crate::syscall::signal::sys_sigreturn_with_frame_aarch64(frame) {
            crate::syscall::SyscallResult::Ok(_) => {
                // X0 was already restored from signal frame - don't overwrite it
                check_and_deliver_signals_aarch64(frame);
                Aarch64PerCpu::preempt_enable();
                return;
            }
            crate::syscall::SyscallResult::Err(errno) => (-(errno as i64)) as u64,
        }
    } else if syscall_num == syscall_nums::PAUSE {
        match crate::syscall::signal::sys_pause_with_frame_aarch64(frame) {
            crate::syscall::SyscallResult::Ok(r) => r,
            crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
        }
    } else if syscall_num == syscall_nums::SIGSUSPEND {
        match crate::syscall::signal::sys_sigsuspend_with_frame_aarch64(arg1, arg2, frame) {
            crate::syscall::SyscallResult::Ok(r) => r,
            crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
        }
    } else {
        dispatch_syscall(syscall_num, arg1, arg2, arg3, arg4, arg5, arg6, frame)
    };

    // Set return value in X0
    frame.set_return_value(result);

    // Check for pending signals before returning to userspace
    check_and_deliver_signals_aarch64(frame);

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
// Signal delivery on syscall return (ARM64)
// =============================================================================

/// Check for and deliver pending signals before returning from a syscall (ARM64)
///
/// This function checks if the current process has any deliverable signals.
/// If so, it modifies the exception frame to jump to the signal handler
/// instead of returning to the original code.
fn check_and_deliver_signals_aarch64(frame: &mut Aarch64ExceptionFrame) {
    // Get current thread ID
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };

    // Thread 0 is the idle thread - no signals
    if current_thread_id == 0 {
        return;
    }

    // Try to acquire process manager lock (non-blocking)
    let mut manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => return, // Lock held, skip - will happen on next interrupt
    };

    if let Some(ref mut manager) = *manager_guard {
        // Find the process for this thread
        if let Some((_pid, process)) = manager.find_process_by_thread_mut(current_thread_id) {
            // Check alarms
            crate::signal::delivery::check_and_fire_alarm(process);
            crate::signal::delivery::check_and_fire_itimer_real(process, 5000);

            // Check if there are any deliverable signals
            if !crate::signal::delivery::has_deliverable_signals(process) {
                return;
            }

            // Switch to process's page table for signal delivery
            if let Some(ref page_table) = process.page_table {
                let ttbr0 = page_table.level_4_frame().start_address().as_u64();
                unsafe {
                    core::arch::asm!(
                        "dsb ishst",
                        "msr ttbr0_el1, {}",
                        "isb",
                        in(reg) ttbr0,
                        options(nostack)
                    );
                }
            }

            // Read current SP_EL0
            let user_sp = super::context::read_sp_el0();

            // Create SavedRegisters from exception frame
            let mut saved_regs = crate::task::process_context::SavedRegisters::from_exception_frame_with_sp(frame, user_sp);

            // Deliver signals
            let signal_result = crate::signal::delivery::deliver_pending_signals(
                process,
                frame,
                &mut saved_regs,
            );

            // Apply changes back to frame
            saved_regs.apply_to_frame(frame);

            // Update SP_EL0 if it changed
            if saved_regs.sp != user_sp {
                unsafe {
                    super::context::write_sp_el0(saved_regs.sp);
                }
            }

            // Handle termination
            if let crate::signal::delivery::SignalDeliveryResult::Terminated(_notification) = signal_result {
                // Process was terminated by signal - switch to idle
                crate::task::scheduler::set_need_resched();
                // Note: We can't call switch_to_idle here - let the scheduler handle it
            }
        }
    }
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
    _frame: &mut Aarch64ExceptionFrame,
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

        syscall_nums::WRITE | syscall_nums::ARM64_WRITE => {
            match crate::syscall::io::sys_write(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::READ => {
            match crate::syscall::io::sys_read(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        syscall_nums::CLOSE => {
            match crate::syscall::pipe::sys_close(arg1 as i32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
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

        // Signal syscalls - now using shared implementations
        syscall_nums::KILL => {
            match crate::syscall::signal::sys_kill(arg1 as i64, arg2 as i32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SIGACTION => {
            match crate::syscall::signal::sys_sigaction(arg1 as i32, arg2, arg3, arg4) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SIGPROCMASK => {
            match crate::syscall::signal::sys_sigprocmask(arg1 as i32, arg2, arg3, arg4) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SIGPENDING => {
            match crate::syscall::signal::sys_sigpending(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SIGALTSTACK => {
            match crate::syscall::signal::sys_sigaltstack(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::ALARM => {
            match crate::syscall::signal::sys_alarm(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::GETITIMER => {
            match crate::syscall::signal::sys_getitimer(arg1 as i32, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SETITIMER => {
            match crate::syscall::signal::sys_setitimer(arg1 as i32, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        // Note: PAUSE, SIGSUSPEND, and SIGRETURN are handled specially in rust_syscall_handler_aarch64
        // because they need access to the frame

        syscall_nums::PIPE => {
            match crate::syscall::pipe::sys_pipe(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::PIPE2 => {
            match crate::syscall::pipe::sys_pipe2(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::IOCTL => {
            match crate::syscall::ioctl::sys_ioctl(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::DUP => {
            match crate::syscall::io::sys_dup(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::DUP2 => {
            match crate::syscall::io::sys_dup2(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::FCNTL => {
            match crate::syscall::io::sys_fcntl(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::POLL => {
            match crate::syscall::io::sys_poll(arg1, arg2, arg3 as i32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SELECT => {
            match crate::syscall::io::sys_select(arg1 as i32, arg2, arg3, arg4, arg5) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
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

        // Filesystem syscalls
        syscall_nums::OPEN => {
            match crate::syscall::fs::sys_open(arg1, arg2 as u32, arg3 as u32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::LSEEK => {
            match crate::syscall::fs::sys_lseek(arg1 as i32, arg2 as i64, arg3 as i32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::FSTAT => {
            match crate::syscall::fs::sys_fstat(arg1 as i32, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::GETDENTS64 => {
            match crate::syscall::fs::sys_getdents64(arg1 as i32, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::ACCESS => {
            match crate::syscall::fs::sys_access(arg1, arg2 as u32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::GETCWD => {
            match crate::syscall::fs::sys_getcwd(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::CHDIR => {
            match crate::syscall::fs::sys_chdir(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::RENAME => {
            match crate::syscall::fs::sys_rename(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::MKDIR => {
            match crate::syscall::fs::sys_mkdir(arg1, arg2 as u32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::RMDIR => {
            match crate::syscall::fs::sys_rmdir(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::LINK => {
            match crate::syscall::fs::sys_link(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::UNLINK => {
            match crate::syscall::fs::sys_unlink(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SYMLINK => {
            match crate::syscall::fs::sys_symlink(arg1, arg2) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::READLINK => {
            match crate::syscall::fs::sys_readlink(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::MKNOD => {
            match crate::syscall::fifo::sys_mknod(arg1, arg2 as u32, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        // Session syscalls
        syscall_nums::SETPGID => {
            match crate::syscall::session::sys_setpgid(arg1 as i32, arg2 as i32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::SETSID => {
            match crate::syscall::session::sys_setsid() {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::GETPGID => {
            match crate::syscall::session::sys_getpgid(arg1 as i32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::GETSID => {
            match crate::syscall::session::sys_getsid(arg1 as i32) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        // PTY syscalls
        syscall_nums::POSIX_OPENPT => {
            match crate::syscall::pty::sys_posix_openpt(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::GRANTPT => {
            match crate::syscall::pty::sys_grantpt(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::UNLOCKPT => {
            match crate::syscall::pty::sys_unlockpt(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::PTSNAME => {
            match crate::syscall::pty::sys_ptsname(arg1, arg2, arg3) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        // Graphics syscalls
        syscall_nums::FBINFO => {
            match crate::syscall::graphics::sys_fbinfo(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        syscall_nums::FBDRAW => {
            match crate::syscall::graphics::sys_fbdraw(arg1) {
                crate::syscall::SyscallResult::Ok(result) => result,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }

        // Testing syscalls (stubs)
        syscall_nums::COW_STATS | syscall_nums::SIMULATE_OOM => {
            (-38_i64) as u64 // -ENOSYS
        }

        syscall_nums::GETPID => sys_getpid(),

        syscall_nums::GETTID => sys_gettid(),

        syscall_nums::YIELD => {
            crate::task::scheduler::yield_current();
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

/// sys_getpid - Get the current process ID (ARM64)
fn sys_getpid() -> u64 {
    let thread_id = crate::task::scheduler::current_thread_id().unwrap_or(0);
    if thread_id == 0 {
        return 0;
    }

    if let Some(ref manager) = *crate::process::manager() {
        if let Some((pid, _process)) = manager.find_process_by_thread(thread_id) {
            return pid.as_u64();
        }
    }

    0
}

/// sys_gettid - Get the current thread ID (ARM64)
fn sys_gettid() -> u64 {
    crate::task::scheduler::current_thread_id().unwrap_or(0)
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
// Exec syscall implementation for ARM64
// =============================================================================

/// sys_exec for ARM64 - Replace current process with a new program
///
/// This function replaces the current process's address space with a new program.
/// On ARM64, we update the exception frame to point to the new program's entry point.
///
/// Arguments:
/// - frame: Mutable reference to the exception frame (to update ELR_EL1/SP_EL0)
/// - program_name_ptr: Pointer to null-terminated program name string
/// - argv_ptr: Pointer to argv array (unused in this simplified implementation)
///
/// Returns:
/// - 0 on success (though exec() never returns on success)
/// - Negative errno on error
fn sys_exec_aarch64(
    frame: &mut Aarch64ExceptionFrame,
    program_name_ptr: u64,
    _argv_ptr: u64,
) -> u64 {
    without_interrupts(|| {
        log::info!(
            "sys_exec_aarch64: program_name_ptr={:#x}",
            program_name_ptr
        );

        // Get current thread ID from scheduler
        let current_thread_id = match crate::task::scheduler::current_thread_id() {
            Some(id) => id,
            None => {
                log::error!("sys_exec_aarch64: No current thread");
                return (-22_i64) as u64; // -EINVAL
            }
        };

        if current_thread_id == 0 {
            log::error!("sys_exec_aarch64: Cannot exec from idle thread");
            return (-22_i64) as u64; // -EINVAL
        }

        // Exec is only supported with the testing feature for now
        #[cfg(not(feature = "testing"))]
        {
            let _ = frame;
            log::error!("sys_exec_aarch64: Testing feature not enabled");
            return (-38_i64) as u64; // -ENOSYS
        }

        #[cfg(feature = "testing")]
        {
            // Get the ELF data and program name
            let (elf_data, exec_program_name): (&[u8], Option<&'static str>) =
                if program_name_ptr != 0 {
                    // Read program name from userspace
                    let program_name = unsafe {
                        let mut len = 0;
                        let ptr = program_name_ptr as *const u8;
                        while *ptr.add(len) != 0 && len < 256 {
                            len += 1;
                        }
                        core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr, len))
                    };

                    log::info!("sys_exec_aarch64: Loading program '{}'", program_name);

                    // Load the binary from the test disk by name
                    let elf_vec = crate::userspace_test::get_test_binary(program_name);
                    // Leak the vector to get a static slice (needed for exec_process)
                    let boxed_slice = elf_vec.into_boxed_slice();
                    let elf_data = Box::leak(boxed_slice) as &'static [u8];
                    // Also leak the program name so we can pass it to exec_process
                    let name_string = alloc::string::String::from(program_name);
                    let leaked_name: &'static str = Box::leak(name_string.into_boxed_str());
                    (elf_data, Some(leaked_name))
                } else {
                    log::info!("sys_exec_aarch64: Using default hello_world test program");
                    (
                        crate::userspace_test::get_test_binary_static("hello_world"),
                        Some("hello_world"),
                    )
                };

            // Find current process
            let current_pid = {
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    if let Some((pid, _)) = manager.find_process_by_thread(current_thread_id) {
                        pid
                    } else {
                        log::error!(
                            "sys_exec_aarch64: Thread {} not found in any process",
                            current_thread_id
                        );
                        return (-3_i64) as u64; // -ESRCH
                    }
                } else {
                    log::error!("sys_exec_aarch64: Process manager not available");
                    return (-12_i64) as u64; // -ENOMEM
                }
            };

            log::info!(
                "sys_exec_aarch64: Replacing process {} (thread {}) with new program",
                current_pid.as_u64(),
                current_thread_id
            );

            // Replace the process's address space
            let mut manager_guard = crate::process::manager();
            if let Some(ref mut manager) = *manager_guard {
                match manager.exec_process(current_pid, elf_data, exec_program_name) {
                    Ok(new_entry_point) => {
                        log::info!(
                            "sys_exec_aarch64: Successfully replaced process address space, entry point: {:#x}",
                            new_entry_point
                        );

                        // Get the new stack pointer from the exec'd process
                        // NOTE: Must match the value used in exec_process() in manager.rs
                        const USER_STACK_TOP: u64 = 0x7FFF_FF01_0000;
                        // SP must be 16-byte aligned on ARM64
                        let new_sp = USER_STACK_TOP & !0xF;

                        // Update the exception frame to jump to the new program
                        frame.elr = new_entry_point; // Entry point (PC on return)

                        // Update SP_EL0 via MSR instruction
                        unsafe {
                            core::arch::asm!(
                                "msr sp_el0, {}",
                                in(reg) new_sp,
                                options(nomem, nostack)
                            );
                        }

                        // Clear all general-purpose registers in the frame for security
                        frame.x0 = 0;
                        frame.x1 = 0;
                        frame.x2 = 0;
                        frame.x3 = 0;
                        frame.x4 = 0;
                        frame.x5 = 0;
                        frame.x6 = 0;
                        frame.x7 = 0;
                        frame.x8 = 0;
                        frame.x9 = 0;
                        frame.x10 = 0;
                        frame.x11 = 0;
                        frame.x12 = 0;
                        frame.x13 = 0;
                        frame.x14 = 0;
                        frame.x15 = 0;
                        frame.x16 = 0;
                        frame.x17 = 0;
                        frame.x18 = 0;
                        frame.x19 = 0;
                        frame.x20 = 0;
                        frame.x21 = 0;
                        frame.x22 = 0;
                        frame.x23 = 0;
                        frame.x24 = 0;
                        frame.x25 = 0;
                        frame.x26 = 0;
                        frame.x27 = 0;
                        frame.x28 = 0;
                        frame.x29 = 0; // Frame pointer
                        frame.x30 = 0; // Link register

                        // Set SPSR for EL0t mode with interrupts enabled
                        frame.spsr = 0x0; // EL0t, DAIF clear

                        // Update TTBR0_EL1 for the new process page table
                        if let Some(process) = manager.get_process(current_pid) {
                            if let Some(ref page_table) = process.page_table {
                                let new_ttbr0 = page_table.level_4_frame().start_address().as_u64();
                                log::info!("sys_exec_aarch64: Setting TTBR0_EL1 to {:#x}", new_ttbr0);
                                unsafe {
                                    core::arch::asm!(
                                        "dsb ishst",       // Ensure stores complete
                                        "msr ttbr0_el1, {}", // Switch page table
                                        "isb",             // Instruction synchronization barrier
                                        "tlbi vmalle1is",  // Invalidate TLB
                                        "dsb ish",         // Ensure TLB invalidation completes
                                        "isb",             // Synchronize
                                        in(reg) new_ttbr0,
                                        options(nostack)
                                    );
                                }
                            }
                        }

                        log::info!(
                            "sys_exec_aarch64: Frame updated - ELR={:#x}, SP_EL0={:#x}",
                            frame.elr,
                            new_sp
                        );

                        // exec() returns 0 on success (but caller never sees it because
                        // we're jumping to a new program)
                        0
                    }
                    Err(e) => {
                        log::error!("sys_exec_aarch64: Failed to exec process: {}", e);
                        (-12_i64) as u64 // -ENOMEM
                    }
                }
            } else {
                log::error!("sys_exec_aarch64: Process manager not available");
                (-12_i64) as u64 // -ENOMEM
            }
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
