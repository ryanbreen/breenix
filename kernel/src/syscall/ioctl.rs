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
pub(crate) const EBADF: u64 = 9; // Bad file descriptor
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
    log::debug!(
        "sys_ioctl: fd={}, request={:#x}, arg={:#x}",
        fd,
        request,
        arg
    );

    #[cfg(feature = "boot_tests")]
    if request == super::blocking_io_oracle::QUERY
        || request == super::blocking_io_oracle::FIFO_FIXTURE
        || request == super::blocking_io_oracle::INPUT_QUERY
        || request == super::blocking_io_oracle::INPUT_INJECT
    {
        return super::blocking_io_oracle::dispatch(fd, request, arg);
    }

    use crate::tty::ctty::TtyRef;
    use crate::tty::ioctl::Side;

    // Resolve the fd to a terminal and the side the ioctl is made on.
    let pty = |pty_num: u32, side: Side| match crate::tty::pty::get(pty_num) {
        Some(pair) => Ok((TtyRef::Pty(pair), side)),
        None => {
            log::warn!("sys_ioctl: PTY {} not found", pty_num);
            Err(SyscallResult::Err(EBADF))
        }
    };
    let resolved = match get_fd_kind(fd as i32) {
        Some(FdKind::PtyMaster(pty_num)) => pty(pty_num, Side::Master),
        Some(FdKind::PtySlave(pty_num)) => pty(pty_num, Side::Slave),
        Some(FdKind::Device(crate::fs::devfs::DeviceType::Rtc)) => return rtc_ioctl(request, arg),
        Some(FdKind::StdIo(_)) => console().map(|tty| (tty, Side::Console)),
        Some(_) => {
            log::debug!("sys_ioctl: fd {} is not a TTY/PTY device", fd);
            return SyscallResult::Err(ENOTTY);
        }
        // Check if this fd is a TTY (stdin/stdout/stderr)
        None if is_tty_fd(fd) => console().map(|tty| (tty, Side::Console)),
        None => {
            log::debug!("sys_ioctl: fd {} is not a TTY", fd);
            return SyscallResult::Err(ENOTTY);
        }
    };
    let (tty, side) = match resolved {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };

    match crate::tty::ioctl::dispatch(&tty, side, request, arg) {
        Ok(ret) => SyscallResult::Ok(ret as u64),
        Err(errno) => SyscallResult::Err(errno as u64),
    }
}

/// The console terminal.
fn console() -> Result<crate::tty::ctty::TtyRef, SyscallResult> {
    match crate::tty::console() {
        Some(tty) => Ok(crate::tty::ctty::TtyRef::Console(tty)),
        None => {
            log::warn!("sys_ioctl: TTY subsystem not initialized");
            Err(SyscallResult::Err(EBADF))
        }
    }
}

/// RTC_RD_TIME on /dev/rtc.
fn rtc_ioctl(request: u64, arg: u64) -> SyscallResult {
    if request != 0x8024_7009 { return SyscallResult::Err(ENOTTY); }
    let stamp = match crate::time::rtc::read_rtc_time() {
        Ok(stamp) => stamp,
        Err(_) => return SyscallResult::Err(5),
    };
    let dt = crate::time::DateTime::from_unix_timestamp(stamp);
    let jan1 = crate::time::DateTime { year: dt.year, month: 1, day: 1, hour: 0, minute: 0, second: 0 }.to_unix_timestamp();
    // Linux struct rtc_time, with struct tm's month/year origins.
    let tm = [i32::from(dt.second), i32::from(dt.minute), i32::from(dt.hour),
        i32::from(dt.day), i32::from(dt.month) - 1, i32::from(dt.year) - 1900,
        ((stamp / 86400 + 4) % 7) as i32, ((stamp - jan1) / 86400) as i32, 0];
    match super::userptr::copy_to_user(arg as *mut [i32; 9], &tm) {
        Ok(()) => SyscallResult::Ok(0), Err(e) => SyscallResult::Err(e),
    }
}

/// The FdKind of the caller's file descriptor `fd`, or None if it is not open.
fn get_fd_kind(fd: i32) -> Option<FdKind> {
    let thread_id = crate::task::scheduler::current_thread_id()?;
    let manager_guard = crate::process::manager();
    let manager = manager_guard.as_ref()?;
    let (_, process) = manager.find_process_by_thread(thread_id)?;
    Some(process.fd_table.get(fd)?.kind.clone())
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
        assert_eq!(EBADF, 9); // Bad file descriptor
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
