//! PTY syscall implementations
//!
//! Implements the POSIX PTY syscalls: posix_openpt, grantpt, unlockpt, ptsname

use super::errno::ENOTTY;
use super::userptr::validate_user_buffer;
use super::SyscallResult;
use crate::ipc::fd::{flags, FileDescriptor, FdKind};
use crate::process::manager;
use crate::tty::pty;

/// Open flags
const O_RDWR: u32 = 0x02;
const O_CLOEXEC: u32 = 0x80000;

/// sys_posix_openpt - Open a new PTY master device
///
/// Allocates a new pseudo-terminal master device and returns a file descriptor.
/// The PTY master is used by terminal emulators, telnet servers, etc.
///
/// # Arguments
/// * `flags` - O_RDWR (required), O_NOCTTY, O_CLOEXEC
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

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_posix_openpt: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    let mut manager_guard = manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_pid, p)) => p,
            None => {
                log::error!("sys_posix_openpt: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_posix_openpt: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Allocate new PTY pair
    let pair = match pty::allocate() {
        Ok(pair) => pair,
        Err(e) => {
            log::error!("sys_posix_openpt: Failed to allocate PTY: {}", e);
            return SyscallResult::Err(e as u64);
        }
    };

    let pty_num = pair.pty_num;

    // Determine fd_flags based on O_CLOEXEC
    let fd_flags = if (flags_u32 & O_CLOEXEC) != 0 {
        flags::FD_CLOEXEC
    } else {
        0
    };

    // Create file descriptor with appropriate flags
    let fd_entry = FileDescriptor::with_flags(
        FdKind::PtyMaster(pty_num),
        fd_flags,
        0, // No status flags for PTY master
    );

    // Allocate file descriptor
    let fd = match process.fd_table.alloc_with_entry(fd_entry) {
        Ok(fd) => fd,
        Err(e) => {
            // Clean up PTY on failure
            pty::release(pty_num);
            log::error!("sys_posix_openpt: Failed to allocate fd: {}", e);
            return SyscallResult::Err(e as u64);
        }
    };

    log::info!(
        "sys_posix_openpt: Created PTY master fd={} (pty {}), flags={:#x}",
        fd,
        pty_num,
        flags_u32
    );

    SyscallResult::Ok(fd as u64)
}

/// sys_grantpt - Grant access to the slave PTY
///
/// In a full implementation, this would change ownership and permissions
/// of the slave device. For Breenix, we just validate the fd is a PTY master.
///
/// # Arguments
/// * `fd` - PTY master file descriptor
///
/// # Returns
/// * `Ok(0)` - Success
/// * `Err(errno)` on failure:
///   - EBADF (9): Bad file descriptor
///   - ENOTTY (25): fd is not a PTY master
///   - ESRCH (3): Process not found
pub fn sys_grantpt(fd: u64) -> SyscallResult {
    let fd_i32 = fd as i32;

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_grantpt: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    let manager_guard = manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_pid, p)) => p,
            None => {
                log::error!("sys_grantpt: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_grantpt: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Get fd entry
    let fd_entry = match process.fd_table.get(fd_i32) {
        Some(entry) => entry,
        None => {
            log::error!("sys_grantpt: Bad file descriptor {}", fd_i32);
            return SyscallResult::Err(9); // EBADF
        }
    };

    // Verify it's a PTY master
    match &fd_entry.kind {
        FdKind::PtyMaster(pty_num) => {
            log::debug!("sys_grantpt: fd {} is PTY master (pty {})", fd_i32, pty_num);
            // In a real system: chown slave to current user, chmod 0620
            // For now, just succeed
            SyscallResult::Ok(0)
        }
        _ => {
            log::error!("sys_grantpt: fd {} is not a PTY master", fd_i32);
            SyscallResult::Err(ENOTTY as u64)
        }
    }
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
        FdKind::PtyMaster(pty_num) => {
            // Get the PTY pair and unlock it
            match pty::get(*pty_num) {
                Some(pair) => {
                    pair.unlock();
                    log::info!("sys_unlockpt: Unlocked PTY {} (fd {})", pty_num, fd_i32);
                    SyscallResult::Ok(0)
                }
                None => {
                    log::error!("sys_unlockpt: PTY {} not found", pty_num);
                    SyscallResult::Err(5) // EIO
                }
            }
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

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_ptsname: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
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

    // Get fd entry
    let fd_entry = match process.fd_table.get(fd_i32) {
        Some(entry) => entry,
        None => {
            log::error!("sys_ptsname: Bad file descriptor {}", fd_i32);
            return SyscallResult::Err(9); // EBADF
        }
    };

    // Verify it's a PTY master and get the slave path
    match &fd_entry.kind {
        FdKind::PtyMaster(pty_num) => {
            // Get the PTY pair
            match pty::get(*pty_num) {
                Some(pair) => {
                    let path = pair.slave_path();
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

                    // Copy path to user buffer
                    let buf_ptr = buf as *mut u8;
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            path_bytes.as_ptr(),
                            buf_ptr,
                            path_bytes.len(),
                        );
                        // Write null terminator
                        core::ptr::write_volatile(buf_ptr.add(path_bytes.len()), 0);
                    }

                    log::debug!(
                        "sys_ptsname: fd {} -> '{}' (pty {})",
                        fd_i32,
                        path,
                        pty_num
                    );
                    SyscallResult::Ok(0)
                }
                None => {
                    log::error!("sys_ptsname: PTY {} not found", pty_num);
                    SyscallResult::Err(5) // EIO
                }
            }
        }
        _ => {
            log::error!("sys_ptsname: fd {} is not a PTY master", fd_i32);
            SyscallResult::Err(ENOTTY as u64)
        }
    }
}
