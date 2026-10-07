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

/// EACCES - Permission denied
const EACCES: u64 = 13;

/// EINVAL - Invalid argument
const EINVAL: u64 = 22;

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
/// * -EPERM (1): The calling process is already a process group leader, or a
///   process group with the caller's PID as its ID exists
///
/// # POSIX Semantics
/// setsid() fails with EPERM when the calling process is a process group
/// leader (which a session leader also is). Linux also refuses while any
/// process group has the caller's PID as its ID, since the new group's ID
/// would collide with it; that is the same test, made over every process.
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

    // The calling process is its thread group, identified by the group's
    // leader: a CLONE_VM thread's own row PID is not the process's.
    let group = match manager
        .find_process_by_thread(thread_id)
        .and_then(|(row, _)| manager.thread_group_of(row))
    {
        Some(group) => group,
        None => {
            log::error!("sys_setsid: process not found for thread {}", thread_id);
            return SyscallResult::Err(ESRCH);
        }
    };
    let pid = ProcessId::new(group);

    // A process group whose ID is the caller's PID exists: the caller leads
    // it, or it outlived the caller's leadership. Zombies are still members.
    if manager
        .all_processes()
        .iter()
        .any(|process| process.pgid == pid)
    {
        return SyscallResult::Err(EPERM);
    }

    // Create new session: set sid = pgid = pid, for every thread of the
    // process.
    let new_sid = pid;
    for row in manager.group_rows_mut(group) {
        row.sid = new_sid;
        row.pgid = new_sid;
    }

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
/// * -EINVAL (22): `pgid` is negative
/// * -ESRCH (3): `pid` is neither the caller nor one of its children
/// * -EACCES (13): `pid` is a child that has called exec
/// * -EPERM (1): `pid` is a child in another session, or a session leader, or
///   `pgid` names no process group in the caller's session
///
/// The checks are POSIX's, made in Linux's order.
pub fn sys_setpgid(pid: i32, pgid: i32) -> SyscallResult {
    if pgid < 0 {
        return SyscallResult::Err(EINVAL);
    }
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

    // Processes are thread groups, identified by their leaders: a CLONE_VM
    // thread's row stands for its whole process here.
    let (caller_pid, caller_sid) = match manager
        .find_process_by_thread(thread_id)
        .and_then(|(row, caller)| Some((manager.thread_group_of(row)?, caller.sid)))
    {
        Some((group, sid)) => (ProcessId::new(group), sid),
        None => return SyscallResult::Err(ESRCH),
    };

    // Determine target process
    let target_pid = match pid {
        0 => caller_pid,
        p if p < 0 => return SyscallResult::Err(ESRCH),
        p => ProcessId::new(p as u64),
    };
    // A PID naming a thread that does not lead its group is not a process
    // (Linux: EINVAL).
    match manager.thread_group_of(target_pid) {
        Some(group) if group != target_pid.as_u64() => return SyscallResult::Err(EINVAL),
        _ => {}
    }

    // Determine new pgid
    let new_pgid = if pgid == 0 {
        target_pid
    } else {
        ProcessId::new(pgid as u64)
    };

    let (target_sid, target_parent, target_exec) = match manager.get_process(target_pid) {
        Some(p) if !p.is_terminated() => (p.sid, p.parent, p.has_exec),
        _ => return SyscallResult::Err(ESRCH),
    };
    // The process the target's parent row belongs to: a child forked by one
    // of the caller's threads is the caller's child.
    let target_parent = target_parent.and_then(|parent| manager.thread_group_of(parent));

    // The target is the caller, or a child of the caller in the caller's
    // session that has not called exec.
    if target_pid != caller_pid {
        if target_parent != Some(caller_pid.as_u64()) {
            return SyscallResult::Err(ESRCH);
        }
        if target_sid != caller_sid {
            return SyscallResult::Err(EPERM);
        }
        if target_exec {
            return SyscallResult::Err(EACCES);
        }
    }

    // A session leader cannot change its process group, not even to its own.
    if target_sid == target_pid {
        return SyscallResult::Err(EPERM);
    }

    // Joining another group: it must exist in the caller's session. Zombies
    // are still members of their group.
    if new_pgid != target_pid
        && !manager
            .all_processes()
            .iter()
            .any(|process| process.pgid == new_pgid && process.sid == caller_sid)
    {
        return SyscallResult::Err(EPERM);
    }

    // Every thread of the target process joins the group.
    for row in manager.group_rows_mut(target_pid.as_u64()) {
        row.pgid = new_pgid;
    }

    log::debug!(
        "sys_setpgid: set pgid of process {} to {}",
        target_pid.as_u64(),
        new_pgid.as_u64()
    );

    SyscallResult::Ok(0)
}
