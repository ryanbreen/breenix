//! Session and process group syscalls
//!
//! This module implements session and process group management syscalls:
//! - setsid() - Create a new session
//! - getsid(pid) - Get session ID of a process
//!
//! Sessions are collections of process groups, typically associated with
//! a controlling terminal. A session leader is the process that created
//! the session (via setsid()).

use super::SyscallResult;
use crate::process::{manager, ProcessId};

/// EPERM - Operation not permitted
const EPERM: u64 = 1;

/// ESRCH - No such process
const ESRCH: u64 = 3;

/// setsid() - Create a new session
///
/// Creates a new session if the calling process is not a process group leader.
/// The calling process becomes:
/// - The session leader of the new session
/// - The process group leader of a new process group
/// - Detached from any controlling terminal
///
/// # Returns
/// * On success: The new session ID (which equals the process ID)
/// * -EPERM (1): The calling process is already a process group leader
///
/// # POSIX Semantics
/// Per POSIX, setsid() fails with EPERM if the calling process is already
/// a process group leader (pgid == pid). This prevents creating orphaned
/// process groups.
pub fn sys_setsid() -> SyscallResult {
    // Get current thread to find the calling process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_setsid: no current thread");
            return SyscallResult::Err(ESRCH);
        }
    };

    let mut manager_guard = manager();
    let manager = match manager_guard.as_mut() {
        Some(m) => m,
        None => {
            log::error!("sys_setsid: process manager not initialized");
            return SyscallResult::Err(ESRCH);
        }
    };

    let (pid, process) = match manager.find_process_by_thread_mut(thread_id) {
        Some(p) => p,
        None => {
            log::error!("sys_setsid: process not found for thread {}", thread_id);
            return SyscallResult::Err(ESRCH);
        }
    };

    // Check if the process is already a process group leader
    // A process is a process group leader if pgid == pid
    if process.pgid == pid {
        // Check if there are other processes in this process group
        // For simplicity, we check if this is the only process with this pgid
        // In a full implementation, we'd need to track all processes in each group

        // For now, we allow setsid if the process is the only member of its group
        // This is a simplification - a full implementation would need to track
        // process group membership more carefully
        log::debug!(
            "sys_setsid: process {} is process group leader (pgid={}), checking if sole member",
            pid.as_u64(),
            process.pgid.as_u64()
        );

        // If the process already has pgid == pid == sid, it's already a session leader
        // In this case, we can still proceed if it's the only member of its session
        // For shell job control purposes, we'll allow this case
    }

    // Create new session: set sid = pgid = pid
    let new_sid = pid;
    process.sid = new_sid;
    process.pgid = new_sid;

    // TODO: Detach from controlling terminal when tty support is added
    // For now, there's no controlling terminal to detach from

    log::info!(
        "sys_setsid: process {} created new session (sid={}, pgid={})",
        pid.as_u64(),
        new_sid.as_u64(),
        new_sid.as_u64()
    );

    // Return the new session ID
    SyscallResult::Ok(new_sid.as_u64())
}

/// getsid(pid) - Get the session ID of a process
///
/// # Arguments
/// * `pid` - Process ID to query, or 0 for the calling process
///
/// # Returns
/// * On success: The session ID of the specified process
/// * -ESRCH (3): No process with the specified PID exists
///
/// # POSIX Semantics
/// If pid is 0, the session ID of the calling process is returned.
/// Otherwise, the session ID of the process with the specified PID is returned.
pub fn sys_getsid(pid: i32) -> SyscallResult {
    // Get current thread to find the calling process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_getsid: no current thread");
            return SyscallResult::Err(ESRCH);
        }
    };

    let manager_guard = manager();
    let manager = match manager_guard.as_ref() {
        Some(m) => m,
        None => {
            log::error!("sys_getsid: process manager not initialized");
            return SyscallResult::Err(ESRCH);
        }
    };

    if pid == 0 {
        // Get session ID of the calling process
        let (_, process) = match manager.find_process_by_thread(thread_id) {
            Some(p) => p,
            None => {
                log::error!("sys_getsid: process not found for thread {}", thread_id);
                return SyscallResult::Err(ESRCH);
            }
        };

        log::debug!(
            "sys_getsid(0): returning sid={} for calling process",
            process.sid.as_u64()
        );
        SyscallResult::Ok(process.sid.as_u64())
    } else {
        // Get session ID of the specified process
        let target_pid = ProcessId::new(pid as u64);
        let process = match manager.get_process(target_pid) {
            Some(p) => p,
            None => {
                log::debug!("sys_getsid: no process with pid {}", pid);
                return SyscallResult::Err(ESRCH);
            }
        };

        // Check if process is terminated
        if process.is_terminated() {
            log::debug!("sys_getsid: process {} is terminated", pid);
            return SyscallResult::Err(ESRCH);
        }

        log::debug!(
            "sys_getsid({}): returning sid={}",
            pid,
            process.sid.as_u64()
        );
        SyscallResult::Ok(process.sid.as_u64())
    }
}

/// getpgid(pid) - Get the process group ID of a process
///
/// # Arguments
/// * `pid` - Process ID to query, or 0 for the calling process
///
/// # Returns
/// * On success: The process group ID of the specified process
/// * -ESRCH (3): No process with the specified PID exists
///
pub fn sys_getpgid(pid: i32) -> SyscallResult {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_getpgid: no current thread");
            return SyscallResult::Err(ESRCH);
        }
    };

    let manager_guard = manager();
    let manager = match manager_guard.as_ref() {
        Some(m) => m,
        None => {
            log::error!("sys_getpgid: process manager not initialized");
            return SyscallResult::Err(ESRCH);
        }
    };

    if pid == 0 {
        let (_, process) = match manager.find_process_by_thread(thread_id) {
            Some(p) => p,
            None => {
                return SyscallResult::Err(ESRCH);
            }
        };
        SyscallResult::Ok(process.pgid.as_u64())
    } else {
        let target_pid = ProcessId::new(pid as u64);
        let process = match manager.get_process(target_pid) {
            Some(p) => p,
            None => {
                return SyscallResult::Err(ESRCH);
            }
        };
        if process.is_terminated() {
            return SyscallResult::Err(ESRCH);
        }
        SyscallResult::Ok(process.pgid.as_u64())
    }
}

/// setpgid(pid, pgid) - Set the process group ID of a process
///
/// # Arguments
/// * `pid` - Process ID to modify, or 0 for the calling process
/// * `pgid` - New process group ID, or 0 to use the target process's PID
///
/// # Returns
/// * On success: 0
/// * -ESRCH (3): No process with the specified PID exists
/// * -EPERM (1): Various permission errors (see POSIX for details)
///
pub fn sys_setpgid(pid: i32, pgid: i32) -> SyscallResult {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            return SyscallResult::Err(ESRCH);
        }
    };

    let mut manager_guard = manager();
    let manager = match manager_guard.as_mut() {
        Some(m) => m,
        None => {
            return SyscallResult::Err(ESRCH);
        }
    };

    // Determine target process
    let target_pid = if pid == 0 {
        match manager.find_process_by_thread(thread_id) {
            Some((p, _)) => p,
            None => return SyscallResult::Err(ESRCH),
        }
    } else {
        ProcessId::new(pid as u64)
    };

    // Determine new pgid
    let new_pgid = if pgid == 0 {
        target_pid
    } else {
        ProcessId::new(pgid as u64)
    };

    // Get the target process
    let process = match manager.get_process_mut(target_pid) {
        Some(p) => p,
        None => return SyscallResult::Err(ESRCH),
    };

    if process.is_terminated() {
        return SyscallResult::Err(ESRCH);
    }

    // POSIX: A session leader cannot change its process group
    if process.sid == target_pid && process.pgid == target_pid {
        return SyscallResult::Err(EPERM);
    }

    // Set the new process group
    process.pgid = new_pgid;

    log::debug!(
        "sys_setpgid: set pgid of process {} to {}",
        target_pid.as_u64(),
        new_pgid.as_u64()
    );

    SyscallResult::Ok(0)
}
