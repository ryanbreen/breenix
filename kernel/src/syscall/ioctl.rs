//! ioctl syscall implementation
//!
//! The ioctl (I/O control) syscall provides a mechanism for device-specific
//! operations that don't fit into the standard read/write model.
//!
//! Supports:
//! - TTY-related ioctls for terminal control
//! - PTY-specific ioctls for pseudo-terminal devices

use super::SyscallResult;
use crate::ipc::fd::{FdKind, FdTable};

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

    if fd > i32::MAX as u64 { return SyscallResult::Err(EBADF); }

    let kind = match get_fd_kind(fd as i32) {
        Some(kind) => kind, None => return SyscallResult::Err(EBADF),
    };
    if matches!(request, 0x5450 | 0x5451 | 0x5421 | 0x5452) {
        let Some(tid) = crate::task::scheduler::current_thread_id() else { return SyscallResult::Err(EBADF); };
        let mut manager = crate::process::manager();
        let Some(pm) = manager.as_mut() else { return SyscallResult::Err(EBADF); };
        let Some((_, process)) = pm.find_process_by_thread_mut(tid) else { return SyscallResult::Err(EBADF); };
        return descriptor_ioctl(&mut process.fd_table, fd as i32, request, arg);
    }
    if request == 0x541b { // FIONREAD / TIOCINQ
        let count = match &kind {
            FdKind::PtyMaster(p) => Some(p.slave_to_master.lock().available()),
            FdKind::PtySlave(p) => Some(p.ldisc.lock().bytes_available()),
            FdKind::StdIo(_) | FdKind::Device(crate::fs::devfs::DeviceType::Tty | crate::fs::devfs::DeviceType::Console) => Some(crate::ipc::stdin::available()),
            FdKind::PipeRead(p) | FdKind::PipeWrite(p) | FdKind::FifoRead(_, p, _) | FdKind::FifoWrite(_, p, _) => Some(p.lock().available()),
            _ => None,
        };
        return match count {
            Some(n) => match super::userptr::copy_to_user(arg as *mut i32, &(n.min(i32::MAX as usize) as i32)) {
                Ok(()) => SyscallResult::Ok(0), Err(e) => SyscallResult::Err(e),
            },
            None => SyscallResult::Err(ENOTTY),
        };
    }
    let readable = descriptor_readable(fd as i32);
    if matches!(kind, FdKind::Device(crate::fs::devfs::DeviceType::Rtc)) {
        return rtc_ioctl(request, arg);
    }
    let resolved = resolve_terminal(kind);
    let (tty, side) = match resolved {
        Ok(resolved) => resolved,
        Err(result) => return result,
    };

    let result = if request == crate::tty::ioctl::TIOCSCTTY {
        crate::tty::ctty::ioctl_with_access(&tty, side, request, arg, readable)
    } else { crate::tty::ioctl::dispatch(&tty, side, request, arg) };
    match result {
        Ok(ret) => SyscallResult::Ok(ret as u64),
        Err(errno) => SyscallResult::Err(errno as u64),
    }
}

/// Resolve an open descriptor after generic controls and fd validation.
fn resolve_terminal(kind: FdKind) -> Result<(crate::tty::ctty::TtyRef, crate::tty::ioctl::Side), SyscallResult> {
    use crate::tty::ctty::TtyRef;
    use crate::tty::ioctl::Side;
    match kind {
        FdKind::PtyMaster(pair) => Ok((TtyRef::Pty(pair), Side::Master)),
        FdKind::PtySlave(pair) => Ok((TtyRef::Pty(pair), Side::Slave)),
        FdKind::Device(crate::fs::devfs::DeviceType::Console | crate::fs::devfs::DeviceType::Tty)
            | FdKind::StdIo(_) => console().map(|tty| (tty, Side::Console)),
        _ => Err(SyscallResult::Err(ENOTTY)),
    }
}

/// Descriptor-wide ioctls apply to open descriptors before driver dispatch.
fn descriptor_ioctl(table: &mut FdTable, fd: i32, request: u64, arg: u64) -> SyscallResult {
    use crate::ipc::fd::{flags::FD_CLOEXEC, status_flags::O_NONBLOCK};
    let Some(entry) = table.get(fd) else { return SyscallResult::Err(EBADF); };
    match request {
        0x5421 => { // FIONBIO: status belongs to the shared open description.
            let enabled: i32 = match super::userptr::copy_from_user(arg as *const i32) {
                Ok(v) => v, Err(e) => return SyscallResult::Err(e),
            };
            entry.update_status_flags(O_NONBLOCK, if enabled != 0 { O_NONBLOCK } else { 0 });
            SyscallResult::Ok(0)
        }
        0x5450 | 0x5451 => { // FIONCLEX / FIOCLEX are per-descriptor.
            let flags = if request == 0x5451 { entry.flags | FD_CLOEXEC } else { entry.flags & !FD_CLOEXEC };
            match table.set_fd_flags(fd, flags) {
                Ok(()) => SyscallResult::Ok(0), Err(e) => SyscallResult::Err(e as u64),
            }
        }
        // No driver has a SIGIO subscription/owner API yet. Do not accept a
        // subscription whose promised notifications cannot be delivered.
        0x5452 => SyscallResult::Err(ENOTTY),
        _ => SyscallResult::Err(ENOTTY),
    }
}

fn descriptor_readable(fd: i32) -> bool {
    let Some(tid) = crate::task::scheduler::current_thread_id() else { return false; };
    let manager = crate::process::manager();
    manager.as_ref().and_then(|pm| pm.find_process_by_thread(tid))
        .and_then(|(_, p)| p.fd_table.get(fd)).is_some_and(|entry| entry.readable())
}

fn fd_kind(table: &FdTable, fd: u64) -> Option<FdKind> {
    if fd > i32::MAX as u64 { return None; }
    Some(table.get(fd as i32)?.kind.clone())
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
    fd_kind(&process.fd_table, fd as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::fd::{flags::FD_CLOEXEC, status_flags::O_NONBLOCK};

    #[test]
    fn closed_standard_descriptors_are_ebadf() {
        let mut table = FdTable::new();
        for fd in 0..=2 {
            assert!(fd_kind(&table, fd).is_some());
            table.close(fd as i32).unwrap();
            assert!(fd_kind(&table, fd).is_none());
            assert_eq!(descriptor_ioctl(&mut table, fd as i32, 0x5451, 0), SyscallResult::Err(EBADF));
        }
    }

    #[test]
    fn fd_lookup_distinguishes_open_nonterminal_from_closed_and_out_of_range() {
        let mut table = FdTable::empty();
        let fd = table.alloc(FdKind::Device(crate::fs::devfs::DeviceType::Null)).unwrap();
        assert!(matches!(fd_kind(&table, fd as u64), Some(FdKind::Device(crate::fs::devfs::DeviceType::Null))));
        assert!(matches!(resolve_terminal(fd_kind(&table, fd as u64).unwrap()),
            Err(SyscallResult::Err(ENOTTY))));
        assert!(fd_kind(&table, i32::MAX as u64 + 1).is_none());
        assert!(fd_kind(&table, u64::MAX).is_none());
        table.close(fd).unwrap();
        assert!(fd_kind(&table, fd as u64).is_none());
    }

    #[test]
    fn descriptor_controls_preserve_independent_flags_and_validate_user_copy() {
        let mut table = FdTable::new();
        let one = 1i32;
        assert_eq!(descriptor_ioctl(&mut table, 0, 0x5421, &one as *const i32 as u64), SyscallResult::Ok(0));
        assert_eq!(table.get(0).unwrap().status_flags() & O_NONBLOCK, O_NONBLOCK);
        assert_eq!(descriptor_ioctl(&mut table, 0, 0x5451, 0), SyscallResult::Ok(0));
        assert_eq!(table.get(0).unwrap().flags & FD_CLOEXEC, FD_CLOEXEC);
        assert_eq!(descriptor_ioctl(&mut table, 0, 0x5450, 0), SyscallResult::Ok(0));
        assert_eq!(table.get(0).unwrap().flags & FD_CLOEXEC, 0);
        assert_eq!(table.get(0).unwrap().status_flags() & O_NONBLOCK, O_NONBLOCK);
        assert_eq!(descriptor_ioctl(&mut table, 0, 0x5421, 0), SyscallResult::Err(14));
        assert_eq!(descriptor_ioctl(&mut table, 0, 0x5452, 0), SyscallResult::Err(ENOTTY));
    }
}
