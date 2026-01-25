//! Signal-related system calls
//!
//! This module implements the signal syscalls:
//! - kill(pid, sig) - Send signal to a process or process group
//! - sigaction(sig, act, oldact, sigsetsize) - Set signal handler
//! - sigprocmask(how, set, oldset, sigsetsize) - Block/unblock signals
//! - sigreturn() - Return from signal handler
//! - sigaltstack(ss, old_ss) - Set/get alternate signal stack

use super::SyscallResult;
use super::userptr::{copy_from_user, copy_to_user};
use crate::process::{manager, ProcessId};
use crate::signal::constants::*;
use crate::signal::types::{SignalAction, StackT};

/// Process ID of the init process (cannot receive signals from kill -1)
const INIT_PID: u64 = 1;

/// Userspace address limit - addresses must be below this to be valid userspace
const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

/// kill(pid, sig) - Send signal to a process or process group
///
/// # Arguments
/// * `pid` - Target process ID or special values:
///   * pid > 0: Send to process with that PID
///   * pid == 0: Send to all processes in caller's process group
///   * pid == -1: Send to all processes caller can signal (except init)
///   * pid < -1: Send to all processes in process group abs(pid)
/// * `sig` - Signal number to send (1-64), or 0 to check if target exists
///
/// # Returns
/// * 0 on success
/// * -EINVAL (22) for invalid signal number
/// * -ESRCH (3) if no such process or process group
/// * -EPERM (1) if permission denied (not implemented - we allow all for now)
pub fn sys_kill(pid: i64, sig: i32) -> SyscallResult {
    let sig = sig as u32;

    // Signal 0 is used to check if process/group exists without sending a signal
    if sig == 0 {
        return check_target_exists(pid);
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
        // Send to all processes in caller's process group
        send_signal_to_caller_process_group(sig)
    } else if pid == -1 {
        // Send to all processes the caller can signal (except init)
        send_signal_to_all_processes(sig)
    } else {
        // pid < -1: Send to process group abs(pid)
        let pgid = ProcessId::new((-pid) as u64);
        send_signal_to_process_group(pgid, sig)
    }
}

/// Check if a target exists (kill with sig=0)
///
/// This handles all pid cases per POSIX:
/// - pid > 0: Check if specific process exists
/// - pid == 0: Check if caller's process group has members
/// - pid == -1: Check if any signalable process exists (always true if we have processes)
/// - pid < -1: Check if process group abs(pid) has members
fn check_target_exists(pid: i64) -> SyscallResult {
    if pid > 0 {
        // Check specific process
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
    } else if pid == 0 {
        // Check if caller's process group has members
        let current_thread_id = match crate::task::scheduler::current_thread_id() {
            Some(id) => id,
            None => return SyscallResult::Err(3), // ESRCH
        };

        let manager_guard = manager();
        if let Some(ref manager) = *manager_guard {
            // Find caller's pgid
            if let Some((_, caller)) = manager.find_process_by_thread(current_thread_id) {
                let caller_pgid = caller.pgid;
                // Check if any non-terminated process is in this group
                for process in manager.all_processes() {
                    if process.pgid == caller_pgid && !process.is_terminated() {
                        return SyscallResult::Ok(0);
                    }
                }
            }
        }
        SyscallResult::Err(3) // ESRCH - No such process group
    } else if pid == -1 {
        // Check if any signalable process exists (excluding init)
        let manager_guard = manager();
        if let Some(ref manager) = *manager_guard {
            for process in manager.all_processes() {
                if process.id.as_u64() != INIT_PID && !process.is_terminated() {
                    return SyscallResult::Ok(0);
                }
            }
        }
        SyscallResult::Err(3) // ESRCH - No signalable processes
    } else {
        // pid < -1: Check if process group abs(pid) has members
        let pgid = ProcessId::new((-pid) as u64);
        let manager_guard = manager();
        if let Some(ref manager) = *manager_guard {
            for process in manager.all_processes() {
                if process.pgid == pgid && !process.is_terminated() {
                    return SyscallResult::Ok(0);
                }
            }
        }
        SyscallResult::Err(3) // ESRCH - No such process group
    }
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
                // NOTE: set_need_resched() is now called inside unblock_for_signal
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

/// Send a signal to all processes in the caller's process group
///
/// This implements kill(0, sig) - sends the signal to all processes
/// that belong to the same process group as the calling process.
///
/// # Returns
/// * 0 on success (signal sent to at least one process)
/// * -ESRCH (3) if no processes found in the caller's process group
fn send_signal_to_caller_process_group(sig: u32) -> SyscallResult {
    // Get the caller's thread ID to find their process group
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("send_signal_to_caller_process_group: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Get the caller's pgid
    let caller_pgid = {
        let manager_guard = manager();
        if let Some(ref manager) = *manager_guard {
            if let Some((_, caller)) = manager.find_process_by_thread(current_thread_id) {
                caller.pgid
            } else {
                log::error!(
                    "send_signal_to_caller_process_group: caller process not found for thread {}",
                    current_thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
        } else {
            log::error!("send_signal_to_caller_process_group: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    log::debug!(
        "send_signal_to_caller_process_group: sending signal {} to pgid {}",
        sig,
        caller_pgid.as_u64()
    );

    send_signal_to_process_group(caller_pgid, sig)
}

/// Send a signal to all processes in a specific process group
///
/// This implements kill(-pgid, sig) - sends the signal to all processes
/// that belong to the specified process group.
///
/// # Arguments
/// * `pgid` - The target process group ID
/// * `sig` - The signal to send
///
/// # Returns
/// * 0 on success (signal sent to at least one process)
/// * -ESRCH (3) if no processes found in the specified process group
fn send_signal_to_process_group(pgid: ProcessId, sig: u32) -> SyscallResult {
    log::info!(
        "send_signal_to_process_group: sending signal {} ({}) to process group {}",
        sig,
        signal_name(sig),
        pgid.as_u64()
    );

    // Collect PIDs of processes in this group (we need to do this first
    // to avoid holding the lock while sending signals)
    let target_pids: alloc::vec::Vec<ProcessId> = {
        let manager_guard = manager();
        if let Some(ref manager) = *manager_guard {
            manager
                .all_processes()
                .iter()
                .filter(|p| p.pgid == pgid && !p.is_terminated())
                .map(|p| p.id)
                .collect()
        } else {
            log::error!("send_signal_to_process_group: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    if target_pids.is_empty() {
        log::warn!(
            "send_signal_to_process_group: no processes found in group {}",
            pgid.as_u64()
        );
        return SyscallResult::Err(3); // ESRCH
    }

    log::debug!(
        "send_signal_to_process_group: found {} processes in group {}",
        target_pids.len(),
        pgid.as_u64()
    );

    // Send signal to each process in the group
    let mut sent_count = 0;
    for pid in target_pids {
        match send_signal_to_process(pid, sig) {
            SyscallResult::Ok(_) => sent_count += 1,
            SyscallResult::Err(e) => {
                log::debug!(
                    "send_signal_to_process_group: failed to send to pid {}: error {}",
                    pid.as_u64(),
                    e
                );
            }
        }
    }

    if sent_count > 0 {
        log::info!(
            "send_signal_to_process_group: sent signal {} to {} processes in group {}",
            sig,
            sent_count,
            pgid.as_u64()
        );
        SyscallResult::Ok(0)
    } else {
        // All sends failed - this shouldn't happen if we found processes
        SyscallResult::Err(3) // ESRCH
    }
}

/// Send a signal to all processes the caller can signal (except init)
///
/// This implements kill(-1, sig) - sends the signal to all processes
/// for which the calling process has permission to send signals,
/// except for the init process (PID 1).
///
/// # Returns
/// * 0 on success (signal sent to at least one process)
/// * -ESRCH (3) if no signalable processes exist
fn send_signal_to_all_processes(sig: u32) -> SyscallResult {
    log::info!(
        "send_signal_to_all_processes: sending signal {} ({}) to all processes",
        sig,
        signal_name(sig)
    );

    // Collect PIDs of all signalable processes (excluding init)
    let target_pids: alloc::vec::Vec<ProcessId> = {
        let manager_guard = manager();
        if let Some(ref manager) = *manager_guard {
            manager
                .all_processes()
                .iter()
                .filter(|p| {
                    // Exclude init process and terminated processes
                    p.id.as_u64() != INIT_PID && !p.is_terminated()
                })
                .map(|p| p.id)
                .collect()
        } else {
            log::error!("send_signal_to_all_processes: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    if target_pids.is_empty() {
        log::warn!("send_signal_to_all_processes: no signalable processes found");
        return SyscallResult::Err(3); // ESRCH
    }

    log::debug!(
        "send_signal_to_all_processes: found {} signalable processes",
        target_pids.len()
    );

    // Send signal to each process
    // Note: We don't fail if some sends fail - POSIX says we succeed if we
    // can send to at least one process
    let mut sent_count = 0;
    for pid in target_pids {
        match send_signal_to_process(pid, sig) {
            SyscallResult::Ok(_) => sent_count += 1,
            SyscallResult::Err(e) => {
                log::debug!(
                    "send_signal_to_all_processes: failed to send to pid {}: error {}",
                    pid.as_u64(),
                    e
                );
            }
        }
    }

    if sent_count > 0 {
        log::info!(
            "send_signal_to_all_processes: sent signal {} to {} processes",
            sig,
            sent_count
        );
        SyscallResult::Ok(0)
    } else {
        // All sends failed
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

/// rt_sigpending() - Get pending signals
///
/// This syscall returns the set of signals that are pending for the calling thread
/// (signals that have been raised but are currently blocked).
///
/// # Arguments
/// * `set` - Pointer to sigset_t to store pending signals
/// * `sigsetsize` - Size of sigset_t (must be 8)
///
/// # Returns
/// * 0 on success
/// * -EFAULT if set pointer is invalid
/// * -EINVAL if sigsetsize is not 8
pub fn sys_sigpending(set: u64, sigsetsize: u64) -> SyscallResult {
    // Validate sigsetsize
    if sigsetsize != 8 {
        log::warn!("sigpending: invalid sigsetsize {} (expected 8)", sigsetsize);
        return SyscallResult::Err(22); // EINVAL
    }

    // Validate pointer
    if set == 0 {
        return SyscallResult::Err(14); // EFAULT
    }

    // Get current process
    let pending = {
        let thread_id = match crate::task::scheduler::current_thread_id() {
            Some(tid) => tid,
            None => {
                log::error!("sigpending: no current thread");
                return SyscallResult::Err(3); // ESRCH
            }
        };

        let manager_guard = crate::process::PROCESS_MANAGER.lock();
        if let Some(ref manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread(thread_id) {
                // Return all pending signals
                process.signals.pending
            } else {
                log::error!("sigpending: process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        } else {
            log::error!("sigpending: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Write pending signal set to userspace
    let set_ptr = set as *mut u64;
    unsafe {
        *set_ptr = pending;
    }

    log::debug!("sigpending: pending signals = {:#x}", pending);
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

    // CRITICAL FIX: Save the userspace context to the SCHEDULER's Thread ATOMICALLY
    // with setting blocked_in_syscall=true. This prevents a race condition where:
    // 1. Parent saves context to process.main_thread
    // 2. Child sends signal (sees thread not in BlockedOnSignal state yet)
    // 3. Parent sets blocked_in_syscall=true (too late - signal already lost)
    //
    // By using block_current_for_signal_with_context(), the context is saved to
    // the SCHEDULER's Thread under the scheduler lock, and the state transition
    // is atomic. The context_switch code reads from the scheduler's Thread, ensuring
    // consistency.
    //
    // Also save to process.main_thread for backwards compatibility with code that
    // reads from there (e.g., some signal delivery paths).
    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
                if let Some(ref mut thread) = process.main_thread {
                    thread.saved_userspace_context = Some(userspace_context.clone());
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
    // CRITICAL: This MUST happen ATOMICALLY with saving the context to the scheduler's Thread
    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_signal_with_context(Some(userspace_context));
    });

    log::info!("sys_pause_with_frame: Thread {} marked BlockedOnSignal, entering HLT loop", thread_id);

    // CRITICAL: Re-enable preemption before entering blocking loop!
    // The syscall handler called preempt_disable() at entry, but we need to allow
    // timer interrupts to schedule other threads while we're blocked.
    // Without this, can_schedule() returns false and no context switches happen.
    crate::per_cpu::preempt_enable();

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

    // Re-disable preemption before returning to balance syscall exit's preempt_enable()
    crate::per_cpu::preempt_disable();

    log::info!("sys_pause_with_frame: Thread {} returning -EINTR", thread_id);
    SyscallResult::Err(4) // EINTR
}

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
                // Check if we're returning from a signal that interrupted sigsuspend
                // If so, restore the original mask that sigsuspend saved, not the
                // temporary mask from the signal frame
                if let Some(saved_mask) = process.signals.sigsuspend_saved_mask.take() {
                    // Restore the original mask from before sigsuspend was called
                    process.signals.set_blocked(saved_mask);
                    log::info!(
                        "sigreturn: restored sigsuspend saved mask to {:#x} (ignoring signal frame mask {:#x})",
                        saved_mask,
                        signal_frame.saved_blocked
                    );
                } else {
                    // Normal case - restore from signal frame
                    process.signals.set_blocked(signal_frame.saved_blocked);
                    log::debug!("sigreturn: restored signal mask to {:#x}", signal_frame.saved_blocked);
                }

                // Clear the on_stack flag - we're leaving the signal handler
                // This allows the alternate stack to be used for future signals
                if process.signals.alt_stack.on_stack {
                    process.signals.alt_stack.on_stack = false;
                    log::debug!("sigreturn: cleared alt_stack.on_stack flag");
                }
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

/// sigaltstack(ss, old_ss) - Set/get alternate signal stack
///
/// This syscall allows a process to define an alternate stack for signal handlers.
/// This is particularly important for handling signals like SIGSEGV that might
/// occur due to stack overflow - without an alternate stack, the signal handler
/// itself would cause another stack overflow.
///
/// # Arguments
/// * `ss` - Pointer to new stack_t, or 0 to only query current
/// * `old_ss` - Pointer to store current stack_t, or 0 to not store
///
/// # Returns
/// * 0 on success
/// * -EINVAL (22) for invalid arguments
/// * -EFAULT (14) for invalid pointers
/// * -EPERM (1) if trying to change while executing on the alternate stack
/// * -ESRCH (3) if current process not found
///
/// # Behavior
/// - If `old_ss` is non-NULL, copies current alt stack info to it
/// - If `ss` is non-NULL:
///   - If SS_DISABLE flag is set, disables the alternate stack
///   - Otherwise, validates and sets the new alternate stack
///   - Size must be >= MINSIGSTKSZ (2048 bytes)
/// - Cannot change the alternate stack while executing on it
pub fn sys_sigaltstack(ss: u64, old_ss: u64) -> SyscallResult {
    // Get current thread/process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_sigaltstack: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = manager();
    let manager_ref = match manager_guard.as_mut() {
        Some(m) => m,
        None => {
            log::error!("sys_sigaltstack: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let (_, process) = match manager_ref.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!(
                "sys_sigaltstack: process not found for thread {}",
                current_thread_id
            );
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // If old_ss is provided, copy current alt stack info to it
    if old_ss != 0 {
        let alt = &process.signals.alt_stack;
        let current_stack = StackT {
            ss_sp: alt.base,
            ss_flags: if alt.on_stack {
                SS_ONSTACK as i32
            } else if alt.flags & SS_DISABLE != 0 {
                SS_DISABLE as i32
            } else {
                0
            },
            _pad: 0,
            ss_size: alt.size,
        };

        let ptr = old_ss as *mut StackT;
        if let Err(errno) = copy_to_user(ptr, &current_stack) {
            return SyscallResult::Err(errno);
        }
    }

    // If ss is provided, set new alt stack
    if ss != 0 {
        // Cannot change while executing on the alternate stack
        if process.signals.alt_stack.on_stack {
            log::warn!("sys_sigaltstack: cannot change while on alternate stack");
            return SyscallResult::Err(1); // EPERM
        }

        // Read the new stack configuration from userspace
        let ptr = ss as *const StackT;
        let new_stack = match copy_from_user(ptr) {
            Ok(s) => s,
            Err(errno) => return SyscallResult::Err(errno),
        };

        // Check if disabling the alternate stack
        if (new_stack.ss_flags as u32 & SS_DISABLE) != 0 {
            process.signals.alt_stack = crate::signal::types::AltStack {
                base: 0,
                size: 0,
                flags: SS_DISABLE,
                on_stack: false,
            };
            log::debug!(
                "sigaltstack: disabled alternate stack for thread {}",
                current_thread_id
            );
        } else {
            // Validate the new stack configuration
            // ss_sp must not be NULL
            if new_stack.ss_sp == 0 {
                log::warn!("sys_sigaltstack: ss_sp is NULL");
                return SyscallResult::Err(22); // EINVAL
            }

            // ss_size must be at least MINSIGSTKSZ
            if new_stack.ss_size < MINSIGSTKSZ {
                log::warn!(
                    "sys_sigaltstack: ss_size {} < MINSIGSTKSZ {}",
                    new_stack.ss_size,
                    MINSIGSTKSZ
                );
                return SyscallResult::Err(22); // EINVAL
            }

            // Validate that ss_sp is in userspace
            if new_stack.ss_sp >= USER_SPACE_END {
                log::warn!(
                    "sys_sigaltstack: ss_sp {:#x} is not in userspace",
                    new_stack.ss_sp
                );
                return SyscallResult::Err(14); // EFAULT
            }

            // Validate that the entire stack range is in userspace
            let stack_end = new_stack.ss_sp.saturating_add(new_stack.ss_size as u64);
            if stack_end > USER_SPACE_END {
                log::warn!(
                    "sys_sigaltstack: stack range {:#x}..{:#x} extends beyond userspace",
                    new_stack.ss_sp,
                    stack_end
                );
                return SyscallResult::Err(14); // EFAULT
            }

            // Set the new alternate stack
            process.signals.alt_stack = crate::signal::types::AltStack {
                base: new_stack.ss_sp,
                size: new_stack.ss_size,
                flags: 0, // Enabled (not SS_DISABLE)
                on_stack: false,
            };
            log::debug!(
                "sigaltstack: set alternate stack for thread {}: base={:#x}, size={}",
                current_thread_id,
                new_stack.ss_sp,
                new_stack.ss_size
            );
        }
    }

    SyscallResult::Ok(0)
}

/// rt_sigsuspend(mask, sigsetsize) - Atomically set signal mask and wait for signal
///
/// sigsuspend() temporarily replaces the signal mask of the calling process with
/// the mask given and then suspends the process until delivery of a signal whose
/// action is to invoke a signal handler or to terminate the process.
///
/// The key property of sigsuspend is ATOMICITY: the mask change and the suspension
/// happen as a single atomic operation. This prevents race conditions like:
///   1. Process unblocks a signal
///   2. Signal arrives (but we haven't called pause yet!)
///   3. Process calls pause()
///   4. Process waits forever (signal was already delivered)
///
/// With sigsuspend, the unblock and wait are atomic, so the signal cannot sneak
/// through between them.
///
/// # Arguments
/// * `mask_ptr` - Pointer to the new signal mask (u64 bitmask)
/// * `sigsetsize` - Size of the signal set (must be 8)
/// * `frame` - Syscall frame for saving userspace context
///
/// # Returns
/// * Always returns -EINTR (4) - sigsuspend always "fails" by being interrupted
///
/// # POSIX Behavior
/// When sigsuspend returns, the original signal mask is restored. The mask provided
/// to sigsuspend is only in effect while the process is suspended.
pub fn sys_sigsuspend_with_frame(
    mask_ptr: u64,
    sigsetsize: u64,
    frame: &super::handler::SyscallFrame,
) -> SyscallResult {
    use super::userptr::copy_from_user;
    use crate::signal::constants::UNCATCHABLE_SIGNALS;

    // Validate sigsetsize (must be 8 for our 64-bit signal mask)
    if sigsetsize != 8 {
        log::warn!(
            "sys_sigsuspend: invalid sigsetsize {} (expected 8)",
            sigsetsize
        );
        return SyscallResult::Err(22); // EINVAL
    }

    // Copy the new mask from userspace
    let new_mask: u64 = if mask_ptr != 0 {
        let ptr = mask_ptr as *const u64;
        match copy_from_user(ptr) {
            Ok(mask) => mask,
            Err(errno) => {
                log::warn!("sys_sigsuspend: invalid mask pointer {:#x}", mask_ptr);
                return SyscallResult::Err(errno);
            }
        }
    } else {
        log::warn!("sys_sigsuspend: NULL mask pointer");
        return SyscallResult::Err(14); // EFAULT
    };

    let thread_id = crate::task::scheduler::current_thread_id().unwrap_or(0);
    log::info!(
        "sys_sigsuspend: Thread {} suspending with temporary mask {:#x}",
        thread_id,
        new_mask
    );

    // CRITICAL: Save the current signal mask BEFORE setting the temporary one.
    // We'll restore this when the syscall returns.
    let saved_mask: u64;

    // Save userspace context and set temporary mask atomically (under lock)
    {
        let userspace_context = crate::task::thread::CpuContext::from_syscall_frame(frame);

        if let Some(mut manager_guard) = crate::process::try_manager() {
            if let Some(ref mut manager) = *manager_guard {
                if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
                    // Save the original mask
                    saved_mask = process.signals.blocked;

                    // Set the temporary mask (SIGKILL and SIGSTOP cannot be blocked)
                    let sanitized_mask = new_mask & !UNCATCHABLE_SIGNALS;
                    process.signals.set_blocked(sanitized_mask);

                    log::info!(
                        "sys_sigsuspend: Thread {} saved mask {:#x}, set temporary mask {:#x}",
                        thread_id,
                        saved_mask,
                        sanitized_mask
                    );

                    // Save userspace context for signal delivery
                    if let Some(ref mut thread) = process.main_thread {
                        thread.saved_userspace_context = Some(userspace_context);
                        log::info!(
                            "sys_sigsuspend: Saved userspace context for thread {}: RIP={:#x}, RSP={:#x}",
                            thread_id,
                            frame.rip,
                            frame.rsp
                        );
                    }
                } else {
                    log::error!("sys_sigsuspend: process not found for thread {}", thread_id);
                    return SyscallResult::Err(3); // ESRCH
                }
            } else {
                log::error!("sys_sigsuspend: process manager not initialized");
                return SyscallResult::Err(3); // ESRCH
            }
        } else {
            log::error!("sys_sigsuspend: could not acquire process manager lock");
            return SyscallResult::Err(3); // ESRCH
        }
    }

    // Block the current thread until a signal arrives
    // (same pattern as pause())
    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_signal();
    });

    log::info!(
        "sys_sigsuspend: Thread {} marked BlockedOnSignal, entering HLT loop",
        thread_id
    );

    // CRITICAL: Re-enable preemption before entering blocking loop!
    // The syscall handler called preempt_disable() at entry, but we need to allow
    // timer interrupts to schedule other threads while we're blocked.
    crate::per_cpu::preempt_enable();

    // HLT loop - wait for timer interrupt which will switch to another thread
    let mut loop_count = 0u64;
    loop {
        crate::task::scheduler::yield_current();
        x86_64::instructions::interrupts::enable_and_hlt();

        loop_count += 1;
        if loop_count % 100 == 0 {
            log::info!(
                "sys_sigsuspend: Thread {} HLT loop iteration {}",
                thread_id,
                loop_count
            );
        }

        // Check if we were unblocked (thread state changed from BlockedOnSignal)
        let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.state == crate::task::thread::ThreadState::BlockedOnSignal
            } else {
                false
            }
        })
        .unwrap_or(false);

        if !still_blocked {
            log::info!(
                "sys_sigsuspend: Thread {} unblocked after {} HLT iterations",
                thread_id,
                loop_count
            );
            break;
        }
    }

    // CRITICAL: Do NOT restore the mask here! The signal handler needs to run first
    // with the temporary mask still in effect. Store the saved mask so sigreturn
    // can restore it after the handler returns.
    //
    // Flow:
    // 1. sigsuspend sets temporary mask (SIGUSR1 unblocked)
    // 2. sigsuspend blocks waiting for signal
    // 3. Signal arrives, thread is unblocked
    // 4. sigsuspend stores saved_mask in sigsuspend_saved_mask (HERE)
    // 5. sigsuspend returns -EINTR (signal delivery happens on syscall return)
    // 6. Signal handler runs (with temporary mask still in effect)
    // 7. Handler calls sigreturn
    // 8. sigreturn restores sigsuspend_saved_mask to blocked
    if let Some(mut manager_guard) = crate::process::try_manager() {
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
                // Store the saved mask for sigreturn to restore after handler returns
                process.signals.sigsuspend_saved_mask = Some(saved_mask);
                log::info!(
                    "sys_sigsuspend: Thread {} stored saved_mask {:#x} for sigreturn to restore",
                    thread_id,
                    saved_mask
                );
            }
        }
    }

    // Clear the blocked_in_syscall flag and saved context
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
            thread.saved_userspace_context = None;
            log::info!(
                "sys_sigsuspend: Thread {} cleared blocked_in_syscall flag",
                thread_id
            );
        }
    });

    // Re-disable preemption before returning to balance syscall exit's preempt_enable()
    crate::per_cpu::preempt_disable();

    log::info!("sys_sigsuspend: Thread {} returning -EINTR", thread_id);
    SyscallResult::Err(4) // EINTR - always returns this per POSIX
}

/// Timer ticks per second (200 Hz from PIT configuration)
const TICKS_PER_SECOND: u64 = 200;

/// alarm(seconds) - Schedule a SIGALRM signal to be delivered after the specified time
///
/// Schedules a SIGALRM signal to be delivered to the calling process after the
/// specified number of seconds. If seconds is 0, any pending alarm is canceled.
///
/// # Arguments
/// * `seconds` - Number of seconds until SIGALRM is delivered, or 0 to cancel
///
/// # Returns
/// * Number of seconds remaining from any previously scheduled alarm, or 0 if none
///
/// # Notes
/// - Only one alarm can be pending per process; a new alarm() call replaces any existing one
/// - SIGALRM's default action is to terminate the process
/// - The alarm is delivered asynchronously via the signal delivery mechanism
pub fn sys_alarm(seconds: u64) -> SyscallResult {
    // Get current tick count for deadline calculation
    let current_ticks = crate::time::get_ticks();

    // Get current process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_alarm: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = manager();
    let manager_ref = match manager_guard.as_mut() {
        Some(m) => m,
        None => {
            log::error!("sys_alarm: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let (pid, process) = match manager_ref.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!(
                "sys_alarm: process not found for thread {}",
                current_thread_id
            );
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Calculate remaining seconds from old alarm (if any)
    let remaining = if let Some(old_deadline) = process.alarm_deadline {
        if old_deadline > current_ticks {
            // Convert remaining ticks to seconds (rounded up)
            let remaining_ticks = old_deadline - current_ticks;
            (remaining_ticks + TICKS_PER_SECOND - 1) / TICKS_PER_SECOND
        } else {
            // Alarm already expired but not yet delivered
            0
        }
    } else {
        0
    };

    // Set new alarm or cancel existing one
    if seconds == 0 {
        // Cancel any pending alarm
        if process.alarm_deadline.is_some() {
            log::debug!("sys_alarm: canceled alarm for process {}", pid.as_u64());
        }
        process.alarm_deadline = None;
    } else {
        // Calculate new deadline in ticks
        let deadline = current_ticks + (seconds * TICKS_PER_SECOND);
        process.alarm_deadline = Some(deadline);
        log::debug!(
            "sys_alarm: set alarm for process {} to fire at tick {} (in {} seconds)",
            pid.as_u64(),
            deadline,
            seconds
        );
    }

    SyscallResult::Ok(remaining)
}

/// getitimer(which, curr_value) - Get the current value of an interval timer
///
/// # Arguments
/// * `which` - Timer type: ITIMER_REAL (0), ITIMER_VIRTUAL (1), or ITIMER_PROF (2)
/// * `curr_value` - Pointer to itimerval structure to receive current timer value
///
/// # Returns
/// * 0 on success
/// * -EINVAL if which is invalid
/// * -EFAULT if curr_value is invalid
/// * -ESRCH if process not found
pub fn sys_getitimer(which: i32, curr_value: u64) -> SyscallResult {
    use crate::signal::types::{itimer::*, Itimerval};

    // Validate timer type
    if which != ITIMER_REAL && which != ITIMER_VIRTUAL && which != ITIMER_PROF {
        log::warn!("sys_getitimer: invalid timer type {}", which);
        return SyscallResult::Err(22); // EINVAL
    }

    // ITIMER_VIRTUAL and ITIMER_PROF require CPU time tracking (not yet implemented)
    if which == ITIMER_VIRTUAL || which == ITIMER_PROF {
        log::debug!(
            "sys_getitimer: ITIMER_{} not yet implemented, returning empty timer",
            if which == ITIMER_VIRTUAL { "VIRTUAL" } else { "PROF" }
        );
        // Return empty timer instead of error for better compatibility
        if curr_value != 0 {
            let empty = Itimerval::empty();
            let ptr = curr_value as *mut Itimerval;
            if let Err(errno) = copy_to_user(ptr, &empty) {
                return SyscallResult::Err(errno);
            }
        }
        return SyscallResult::Ok(0);
    }

    // Get current process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_getitimer: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let manager_guard = manager();
    let manager_ref = match manager_guard.as_ref() {
        Some(m) => m,
        None => {
            log::error!("sys_getitimer: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let (_, process) = match manager_ref.find_process_by_thread(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_getitimer: process not found for thread {}", current_thread_id);
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Get current timer value
    let value = process.itimers.real.get_value();

    // Write to userspace
    if curr_value != 0 {
        let ptr = curr_value as *mut Itimerval;
        if let Err(errno) = copy_to_user(ptr, &value) {
            return SyscallResult::Err(errno);
        }
    }

    SyscallResult::Ok(0)
}

/// setitimer(which, new_value, old_value) - Set an interval timer
///
/// Sets the specified interval timer to fire after the time specified in new_value.
/// When the timer expires, the appropriate signal is delivered (SIGALRM for ITIMER_REAL).
/// If it_interval is non-zero, the timer automatically rearms.
///
/// # Arguments
/// * `which` - Timer type: ITIMER_REAL (0), ITIMER_VIRTUAL (1), or ITIMER_PROF (2)
/// * `new_value` - Pointer to itimerval with new timer value (NULL to just query)
/// * `old_value` - Pointer to itimerval to receive old value (NULL to skip)
///
/// # Returns
/// * 0 on success
/// * -EINVAL if which is invalid or timer values are invalid
/// * -EFAULT if pointers are invalid
/// * -ESRCH if process not found
pub fn sys_setitimer(which: i32, new_value: u64, old_value: u64) -> SyscallResult {
    use crate::signal::types::{itimer::*, Itimerval};

    // Validate timer type
    if which != ITIMER_REAL && which != ITIMER_VIRTUAL && which != ITIMER_PROF {
        log::warn!("sys_setitimer: invalid timer type {}", which);
        return SyscallResult::Err(22); // EINVAL
    }

    // ITIMER_VIRTUAL and ITIMER_PROF require CPU time tracking (not yet implemented)
    if which == ITIMER_VIRTUAL || which == ITIMER_PROF {
        log::warn!(
            "sys_setitimer: ITIMER_{} not implemented",
            if which == ITIMER_VIRTUAL { "VIRTUAL" } else { "PROF" }
        );
        return SyscallResult::Err(38); // ENOSYS
    }

    // Get current process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_setitimer: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let mut manager_guard = manager();
    let manager_ref = match manager_guard.as_mut() {
        Some(m) => m,
        None => {
            log::error!("sys_setitimer: process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let (pid, process) = match manager_ref.find_process_by_thread_mut(current_thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_setitimer: process not found for thread {}", current_thread_id);
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Read new value from userspace (if provided)
    let new_itimerval = if new_value != 0 {
        let ptr = new_value as *const Itimerval;
        match copy_from_user(ptr) {
            Ok(val) => Some(val),
            Err(errno) => return SyscallResult::Err(errno),
        }
    } else {
        None
    };

    // Validate timer values if provided
    if let Some(ref val) = new_itimerval {
        // tv_usec must be < 1,000,000
        if val.it_value.tv_usec >= 1_000_000 || val.it_value.tv_usec < 0 {
            log::warn!("sys_setitimer: invalid it_value.tv_usec: {}", val.it_value.tv_usec);
            return SyscallResult::Err(22); // EINVAL
        }
        if val.it_interval.tv_usec >= 1_000_000 || val.it_interval.tv_usec < 0 {
            log::warn!("sys_setitimer: invalid it_interval.tv_usec: {}", val.it_interval.tv_usec);
            return SyscallResult::Err(22); // EINVAL
        }
        // tv_sec must be non-negative
        if val.it_value.tv_sec < 0 || val.it_interval.tv_sec < 0 {
            log::warn!("sys_setitimer: negative seconds not allowed");
            return SyscallResult::Err(22); // EINVAL
        }
    }

    // Get old value before modifying
    let old_itimerval = process.itimers.real.get_value();

    // Set new value if provided
    if let Some(new_val) = new_itimerval {
        process.itimers.real.set_value(&new_val);

        if new_val.it_value.is_zero() {
            log::debug!("sys_setitimer: disabled ITIMER_REAL for process {}", pid.as_u64());
        } else {
            log::debug!(
                "sys_setitimer: set ITIMER_REAL for process {}: value={}.{:06}s, interval={}.{:06}s",
                pid.as_u64(),
                new_val.it_value.tv_sec,
                new_val.it_value.tv_usec,
                new_val.it_interval.tv_sec,
                new_val.it_interval.tv_usec
            );
        }
    }

    // Write old value to userspace (if requested)
    if old_value != 0 {
        let ptr = old_value as *mut Itimerval;
        if let Err(errno) = copy_to_user(ptr, &old_itimerval) {
            return SyscallResult::Err(errno);
        }
    }

    SyscallResult::Ok(0)
}
