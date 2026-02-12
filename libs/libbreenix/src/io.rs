//! I/O syscall wrappers
//!
//! This module provides both POSIX-named syscall wrappers (read, write, close, pipe, dup, dup2,
//! fcntl, poll, select) and Rust convenience functions (println, eprintln, Stdout, Stderr).
//! Both layers coexist for flexibility.

use crate::error::Error;
use crate::syscall::{nr, raw};
use crate::types::Fd;

/// Write bytes to a file descriptor.
///
/// # Arguments
/// * `fd` - File descriptor to write to
/// * `buf` - Buffer containing data to write
///
/// # Returns
/// Number of bytes written on success, `Err(Error)` on error.
#[inline]
pub fn write(fd: Fd, buf: &[u8]) -> Result<usize, Error> {
    let ret = unsafe { raw::syscall3(nr::WRITE, fd.raw(), buf.as_ptr() as u64, buf.len() as u64) };
    Error::from_syscall(ret as i64).map(|v| v as usize)
}

/// Read bytes from a file descriptor.
///
/// Note: Currently only works with keyboard input (async, non-blocking).
///
/// # Arguments
/// * `fd` - File descriptor to read from
/// * `buf` - Buffer to read data into
///
/// # Returns
/// Number of bytes read on success, `Err(Error)` on error.
#[inline]
pub fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, Error> {
    let ret = unsafe { raw::syscall3(nr::READ, fd.raw(), buf.as_mut_ptr() as u64, buf.len() as u64) };
    Error::from_syscall(ret as i64).map(|v| v as usize)
}

/// Standard output writer
pub struct Stdout;

impl Stdout {
    /// Write bytes to stdout
    #[inline]
    pub fn write(&self, buf: &[u8]) -> Result<usize, Error> {
        write(Fd::STDOUT, buf)
    }

    /// Write a string to stdout
    #[inline]
    pub fn write_str(&self, s: &str) -> Result<usize, Error> {
        self.write(s.as_bytes())
    }
}

/// Standard error writer
pub struct Stderr;

impl Stderr {
    /// Write bytes to stderr
    #[inline]
    pub fn write(&self, buf: &[u8]) -> Result<usize, Error> {
        write(Fd::STDERR, buf)
    }

    /// Write a string to stderr
    #[inline]
    pub fn write_str(&self, s: &str) -> Result<usize, Error> {
        self.write(s.as_bytes())
    }
}

/// Get a handle to stdout
#[inline]
pub fn stdout() -> Stdout {
    Stdout
}

/// Get a handle to stderr
#[inline]
pub fn stderr() -> Stderr {
    Stderr
}

/// Print a string to stdout (convenience function)
#[inline]
pub fn print(s: &str) {
    let _ = stdout().write_str(s);
}

/// Print a string to stdout with newline (convenience function)
#[inline]
pub fn println(s: &str) {
    let _ = stdout().write_str(s);
    let _ = stdout().write(b"\n");
}

/// Close a file descriptor.
///
/// # Arguments
/// * `fd` - File descriptor to close
///
/// # Returns
/// `Ok(())` on success, `Err(Error)` on error.
#[inline]
pub fn close(fd: Fd) -> Result<(), Error> {
    let ret = unsafe { raw::syscall1(nr::CLOSE, fd.raw()) };
    Error::from_syscall(ret as i64).map(|_| ())
}

/// Create a pipe.
///
/// Creates a unidirectional data channel. Returns (read_end, write_end).
///
/// # Returns
/// `Ok((read_fd, write_fd))` on success, `Err(Error)` on error.
#[inline]
pub fn pipe() -> Result<(Fd, Fd), Error> {
    let mut pipefd = [0i32; 2];
    let ret = unsafe { raw::syscall1(nr::PIPE, pipefd.as_mut_ptr() as u64) };
    Error::from_syscall(ret as i64).map(|_| {
        (Fd::from_raw(pipefd[0] as u64), Fd::from_raw(pipefd[1] as u64))
    })
}

/// Create a pipe with flags.
///
/// Like pipe(), but allows setting additional flags on the pipe file descriptors.
///
/// # Arguments
/// * `flags` - Flags to apply:
///   - O_CLOEXEC (0x80000): Set close-on-exec flag on both fds
///   - O_NONBLOCK (0x800): Set non-blocking mode on both fds
///
/// # Returns
/// `Ok((read_fd, write_fd))` on success, `Err(Error)` on error.
#[inline]
pub fn pipe2(flags: i32) -> Result<(Fd, Fd), Error> {
    let mut pipefd = [0i32; 2];
    let ret = unsafe { raw::syscall2(nr::PIPE2, pipefd.as_mut_ptr() as u64, flags as u64) };
    Error::from_syscall(ret as i64).map(|_| {
        (Fd::from_raw(pipefd[0] as u64), Fd::from_raw(pipefd[1] as u64))
    })
}

/// Duplicate a file descriptor.
///
/// Creates a copy of the file descriptor `old_fd`, using the lowest-numbered
/// unused file descriptor for the new descriptor.
///
/// # Arguments
/// * `old_fd` - File descriptor to duplicate
///
/// # Returns
/// New file descriptor on success, `Err(Error)` on error.
#[inline]
pub fn dup(old_fd: Fd) -> Result<Fd, Error> {
    let ret = unsafe { raw::syscall1(nr::DUP, old_fd.raw()) };
    Error::from_syscall(ret as i64).map(|v| Fd::from_raw(v))
}

/// Duplicate a file descriptor to a specific number.
///
/// Creates a copy of the file descriptor `old_fd`, using `new_fd` for the new
/// descriptor. If `new_fd` was previously open, it is silently closed before
/// being reused.
///
/// Per POSIX: if old_fd == new_fd, dup2 just validates old_fd and returns it.
///
/// # Arguments
/// * `old_fd` - File descriptor to duplicate
/// * `new_fd` - Target file descriptor number
///
/// # Returns
/// `new_fd` on success, `Err(Error)` on error.
#[inline]
pub fn dup2(old_fd: Fd, new_fd: Fd) -> Result<Fd, Error> {
    let ret = unsafe { raw::syscall2(nr::DUP2, old_fd.raw(), new_fd.raw()) };
    Error::from_syscall(ret as i64).map(|v| Fd::from_raw(v))
}

/// fcntl command constants
pub mod fcntl_cmd {
    /// Duplicate file descriptor to lowest available >= arg
    pub const F_DUPFD: i32 = 0;
    /// Get file descriptor flags (FD_CLOEXEC)
    pub const F_GETFD: i32 = 1;
    /// Set file descriptor flags
    pub const F_SETFD: i32 = 2;
    /// Get file status flags (O_NONBLOCK, etc.)
    pub const F_GETFL: i32 = 3;
    /// Set file status flags
    pub const F_SETFL: i32 = 4;
    /// Duplicate fd with close-on-exec flag set
    pub const F_DUPFD_CLOEXEC: i32 = 1030;
}

/// File descriptor flags (for F_GETFD/F_SETFD)
pub mod fd_flags {
    /// Close-on-exec flag
    pub const FD_CLOEXEC: i32 = 1;
}

/// File status flags (for F_GETFL/F_SETFL, open, pipe2)
pub mod status_flags {
    /// Non-blocking I/O mode
    pub const O_NONBLOCK: i32 = 0x800; // 2048
    /// Append mode
    pub const O_APPEND: i32 = 0x400; // 1024
    /// Close-on-exec (for open/pipe2, stored as FD_CLOEXEC)
    pub const O_CLOEXEC: i32 = 0x80000; // 524288
}

/// Perform file control operations on a file descriptor.
///
/// # Arguments
/// * `fd` - File descriptor to operate on
/// * `cmd` - Command (F_DUPFD, F_GETFD, F_SETFD, F_GETFL, F_SETFL, etc.)
/// * `arg` - Command-specific argument
///
/// # Returns
/// Command-dependent value on success, `Err(Error)` on error.
#[inline]
pub fn fcntl(fd: Fd, cmd: i32, arg: i64) -> Result<i64, Error> {
    let ret = unsafe { raw::syscall3(nr::FCNTL, fd.raw(), cmd as u64, arg as u64) };
    Error::from_syscall(ret as i64).map(|v| v as i64)
}

/// Get file descriptor flags.
///
/// # Returns
/// Flags on success (FD_CLOEXEC), `Err(Error)` on error.
#[inline]
pub fn fcntl_getfd(fd: Fd) -> Result<i64, Error> {
    fcntl(fd, fcntl_cmd::F_GETFD, 0)
}

/// Set file descriptor flags.
///
/// # Arguments
/// * `fd` - File descriptor
/// * `flags` - New flags (typically FD_CLOEXEC)
///
/// # Returns
/// `Ok(0)` on success, `Err(Error)` on error.
#[inline]
pub fn fcntl_setfd(fd: Fd, flags: i32) -> Result<i64, Error> {
    fcntl(fd, fcntl_cmd::F_SETFD, flags as i64)
}

/// Get file status flags.
///
/// # Returns
/// Flags on success (O_NONBLOCK, etc.), `Err(Error)` on error.
#[inline]
pub fn fcntl_getfl(fd: Fd) -> Result<i64, Error> {
    fcntl(fd, fcntl_cmd::F_GETFL, 0)
}

/// Set file status flags.
///
/// # Arguments
/// * `fd` - File descriptor
/// * `flags` - New flags (O_NONBLOCK, O_APPEND)
///
/// # Returns
/// `Ok(0)` on success, `Err(Error)` on error.
#[inline]
pub fn fcntl_setfl(fd: Fd, flags: i32) -> Result<i64, Error> {
    fcntl(fd, fcntl_cmd::F_SETFL, flags as i64)
}

/// Poll event constants
pub mod poll_events {
    /// Data available to read
    pub const POLLIN: i16 = 0x0001;
    /// Write won't block
    pub const POLLOUT: i16 = 0x0004;
    /// Error condition (output only)
    pub const POLLERR: i16 = 0x0008;
    /// Hang up (output only)
    pub const POLLHUP: i16 = 0x0010;
    /// Invalid fd (output only)
    pub const POLLNVAL: i16 = 0x0020;
}

/// pollfd structure for poll()
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PollFd {
    /// File descriptor to poll
    pub fd: i32,
    /// Events to poll for (input)
    pub events: i16,
    /// Events that occurred (output)
    pub revents: i16,
}

impl PollFd {
    /// Create a new pollfd for a given fd and events
    pub fn new(fd: Fd, events: i16) -> Self {
        PollFd {
            fd: fd.raw() as i32,
            events,
            revents: 0,
        }
    }
}

/// Poll file descriptors for I/O readiness.
///
/// Waits for events on a set of file descriptors.
///
/// # Arguments
/// * `fds` - Slice of pollfd structures specifying fds and events to monitor
/// * `timeout` - Timeout in milliseconds (-1 = infinite, 0 = return immediately)
///
/// # Returns
/// Number of fds with non-zero revents on success, 0 on timeout, `Err(Error)` on error.
#[inline]
pub fn poll(fds: &mut [PollFd], timeout: i32) -> Result<usize, Error> {
    let ret = unsafe {
        raw::syscall3(
            nr::POLL,
            fds.as_mut_ptr() as u64,
            fds.len() as u64,
            timeout as u64,
        )
    };
    Error::from_syscall(ret as i64).map(|v| v as usize)
}

// ============ select() implementation ============

/// fd_set type for select() - a u64 bitmap supporting fds 0-63
pub type FdSet = u64;

/// Clear an fd_set (set all bits to zero)
#[inline]
pub fn fd_zero(set: &mut FdSet) {
    *set = 0;
}

/// Set a bit in an fd_set
#[inline]
pub fn fd_set_bit(fd: Fd, set: &mut FdSet) {
    let raw = fd.raw() as i32;
    if raw >= 0 && raw < 64 {
        *set |= 1u64 << raw;
    }
}

/// Clear a bit in an fd_set
#[inline]
pub fn fd_clr(fd: Fd, set: &mut FdSet) {
    let raw = fd.raw() as i32;
    if raw >= 0 && raw < 64 {
        *set &= !(1u64 << raw);
    }
}

/// Check if a bit is set in an fd_set
#[inline]
pub fn fd_isset(fd: Fd, set: &FdSet) -> bool {
    let raw = fd.raw() as i32;
    if raw >= 0 && raw < 64 {
        (*set & (1u64 << raw)) != 0
    } else {
        false
    }
}

/// Synchronous I/O multiplexing using select().
///
/// Monitors multiple file descriptors for I/O readiness.
///
/// # Arguments
/// * `nfds` - Highest-numbered file descriptor + 1
/// * `readfds` - Optional fd_set for read monitoring (modified in place)
/// * `writefds` - Optional fd_set for write monitoring (modified in place)
/// * `exceptfds` - Optional fd_set for exception monitoring (modified in place)
/// * `timeout_ptr` - Timeout pointer (0 for non-blocking, currently only 0 supported)
///
/// # Returns
/// Number of ready fds on success, 0 on timeout, `Err(Error)` on error.
///
/// # Note
/// Currently only non-blocking select (timeout=0/NULL) is supported.
/// The fd_sets are modified in place to indicate which fds are ready.
#[inline]
pub fn select(
    nfds: i32,
    readfds: Option<&mut FdSet>,
    writefds: Option<&mut FdSet>,
    exceptfds: Option<&mut FdSet>,
    timeout_ptr: u64,
) -> Result<usize, Error> {
    let readfds_ptr = readfds.map(|p| p as *mut FdSet as u64).unwrap_or(0);
    let writefds_ptr = writefds.map(|p| p as *mut FdSet as u64).unwrap_or(0);
    let exceptfds_ptr = exceptfds.map(|p| p as *mut FdSet as u64).unwrap_or(0);

    let ret = unsafe {
        raw::syscall5(
            nr::SELECT,
            nfds as u64,
            readfds_ptr,
            writefds_ptr,
            exceptfds_ptr,
            timeout_ptr,
        )
    };
    Error::from_syscall(ret as i64).map(|v| v as usize)
}
