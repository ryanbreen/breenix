//! ioctl syscall implementation
//!
//! The ioctl (I/O control) syscall provides a mechanism for device-specific
//! operations that don't fit into the standard read/write model.
//!
//! Supports:
//! - TTY-related ioctls for terminal control
//! - PTY-specific ioctls for pseudo-terminal devices

use super::SyscallResult;
use crate::ipc::fd::FdKind;

/// POSIX error codes
pub(crate) const EBADF: u64 = 9;   // Bad file descriptor
pub(crate) const ENOTTY: u64 = 25; // Inappropriate ioctl for device

/// sys_ioctl - Perform I/O control operation on a file descriptor
///
/// # Arguments
/// * `fd` - File descriptor
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
///
/// For PTY devices:
/// - All TTY ioctls above, plus:
/// - TIOCSWINSZ (0x5414): Set window size
/// - TIOCSCTTY (0x540E): Set controlling terminal
/// - TIOCNOTTY (0x5422): Release controlling terminal
/// - TIOCGPTN (0x80045430): Get PTY number
/// - TIOCSPTLCK (0x40045431): Lock/unlock PTY slave
/// - TIOCGPTLCK (0x80045439): Get PTY lock status
pub fn sys_ioctl(fd: u64, request: u64, arg: u64) -> SyscallResult {
    log::debug!("sys_ioctl: fd={}, request={:#x}, arg={:#x}", fd, request, arg);

    // First, try to look up the fd in the process's fd table
    // to check if it's a PTY device
    if let Some((fd_kind, pid)) = get_fd_kind_and_pid(fd as i32) {
        #[cfg(not(target_arch = "x86_64"))]
        let _ = pid;
        match fd_kind {
            #[cfg(target_arch = "x86_64")]
            FdKind::PtyMaster(pty_num) | FdKind::PtySlave(pty_num) => {
                // Dispatch to PTY ioctl handler
                let pair = match crate::tty::pty::get(pty_num) {
                    Some(p) => p,
                    None => {
                        log::warn!("sys_ioctl: PTY {} not found", pty_num);
                        return SyscallResult::Err(EBADF);
                    }
                };

                match crate::tty::ioctl::pty_ioctl(&pair, request, arg, pid) {
                    Ok(ret) => return SyscallResult::Ok(ret as u64),
                    Err(errno) => return SyscallResult::Err(errno as u64),
                }
            }
            FdKind::StdIo(_) => {
                // Fall through to console TTY handling
            }
            _ => {
                // Not a TTY or PTY device
                log::debug!("sys_ioctl: fd {} is not a TTY/PTY device", fd);
                return SyscallResult::Err(ENOTTY);
            }
        }
    }

    // Check if this fd is a TTY (stdin/stdout/stderr)
    if !is_tty_fd(fd) {
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

/// Get the FdKind for a file descriptor and the calling process's PID
///
/// Returns None if the process or fd is not found.
fn get_fd_kind_and_pid(fd: i32) -> Option<(FdKind, u32)> {
    // Get current thread ID
    let thread_id = crate::task::scheduler::current_thread_id()?;

    // Get the process manager
    let manager_guard = crate::process::manager();
    let manager = manager_guard.as_ref()?;

    // Find process by thread
    let (pid, process) = manager.find_process_by_thread(thread_id)?;

    // Look up the fd
    let fd_entry = process.fd_table.get(fd)?;

    Some((fd_entry.kind.clone(), pid.as_u64() as u32))
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
