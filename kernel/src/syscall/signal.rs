//! Signal-related system calls
//!
//! This module implements the signal syscalls:
//! - kill(pid, sig) - Send signal to a process
//! - sigaction(sig, act, oldact, sigsetsize) - Set signal handler
//! - sigprocmask(how, set, oldset, sigsetsize) - Block/unblock signals
//! - sigreturn() - Return from signal handler

use super::SyscallResult;
use super::userptr::{copy_from_user, copy_to_user};
use crate::process::{manager, ProcessId};
use crate::signal::constants::*;
use crate::signal::types::SignalAction;

/// kill(pid, sig) - Send signal to a process
///
/// # Arguments
/// * `pid` - Target process ID (positive), or special values:
///   * pid > 0: Send to process with that PID
///   * pid == 0: Send to all processes in caller's process group (not implemented)
///   * pid == -1: Send to all processes (not implemented)
///   * pid < -1: Send to process group -pid (not implemented)
/// * `sig` - Signal number to send (1-64), or 0 to check if process exists
///
/// # Returns
/// * 0 on success
/// * -EINVAL (22) for invalid signal number
/// * -ESRCH (3) if no such process
/// * -EPERM (1) if permission denied (not implemented)
pub fn sys_kill(pid: i64, sig: i32) -> SyscallResult {
    let sig = sig as u32;

    // Signal 0 is used to check if process exists without sending a signal
    if sig == 0 {
        return check_process_exists(pid);
    }

    // Validate signal number
    if !is_valid_signal(sig) {
        log::warn!("sys_kill: invalid signal number {}", sig);
        return SyscallResult::Err(22); // EINVAL
    }

    if pid > 0 {
        // Send to specific process
        send_signal_to_process(ProcessId::new(pid as u64), sig)
    } else if pid == 0 {
        // Send to process group of caller (not implemented)
        log::warn!("sys_kill: process groups not implemented (pid=0)");
        SyscallResult::Err(38) // ENOSYS
    } else if pid == -1 {
        // Send to all processes (not implemented)
        log::warn!("sys_kill: broadcast signals not implemented (pid=-1)");
        SyscallResult::Err(38) // ENOSYS
    } else {
        // Send to process group -pid (not implemented)
        log::warn!(
            "sys_kill: process groups not implemented (pid={})",
            pid
        );
        SyscallResult::Err(38) // ENOSYS
    }
}

/// Check if a process exists (kill with sig=0)
fn check_process_exists(pid: i64) -> SyscallResult {
    if pid <= 0 {
        return SyscallResult::Err(22); // EINVAL
    }

    let target_pid = ProcessId::new(pid as u64);
    let manager_guard = manager();

    if let Some(ref manager) = *manager_guard {
        if let Some(process) = manager.get_process(target_pid) {
            if !process.is_terminated() {
                return SyscallResult::Ok(0);
            }
        }
    }

    SyscallResult::Err(3) // ESRCH - No such process
}

/// Send a signal to a specific process
fn send_signal_to_process(target_pid: ProcessId, sig: u32) -> SyscallResult {
    let mut manager_guard = manager();

    if let Some(ref mut manager) = *manager_guard {
        if let Some(process) = manager.get_process_mut(target_pid) {
            // Check if process is alive
            if process.is_terminated() {
                return SyscallResult::Err(3); // ESRCH
            }

            // SIGKILL and SIGSTOP are special - cannot be caught or blocked
            if sig == SIGKILL {
                log::info!(
                    "SIGKILL sent to process {} - terminating immediately",
                    target_pid.as_u64()
                );
                process.terminate(-9); // Exit code for SIGKILL
                // Wake up process if blocked so scheduler removes it
                if matches!(process.state, crate::process::ProcessState::Blocked) {
                    process.set_ready();
                }
                // Trigger reschedule
                crate::task::scheduler::set_need_resched();
                return SyscallResult::Ok(0);
            }

            if sig == SIGSTOP {
                log::info!(
                    "SIGSTOP sent to process {} - stopping",
                    target_pid.as_u64()
                );
                process.set_blocked();
                return SyscallResult::Ok(0);
            }

            if sig == SIGCONT {
                log::info!(
                    "SIGCONT sent to process {} - continuing",
                    target_pid.as_u64()
                );
                if matches!(process.state, crate::process::ProcessState::Blocked) {
                    process.set_ready();
                    crate::task::scheduler::set_need_resched();
                }
                // SIGCONT also gets queued if there's a handler
                if !process.signals.get_handler(sig).is_default() {
                    process.signals.set_pending(sig);
                }
                return SyscallResult::Ok(0);
            }

            // For other signals, queue them for delivery
            process.signals.set_pending(sig);
            log::debug!(
                "Signal {} ({}) queued for process {}",
                sig,
                signal_name(sig),
                target_pid.as_u64()
            );

            // Wake up process if blocked (so it can receive the signal)
            if matches!(process.state, crate::process::ProcessState::Blocked) {
                process.set_ready();
                crate::task::scheduler::set_need_resched();
            }

            // Also wake up the thread if it's blocked on a signal (pause() syscall)
            // We need the thread ID from the process's main thread
            if let Some(ref thread) = process.main_thread {
                let thread_id = thread.id;
                log::info!(
                    "kill: Found main_thread {} for process {}, will unblock if BlockedOnSignal",
                    thread_id,
                    target_pid.as_u64()
                );
                // Release the manager lock before acquiring the scheduler lock
                // to avoid deadlock
                drop(manager_guard);
                crate::task::scheduler::with_scheduler(|sched| {
                    sched.unblock_for_signal(thread_id);
                });
                return SyscallResult::Ok(0);
            } else {
                log::warn!(
                    "kill: Process {} has no main_thread - cannot unblock for signal",
                    target_pid.as_u64()
                );
            }

            SyscallResult::Ok(0)
        } else {
            SyscallResult::Err(3) // ESRCH - No such process
        }
    } else {
        log::error!("sys_kill: process manager not initialized");
        SyscallResult::Err(3) // ESRCH
    }
}

/// rt_sigaction(sig, act, oldact, sigsetsize) - Set signal handler
///
/// # Arguments
/// * `sig` - Signal number (1-64, cannot be SIGKILL or SIGSTOP)
/// * `new_act` - Pointer to new SignalAction, or 0 to query current
/// * `old_act` - Pointer to store old SignalAction, or 0 to not store
/// * `sigsetsize` - Size of signal set (must be 8)
///
/// # Returns
/// * 0 on success
/// * -EINVAL (22) for invalid arguments
/// * -ESRCH (3) if current process not found
pub fn sys_sigaction(sig: i32, new_act: u64, old_act: u64, sigsetsize: u64) -> SyscallResult {
    let sig = sig as u32;

    // Validate signal number
    if !is_valid_signal(sig) {
        log::warn!("sys_sigaction: invalid signal number {}", sig);
        return SyscallResult::Err(22); // EINVAL
    }

    // Cannot change handler for SIGKILL or SIGSTOP
    if !is_catchable(sig) {
        log::warn!(
            "sys_sigaction: cannot set handler for {} (uncatchable)",
            signal_name(sig)
        );
        return SyscallResult::Err(22); // EINVAL
    }

    // sigsetsize must be 8 (size of u64 bitmask)
    if sigsetsize != 8 {
        log::warn!(
            "sys_sigaction: invalid sigsetsize {} (expected 8)",
            sigsetsize
        );
        return SyscallResult::Err(22); // EINVAL
    }

    // Get current process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_sigaction: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = manager();
    let manager = match manager_guard.as_mut() {
        Some(m) => m,
        None => {
            log::error!("sys_sigaction: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let (_, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_sigaction: process not found for thread {}", current_thread_id);
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Save old action if requested
    if old_act != 0 {
        let old_action = process.signals.get_handler(sig);
        // Write the old action to userspace (with validation)
        let ptr = old_act as *mut SignalAction;
        if let Err(errno) = copy_to_user(ptr, old_action) {
            return SyscallResult::Err(errno);
        }
    }

    // Set new action if provided
    if new_act != 0 {
        // Read the new action from userspace (with validation)
        let ptr = new_act as *const SignalAction;
        let new_action = match copy_from_user(ptr) {
            Ok(action) => action,
            Err(errno) => return SyscallResult::Err(errno),
        };

        // Sanitize the mask - cannot block SIGKILL or SIGSTOP
        let sanitized_action = SignalAction {
            handler: new_action.handler,
            mask: new_action.mask & !UNCATCHABLE_SIGNALS,
            flags: new_action.flags,
            restorer: new_action.restorer,
        };

        process.signals.set_handler(sig, sanitized_action);
        log::debug!(
            "Signal {} ({}) handler set to {:#x} for process {} (thread {})",
            sig,
            signal_name(sig),
            sanitized_action.handler,
            process.id.as_u64(),
            current_thread_id
        );
    }

    SyscallResult::Ok(0)
}

/// rt_sigprocmask(how, set, oldset, sigsetsize) - Block/unblock signals
///
/// # Arguments
/// * `how` - SIG_BLOCK (0), SIG_UNBLOCK (1), or SIG_SETMASK (2)
/// * `new_set` - Pointer to u64 signal mask, or 0 to not change
/// * `old_set` - Pointer to store old mask, or 0 to not store
/// * `sigsetsize` - Size of signal set (must be 8)
///
/// # Returns
/// * 0 on success
/// * -EINVAL (22) for invalid arguments
/// * -ESRCH (3) if current process not found
pub fn sys_sigprocmask(how: i32, new_set: u64, old_set: u64, sigsetsize: u64) -> SyscallResult {
    // sigsetsize must be 8
    if sigsetsize != 8 {
        log::warn!(
            "sys_sigprocmask: invalid sigsetsize {} (expected 8)",
            sigsetsize
        );
        return SyscallResult::Err(22); // EINVAL
    }

    // Validate 'how' parameter
    if new_set != 0 && how != SIG_BLOCK && how != SIG_UNBLOCK && how != SIG_SETMASK {
        log::warn!("sys_sigprocmask: invalid 'how' value {}", how);
        return SyscallResult::Err(22); // EINVAL
    }

    // Get current process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_sigprocmask: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = manager();
    let manager = match manager_guard.as_mut() {
        Some(m) => m,
        None => {
            log::error!("sys_sigprocmask: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let (_, process) = match manager.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_sigprocmask: process not found for thread {}", current_thread_id);
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Save old mask if requested
    if old_set != 0 {
        let ptr = old_set as *mut u64;
        if let Err(errno) = copy_to_user(ptr, &process.signals.blocked) {
            return SyscallResult::Err(errno);
        }
    }

    // Modify mask if new_set is provided
    if new_set != 0 {
        let ptr = new_set as *const u64;
        let set = match copy_from_user(ptr) {
            Ok(mask) => mask,
            Err(errno) => return SyscallResult::Err(errno),
        };

        match how {
            SIG_BLOCK => {
                process.signals.block_signals(set);
                log::debug!("Blocked signals: {:#x}", set);
            }
            SIG_UNBLOCK => {
                process.signals.unblock_signals(set);
                log::debug!("Unblocked signals: {:#x}", set);
            }
            SIG_SETMASK => {
                process.signals.set_blocked(set);
                log::debug!("Set signal mask to: {:#x}", set);
            }
            _ => unreachable!(), // Already validated above
        }
    }

    SyscallResult::Ok(0)
}

/// rt_sigreturn() - Return from signal handler (legacy - use sys_sigreturn_with_frame)
#[allow(dead_code)]
pub fn sys_sigreturn() -> SyscallResult {
    log::warn!("sys_sigreturn called without frame access - use sys_sigreturn_with_frame");
    SyscallResult::Err(38) // ENOSYS
}

/// pause() - Wait until a signal is delivered (legacy version without frame access)
///
/// This version is kept for backward compatibility but should not be used.
/// Use sys_pause_with_frame() instead for proper signal delivery.
#[allow(dead_code)]
pub fn sys_pause() -> SyscallResult {
    log::warn!("sys_pause called without frame access - signals may not work correctly");
    // Fall through to basic pause implementation without signal handler support
    let _thread_id = crate::task::scheduler::current_thread_id().unwrap_or(0);

    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_signal();
    });

    let mut _loop_count = 0u64;
    loop {
        crate::task::scheduler::yield_current();
        x86_64::instructions::interrupts::enable_and_hlt();

        _loop_count += 1;
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

    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
        }
    });

    SyscallResult::Err(4) // EINTR
}

/// pause() - Wait until a signal is delivered (with frame access)
///
/// pause() causes the calling process (or thread) to sleep until a signal
/// is delivered that either terminates the process or causes the invocation
/// of a signal-catching function.
///
/// This version takes the syscall frame so we can save the userspace context
/// for proper signal handler delivery when the thread is woken.
///
/// # Returns
/// * Always returns -EINTR (4) - pause() only returns when interrupted by a signal
pub fn sys_pause_with_frame(frame: &super::handler::SyscallFrame) -> SyscallResult {
    let thread_id = crate::task::scheduler::current_thread_id().unwrap_or(0);
    log::info!("sys_pause_with_frame: Thread {} blocking until signal arrives", thread_id);

    // CRITICAL: Save the userspace context BEFORE blocking.
    // When a signal arrives, the context switch code will use this saved context
    // to set up the signal handler frame (with RAX = -EINTR).
    let userspace_context = crate::task::thread::CpuContext::from_syscall_frame(frame);

    // Save the userspace context to the thread for signal delivery
    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
                if let Some(ref mut thread) = process.main_thread {
                    thread.saved_userspace_context = Some(userspace_context);
                    log::info!(
                        "sys_pause_with_frame: Saved userspace context for thread {}: RIP={:#x}, RSP={:#x}",
                        thread_id,
                        frame.rip,
                        frame.rsp
                    );
                }
            }
        }
    }

    // Block the current thread until a signal arrives
    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_signal();
    });

    log::info!("sys_pause_with_frame: Thread {} marked BlockedOnSignal, entering HLT loop", thread_id);

    // HLT loop - wait for timer interrupt which will switch to another thread
    let mut loop_count = 0u64;
    loop {
        crate::task::scheduler::yield_current();
        x86_64::instructions::interrupts::enable_and_hlt();

        loop_count += 1;
        if loop_count % 100 == 0 {
            log::info!("sys_pause_with_frame: Thread {} HLT loop iteration {}", thread_id, loop_count);
        }

        // Check if we were unblocked (thread state changed from BlockedOnSignal)
        let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.state == crate::task::thread::ThreadState::BlockedOnSignal
            } else {
                false
            }
        }).unwrap_or(false);

        if !still_blocked {
            log::info!("sys_pause_with_frame: Thread {} unblocked after {} HLT iterations", thread_id, loop_count);
            break;
        }
    }

    // CRITICAL: Clear the blocked_in_syscall flag and saved context now that the syscall is completing.
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
            thread.saved_userspace_context = None;
            log::info!("sys_pause_with_frame: Thread {} cleared blocked_in_syscall flag", thread_id);
        }
    });

    log::info!("sys_pause_with_frame: Thread {} returning -EINTR", thread_id);
    SyscallResult::Err(4) // EINTR
}

/// Userspace address limit - addresses must be below this to be valid userspace
const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

/// RFLAGS bits that userspace is allowed to modify
/// User can modify: CF, PF, AF, ZF, SF, DF, OF (arithmetic flags)
const USER_RFLAGS_MASK: u64 = 0x0000_0CD5;

/// RFLAGS bits that must always be set (IF = interrupts enabled)
const REQUIRED_RFLAGS: u64 = 0x0000_0200;

/// rt_sigreturn() - Return from signal handler with frame access
///
/// This syscall is called by the signal trampoline after a signal handler
/// returns. It restores the pre-signal execution context from the SignalFrame
/// that was pushed to the user stack when the signal was delivered.
///
/// The SignalFrame is located at the current user RSP (the stack pointer
/// when the syscall was made from the signal handler).
///
/// # Security
/// This function validates the signal frame to prevent privilege escalation:
/// - Verifies magic number to detect forged/corrupt frames
/// - Ensures saved_rip points to userspace (prevents jumping to kernel code)
/// - Ensures saved_rsp points to userspace (prevents using kernel stack)
/// - Sanitizes saved_rflags (prevents disabling interrupts, changing IOPL)
pub fn sys_sigreturn_with_frame(frame: &mut super::handler::SyscallFrame) -> SyscallResult {
    use crate::signal::types::SignalFrame;

    // The signal frame is at RSP - 8
    // When we delivered the signal, we set RSP to point to the signal frame.
    // The signal handler's 'ret' instruction popped the return address (trampoline_addr)
    // from RSP, incrementing it by 8. So the signal frame starts 8 bytes below
    // the current RSP.
    let signal_frame_ptr = (frame.rsp - 8) as *const SignalFrame;

    // Read the signal frame from userspace (with validation)
    let signal_frame = match copy_from_user(signal_frame_ptr) {
        Ok(frame) => frame,
        Err(errno) => {
            log::error!("sys_sigreturn: invalid signal frame pointer at {:#x}", frame.rsp);
            return SyscallResult::Err(errno);
        }
    };

    // SECURITY: Verify magic number to detect forged or corrupt frames
    // This prevents attackers from crafting fake signal frames for privilege escalation
    if signal_frame.magic != SignalFrame::MAGIC {
        log::error!(
            "sys_sigreturn: invalid magic {:#x} (expected {:#x}) - possible attack!",
            signal_frame.magic,
            SignalFrame::MAGIC
        );
        return SyscallResult::Err(14); // EFAULT
    }

    // SECURITY: Validate saved_rip is in userspace
    // Prevents returning to kernel code for privilege escalation
    if signal_frame.saved_rip >= USER_SPACE_END {
        log::error!(
            "sys_sigreturn: saved_rip {:#x} is not in userspace - privilege escalation attempt!",
            signal_frame.saved_rip
        );
        return SyscallResult::Err(14); // EFAULT
    }

    // SECURITY: Validate saved_rsp is in userspace
    // Prevents using kernel stack after sigreturn
    if signal_frame.saved_rsp >= USER_SPACE_END {
        log::error!(
            "sys_sigreturn: saved_rsp {:#x} is not in userspace - privilege escalation attempt!",
            signal_frame.saved_rsp
        );
        return SyscallResult::Err(14); // EFAULT
    }

    log::debug!(
        "sigreturn: restoring context from frame at {:#x}, saved_rip={:#x}",
        frame.rsp,
        signal_frame.saved_rip
    );

    // Restore the original execution context by modifying the syscall frame
    // When the syscall returns, IRETQ will use these values
    frame.rip = signal_frame.saved_rip;
    frame.rsp = signal_frame.saved_rsp;

    // SECURITY: Sanitize RFLAGS - only allow user-modifiable bits
    // Must keep IF (interrupt flag) set, IOPL=0, VM=0, etc.
    // This prevents userspace from disabling interrupts or escalating privilege
    let sanitized_rflags = (signal_frame.saved_rflags & USER_RFLAGS_MASK) | REQUIRED_RFLAGS;
    frame.rflags = sanitized_rflags;

    // Restore general-purpose registers
    frame.rax = signal_frame.saved_rax;
    frame.rbx = signal_frame.saved_rbx;
    frame.rcx = signal_frame.saved_rcx;
    frame.rdx = signal_frame.saved_rdx;
    frame.rdi = signal_frame.saved_rdi;
    frame.rsi = signal_frame.saved_rsi;
    frame.rbp = signal_frame.saved_rbp;
    frame.r8 = signal_frame.saved_r8;
    frame.r9 = signal_frame.saved_r9;
    frame.r10 = signal_frame.saved_r10;
    frame.r11 = signal_frame.saved_r11;
    frame.r12 = signal_frame.saved_r12;
    frame.r13 = signal_frame.saved_r13;
    frame.r14 = signal_frame.saved_r14;
    frame.r15 = signal_frame.saved_r15;

    // Restore the signal mask
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_sigreturn: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread_mut(current_thread_id) {
                process.signals.set_blocked(signal_frame.saved_blocked);
                log::debug!("sigreturn: restored signal mask to {:#x}", signal_frame.saved_blocked);
            }
        }
    }

    log::info!(
        "sigreturn: restored context, returning to RIP={:#x} RSP={:#x}",
        signal_frame.saved_rip,
        signal_frame.saved_rsp
    );

    // Return value is ignored - the original RAX was restored above
    // But return 0 to indicate success in case anything checks
    SyscallResult::Ok(0)
}
