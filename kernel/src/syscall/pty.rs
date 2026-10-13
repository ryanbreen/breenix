//! PTY syscall implementations
//!
//! Implements the POSIX PTY syscalls: posix_openpt, grantpt, unlockpt, ptsname

use super::errno::ENOTTY;
use super::userptr::{copy_to_user, validate_user_buffer};
use super::SyscallResult;
use crate::ipc::fd::FdKind;
use crate::process::manager;
use crate::tty::pty;

/// Open flags
const O_RDWR: u32 = 0x02;

/// sys_posix_openpt - Open a new PTY master device
///
/// Allocates a new pseudo-terminal master device and returns a file descriptor.
/// The PTY master is used by terminal emulators, telnet servers, etc.
///
/// # Arguments
/// * `flags` - O_RDWR (required), O_NOCTTY, O_CLOEXEC, O_NONBLOCK
///
/// # Returns
/// * `Ok(fd)` - File descriptor for the PTY master
/// * `Err(errno)` on failure:
///   - EINVAL (22): Invalid flags (O_RDWR must be set)
///   - EMFILE (24): Too many open files
///   - ENOSPC (28): No PTY slots available
///   - ESRCH (3): Process not found
pub fn sys_posix_openpt(flags: u64) -> SyscallResult {
    let flags_u32 = flags as u32;

    // Validate flags - O_RDWR must be set
    if (flags_u32 & O_RDWR) != O_RDWR {
        log::error!("sys_posix_openpt: O_RDWR not set in flags {:#x}", flags_u32);
        return SyscallResult::Err(22); // EINVAL
    }

    pty::open_master(flags_u32)
}

/// sys_grantpt - Grant access to the slave PTY
///
/// POSIX grantpt: the slave's owner becomes the caller's real user ID and its
/// mode owner read and write, group write (0620). Its group stays the one it
/// was allocated with (the master opener's), as Breenix has no tty group.
/// Opening the slave checks this ownership.
///
/// # Arguments
/// * `fd` - PTY master file descriptor
///
/// # Returns
/// * `Ok(0)` - Success
/// * `Err(errno)` on failure:
///   - EBADF (9): Bad file descriptor
///   - ENOTTY (25): fd is not a PTY master (the C library reports EINVAL)
///   - ESRCH (3): Process not found
pub fn sys_grantpt(fd: u64) -> SyscallResult {
    let fd_i32 = fd as i32;

    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(3); // ESRCH
    };
    let (pair, uid) = {
        let manager_guard = manager();
        let Some((_, process)) = manager_guard
            .as_ref()
            .and_then(|manager| manager.find_process_by_thread(thread_id))
        else {
            return SyscallResult::Err(3); // ESRCH
        };
        match process.fd_table.get(fd_i32).map(|entry| &entry.kind) {
            Some(FdKind::PtyMaster(pair)) => (pair.clone(), process.cred.uid),
            Some(_) => return SyscallResult::Err(ENOTTY as u64),
            None => return SyscallResult::Err(9), // EBADF
        }
    };
    pair.grant(uid);
    SyscallResult::Ok(0)
}

/// sys_unlockpt - Unlock the slave PTY for opening
///
/// After calling this, the corresponding PTY slave device can be opened.
///
/// # Arguments
/// * `fd` - PTY master file descriptor
///
/// # Returns
/// * `Ok(0)` - Success
/// * `Err(errno)` on failure:
///   - EBADF (9): Bad file descriptor
///   - ENOTTY (25): fd is not a PTY master
///   - EIO (5): PTY pair not found (internal error)
///   - ESRCH (3): Process not found
pub fn sys_unlockpt(fd: u64) -> SyscallResult {
    let fd_i32 = fd as i32;

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_unlockpt: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    let manager_guard = manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_pid, p)) => p,
            None => {
                log::error!("sys_unlockpt: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_unlockpt: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Get fd entry
    let fd_entry = match process.fd_table.get(fd_i32) {
        Some(entry) => entry,
        None => {
            log::error!("sys_unlockpt: Bad file descriptor {}", fd_i32);
            return SyscallResult::Err(9); // EBADF
        }
    };

    // Verify it's a PTY master and unlock
    match &fd_entry.kind {
        FdKind::PtyMaster(pair) => {
            pair.unlock();
            crate::serial_println!("[pty] Unlocked PTY {} (fd {})", pair.pty_num, fd_i32);
            SyscallResult::Ok(0)
        }
        _ => {
            log::error!("sys_unlockpt: fd {} is not a PTY master", fd_i32);
            SyscallResult::Err(ENOTTY as u64)
        }
    }
}

/// sys_ptsname - Get the path to the slave PTY device
///
/// Writes the path (e.g., "/dev/pts/0") to the provided buffer.
///
/// # Arguments
/// * `fd` - PTY master file descriptor
/// * `buf` - User buffer to receive the path
/// * `buflen` - Size of the buffer
///
/// # Returns
/// * `Ok(0)` - Success (path written to buffer with null terminator)
/// * `Err(errno)` on failure:
///   - EBADF (9): Bad file descriptor
///   - ENOTTY (25): fd is not a PTY master
///   - ERANGE (34): Buffer too small
///   - EFAULT (14): Invalid buffer pointer
///   - EIO (5): PTY pair not found (internal error)
///   - ESRCH (3): Process not found
pub fn sys_ptsname(fd: u64, buf: u64, buflen: u64) -> SyscallResult {
    let fd_i32 = fd as i32;

    // Validate buffer pointer
    if buf == 0 {
        log::error!("sys_ptsname: null buffer");
        return SyscallResult::Err(14); // EFAULT
    }

    if let Err(e) = validate_user_buffer(buf as *const u8, buflen as usize) {
        log::error!("sys_ptsname: invalid buffer");
        return SyscallResult::Err(e);
    }

    // Get current process and validate the fd while holding the process-manager
    // lock, but do not copy to userspace until after the lock is dropped.
    // A ptsname() buffer can live on a CoW stack after fork(); writing it while
    // holding the PM lock can deadlock the CoW fault handler.
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_ptsname: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    let pair = {
        let manager_guard = manager();
        let process = match &*manager_guard {
            Some(manager) => match manager.find_process_by_thread(thread_id) {
                Some((_pid, p)) => p,
                None => {
                    log::error!("sys_ptsname: Process not found for thread {}", thread_id);
                    return SyscallResult::Err(3); // ESRCH
                }
            },
            None => {
                log::error!("sys_ptsname: Process manager not initialized");
                return SyscallResult::Err(3); // ESRCH
            }
        };

        match process.fd_table.get(fd_i32) {
            Some(entry) => match &entry.kind {
                FdKind::PtyMaster(pair) => pair.clone(),
                _ => {
                    log::error!("sys_ptsname: fd {} is not a PTY master", fd_i32);
                    return SyscallResult::Err(ENOTTY as u64);
                }
            },
            None => {
                log::error!("sys_ptsname: Bad file descriptor {}", fd_i32);
                return SyscallResult::Err(9); // EBADF
            }
        }
    };

    let path = match pair.slave_path() { Ok(path) => path, Err(e) => return SyscallResult::Err(e as u64) };
    let path_bytes = path.as_bytes();

    // Check if buffer is large enough (need space for null terminator)
    if path_bytes.len() + 1 > buflen as usize {
        log::error!(
            "sys_ptsname: buffer too small ({} < {})",
            buflen,
            path_bytes.len() + 1
        );
        return SyscallResult::Err(34); // ERANGE
    }

    let buf_ptr = buf as *mut u8;
    for (idx, byte) in path_bytes.iter().copied().enumerate() {
        if let Err(errno) = copy_to_user(buf_ptr.wrapping_add(idx), &byte) {
            return SyscallResult::Err(errno);
        }
    }
    let nul = 0u8;
    if let Err(errno) = copy_to_user(buf_ptr.wrapping_add(path_bytes.len()), &nul) {
        return SyscallResult::Err(errno);
    }

    log::debug!("sys_ptsname: fd {} -> '{}' (pty {})", fd_i32, path, pair.pty_num);
    SyscallResult::Ok(0)
}
