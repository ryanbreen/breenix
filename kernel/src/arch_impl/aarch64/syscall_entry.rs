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
use crate::tracing::providers::syscall::{trace_entry, trace_exit};

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

/// Emit one-time marker when first syscall from EL0 (userspace) is received.
/// Uses raw UART writes to avoid any locks (safe in syscall context).
/// Also advances test framework to Userspace stage if boot_tests is enabled.
#[inline(never)]
fn emit_el0_syscall_marker() {
    // PL011 UART virtual address (physical 0x0900_0000 mapped via HHDM)
    // The HHDM base is 0xFFFF_0000_0000_0000, so UART is at that + 0x0900_0000
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    const PL011_PHYS: u64 = 0x0900_0000;
    const PL011_VIRT: u64 = HHDM_BASE + PL011_PHYS;

    let msg = b"EL0_SYSCALL: First syscall from userspace (SPSR confirms EL0)\n[ OK ] syscall path verified\n";
    for &byte in msg {
        unsafe {
            core::ptr::write_volatile(PL011_VIRT as *mut u8, byte);
        }
    }

    // Advance test framework to Userspace stage - we have confirmed EL0 execution
    // Note: We use advance_stage_marker_only() instead of advance_to_stage() because
    // we're in syscall context and cannot spawn kthreads or block on joins here.
    // The Userspace stage tests verify is_el0_confirmed() which is already true.
    #[cfg(feature = "boot_tests")]
    {
        crate::test_framework::advance_stage_marker_only(
            crate::test_framework::TestStage::Userspace
        );
    }
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
    // Increment preempt count FIRST (prevents scheduling during syscall)
    // CRITICAL: No logging before this point - timer interrupt + logger lock = deadlock
    Aarch64PerCpu::preempt_disable();

    // Check if this is from EL0 (userspace) by examining SPSR
    let from_el0 = (frame.spsr & 0xF) == 0; // M[3:0] = 0 means EL0

    // Verify this came from userspace (security check)
    if !from_el0 {
        // Don't log here - just return error
        frame.set_return_value(u64::MAX); // Error
        Aarch64PerCpu::preempt_enable();
        return;
    }

    // One-time marker for first syscall from EL0 (userspace confirmed)
    // This is the ARM64 equivalent of x86_64's RING3_CONFIRMED marker
    if !EL0_CONFIRMED.swap(true, Ordering::Relaxed) {
        // First syscall from userspace! Print marker via raw UART (no locks)
        emit_el0_syscall_marker();
    }

    let syscall_num = frame.syscall_number();
    trace_entry(syscall_num);
    let arg1 = frame.arg1();
    let arg2 = frame.arg2();
    let arg3 = frame.arg3();
    let arg4 = frame.arg4();
    let arg5 = frame.arg5();
    let arg6 = frame.arg6();

    // Dispatch to syscall handler.
    // Some syscalls need special handling because they require access to the frame.
    // Resolve the raw number to a SyscallNumber via from_u64().
    use crate::syscall::SyscallNumber;

    let resolved_num = SyscallNumber::from_u64(syscall_num);

    let result = match resolved_num {
        Some(SyscallNumber::Fork) => sys_fork_aarch64(frame),
        Some(SyscallNumber::Exec) => {
            let exec_result = sys_exec_aarch64(frame, arg1, arg2);
            super::trace::trace_exec(b'H');
            exec_result
        }
        Some(SyscallNumber::Sigreturn) => {
            // SIGRETURN restores ALL registers from signal frame - don't overwrite X0 after
            match crate::syscall::signal::sys_sigreturn_with_frame_aarch64(frame) {
                crate::syscall::SyscallResult::Ok(_) => {
                    check_and_deliver_signals_aarch64(frame);
                    Aarch64PerCpu::preempt_enable();
                    return;
                }
                crate::syscall::SyscallResult::Err(errno) => (-(errno as i64)) as u64,
            }
        }
        Some(SyscallNumber::Pause) => {
            match crate::syscall::signal::sys_pause_with_frame_aarch64(frame) {
                crate::syscall::SyscallResult::Ok(r) => r,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        Some(SyscallNumber::Sigsuspend) => {
            match crate::syscall::signal::sys_sigsuspend_with_frame_aarch64(arg1, arg2, frame) {
                crate::syscall::SyscallResult::Ok(r) => r,
                crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
            }
        }
        Some(SyscallNumber::Clone) => {
            // ARM64 has no fork syscall; libbreenix emulates fork via clone(SIGCHLD, 0, 0, 0, 0).
            // Detect fork-style clone (no CLONE_VM) and route to sys_fork_aarch64 which
            // needs the exception frame to snapshot parent registers.
            const CLONE_VM: u64 = 0x00000100;
            if arg1 & CLONE_VM == 0 {
                sys_fork_aarch64(frame)
            } else {
                result_to_u64(crate::syscall::clone::sys_clone(arg1, arg2, arg3, arg4, arg5))
            }
        }
        Some(syscall) => dispatch_syscall_enum(syscall, arg1, arg2, arg3, arg4, arg5, arg6, frame),
        None => {
            crate::serial_println!("[syscall] Unknown ARM64 syscall {} - returning ENOSYS", syscall_num);
            (-38_i64) as u64 // -ENOSYS
        }
    };

    // Set return value in X0
    trace_exit(result as i64);
    frame.set_return_value(result);

    // Check for pending signals before returning to userspace
    check_and_deliver_signals_aarch64(frame);

    // Trace: about to return from syscall handler to assembly (will ERET)
    super::trace::trace_exec(b'D');

    // Decrement preempt count on syscall exit
    Aarch64PerCpu::preempt_enable();
}

/// Trace function called before ERET to EL0 (for debugging).
///
/// This is intentionally minimal to avoid slowing down the return path.
#[no_mangle]
pub extern "C" fn trace_eret_to_el0(_elr: u64, _spsr: u64) {
    // Trace: about to ERET back to userspace
    super::trace::trace_exec(b'B');
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

    // Track if signal termination happened (for parent notification after lock release)
    let mut signal_termination_info: Option<crate::signal::delivery::ParentNotification> = None;

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
                    // CRITICAL: Flush TLB after TTBR0 switch for CoW correctness
                    core::arch::asm!(
                        "dsb ishst",           // Ensure previous stores complete
                        "msr ttbr0_el1, {}",   // Set new page table
                        "isb",                 // Synchronize context
                        "tlbi vmalle1is",      // FLUSH ENTIRE TLB
                        "dsb ish",             // Ensure TLB flush completes
                        "isb",                 // Synchronize instruction stream
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
            if let crate::signal::delivery::SignalDeliveryResult::Terminated(notification) = signal_result {
                // Process was terminated by signal - switch to idle
                crate::task::scheduler::set_need_resched();
                // Save notification to notify parent after releasing lock
                signal_termination_info = Some(notification);
            }
        }
    }

    // Drop manager guard BEFORE notifying parent to avoid deadlock
    drop(manager_guard);

    // Notify parent if signal terminated a child - this unblocks waitpid()
    if let Some(notification) = signal_termination_info {
        crate::signal::delivery::notify_parent_of_termination_deferred(&notification);
    }
}

// =============================================================================
// Syscall dispatch
// =============================================================================

/// Convert SyscallResult to raw u64 return value (positive or negative errno)
#[inline]
fn result_to_u64(result: crate::syscall::SyscallResult) -> u64 {
    match result {
        crate::syscall::SyscallResult::Ok(v) => v,
        crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
    }
}

/// ARM64-specific exit implementation.
///
/// This is separate from the shared dispatcher because ARM64 needs to:
/// 1. Use `wfi` (not `hlt`) when no more userspace threads remain
/// 2. Inline the exit logic since `handlers::sys_exit` is x86_64-only
///
/// CRITICAL: This function must NEVER return. After terminating the thread,
/// it enters a WFI loop. The timer interrupt will fire and context-switch
/// to another thread; the terminated thread will never be re-scheduled.
/// If this function returned, the userspace exit() caller (e.g., musl's
/// `for(;;) __syscall(SYS_exit, ec)` loop) would re-enter exit, causing
/// double-terminate and double-decrement of COW page refcounts.
fn sys_exit_aarch64(exit_code: i32) -> u64 {
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        // Handle clear_child_tid for clone threads (CLONE_CHILD_CLEARTID).
        // Extract info under PM lock, but do NOT log while holding it.
        let (pid_for_log, name_for_log, futex_info) = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                    let pid_val = _pid.as_u64();
                    let name_val = process.name.clone();
                    let futex = process.clear_child_tid.map(|tid_addr| {
                        let tg_id = process.thread_group_id.unwrap_or(pid_val);
                        unsafe {
                            let ptr = tid_addr as *mut u32;
                            if !ptr.is_null() && tid_addr < 0x7FFF_FFFF_FFFF {
                                core::ptr::write_volatile(ptr, 0);
                            }
                        }
                        (tg_id, tid_addr)
                    });
                    (Some(pid_val), Some(name_val), futex)
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            }
        }; // PM lock dropped here

        // Futex wake outside PM lock
        if let Some((tg_id, tid_addr)) = futex_info {
            crate::syscall::futex::futex_wake_for_thread_group(tg_id, tid_addr, u32::MAX);
        }

        // Log outside PM lock
        if let (Some(pid), Some(name)) = (pid_for_log, &name_for_log) {
            crate::serial_println!("[syscall] exit({}) pid={} name={}", exit_code, pid, name);
        } else {
            crate::serial_println!("[syscall] exit({}) thread={}", exit_code, thread_id);
        }

        crate::task::process_task::ProcessScheduler::handle_thread_exit(thread_id, exit_code);

        crate::task::scheduler::with_scheduler(|scheduler| {
            if let Some(thread) = scheduler.current_thread_mut() {
                thread.set_terminated();
            }
        });

        let has_other_userspace_threads =
            crate::task::scheduler::with_scheduler(|sched| sched.has_userspace_threads())
                .unwrap_or(false);

        if !has_other_userspace_threads {
            crate::serial_println!();
            crate::serial_println!("========================================");
            crate::serial_println!("  Userspace Test Complete!");
            crate::serial_println!("  Exit code: {}", exit_code);
            crate::serial_println!("========================================");
            crate::serial_println!();
        }
    }

    // Re-enable preemption (balances the preempt_disable in rust_syscall_handler_aarch64)
    // so timer interrupts can trigger context-switch to another thread.
    Aarch64PerCpu::preempt_enable();

    // NEVER return to userspace. The thread is terminated; wait for the timer
    // interrupt to context-switch away. The scheduler will not re-schedule a
    // terminated thread, so this loop runs at most until the next timer tick.
    //
    // CRITICAL: Must unmask IRQ before WFI. The syscall entry assembly masks IRQ
    // (daifset #0x2) and we never return to the assembly epilogue (which would
    // call check_need_resched_and_switch_arm64 and restore interrupt state via
    // ERET). Without unmasking IRQ here, the timer interrupt is pending but never
    // handled — this CPU becomes permanently stuck, unable to process deferred
    // thread requeues or context-switch to other threads.
    loop {
        unsafe {
            core::arch::asm!(
                "msr daifclr, #2",  // Unmask IRQ so timer interrupt can fire
                "wfi",              // Wait for interrupt — timer will context-switch us away
                options(nomem, nostack)
            );
        }
    }
}

/// Dispatch a syscall to the appropriate handler using the resolved SyscallNumber.
///
/// Uses the shared SyscallNumber enum to ensure new syscalls are automatically
/// picked up by both architectures. Only EXIT requires arch-specific handling
/// (wfi vs hlt). All other syscalls delegate to shared implementations.
///
/// Returns the syscall result (positive for success, negative errno for error).
fn dispatch_syscall_enum(
    syscall: crate::syscall::SyscallNumber,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    arg6: u64,
    _frame: &mut Aarch64ExceptionFrame,
) -> u64 {
    use crate::syscall::SyscallNumber;

    // Dispatch using the shared enum — adding a new SyscallNumber variant
    // without adding a match arm here will produce a compiler warning.
    match syscall {
        // EXIT is arch-specific (uses wfi instead of hlt)
        SyscallNumber::Exit | SyscallNumber::ExitGroup => sys_exit_aarch64(arg1 as i32),

        // FORK, EXEC, SIGRETURN, PAUSE, SIGSUSPEND are handled before
        // dispatch_syscall_enum is called (they need frame access).
        // If they somehow reach here, return ENOSYS.
        SyscallNumber::Fork | SyscallNumber::Exec | SyscallNumber::Sigreturn => (-38_i64) as u64,
        // PAUSE and SIGSUSPEND also handled before dispatch
        SyscallNumber::Pause | SyscallNumber::Sigsuspend => (-38_i64) as u64,

        // I/O syscalls (ARM64 io module)
        SyscallNumber::Write => result_to_u64(crate::syscall::handlers::sys_write(arg1, arg2, arg3)),
        SyscallNumber::Read => result_to_u64(crate::syscall::handlers::sys_read(arg1, arg2, arg3)),
        SyscallNumber::Close => result_to_u64(crate::syscall::pipe::sys_close(arg1 as i32)),
        SyscallNumber::Dup => result_to_u64(crate::syscall::handlers::sys_dup(arg1)),
        SyscallNumber::Dup2 => result_to_u64(crate::syscall::handlers::sys_dup2(arg1, arg2)),
        SyscallNumber::Fcntl => result_to_u64(crate::syscall::handlers::sys_fcntl(arg1, arg2, arg3)),
        SyscallNumber::Poll => result_to_u64(crate::syscall::handlers::sys_poll(arg1, arg2, arg3 as i32)),
        SyscallNumber::Select => result_to_u64(crate::syscall::handlers::sys_select(arg1 as i32, arg2, arg3, arg4, arg5)),
        SyscallNumber::Ioctl => result_to_u64(crate::syscall::ioctl::sys_ioctl(arg1, arg2, arg3)),
        SyscallNumber::Pipe => result_to_u64(crate::syscall::pipe::sys_pipe(arg1)),
        SyscallNumber::Pipe2 => result_to_u64(crate::syscall::pipe::sys_pipe2(arg1, arg2)),

        // Memory syscalls
        SyscallNumber::Brk => result_to_u64(crate::syscall::memory::sys_brk(arg1)),
        SyscallNumber::Mmap => result_to_u64(crate::syscall::mmap::sys_mmap(arg1, arg2, arg3 as u32, arg4 as u32, arg5 as i64, arg6)),
        SyscallNumber::Munmap => result_to_u64(crate::syscall::mmap::sys_munmap(arg1, arg2)),
        SyscallNumber::Mprotect => result_to_u64(crate::syscall::mmap::sys_mprotect(arg1, arg2, arg3 as u32)),

        // Signal syscalls
        SyscallNumber::Kill => result_to_u64(crate::syscall::signal::sys_kill(arg1 as i64, arg2 as i32)),
        SyscallNumber::Sigaction => result_to_u64(crate::syscall::signal::sys_sigaction(arg1 as i32, arg2, arg3, arg4)),
        SyscallNumber::Sigprocmask => result_to_u64(crate::syscall::signal::sys_sigprocmask(arg1 as i32, arg2, arg3, arg4)),
        SyscallNumber::Sigpending => result_to_u64(crate::syscall::signal::sys_sigpending(arg1, arg2)),
        SyscallNumber::Sigaltstack => result_to_u64(crate::syscall::signal::sys_sigaltstack(arg1, arg2)),
        SyscallNumber::Alarm => result_to_u64(crate::syscall::signal::sys_alarm(arg1)),
        SyscallNumber::Getitimer => result_to_u64(crate::syscall::signal::sys_getitimer(arg1 as i32, arg2)),
        SyscallNumber::Setitimer => result_to_u64(crate::syscall::signal::sys_setitimer(arg1 as i32, arg2, arg3)),

        // Process syscalls
        SyscallNumber::GetPid => sys_getpid(),
        SyscallNumber::Getppid => sys_getppid(),
        SyscallNumber::GetTid => sys_gettid(),
        SyscallNumber::SetTidAddress => crate::task::scheduler::current_thread_id().unwrap_or(0),
        SyscallNumber::Wait4 => result_to_u64(crate::syscall::wait::sys_waitpid(arg1 as i64, arg2, arg3 as u32)),
        SyscallNumber::Yield => { crate::task::scheduler::yield_current(); 0 }
        SyscallNumber::GetTime => sys_get_time(),
        SyscallNumber::ClockGetTime => sys_clock_gettime(arg1 as u32, arg2 as *mut Timespec),
        SyscallNumber::Nanosleep => result_to_u64(crate::syscall::time::sys_nanosleep(arg1, arg2)),
        SyscallNumber::Clone => result_to_u64(crate::syscall::clone::sys_clone(arg1, arg2, arg3, arg4, arg5)),
        SyscallNumber::Futex => result_to_u64(crate::syscall::futex::sys_futex(arg1, arg2 as u32, arg3 as u32, arg4, arg5, arg6 as u32)),
        SyscallNumber::GetRandom => result_to_u64(crate::syscall::random::sys_getrandom(arg1, arg2, arg3 as u32)),

        // Session syscalls
        SyscallNumber::SetPgid => result_to_u64(crate::syscall::session::sys_setpgid(arg1 as i32, arg2 as i32)),
        SyscallNumber::SetSid => result_to_u64(crate::syscall::session::sys_setsid()),
        SyscallNumber::GetPgid => result_to_u64(crate::syscall::session::sys_getpgid(arg1 as i32)),
        SyscallNumber::GetSid => result_to_u64(crate::syscall::session::sys_getsid(arg1 as i32)),

        // Filesystem syscalls
        SyscallNumber::Access => result_to_u64(crate::syscall::fs::sys_access(arg1, arg2 as u32)),
        SyscallNumber::Getcwd => result_to_u64(crate::syscall::fs::sys_getcwd(arg1, arg2)),
        SyscallNumber::Chdir => result_to_u64(crate::syscall::fs::sys_chdir(arg1)),
        SyscallNumber::Open => result_to_u64(crate::syscall::fs::sys_open(arg1, arg2 as u32, arg3 as u32)),
        SyscallNumber::Lseek => result_to_u64(crate::syscall::fs::sys_lseek(arg1 as i32, arg2 as i64, arg3 as i32)),
        SyscallNumber::Fstat => result_to_u64(crate::syscall::fs::sys_fstat(arg1 as i32, arg2)),
        SyscallNumber::Getdents64 => result_to_u64(crate::syscall::fs::sys_getdents64(arg1 as i32, arg2, arg3)),
        SyscallNumber::Rename => result_to_u64(crate::syscall::fs::sys_rename(arg1, arg2)),
        SyscallNumber::Mkdir => result_to_u64(crate::syscall::fs::sys_mkdir(arg1, arg2 as u32)),
        SyscallNumber::Rmdir => result_to_u64(crate::syscall::fs::sys_rmdir(arg1)),
        SyscallNumber::Link => result_to_u64(crate::syscall::fs::sys_link(arg1, arg2)),
        SyscallNumber::Unlink => result_to_u64(crate::syscall::fs::sys_unlink(arg1)),
        SyscallNumber::Symlink => result_to_u64(crate::syscall::fs::sys_symlink(arg1, arg2)),
        SyscallNumber::Readlink => result_to_u64(crate::syscall::fs::sys_readlink(arg1, arg2, arg3)),
        SyscallNumber::Mknod => result_to_u64(crate::syscall::fifo::sys_mknod(arg1, arg2 as u32, arg3)),

        // *at variants (ARM64 Linux uses these instead of legacy syscalls)
        SyscallNumber::Openat => result_to_u64(crate::syscall::fs::sys_openat(arg1 as i32, arg2, arg3 as u32, arg4 as u32)),
        SyscallNumber::Faccessat => result_to_u64(crate::syscall::fs::sys_faccessat(arg1 as i32, arg2, arg3 as u32, arg4 as u32)),
        SyscallNumber::Mkdirat => result_to_u64(crate::syscall::fs::sys_mkdirat(arg1 as i32, arg2, arg3 as u32)),
        SyscallNumber::Mknodat => result_to_u64(crate::syscall::fs::sys_mknodat(arg1 as i32, arg2, arg3 as u32, arg4)),
        SyscallNumber::Unlinkat => result_to_u64(crate::syscall::fs::sys_unlinkat(arg1 as i32, arg2, arg3 as i32)),
        SyscallNumber::Symlinkat => result_to_u64(crate::syscall::fs::sys_symlinkat(arg1, arg2 as i32, arg3)),
        SyscallNumber::Linkat => result_to_u64(crate::syscall::fs::sys_linkat(arg1 as i32, arg2, arg3 as i32, arg4, arg5 as i32)),
        SyscallNumber::Renameat => result_to_u64(crate::syscall::fs::sys_renameat(arg1 as i32, arg2, arg3 as i32, arg4)),
        SyscallNumber::Readlinkat => result_to_u64(crate::syscall::fs::sys_readlinkat(arg1 as i32, arg2, arg3, arg4)),
        SyscallNumber::Dup3 => result_to_u64(crate::syscall::handlers::sys_dup2(arg1, arg2)), // dup3 with flags=0 is dup2
        SyscallNumber::Pselect6 => result_to_u64(crate::syscall::handlers::sys_select(arg1 as i32, arg2, arg3, arg4, arg5)), // simplified

        // PTY syscalls
        SyscallNumber::PosixOpenpt => result_to_u64(crate::syscall::pty::sys_posix_openpt(arg1)),
        SyscallNumber::Grantpt => result_to_u64(crate::syscall::pty::sys_grantpt(arg1)),
        SyscallNumber::Unlockpt => result_to_u64(crate::syscall::pty::sys_unlockpt(arg1)),
        SyscallNumber::Ptsname => result_to_u64(crate::syscall::pty::sys_ptsname(arg1, arg2, arg3)),

        // Socket syscalls
        SyscallNumber::Socket => result_to_u64(crate::syscall::socket::sys_socket(arg1, arg2, arg3)),
        SyscallNumber::Connect => result_to_u64(crate::syscall::socket::sys_connect(arg1, arg2, arg3)),
        SyscallNumber::Accept => result_to_u64(crate::syscall::socket::sys_accept(arg1, arg2, arg3)),
        SyscallNumber::SendTo => result_to_u64(crate::syscall::socket::sys_sendto(arg1, arg2, arg3, arg4, arg5, arg6)),
        SyscallNumber::RecvFrom => result_to_u64(crate::syscall::socket::sys_recvfrom(arg1, arg2, arg3, arg4, arg5, arg6)),
        SyscallNumber::Bind => result_to_u64(crate::syscall::socket::sys_bind(arg1, arg2, arg3)),
        SyscallNumber::Listen => result_to_u64(crate::syscall::socket::sys_listen(arg1, arg2)),
        SyscallNumber::Shutdown => result_to_u64(crate::syscall::socket::sys_shutdown(arg1, arg2)),
        SyscallNumber::Socketpair => result_to_u64(crate::syscall::socket::sys_socketpair(arg1, arg2, arg3, arg4)),
        SyscallNumber::Getsockname => result_to_u64(crate::syscall::socket::sys_getsockname(arg1, arg2, arg3)),
        SyscallNumber::Getpeername => result_to_u64(crate::syscall::socket::sys_getpeername(arg1, arg2, arg3)),
        SyscallNumber::Setsockopt => result_to_u64(crate::syscall::socket::sys_setsockopt(arg1, arg2, arg3, arg4, arg5)),
        SyscallNumber::Getsockopt => result_to_u64(crate::syscall::socket::sys_getsockopt(arg1, arg2, arg3, arg4, arg5)),

        // Graphics syscalls
        SyscallNumber::FbInfo => result_to_u64(crate::syscall::graphics::sys_fbinfo(arg1)),
        SyscallNumber::FbDraw => result_to_u64(crate::syscall::graphics::sys_fbdraw(arg1)),
        SyscallNumber::FbMmap => result_to_u64(crate::syscall::graphics::sys_fbmmap()),
        SyscallNumber::GetMousePos => result_to_u64(crate::syscall::graphics::sys_get_mouse_pos(arg1)),

        // Audio syscalls
        SyscallNumber::AudioInit => result_to_u64(crate::syscall::audio::sys_audio_init()),
        SyscallNumber::AudioWrite => result_to_u64(crate::syscall::audio::sys_audio_write(arg1, arg2)),

        // Display takeover
        SyscallNumber::TakeOverDisplay => result_to_u64(crate::syscall::handlers::sys_take_over_display()),
        SyscallNumber::GiveBackDisplay => result_to_u64(crate::syscall::handlers::sys_give_back_display()),
        // Vectored I/O
        SyscallNumber::Readv => result_to_u64(crate::syscall::iovec::sys_readv(arg1, arg2, arg3)),
        SyscallNumber::Writev => result_to_u64(crate::syscall::iovec::sys_writev(arg1, arg2, arg3)),
        // Stubs for musl libc compatibility
        SyscallNumber::Mremap => (-(crate::syscall::errno::ENOMEM as i64)) as u64,
        SyscallNumber::Madvise => 0,
        SyscallNumber::Ppoll => result_to_u64(crate::syscall::handlers::sys_ppoll(arg1, arg2, arg3, arg4, arg5)),
        SyscallNumber::SetRobustList => 0,
        // arch_prctl is x86_64 only - return ENOSYS on ARM64
        SyscallNumber::ArchPrctl => (-(crate::syscall::errno::ENOSYS as i64)) as u64,
        // Filesystem: newfstatat
        SyscallNumber::Newfstatat => result_to_u64(crate::syscall::fs::sys_newfstatat(arg1 as i32, arg2, arg3, arg4 as u32)),
        // Testing/diagnostic syscalls
        SyscallNumber::CowStats => sys_cow_stats_aarch64(arg1),
        SyscallNumber::SimulateOom => sys_simulate_oom_aarch64(arg1),
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

/// sys_getppid - Get the parent process ID (ARM64)
fn sys_getppid() -> u64 {
    let thread_id = crate::task::scheduler::current_thread_id().unwrap_or(0);
    if thread_id == 0 {
        return 0;
    }

    if let Some(ref manager) = *crate::process::manager() {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            if let Some(parent_pid) = process.parent {
                return parent_pid.as_u64();
            }
        }
    }

    0
}

/// sys_gettid - Get the current thread ID (ARM64)
fn sys_gettid() -> u64 {
    crate::task::scheduler::current_thread_id().unwrap_or(0)
}

/// sys_clock_gettime implementation - delegates to shared syscall/time.rs
fn sys_clock_gettime(clock_id: u32, user_timespec_ptr: *mut Timespec) -> u64 {
    // Use the shared implementation which properly uses copy_to_user
    // This is critical for CoW (Copy-on-Write) support - writing directly
    // to userspace memory would fail on CoW pages in forked children.
    match crate::syscall::time::sys_clock_gettime(
        clock_id,
        user_timespec_ptr as *mut crate::syscall::time::Timespec,
    ) {
        crate::syscall::SyscallResult::Ok(v) => v,
        crate::syscall::SyscallResult::Err(e) => (-(e as i64)) as u64,
    }
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
                            crate::task::scheduler::spawn_front(Box::new(child_thread_clone));
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
    argv_ptr: u64,
) -> u64 {
    // Trace: exec syscall entered
    super::trace::trace_exec(b'E');

    log::info!(
        "sys_exec_aarch64: program_name_ptr={:#x}, argv_ptr={:#x}",
        program_name_ptr,
        argv_ptr
    );

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

    use crate::syscall::userptr::{copy_cstr_from_user, copy_from_user};

        if program_name_ptr == 0 {
            log::error!("sys_exec_aarch64: NULL program name");
            return (-14_i64) as u64; // -EFAULT
        }

        let program_name = match copy_cstr_from_user(program_name_ptr) {
            Ok(name) => name,
            Err(errno) => {
                super::trace::trace_exec(b'X'); // Error path
                log::error!("sys_exec_aarch64: Failed to read program name: {}", errno);
                return (-(errno as i64)) as u64;
            }
        };

        // Trace: program name parsed successfully
        super::trace::trace_exec(b'N');

        log::info!("sys_exec_aarch64: Loading program '{}'", program_name);

        // Parse argv from userspace
        let mut argv_vec: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
        if argv_ptr != 0 {
            const MAX_ARGS: usize = 64;
            const MAX_ARG_LEN: usize = 4096;
            for i in 0..MAX_ARGS {
                let arg_ptr_addr = argv_ptr + (i * core::mem::size_of::<u64>()) as u64;
                let arg_ptr = match copy_from_user(arg_ptr_addr as *const u64) {
                    Ok(ptr) => ptr,
                    Err(errno) => {
                        log::error!(
                            "sys_exec_aarch64: Failed to read argv[{}] pointer: {}",
                            i,
                            errno
                        );
                        return (-(errno as i64)) as u64;
                    }
                };

                if arg_ptr == 0 {
                    break;
                }

                let arg_string = match copy_cstr_from_user(arg_ptr) {
                    Ok(s) => s,
                    Err(errno) => {
                        log::error!(
                            "sys_exec_aarch64: Failed to read argv[{}] string: {}",
                            i,
                            errno
                        );
                        return (-(errno as i64)) as u64;
                    }
                };

                let mut arg = arg_string.into_bytes();
                if arg.len() >= MAX_ARG_LEN {
                    arg.truncate(MAX_ARG_LEN.saturating_sub(1));
                }
                arg.push(0);
                argv_vec.push(arg);
            }
        }

        if argv_vec.is_empty() {
            let mut arg0 = program_name.as_bytes().to_vec();
            arg0.push(0);
            argv_vec.push(arg0);
        }

        // Trace: attempting to open ELF file
        super::trace::trace_exec(b'O');

        let elf_vec = if program_name.contains('/') {
            match load_elf_from_ext2(&program_name) {
                Ok(data) => data,
                Err(errno) => {
                    super::trace::trace_exec(b'X'); // Error path
                    return (-(errno as i64)) as u64;
                }
            }
        } else {
            let bin_path = alloc::format!("/bin/{}", program_name);
            match load_elf_from_ext2(&bin_path) {
                Ok(data) => data,
                Err(errno) => {
                    super::trace::trace_exec(b'X'); // Error path
                    // ARM64 doesn't have userspace_test module fallback
                    log::error!(
                        "sys_exec_aarch64: Failed to load /bin/{}: {}",
                        program_name,
                        errno
                    );
                    return (-(errno as i64)) as u64;
                }
            }
        };

        // Trace: ELF file loaded from filesystem
        super::trace::trace_exec(b'L');

        let elf_data = elf_vec.as_slice();

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

        let argv_slices: alloc::vec::Vec<&[u8]> =
            argv_vec.iter().map(|v| v.as_slice()).collect();

        without_interrupts(|| {
            let mut manager_guard = crate::process::manager();
            if let Some(ref mut manager) = *manager_guard {
                // Trace: calling exec_process_with_argv (process manager)
                super::trace::trace_exec(b'M');

                match manager.exec_process_with_argv(current_pid, elf_data, Some(&program_name), &argv_slices) {
                    Ok((new_entry_point, new_rsp)) => {
                        // Trace: exec_process_with_argv succeeded
                        super::trace::trace_exec(b'S');

                        log::info!(
                            "sys_exec_aarch64: Successfully replaced process address space, entry point: {:#x}",
                            new_entry_point
                        );

                        frame.elr = new_entry_point;

                        unsafe {
                            core::arch::asm!(
                                "msr sp_el0, {}",
                                in(reg) new_rsp,
                                options(nomem, nostack)
                            );
                        }

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
                        frame.x29 = 0;
                        frame.x30 = 0;

                        frame.spsr = 0x0; // EL0t, DAIF clear

                        // Trace: frame registers zeroed, SPSR set
                        super::trace::trace_exec(b'F');

                        if let Some(process) = manager.get_process(current_pid) {
                            if let Some(ref page_table) = process.page_table {
                                let new_ttbr0 = page_table.level_4_frame().start_address().as_u64();
                                log::info!("sys_exec_aarch64: Setting TTBR0_EL1 to {:#x}", new_ttbr0);
                                unsafe {
                                    core::arch::asm!(
                                        "dsb ishst",
                                        "msr ttbr0_el1, {}",
                                        "isb",
                                        "tlbi vmalle1is",
                                        "dsb ish",
                                        "isb",
                                        in(reg) new_ttbr0,
                                        options(nostack)
                                    );
                                }
                                // Trace: TTBR0 page table switched
                                super::trace::trace_exec(b'P');

                                // CRITICAL: Update saved_process_cr3 so the assembly ERET
                                // path doesn't restore the OLD (now-freed) page table.
                                // Without this, the .Lrestore_saved_ttbr path in syscall_entry.S
                                // switches TTBR0 back to the pre-exec page table, which has
                                // been deallocated by exec_process_with_argv.
                                unsafe {
                                    Aarch64PerCpu::set_saved_process_cr3(new_ttbr0);
                                }
                            }
                        }

                        log::info!(
                            "sys_exec_aarch64: Frame updated - ELR={:#x}, SP_EL0={:#x}",
                            frame.elr,
                            new_rsp
                        );
                        // Trace: about to return 0 from exec syscall
                        super::trace::trace_exec(b'R');

                        0
                    }
                    Err(e) => {
                        log::error!("sys_exec_aarch64: Failed to exec process: {}", e);
                        (-12_i64) as u64
                    }
                }
            } else {
                log::error!("sys_exec_aarch64: Process manager not available");
                (-12_i64) as u64
            }
        })
}

/// Load ELF binary from ext2 filesystem path.
///
/// Returns the file content as Vec<u8> on success, or an errno on failure.
///
/// NOTE: This function intentionally has NO logging to avoid timing overhead.
fn load_elf_from_ext2(path: &str) -> Result<alloc::vec::Vec<u8>, i32> {
    use crate::fs::ext2;
    use crate::syscall::errno::EIO;

    // Trace: entering load_elf_from_ext2
    super::trace::trace_exec(b'1');

    // Determine which filesystem to use based on path
    let is_home = ext2::is_home_path(path);
    let fs_path = if is_home { ext2::strip_home_prefix(path) } else { path };

    if is_home {
        let fs_guard = ext2::home_fs_read();
        super::trace::trace_exec(b'2');
        let fs = fs_guard.as_ref().ok_or(EIO)?;
        super::trace::trace_exec(b'3');
        load_elf_from_ext2_inner(fs, fs_path)
    } else {
        let fs_guard = ext2::root_fs_read();
        super::trace::trace_exec(b'2');
        let fs = fs_guard.as_ref().ok_or(EIO)?;
        super::trace::trace_exec(b'3');
        load_elf_from_ext2_inner(fs, fs_path)
    }
}

/// Inner helper for loading ELF from any ext2 filesystem instance (ARM64).
fn load_elf_from_ext2_inner(fs: &crate::fs::ext2::Ext2Fs, path: &str) -> Result<alloc::vec::Vec<u8>, i32> {
    use crate::syscall::errno::{EACCES, EIO, ENOENT, ENOTDIR};

    let inode_num = fs.resolve_path(path).map_err(|e| {
        super::trace::trace_exec(b'!');
        if e.contains("not found") { ENOENT } else { EIO }
    })?;
    super::trace::trace_exec(b'4');

    let inode = fs.read_inode(inode_num).map_err(|_| {
        super::trace::trace_exec(b'@');
        EIO
    })?;
    super::trace::trace_exec(b'5');

    if inode.is_dir() {
        super::trace::trace_exec(b'#');
        return Err(ENOTDIR);
    }

    let perms = inode.permissions();
    if (perms & 0o100) == 0 {
        super::trace::trace_exec(b'$');
        return Err(EACCES);
    }
    super::trace::trace_exec(b'6');

    let data = fs.read_file_content(&inode).map_err(|_| {
        super::trace::trace_exec(b'%');
        EIO
    })?;
    super::trace::trace_exec(b'7');

    Ok(data)
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

// =============================================================================
// Testing/diagnostic syscall implementations for ARM64
// =============================================================================

/// CowStatsResult structure returned by sys_cow_stats
/// Matches the layout expected by userspace
#[repr(C)]
struct CowStatsResultAarch64 {
    total_faults: u64,
    manager_path: u64,
    direct_path: u64,
    pages_copied: u64,
    sole_owner_opt: u64,
}

/// sys_cow_stats - Get Copy-on-Write statistics (for testing) - ARM64 implementation
///
/// This syscall is used to verify that the CoW optimization paths are working.
/// It returns the current CoW statistics to userspace.
///
/// Parameters:
/// - stats_ptr: pointer to a CowStatsResult structure in userspace
///
/// Returns: 0 on success, negative error code on failure
fn sys_cow_stats_aarch64(stats_ptr: u64) -> u64 {
    use crate::memory::cow_stats;

    if stats_ptr == 0 {
        return (-14_i64) as u64; // -EFAULT - null pointer
    }

    // Validate the address is in userspace
    if !crate::memory::layout::is_valid_user_address(stats_ptr) {
        log::error!("sys_cow_stats_aarch64: Invalid userspace address {:#x}", stats_ptr);
        return (-14_i64) as u64; // -EFAULT
    }

    // Get the current stats from the shared module
    let stats = cow_stats::get_stats();

    // Copy to userspace
    unsafe {
        let user_stats = stats_ptr as *mut CowStatsResultAarch64;
        (*user_stats).total_faults = stats.total_faults;
        (*user_stats).manager_path = stats.manager_path;
        (*user_stats).direct_path = stats.direct_path;
        (*user_stats).pages_copied = stats.pages_copied;
        (*user_stats).sole_owner_opt = stats.sole_owner_opt;
    }

    log::debug!(
        "sys_cow_stats_aarch64: total={}, manager={}, direct={}, copied={}, sole_owner={}",
        stats.total_faults,
        stats.manager_path,
        stats.direct_path,
        stats.pages_copied,
        stats.sole_owner_opt
    );

    0
}

/// sys_simulate_oom - Enable or disable OOM simulation (for testing) - ARM64 implementation
///
/// This syscall is used to test the kernel's behavior when frame allocation fails
/// during Copy-on-Write page faults. When OOM simulation is enabled, all frame
/// allocations will return None, causing CoW faults to fail and processes to be
/// terminated with SIGSEGV.
///
/// Parameters:
/// - enable: 1 to enable OOM simulation, 0 to disable
///
/// Returns: 0 on success, -ENOSYS if testing feature is not compiled in
///
/// # Safety
/// Only enable OOM simulation briefly for testing! Extended OOM simulation will
/// crash the kernel because it affects ALL frame allocations.
fn sys_simulate_oom_aarch64(enable: u64) -> u64 {
    #[cfg(feature = "testing")]
    {
        if enable != 0 {
            crate::memory::frame_allocator::enable_oom_simulation();
            log::info!("sys_simulate_oom_aarch64: OOM simulation ENABLED");
        } else {
            crate::memory::frame_allocator::disable_oom_simulation();
            log::info!("sys_simulate_oom_aarch64: OOM simulation disabled");
        }
        0
    }

    #[cfg(not(feature = "testing"))]
    {
        let _ = enable; // suppress unused warning
        log::warn!("sys_simulate_oom_aarch64: testing feature not compiled in");
        (-38_i64) as u64 // -ENOSYS - function not implemented
    }
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
