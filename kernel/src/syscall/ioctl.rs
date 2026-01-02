//! ioctl syscall implementation
//!
//! The ioctl (I/O control) syscall provides a mechanism for device-specific
//! operations that don't fit into the standard read/write model.
//!
//! Currently supports TTY-related ioctls for terminal control.

use super::SyscallResult;

/// POSIX error codes
pub(crate) const EBADF: u64 = 9;   // Bad file descriptor
pub(crate) const ENOTTY: u64 = 25; // Inappropriate ioctl for device

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

    // Check if this fd is a TTY
    // In a full implementation, we'd look up the fd in the process's fd table
    // and check if it's a TTY device
    if !is_tty_fd(fd) {
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

/// Check if a file descriptor is a TTY
///
/// Currently, only fd 0, 1, 2 (stdin, stdout, stderr) are considered TTYs.
/// This will be expanded when we have a proper fd table.
#[inline]
pub(crate) fn is_tty_fd(fd: u64) -> bool {
    fd <= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    // =============================================================================
    // Error Code Tests
    // =============================================================================

    #[test]
    fn test_error_codes_match_posix() {
        assert_eq!(EBADF, 9);   // Bad file descriptor
        assert_eq!(ENOTTY, 25); // Not a typewriter
    }

    // =============================================================================
    // TTY FD Detection Tests
    // =============================================================================

    #[test]
    fn test_is_tty_fd_stdin() {
        assert!(is_tty_fd(0), "fd 0 (stdin) should be a TTY");
    }

    #[test]
    fn test_is_tty_fd_stdout() {
        assert!(is_tty_fd(1), "fd 1 (stdout) should be a TTY");
    }

    #[test]
    fn test_is_tty_fd_stderr() {
        assert!(is_tty_fd(2), "fd 2 (stderr) should be a TTY");
    }

    #[test]
    fn test_is_tty_fd_first_non_tty() {
        assert!(!is_tty_fd(3), "fd 3 should not be a TTY");
    }

    #[test]
    fn test_is_tty_fd_large_fd() {
        assert!(!is_tty_fd(100), "fd 100 should not be a TTY");
    }

    #[test]
    fn test_is_tty_fd_max_u64() {
        assert!(!is_tty_fd(u64::MAX), "fd u64::MAX should not be a TTY");
    }

    // =============================================================================
    // sys_ioctl Non-TTY FD Tests
    //
    // These tests verify that sys_ioctl returns ENOTTY for non-TTY file descriptors.
    // Note: Full sys_ioctl tests require the TTY subsystem to be initialized,
    // which happens during kernel boot. These tests focus on the fd validation
    // logic that happens before TTY dispatch.
    // =============================================================================

    #[test]
    fn test_sys_ioctl_non_tty_fd_returns_enotty() {
        // fd 3 is not a TTY
        let result = sys_ioctl(3, 0x5401, 0); // TCGETS
        assert_eq!(result, SyscallResult::Err(ENOTTY));
    }

    #[test]
    fn test_sys_ioctl_fd_4_returns_enotty() {
        let result = sys_ioctl(4, 0x5401, 0);
        assert_eq!(result, SyscallResult::Err(ENOTTY));
    }

    #[test]
    fn test_sys_ioctl_large_fd_returns_enotty() {
        // Large fd values should return ENOTTY (not a TTY)
        let result = sys_ioctl(1000, 0x5401, 0);
        assert_eq!(result, SyscallResult::Err(ENOTTY));
    }

    #[test]
    fn test_sys_ioctl_max_fd_returns_enotty() {
        // Max u64 fd should return ENOTTY
        let result = sys_ioctl(u64::MAX, 0x5401, 0);
        assert_eq!(result, SyscallResult::Err(ENOTTY));
    }

    #[test]
    fn test_sys_ioctl_boundary_fd_3_returns_enotty() {
        // fd 3 is the first non-TTY fd
        let result = sys_ioctl(3, 0x5402, 0); // TCSETS
        assert_eq!(result, SyscallResult::Err(ENOTTY));
    }

    // =============================================================================
    // sys_ioctl with various request codes on non-TTY fd
    // =============================================================================

    #[test]
    fn test_sys_ioctl_non_tty_fd_all_tty_requests_return_enotty() {
        // All TTY ioctl requests should fail with ENOTTY on non-TTY fd
        let requests = [
            0x5401, // TCGETS
            0x5402, // TCSETS
            0x5403, // TCSETSW
            0x5404, // TCSETSF
            0x540F, // TIOCGPGRP
            0x5410, // TIOCSPGRP
            0x5413, // TIOCGWINSZ
        ];

        for &request in &requests {
            let result = sys_ioctl(5, request, 0);
            assert_eq!(
                result,
                SyscallResult::Err(ENOTTY),
                "Request {:#x} on non-TTY fd should return ENOTTY",
                request
            );
        }
    }

    #[test]
    fn test_sys_ioctl_non_tty_fd_unknown_request_returns_enotty() {
        // Unknown request on non-TTY fd should still return ENOTTY
        // (not ENOTTY from "unknown request", but from "not a TTY")
        let result = sys_ioctl(10, 0xDEAD, 0);
        assert_eq!(result, SyscallResult::Err(ENOTTY));
    }
}
