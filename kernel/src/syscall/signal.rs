//! Signal-related system calls
//!
//! This module implements the signal syscalls:
//! - kill(pid, sig) - Send signal to a process or process group
//! - sigaction(sig, act, oldact, sigsetsize) - Set signal handler
//! - sigprocmask(how, set, oldset, sigsetsize) - Block/unblock signals
//! - sigreturn() - Return from signal handler
//! - sigaltstack(ss, old_ss) - Set/get alternate signal stack

use super::userptr::{copy_from_user, copy_to_user};
use super::SyscallResult;
use crate::process::{manager, ProcessId};
use crate::signal::constants::*;
use crate::signal::types::{SigInfo, SignalAction, StackT};

// Architecture-specific imports
use crate::arch_impl::traits::CpuOps;

#[cfg(target_arch = "x86_64")]
type Cpu = crate::arch_impl::x86_64::X86Cpu;

#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

/// Userspace address limit - addresses must be below this to be valid userspace (x86_64)
#[cfg(target_arch = "x86_64")]
const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

/// kill(pid, sig) - Send signal to a process or process group
///
/// # Arguments
/// * `pid` - Target process ID or special values:
///   * pid > 0: Send to process with that PID
///   * pid == 0: Send to all processes in caller's process group
///   * pid == -1: Send to all processes caller can signal (except the designated init, if any)
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
        // Send to all processes the caller can signal. The designated init is excluded when one
        // exists; with no designated init, no process is excluded by identity.
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

        // A child that has exited is a zombie, still a process, until it is
        // reaped; only a reaped row (invisible to this lookup) is gone.
        if let Some(ref manager) = *manager_guard {
            if manager.get_process(target_pid).is_some() {
                return SyscallResult::Ok(0);
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
                // Check if any process, a zombie included, is in this group
                for process in manager.all_processes() {
                    if process.pgid == caller_pgid {
                        return SyscallResult::Ok(0);
                    }
                }
            }
        }
        SyscallResult::Err(3) // ESRCH - No such process group
    } else if pid == -1 {
        // Check if any signalable process exists, excluding the designated init when present.
        let manager_guard = manager();
        if let Some(ref manager) = *manager_guard {
            let designated_init = manager.designated_init();
            for process in manager.all_processes() {
                // With no designated init, no process is excluded by identity.
                if Some(process.id) != designated_init && !process.is_terminated() {
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
                if process.pgid == pgid {
                    return SyscallResult::Ok(0);
                }
            }
        }
        SyscallResult::Err(3) // ESRCH - No such process group
    }
}

/// Generate SIGPIPE for the calling thread's own process after a write to a
/// pipe, FIFO or stream socket with no reader (POSIX write(), EPIPE). The
/// writer is running, so nothing needs waking: the signal is acted on when
/// this syscall returns. An ignored SIGPIPE is discarded and the caller sees
/// only EPIPE. Must be called with no pipe or socket lock held.
pub(crate) fn raise_sigpipe() {
    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return;
    };
    let mut manager_guard = manager();
    if let Some(ref mut manager) = *manager_guard {
        if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
            process.signals.set_pending(SIGPIPE);
        }
    }
}

/// Terminate `victim` at once with `exit_code`: its threads stop being
/// scheduled, a CPU running one is told to switch away, and the row exits.
/// SIGKILL's delivery, also used when a thread group dies with one of its
/// members. Must be called with no process-manager lock held, from a thread
/// that is not one of the victim's.
///
/// A victim thread inside a kill-custody section, which every syscall is, owns
/// what its kernel stack holds and may hold, or be queued for, a lock that
/// dying would leave held for every other thread (#1025). Then the kill is
/// left pending as SIGKILL instead and the victim's waits are woken: the
/// thread finishes its syscall, and its return to user mode ends the process.
pub(crate) fn kill_process_now(victim: ProcessId, exit_code: i32) {
    // Publish before checking custody: a peer may leave its section and enter
    // a blocking syscall while we acquire the scheduler lock. Its signal check
    // must already see SIGKILL if immediate termination has to be deferred.
    crate::process::with_process_manager(|manager| {
        if let Some(process) = manager.get_process_mut(victim) {
            process.signals.set_pending(SIGKILL);
            // A deferred kill reports this status, not SIGKILL's own.
            process.group_exit_code.get_or_insert(exit_code);
        }
    });
    let claimed = crate::task::scheduler::with_scheduler(|scheduler| {
        scheduler.claim_process_threads_for_kill(victim.as_u64())
    })
    .unwrap_or(true);
    if !claimed {
        crate::task::scheduler::with_scheduler(|scheduler| {
            scheduler.wake_process_threads_for_kill(victim.as_u64());
        });
        return;
    }
    crate::trace_count!(crate::tracing::providers::teardown::TEARDOWN_ENTRY_SIGNAL);
    crate::task::scheduler::with_scheduler(|scheduler| {
        scheduler.terminate_process_threads(victim.as_u64());
    });

    let batch = crate::task::scheduler::GroupBatchId::for_single_victim(victim.as_u64());
    crate::task::scheduler::Scheduler::send_exit_expedite_sgi(victim.as_u64(), batch);
    let _ = crate::process::exit_process_and_retire(victim, exit_code);
    crate::task::scheduler::set_need_resched();
}

/// Send SIGHUP and then SIGCONT to every member of `pgid`, a process group an
/// exit has just orphaned while one of its members is stopped (POSIX _exit).
/// Must be called with no process-manager lock held.
pub(crate) fn signal_orphaned_group(pgid: ProcessId) {
    let _ = send_signal_to_process_group(pgid, SIGHUP);
    let _ = send_signal_to_process_group(pgid, SIGCONT);
}

/// The siginfo of a signal the calling process sends: SI_USER, with its PID
/// and real user ID.
fn sender_info(manager: &crate::process::ProcessManager, caller_tid: Option<u64>) -> SigInfo {
    caller_tid
        .and_then(|tid| manager.find_process_by_thread(tid))
        .map(|(pid, process)| {
            let tgid = process.thread_group_id.unwrap_or(pid.as_u64());
            SigInfo::sender(SI_USER, tgid as u32, process.cred.uid)
        })
        .unwrap_or_else(SigInfo::kernel)
}

/// Send a signal to a specific process
fn send_signal_to_process(target_pid: ProcessId, sig: u32) -> SyscallResult {
    let caller_tid = crate::task::scheduler::current_thread_id();
    let mut manager_guard = manager();

    if let Some(ref mut manager) = *manager_guard {
        let info = sender_info(manager, caller_tid);
        match manager.get_process(target_pid) {
            None => return SyscallResult::Err(3), // ESRCH
            // A zombie is still a process until it is reaped: the signal is
            // accepted and has no effect.
            Some(process) if process.is_terminated() => return SyscallResult::Ok(0),
            Some(_) => {}
        }

        // SIGKILL cannot be caught or blocked
        if sig == SIGKILL {
            drop(manager_guard);
            kill_process_now(target_pid, -(SIGKILL as i32));
            return SyscallResult::Ok(0);
        }

        if sig == SIGCONT {
            // SIGCONT continues a stopped process even when it is ignored or
            // blocked; it is then queued when caught, and discarded otherwise.
            crate::signal::delivery::continue_thread_group_locked(manager, target_pid);
        } else if sig_mask(sig) & STOP_SIGNALS != 0
            && crate::signal::delivery::generate_stop_locked(manager, target_pid, sig, caller_tid)
        {
            // The stop was taken, or discarded for an orphaned process group.
            return SyscallResult::Ok(0);
        }

        let group = manager
            .get_process(target_pid)
            .unwrap()
            .thread_group_id
            .unwrap_or(target_pid.as_u64());
        let recipient = manager
            .iter_processes()
            .find(|(pid, p)| {
                !p.is_terminated()
                    && p.thread_group_id.unwrap_or(pid.as_u64()) == group
                    && (!p.signals.is_blocked(sig)
                        || p.signals
                            .thread
                            .wait_set
                            .load(core::sync::atomic::Ordering::Acquire)
                            & sig_mask(sig)
                            != 0)
            })
            .map(|(pid, _)| pid)
            .unwrap_or(target_pid);

        if let Some(process) = manager.get_process_mut(recipient) {
            // Ignored signals are discarded at generation.
            process.signals.set_pending_info(sig, info);
            process.signals.process_pending |= sig_mask(sig) & process.signals.pending;

            // A stopped process runs nothing until SIGCONT; what is pending
            // waits for it. Every other wakeup below requires a pending,
            // unblocked, non-ignored disposition.
            if process.job.stopped.is_some()
                || (!process.signals.has_deliverable_signals()
                    && process
                        .signals
                        .thread
                        .wait_set
                        .load(core::sync::atomic::Ordering::Acquire)
                        & sig_mask(sig)
                        == 0)
            {
                return SyscallResult::Ok(0);
            }
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
                    // Wake thread if it's blocked on pause()/sigsuspend()
                    sched.unblock_for_signal(thread_id);
                    // Also wake thread if it's blocked on waitpid() - signals should
                    // interrupt waitpid with EINTR so the signal can be delivered
                    sched.unblock_for_child_exit(thread_id);
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
            // Zombies are members until reaped; signalling one has no effect.
            manager
                .all_processes()
                .iter()
                .filter(|p| p.pgid == pgid)
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

/// Send a signal to all processes the caller can signal (except the designated init)
///
/// This implements kill(-1, sig) - sends the signal to all processes
/// for which the calling process has permission to send signals,
/// except for the designated init process. If no init is designated, no process is excluded.
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

    // Collect PIDs of all signalable processes, excluding the designated init when present.
    let target_pids: alloc::vec::Vec<ProcessId> = {
        let manager_guard = manager();
        if let Some(ref manager) = *manager_guard {
            let designated_init = manager.designated_init();
            manager
                .all_processes()
                .iter()
                .filter(|p| {
                    // With no designated init, no process is excluded by identity.
                    Some(p.id) != designated_init && !p.is_terminated()
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

    // SIGKILL's and SIGSTOP's action can be queried (SIG_DFL) but not changed
    if new_act != 0 && !is_catchable(sig) {
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

    let sanitized_action = if new_act != 0 {
        let ptr = new_act as *const SignalAction;
        let new_action = match copy_from_user(ptr) {
            Ok(action) => action,
            Err(errno) => return SyscallResult::Err(errno),
        };

        Some(SignalAction {
            handler: new_action.handler,
            flags: new_action.flags,
            restorer: new_action.restorer,
            mask: new_action.mask & !UNCATCHABLE_SIGNALS,
        })
    } else {
        None
    };

    if old_act != 0 {
        let old_action = {
            let manager_guard = manager();
            let manager = match manager_guard.as_ref() {
                Some(m) => m,
                None => {
                    log::error!("sys_sigaction: process manager not initialized");
                    return SyscallResult::Err(3); // ESRCH
                }
            };

            let (_, process) = match manager.find_process_by_thread(current_thread_id) {
                Some(p) => p,
                None => {
                    log::error!(
                        "sys_sigaction: process not found for thread {}",
                        current_thread_id
                    );
                    return SyscallResult::Err(3); // ESRCH
                }
            };

            *process.signals.get_handler(sig)
        };

        let ptr = old_act as *mut SignalAction;
        if let Err(errno) = copy_to_user(ptr, &old_action) {
            return SyscallResult::Err(errno);
        }
    }

    if let Some(sanitized_action) = sanitized_action {
        let mut manager_guard = manager();
        let manager = match manager_guard.as_mut() {
            Some(m) => m,
            None => {
                log::error!("sys_sigaction: process manager not initialized");
                return SyscallResult::Err(3); // ESRCH
            }
        };

        let pid = match manager.find_process_by_thread(current_thread_id) {
            Some((pid, _)) => pid,
            None => {
                log::error!(
                    "sys_sigaction: process not found for thread {}",
                    current_thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
        };

        // Dispositions belong to the process, so every thread of the group
        // sees the change (each thread is its own row here).
        let mut rows = manager.thread_group_peers(pid);
        rows.push(pid);
        for row in rows {
            if let Some(process) = manager.get_process_mut(row) {
                process.signals.set_handler(sig, sanitized_action);
            }
        }
        log::debug!(
            "Signal {} ({}) handler set to {:#x} for process {} (thread {})",
            sig,
            signal_name(sig),
            sanitized_action.handler,
            pid.as_u64(),
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

    let new_mask = if new_set != 0 {
        let ptr = new_set as *const u64;
        Some(match copy_from_user(ptr) {
            Ok(mask) => mask,
            Err(errno) => return SyscallResult::Err(errno),
        })
    } else {
        None
    };

    if old_set != 0 {
        let old_mask = {
            let manager_guard = manager();
            let manager = match manager_guard.as_ref() {
                Some(m) => m,
                None => {
                    log::error!("sys_sigprocmask: process manager not initialized");
                    return SyscallResult::Err(3); // ESRCH
                }
            };

            let (_, process) = match manager.find_process_by_thread(current_thread_id) {
                Some(p) => p,
                None => {
                    log::error!(
                        "sys_sigprocmask: process not found for thread {}",
                        current_thread_id
                    );
                    return SyscallResult::Err(3); // ESRCH
                }
            };

            process.signals.blocked()
        };

        let ptr = old_set as *mut u64;
        if let Err(errno) = copy_to_user(ptr, &old_mask) {
            return SyscallResult::Err(errno);
        }
    }

    if let Some(set) = new_mask {
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
                log::error!(
                    "sys_sigprocmask: process not found for thread {}",
                    current_thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
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
        manager.route_pending_signals_to(current_thread_id);
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

        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread(thread_id) {
                // Return all pending signals
                let group = process.thread_group_id.unwrap_or(process.id.as_u64());
                let group_pending = manager.group_rows(group)
                    .fold(0, |pending, p| pending | p.signals.process_pending);
                (process.signals.pending_set() | group_pending) & process.signals.blocked()
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
    let Some(tid) = crate::task::scheduler::current_thread_id() else { return SyscallResult::Err(3); };
    wait_for_signal(tid);
    SyscallResult::Err(4)
}

/// Publish before checking pending, with preemption disabled until both are
/// complete. Wakes may be stale or spurious; only signal eligibility ends a wait.
/// Resume the syscall's kernel frame, then deliver on its normal user return.
fn wait_for_signal(thread_id: u64) {
    loop {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current_for_signal();
        });
        let eligible = manager()
            .as_ref()
            .and_then(|m| m.find_process_by_thread(thread_id))
            .is_some_and(|(_, p)| p.signals.has_deliverable_signals());
        if eligible {
            break;
        }
        crate::per_cpu::preempt_enable();
        crate::task::scheduler::yield_current();
        Cpu::halt_with_interrupts();
        crate::per_cpu::preempt_disable();
    }
    finish_signal_wait(thread_id);
}

fn finish_signal_wait(thread_id: u64) {
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.get_thread_mut(thread_id) {
            thread.blocked_in_syscall = false;
            thread.saved_userspace_context = None;
            thread.wake_time_ns = None;
            thread
                .signals
                .wait_set
                .store(0, core::sync::atomic::Ordering::Release);
            thread.set_running();
        }
    });
}

#[cfg(target_arch = "x86_64")]
pub fn sys_pause_with_frame(_frame: &super::handler::SyscallFrame) -> SyscallResult {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(3);
    };
    wait_for_signal(tid);
    SyscallResult::Err(4)
}

#[cfg(target_arch = "x86_64")]
const USER_RFLAGS_MASK: u64 = 0x0000_0CD5;

/// RFLAGS bits that must always be set (IF = interrupts enabled) (x86_64)
#[cfg(target_arch = "x86_64")]
const REQUIRED_RFLAGS: u64 = 0x0000_0200;

/// rt_sigreturn() - Return from signal handler with frame access (x86_64)
///
/// This syscall is called by the restorer after a signal handler returns. It
/// restores the interrupted context from the `ucontext_t` of the signal frame
/// delivery pushed (`SignalFrame`, Linux's rt_sigframe), including any
/// changes the handler made to it: the general registers, RIP, RSP and the
/// arithmetic flags, the signal mask, and the x87/SSE state from the FXSAVE
/// image `uc_mcontext.fpstate` points to (the initial state when it is null).
///
/// The handler's `ret` popped the frame's return address, so the frame
/// starts 8 bytes below the current RSP.
///
/// # Security
/// The restored context cannot leave user mode: RIP and RSP must be user
/// addresses, RFLAGS keeps only the bits user code may change, CS and SS are
/// not taken from the frame, and MXCSR bits the CPU does not implement are
/// cleared. A frame that cannot be read or fails these checks raises SIGSEGV,
/// as on Linux.
#[cfg(target_arch = "x86_64")]
pub fn sys_sigreturn_with_frame(frame: &mut super::handler::SyscallFrame) -> SyscallResult {
    use crate::arch_impl::x86_64::fpu::{self, FpuState};
    use crate::signal::types::SignalFrame;

    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_sigreturn: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let handler_sp = frame.rsp;
    let signal_frame_ptr = frame.rsp.wrapping_sub(8) as *const SignalFrame;
    let restored = copy_from_user(signal_frame_ptr).ok().and_then(|signal_frame| {
        let ctx = signal_frame.uc.uc_mcontext;
        if ctx.rip >= USER_SPACE_END || ctx.rsp >= USER_SPACE_END {
            return None;
        }
        let fp_state = match ctx.fpstate {
            0 => FpuState::initial(),
            addr => FpuState::from_signal_frame(copy_from_user(addr as *const [u8; 512]).ok()?),
        };
        Some((ctx, signal_frame.uc.uc_sigmask, signal_frame.uc.uc_stack, fp_state))
    });
    let Some((ctx, sigmask, uc_stack, fp_state)) = restored else {
        log::error!("sys_sigreturn: bad signal frame at {:#x}", frame.rsp);
        force_sigreturn_segv(current_thread_id);
        return SyscallResult::Err(14); // EFAULT
    };

    log::debug!(
        "sigreturn: restoring context from frame at {:#x}, rip={:#x}",
        frame.rsp,
        ctx.rip
    );

    // Restore the original execution context by modifying the syscall frame
    // When the syscall returns, IRETQ will use these values
    frame.rip = ctx.rip;
    frame.rsp = ctx.rsp;

    // SECURITY: Sanitize RFLAGS - only allow user-modifiable bits
    // Must keep IF (interrupt flag) set, IOPL=0, VM=0, etc.
    // This prevents userspace from disabling interrupts or escalating privilege
    frame.rflags = (ctx.eflags & USER_RFLAGS_MASK) | REQUIRED_RFLAGS;

    // Restore general-purpose registers
    frame.rax = ctx.rax;
    frame.rbx = ctx.rbx;
    frame.rcx = ctx.rcx;
    frame.rdx = ctx.rdx;
    frame.rdi = ctx.rdi;
    frame.rsi = ctx.rsi;
    frame.rbp = ctx.rbp;
    frame.r8 = ctx.r8;
    frame.r9 = ctx.r9;
    frame.r10 = ctx.r10;
    frame.r11 = ctx.r11;
    frame.r12 = ctx.r12;
    frame.r13 = ctx.r13;
    frame.r14 = ctx.r14;
    frame.r15 = ctx.r15;

    fpu::set_user_state(current_thread_id, &fp_state);

    {
        // This is a userspace syscall with no PM guard held. Contention must
        // wait: skipping restoration would leave the handler's mask installed.
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread_mut(current_thread_id) {
                process.signals.set_blocked(sigmask);
                restore_alt_stack(process, &uc_stack, handler_sp);
            }
            manager.route_pending_signals_to(current_thread_id);
        }
    }

    log::debug!(
        "sigreturn: restored context, returning to RIP={:#x} RSP={:#x}",
        ctx.rip,
        ctx.rsp
    );

    // Return value is ignored - the original RAX was restored above
    // But return 0 to indicate success in case anything checks
    SyscallResult::Ok(0)
}

/// rt_sigreturn found no usable signal frame: the thread gets SIGSEGV, as
/// on Linux, which its return to user mode acts on.
fn force_sigreturn_segv(thread_id: u64) {
    let mut manager_guard = crate::process::manager();
    if let Some(ref mut manager) = *manager_guard {
        if let Some((_, process)) = manager.find_process_by_thread_mut(thread_id) {
            process.signals.force_signal(SIGSEGV, SigInfo::kernel());
        }
    }
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
/// * `user_sp` - The caller's user stack pointer, which says whether it is
///   running on the alternate stack
///
/// # Returns
/// * 0 on success
/// * -EINVAL (22) for an undefined ss_flags value or a null ss_sp
/// * -ENOMEM (12) for a stack smaller than MINSIGSTKSZ
/// * -EFAULT (14) for invalid pointers
/// * -EPERM (1) if trying to change while executing on the alternate stack
/// * -ESRCH (3) if current process not found
///
/// # Behavior
/// - If `old_ss` is non-NULL, copies current alt stack info to it, with
///   SS_ONSTACK when the caller is running on it
/// - If `ss` is non-NULL:
///   - If SS_DISABLE flag is set, disables the alternate stack
///   - Otherwise, validates and sets the new alternate stack
///   - Size must be >= MINSIGSTKSZ
/// - Cannot change the alternate stack while executing on it
pub fn sys_sigaltstack(ss: u64, old_ss: u64, user_sp: u64) -> SyscallResult {
    // Get current thread/process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_sigaltstack: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let new_stack = if ss != 0 {
        let ptr = ss as *const StackT;
        Some(match copy_from_user(ptr) {
            Ok(s) => s,
            Err(errno) => return SyscallResult::Err(errno),
        })
    } else {
        None
    };

    let (current_stack, on_alt_stack) = {
        let manager_guard = manager();
        let manager_ref = match manager_guard.as_ref() {
            Some(m) => m,
            None => {
                log::error!("sys_sigaltstack: process manager not initialized");
                return SyscallResult::Err(3); // ESRCH
            }
        };

        let (_, process) = match manager_ref.find_process_by_thread(current_thread_id) {
            Some(p) => p,
            None => {
                log::error!(
                    "sys_sigaltstack: process not found for thread {}",
                    current_thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
        };

        let alt = &process.signals.alt_stack;
        (alt.stack_t(user_sp), alt.on_stack(user_sp))
    };

    if old_ss != 0 {
        let ptr = old_ss as *mut StackT;
        if let Err(errno) = copy_to_user(ptr, &current_stack) {
            return SyscallResult::Err(errno);
        }
    }

    if let Some(new_stack) = new_stack {
        if on_alt_stack {
            log::warn!("sys_sigaltstack: cannot change while on alternate stack");
            return SyscallResult::Err(1); // EPERM
        }

        let next_alt_stack = match alt_stack_from(&new_stack) {
            Ok(alt) => alt,
            Err(errno) => return SyscallResult::Err(errno),
        };
        log::debug!(
            "sigaltstack: thread {}: base={:#x}, size={}, flags={:#x}",
            current_thread_id,
            next_alt_stack.base,
            next_alt_stack.size,
            next_alt_stack.flags
        );

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

        process.signals.alt_stack = next_alt_stack;
    }

    SyscallResult::Ok(0)
}

/// The alternate stack `new_stack` asks for, or the errno sigaltstack
/// refuses it with.
fn alt_stack_from(new_stack: &StackT) -> Result<crate::signal::types::AltStack, u64> {
    // POSIX: EINVAL for any flag but SS_DISABLE. SS_ONSTACK is accepted
    // and ignored, as Linux does. Linux's SS_AUTODISARM is not
    // implemented, and is refused as kernels before it refused it.
    let mode = new_stack.ss_flags as u32;
    if mode != 0 && mode != SS_DISABLE && mode != SS_ONSTACK {
        log::warn!("sys_sigaltstack: invalid ss_flags {:#x}", mode);
        return Err(22); // EINVAL
    }

    if mode == SS_DISABLE {
        return Ok(crate::signal::types::AltStack::default());
    }

    // POSIX: ENOMEM for a stack smaller than MINSIGSTKSZ
    if new_stack.ss_size < MINSIGSTKSZ {
        log::warn!(
            "sys_sigaltstack: ss_size {} < MINSIGSTKSZ {}",
            new_stack.ss_size,
            MINSIGSTKSZ
        );
        return Err(12); // ENOMEM
    }

    // ss_sp must not be NULL
    if new_stack.ss_sp == 0 {
        log::warn!("sys_sigaltstack: ss_sp is NULL");
        return Err(22); // EINVAL
    }

    // The entire stack range must be in userspace
    let stack_end = new_stack.ss_sp.saturating_add(new_stack.ss_size as u64);
    if new_stack.ss_sp >= USER_SPACE_END || stack_end > USER_SPACE_END {
        log::warn!(
            "sys_sigaltstack: stack range {:#x}..{:#x} is not in userspace",
            new_stack.ss_sp,
            stack_end
        );
        return Err(14); // EFAULT
    }

    Ok(crate::signal::types::AltStack {
        base: new_stack.ss_sp,
        size: new_stack.ss_size,
        flags: 0, // Enabled (not SS_DISABLE)
    })
}

/// rt_sigreturn's half of sigaltstack, as Linux's restore_altstack: the
/// alternate stack the frame's `uc_stack` names (which the handler may have
/// changed) is installed, unless the thread, at `sp`, is running on the
/// current one. A `uc_stack` sigaltstack would refuse leaves it unchanged.
fn restore_alt_stack(process: &mut crate::process::Process, uc_stack: &StackT, sp: u64) {
    if process.signals.alt_stack.on_stack(sp) {
        return;
    }
    if let Ok(alt) = alt_stack_from(uc_stack) {
        process.signals.alt_stack = alt;
    }
}

/// rt_sigsuspend(mask, sigsetsize) - Atomically set signal mask and wait for signal (x86_64)
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
#[cfg(target_arch = "x86_64")]
pub fn sys_sigsuspend_with_frame(
    mask_ptr: u64,
    sigsetsize: u64,
    _frame: &super::handler::SyscallFrame,
) -> SyscallResult {
    sigsuspend(mask_ptr, sigsetsize)
}

fn sigsuspend(mask_ptr: u64, sigsetsize: u64) -> SyscallResult {
    if sigsetsize != 8 {
        return SyscallResult::Err(22);
    }
    let mask: u64 = match copy_from_user(mask_ptr as *const u64) {
        Ok(mask) => mask,
        Err(e) => return SyscallResult::Err(e),
    };
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(3);
    };
    {
        let mut guard = manager();
        let Some(m) = guard.as_mut() else {
            return SyscallResult::Err(3);
        };
        let Some((_, p)) = m.find_process_by_thread_mut(tid) else {
            return SyscallResult::Err(3);
        };
        p.signals.thread.save_wait_mask(p.signals.blocked());
        p.signals.set_blocked(mask);
        m.route_pending_signals_to(tid);
    }
    wait_for_signal(tid);
    SyscallResult::Err(4)
}

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
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(3);
    };
    let mut guard = manager();
    let Some(m) = guard.as_mut() else {
        return SyscallResult::Err(3);
    };
    let Some((_, p)) = m.find_process_by_thread_mut(tid) else {
        return SyscallResult::Err(3);
    };
    let value = crate::signal::Itimerval {
        it_interval: crate::signal::Timeval::zero(),
        it_value: crate::signal::Timeval::from_micros(seconds.saturating_mul(1_000_000)),
    };
    let old = p
        .itimers
        .real
        .set_value(&value, crate::signal::monotonic_micros());
    SyscallResult::Ok(old.it_value.to_micros().div_ceil(1_000_000))
}

fn timer_clock(process: &crate::process::Process, which: i32) -> u64 {
    use core::sync::atomic::Ordering;
    if which == crate::signal::itimer::ITIMER_REAL {
        return crate::signal::monotonic_micros();
    }
    let user = process.cpu.user_ns.load(Ordering::Relaxed);
    let system = if which == crate::signal::itimer::ITIMER_PROF {
        process.cpu.system_ns.load(Ordering::Relaxed)
    } else {
        0
    };
    user.saturating_add(system) / 1000
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

    // Get current process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_getitimer: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let value = {
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
                log::error!(
                    "sys_getitimer: process not found for thread {}",
                    current_thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
        };

        process
            .itimers
            .timer(which)
            .get_value(timer_clock(process, which))
    };

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

    // Get current process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_setitimer: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    let new_itimerval = if new_value != 0 {
        let ptr = new_value as *const Itimerval;
        match copy_from_user(ptr) {
            Ok(val) => Some(val),
            Err(errno) => return SyscallResult::Err(errno),
        }
    } else {
        None
    };

    if let Some(ref val) = new_itimerval {
        // tv_usec must be < 1,000,000
        if val.it_value.tv_usec >= 1_000_000 || val.it_value.tv_usec < 0 {
            log::warn!(
                "sys_setitimer: invalid it_value.tv_usec: {}",
                val.it_value.tv_usec
            );
            return SyscallResult::Err(22); // EINVAL
        }
        if val.it_interval.tv_usec >= 1_000_000 || val.it_interval.tv_usec < 0 {
            log::warn!(
                "sys_setitimer: invalid it_interval.tv_usec: {}",
                val.it_interval.tv_usec
            );
            return SyscallResult::Err(22); // EINVAL
        }
        // tv_sec must be non-negative
        if val.it_value.tv_sec < 0 || val.it_interval.tv_sec < 0 {
            log::warn!("sys_setitimer: negative seconds not allowed");
            return SyscallResult::Err(22); // EINVAL
        }
    }

    let old_itimerval = {
        let mut manager_guard = manager();
        let manager_ref = match manager_guard.as_mut() {
            Some(m) => m,
            None => {
                log::error!("sys_setitimer: process manager not initialized");
                return SyscallResult::Err(3); // ESRCH
            }
        };

        let (_, process) = match manager_ref.find_process_by_thread_mut(current_thread_id) {
            Some(p) => p,
            None => {
                log::error!(
                    "sys_setitimer: process not found for thread {}",
                    current_thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
        };

        let now = timer_clock(process, which);
        let timer = process.itimers.timer(which);
        let old_itimerval = if let Some(new_val) = new_itimerval {
            timer.set_value(&new_val, now)
        } else {
            timer.get_value(now)
        };

        old_itimerval
    };

    // Write old value to userspace (if requested)
    if old_value != 0 {
        let ptr = old_value as *mut Itimerval;
        if let Err(errno) = copy_to_user(ptr, &old_itimerval) {
            return SyscallResult::Err(errno);
        }
    }

    SyscallResult::Ok(0)
}

// =============================================================================
// ARM64 Signal Syscalls
// =============================================================================

/// Userspace address range end - addresses at or above this are kernel addresses
#[cfg(target_arch = "aarch64")]
const USER_SPACE_END: u64 = crate::memory::layout::USER_STACK_REGION_END;

#[cfg(target_arch = "aarch64")]
pub fn sys_pause_with_frame_aarch64(
    _frame: &mut crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame,
) -> SyscallResult {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(3);
    };
    wait_for_signal(tid);
    SyscallResult::Err(4)
}

/// rt_sigreturn() - Return from signal handler with frame access (ARM64)
///
/// This syscall is called by the restorer after a signal handler returns. It
/// restores the interrupted context from the `ucontext_t` of the signal frame
/// at SP (`SignalFrame`, Linux's rt_sigframe), including any changes the
/// handler made to it: x0-x30, SP, PC and the condition flags, the signal
/// mask, and v0-v31 with FPSR and FPCR from the frame's FP/SIMD record.
///
/// # Security
/// The restored context cannot leave EL0: PC and SP must be user addresses
/// and PSTATE keeps only the NZCV, DIT and SSBS bits, so the return is to
/// EL0t with interrupts unmasked. A frame that cannot be read, or whose
/// FP/SIMD record is missing, raises SIGSEGV, as on Linux.
#[cfg(target_arch = "aarch64")]
pub fn sys_sigreturn_with_frame_aarch64(
    frame: &mut crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame,
) -> SyscallResult {
    use crate::signal::types::{FpsimdContext, SignalFrame};

    /// PSTATE bits user code may set: NZCV, DIT and SSBS.
    const USER_PSTATE_MASK: u64 = 0xf000_0000 | (1 << 24) | (1 << 12);

    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_sigreturn_aarch64: no current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // On ARM64, signal frame is at current SP_EL0
    // The signal handler returns via RET to the restorer, which calls
    // sigreturn with SP where delivery set it.
    let sp = crate::arch_impl::aarch64::context::read_sp_el0();
    let restored = copy_from_user(sp as *const SignalFrame).ok().and_then(|signal_frame| {
        let ctx = signal_frame.uc.uc_mcontext;
        if ctx.pc >= USER_SPACE_END || ctx.sp >= USER_SPACE_END {
            return None;
        }
        // SAFETY: the reserved area is 16-byte aligned and holds a record
        // header's worth of bytes; the header is checked before the record
        // is trusted.
        let fpsimd = unsafe {
            core::ptr::read(ctx.reserved.0.as_ptr() as *const FpsimdContext)
        };
        if fpsimd.magic != FpsimdContext::MAGIC || fpsimd.size != FpsimdContext::SIZE {
            return None;
        }
        Some((ctx, signal_frame.uc.uc_sigmask, signal_frame.uc.uc_stack, fpsimd))
    });
    let Some((ctx, sigmask, uc_stack, fpsimd)) = restored else {
        log::error!("sys_sigreturn_aarch64: bad signal frame at {:#x}", sp);
        force_sigreturn_segv(current_thread_id);
        return SyscallResult::Err(14); // EFAULT
    };

    log::debug!(
        "sigreturn_aarch64: restoring context from frame at {:#x}, pc={:#x}",
        sp,
        ctx.pc
    );

    // Restore the original execution context
    frame.elr = ctx.pc;

    // Restore SP_EL0
    unsafe {
        crate::arch_impl::aarch64::context::write_sp_el0(ctx.sp);
    }

    // EL0t (M = 0) with D, A, I and F clear, and only the flags user code owns.
    frame.spsr = ctx.pstate & USER_PSTATE_MASK;

    // Restore general-purpose registers (X0-X30)
    frame.x0 = ctx.regs[0];
    frame.x1 = ctx.regs[1];
    frame.x2 = ctx.regs[2];
    frame.x3 = ctx.regs[3];
    frame.x4 = ctx.regs[4];
    frame.x5 = ctx.regs[5];
    frame.x6 = ctx.regs[6];
    frame.x7 = ctx.regs[7];
    frame.x8 = ctx.regs[8];
    frame.x9 = ctx.regs[9];
    frame.x10 = ctx.regs[10];
    frame.x11 = ctx.regs[11];
    frame.x12 = ctx.regs[12];
    frame.x13 = ctx.regs[13];
    frame.x14 = ctx.regs[14];
    frame.x15 = ctx.regs[15];
    frame.x16 = ctx.regs[16];
    frame.x17 = ctx.regs[17];
    frame.x18 = ctx.regs[18];
    frame.x19 = ctx.regs[19];
    frame.x20 = ctx.regs[20];
    frame.x21 = ctx.regs[21];
    frame.x22 = ctx.regs[22];
    frame.x23 = ctx.regs[23];
    frame.x24 = ctx.regs[24];
    frame.x25 = ctx.regs[25];
    frame.x26 = ctx.regs[26];
    frame.x27 = ctx.regs[27];
    frame.x28 = ctx.regs[28];
    frame.x29 = ctx.regs[29];
    frame.x30 = ctx.regs[30];

    // This thread is running here and the kernel uses no FP/SIMD register,
    // so the restored state reaches EL0 unchanged.
    crate::arch_impl::aarch64::fpsimd::restore(&fpsimd);

    {
        // This is a userspace syscall with no PM guard held. Contention must
        // wait: skipping restoration would leave the handler's mask installed.
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_, process)) = manager.find_process_by_thread_mut(current_thread_id) {
                process.signals.set_blocked(sigmask);
                restore_alt_stack(process, &uc_stack, sp);
            }
            manager.route_pending_signals_to(current_thread_id);
        }
    }

    log::info!(
        "sigreturn_aarch64: restored context, returning to PC={:#x} SP={:#x}",
        ctx.pc,
        ctx.sp
    );

    // Return value is ignored - original X0 was restored above
    SyscallResult::Ok(0)
}

/// rt_sigsuspend(mask, sigsetsize) - Atomically set signal mask and wait for signal (ARM64)
#[cfg(target_arch = "aarch64")]
pub fn sys_sigsuspend_with_frame_aarch64(
    mask_ptr: u64,
    sigsetsize: u64,
    _frame: &mut crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame,
) -> SyscallResult {
    sigsuspend(mask_ptr, sigsetsize)
}

/// Linux rt_sigtimedwait: accept one signal without invoking its handler.
/// Register the set before publishing the sleep so generation wakes it even
/// though the signal remains blocked for asynchronous delivery.
pub fn sys_sigtimedwait(set_ptr: u64, info_ptr: u64, timeout_ptr: u64, size: u64) -> SyscallResult {
    use core::sync::atomic::Ordering;
    if size != 8 {
        return SyscallResult::Err(22);
    }
    let set: u64 = match copy_from_user(set_ptr as *const u64) {
        Ok(mask) => mask & !UNCATCHABLE_SIGNALS,
        Err(e) => return SyscallResult::Err(e),
    };
    let deadline = if timeout_ptr == 0 {
        None
    } else {
        let ts: super::time::Timespec =
            match copy_from_user(timeout_ptr as *const super::time::Timespec) {
                Ok(ts) => ts,
                Err(e) => return SyscallResult::Err(e),
            };
        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return SyscallResult::Err(22);
        }
        Some(
            crate::signal::monotonic_micros()
                .saturating_mul(1000)
                .saturating_add((ts.tv_sec as u64).saturating_mul(1_000_000_000))
                .saturating_add(ts.tv_nsec as u64),
        )
    };
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(3);
    };
    {
        let mut guard = manager();
        let Some(m) = guard.as_mut() else {
            return SyscallResult::Err(3);
        };
        let Some((_, p)) = m.find_process_by_thread_mut(tid) else {
            return SyscallResult::Err(3);
        };
        p.signals.thread.wait_set.store(set, Ordering::Release);
        m.route_pending_signals_to(tid);
    }
    let result = loop {
        crate::task::scheduler::with_scheduler(|sched| {
            if let Some(deadline) = deadline {
                sched.block_current_for_timer(deadline);
            } else {
                sched.block_current_for_signal();
            }
        });
        {
            let mut guard = manager();
            let Some((_, p)) = guard
                .as_mut()
                .and_then(|m| m.find_process_by_thread_mut(tid))
            else {
                break SyscallResult::Err(3);
            };
            p.signals.collect_timer_signals();
            let pending = p.signals.pending & set;
            if pending != 0 {
                let sig = pending.trailing_zeros() + 1;
                let info = p.signals.pending_info(sig).to_linux(sig);
                // Do not consume a signal if the output buffer is invalid.
                if info_ptr != 0 {
                    if let Err(e) =
                        copy_to_user(info_ptr as *mut crate::signal::LinuxSigInfo, &info)
                    {
                        break SyscallResult::Err(e);
                    }
                }
                p.signals.clear_pending(sig);
                break SyscallResult::Ok(sig as u64);
            }
            if p.signals.has_deliverable_signals() {
                break SyscallResult::Err(4);
            }
        }
        if deadline.is_some_and(|end| crate::signal::monotonic_micros().saturating_mul(1000) >= end)
        {
            break SyscallResult::Err(11);
        }
        crate::per_cpu::preempt_enable();
        crate::task::scheduler::yield_current();
        Cpu::halt_with_interrupts();
        crate::per_cpu::preempt_disable();
    };
    finish_signal_wait(tid);
    result
}
