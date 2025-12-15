//! I/O syscall wrappers

use crate::syscall::{nr, raw};
use crate::types::{fd, Fd};

/// Write bytes to a file descriptor.
///
/// # Arguments
/// * `fd` - File descriptor to write to
/// * `buf` - Buffer containing data to write
///
/// # Returns
/// Number of bytes written on success, negative errno on error.
#[inline]
pub fn write(file: Fd, buf: &[u8]) -> i64 {
    unsafe { raw::syscall3(nr::WRITE, file, buf.as_ptr() as u64, buf.len() as u64) as i64 }
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
/// Number of bytes read on success, negative errno on error.
#[inline]
pub fn read(file: Fd, buf: &mut [u8]) -> i64 {
    unsafe { raw::syscall3(nr::READ, file, buf.as_mut_ptr() as u64, buf.len() as u64) as i64 }
}

/// Standard output writer
pub struct Stdout;

impl Stdout {
    /// Write bytes to stdout
    #[inline]
    pub fn write(&self, buf: &[u8]) -> i64 {
        write(fd::STDOUT, buf)
    }

    /// Write a string to stdout
    #[inline]
    pub fn write_str(&self, s: &str) -> i64 {
        self.write(s.as_bytes())
    }
}

/// Standard error writer
pub struct Stderr;

impl Stderr {
    /// Write bytes to stderr
    #[inline]
    pub fn write(&self, buf: &[u8]) -> i64 {
        write(fd::STDERR, buf)
    }

    /// Write a string to stderr
    #[inline]
    pub fn write_str(&self, s: &str) -> i64 {
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
    stdout().write_str(s);
}

/// Print a string to stdout with newline (convenience function)
#[inline]
pub fn println(s: &str) {
    stdout().write_str(s);
    stdout().write(b"\n");
}

/// Close a file descriptor.
///
/// # Arguments
/// * `fd` - File descriptor to close
///
/// # Returns
/// 0 on success, negative errno on error.
#[inline]
pub fn close(file: Fd) -> i64 {
    unsafe { raw::syscall1(nr::CLOSE, file) as i64 }
}

/// Create a pipe.
///
/// Creates a unidirectional data channel. pipefd[0] is the read end,
/// pipefd[1] is the write end.
///
/// # Arguments
/// * `pipefd` - Array to receive the two file descriptors
///
/// # Returns
/// 0 on success, negative errno on error.
#[inline]
pub fn pipe(pipefd: &mut [i32; 2]) -> i64 {
    unsafe { raw::syscall1(nr::PIPE, pipefd.as_mut_ptr() as u64) as i64 }
}
