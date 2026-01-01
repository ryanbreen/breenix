//! ioctl syscall implementation
//!
//! The ioctl (I/O control) syscall provides a mechanism for device-specific
//! operations that don't fit into the standard read/write model.
//!
//! Currently supports TTY-related ioctls for terminal control.

use super::SyscallResult;

/// POSIX error codes
const EBADF: u64 = 9;   // Bad file descriptor
const ENOTTY: u64 = 25; // Inappropriate ioctl for device

/// sys_ioctl - Perform I/O control operation on a file descriptor
///
/// # Arguments
/// * `fd` - File descriptor (currently only 0/1/2 for console TTY)
/// * `request` - The ioctl request code
/// * `arg` - Request-specific argument (typically a pointer)
///
/// # Returns
/// * `SyscallResult::Ok(0)` on success
/// * `SyscallResult::Err(errno)` on failure
///
/// # Supported Requests
/// For TTY devices (fd 0, 1, 2):
/// - TCGETS (0x5401): Get terminal attributes
/// - TCSETS (0x5402): Set terminal attributes immediately
/// - TCSETSW (0x5403): Set terminal attributes after output drain
/// - TCSETSF (0x5404): Set terminal attributes after flush
/// - TIOCGPGRP (0x540F): Get foreground process group
/// - TIOCSPGRP (0x5410): Set foreground process group
/// - TIOCGWINSZ (0x5413): Get window size
pub fn sys_ioctl(fd: u64, request: u64, arg: u64) -> SyscallResult {
    log::debug!("sys_ioctl: fd={}, request={:#x}, arg={:#x}", fd, request, arg);

    // Get the console TTY - for now, only fd 0, 1, 2 are TTY
    // In a full implementation, we'd look up the fd in the process's fd table
    // and check if it's a TTY device
    let is_tty = fd <= 2;

    if !is_tty {
        // Not a TTY - check if it's a valid fd at all
        // For now, return ENOTTY for any non-TTY fd
        log::debug!("sys_ioctl: fd {} is not a TTY", fd);
        return SyscallResult::Err(ENOTTY);
    }

    // Get the console TTY device
    let tty = match crate::tty::console() {
        Some(tty) => tty,
        None => {
            log::warn!("sys_ioctl: TTY subsystem not initialized");
            return SyscallResult::Err(EBADF);
        }
    };

    // Dispatch to TTY ioctl handler
    match crate::tty::ioctl::tty_ioctl(&tty, request, arg) {
        Ok(ret) => SyscallResult::Ok(ret as u64),
        Err(errno) => SyscallResult::Err(errno as u64),
    }
}
