//! Process management syscall wrappers
//!
//! This module provides both POSIX-named syscall wrappers (fork, getpid, waitpid, exec, etc.)
//! and Rust-idiomatic convenience functions (yield_now). Both layers coexist for flexibility.

use crate::error::Error;
use crate::syscall::{nr, raw};
use crate::types::{Pid, Tid};

/// Result of a fork() call.
pub enum ForkResult {
    /// We are the parent; contains the child's PID.
    Parent(Pid),
    /// We are the child.
    Child,
}

/// Exit the current process with the given exit code.
///
/// This function never returns.
#[inline]
pub fn exit(code: i32) -> ! {
    unsafe {
        raw::syscall1(nr::EXIT, code as u64);
    }
    // Should never reach here, but need this for the ! return type
    loop {
        core::hint::spin_loop();
    }
}

/// Create a child process (fork).
///
/// Returns:
/// - `Ok(ForkResult::Parent(child_pid))` in the parent
/// - `Ok(ForkResult::Child)` in the child
/// - `Err(Error)` on failure
#[inline]
pub fn fork() -> Result<ForkResult, Error> {
    let ret = unsafe { raw::syscall0(nr::FORK) };
    let val = Error::from_syscall(ret as i64)?;
    if val == 0 {
        Ok(ForkResult::Child)
    } else {
        Ok(ForkResult::Parent(Pid::from_raw(val)))
    }
}

/// Replace the current process image with a new program (no arguments).
///
/// Note: Currently only supports embedded binaries, not filesystem loading.
/// IMPORTANT: path must be a null-terminated C string slice (ending with \0)
///
/// # Arguments
/// * `path` - Path to the program (must end with \0 byte). Use b"program_name\0"
///
/// # Returns
/// This function should not return on success. On error, returns `Err(Error)`.
///
/// # Safety
/// The path MUST be a null-terminated byte slice. Rust &str is NOT null-terminated.
/// Use: `exec(b"program_name\0")` instead of `exec("program_name")`
#[inline]
pub fn exec(path: &[u8]) -> Result<core::convert::Infallible, Error> {
    // Verify path is null-terminated
    debug_assert!(path.last() == Some(&0), "exec path must be null-terminated");
    // Pass null argv - kernel will use program name as argv[0]
    let ret = unsafe {
        raw::syscall2(nr::EXEC, path.as_ptr() as u64, 0)
    };
    // exec only returns on error
    Err(Error::from_syscall(ret as i64).unwrap_err())
}

/// Replace the current process image with a new program (with arguments).
///
/// This implements execv() which allows passing command-line arguments.
/// The kernel sets up argc/argv on the new process's stack following Linux ABI.
///
/// IMPORTANT: path and each argv element must be null-terminated.
///
/// # Arguments
/// * `path` - Path to the program (must end with \0 byte)
/// * `argv` - Array of argument pointers, must end with a null pointer
///
/// # Example
/// ```
/// // Execute "cat" with argument "/hello.txt"
/// let path = b"cat\0";
/// let arg0 = b"cat\0";
/// let arg1 = b"/hello.txt\0";
/// let argv: [*const u8; 3] = [arg0.as_ptr(), arg1.as_ptr(), core::ptr::null()];
/// execv(path, argv.as_ptr());
/// ```
///
/// # Returns
/// This function should not return on success. On error, returns `Err(Error)`.
#[inline]
pub fn execv(path: &[u8], argv: *const *const u8) -> Result<core::convert::Infallible, Error> {
    debug_assert!(path.last() == Some(&0), "execv path must be null-terminated");
    let ret = unsafe {
        raw::syscall2(nr::EXEC, path.as_ptr() as u64, argv as u64)
    };
    // exec only returns on error
    Err(Error::from_syscall(ret as i64).unwrap_err())
}

/// Get the current process ID.
#[inline]
pub fn getpid() -> Result<Pid, Error> {
    let ret = unsafe { raw::syscall0(nr::GETPID) };
    Error::from_syscall(ret as i64).map(Pid::from_raw)
}

/// Get the current thread ID.
#[inline]
pub fn gettid() -> Result<Tid, Error> {
    let ret = unsafe { raw::syscall0(nr::GETTID) };
    Error::from_syscall(ret as i64).map(Tid::from_raw)
}

/// Yield the CPU to the scheduler.
///
/// This is a hint to the scheduler that the current thread is willing
/// to give up its time slice. The scheduler may or may not honor this.
#[inline]
pub fn yield_now() -> Result<(), Error> {
    let ret = unsafe { raw::syscall0(nr::YIELD) };
    Error::from_syscall(ret as i64).map(|_| ())
}

/// waitpid options
pub const WNOHANG: i32 = 1;
#[allow(dead_code)]
pub const WUNTRACED: i32 = 2;

/// Wait for a child process to change state.
///
/// # Arguments
/// * `pid` - Process ID to wait for:
///   - `pid > 0`: Wait for specific child
///   - `pid == -1`: Wait for any child
///   - `pid == 0`: Wait for any child in same process group (not implemented)
///   - `pid < -1`: Wait for any child in process group |pid| (not implemented)
/// * `status` - Pointer to store exit status (can be null)
/// * `options` - Options flags (e.g., WNOHANG)
///
/// # Returns
/// * On success: PID of the terminated child (or 0 if WNOHANG and no child terminated)
/// * On error: `Err(Error)`
#[inline]
pub fn waitpid(pid: i32, status: *mut i32, options: i32) -> Result<Pid, Error> {
    let ret = unsafe {
        raw::syscall3(nr::WAIT4, pid as u64, status as u64, options as u64)
    };
    Error::from_syscall(ret as i64).map(Pid::from_raw)
}

/// Macros for extracting information from waitpid status
///
/// Check if child exited normally (via exit() or return from main)
#[inline]
pub fn wifexited(status: i32) -> bool {
    // In Linux, normal exit is when low 7 bits are 0
    (status & 0x7f) == 0
}

/// Get exit code from status (only valid if WIFEXITED is true)
#[inline]
pub fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Check if child was terminated by a signal
#[inline]
pub fn wifsignaled(status: i32) -> bool {
    // Signaled if low 7 bits are non-zero and not 0x7f (stopped)
    let sig = status & 0x7f;
    sig != 0 && sig != 0x7f
}

/// Get signal number that terminated the child (only valid if WIFSIGNALED is true)
#[inline]
pub fn wtermsig(status: i32) -> i32 {
    status & 0x7f
}

/// Check if child was stopped by a signal (job control)
#[inline]
pub fn wifstopped(status: i32) -> bool {
    // Stopped if low 8 bits are 0x7f
    (status & 0xff) == 0x7f
}

/// Get signal number that stopped the child (only valid if WIFSTOPPED is true)
#[inline]
pub fn wstopsig(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Set the process group ID for a process.
///
/// # Arguments
/// * `pid` - Process ID to set the group for:
///   - `pid == 0`: Use the current process's PID
///   - `pid > 0`: Set the group for the specified process
/// * `pgid` - New process group ID:
///   - `pgid == 0`: Use the PID of the target process as the new PGID
///   - `pgid > 0`: Use the specified PGID
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(Error)` on error
#[inline]
pub fn setpgid(pid: i32, pgid: i32) -> Result<(), Error> {
    let ret = unsafe { raw::syscall2(nr::SETPGID, pid as u64, pgid as u64) };
    Error::from_syscall(ret as i64).map(|_| ())
}

/// Get the process group ID for a process.
///
/// # Arguments
/// * `pid` - Process ID to query:
///   - `pid == 0`: Get the PGID of the current process
///   - `pid > 0`: Get the PGID of the specified process
///
/// # Returns
/// * On success: the process group ID
/// * On error: `Err(Error)`
#[inline]
pub fn getpgid(pid: i32) -> Result<Pid, Error> {
    let ret = unsafe { raw::syscall1(nr::GETPGID, pid as u64) };
    Error::from_syscall(ret as i64).map(Pid::from_raw)
}

/// Set the calling process as a new process group leader.
///
/// This is equivalent to `setpgid(0, 0)`, which sets the current process's
/// PGID to its own PID, making it a process group leader.
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(Error)` on error
#[inline]
pub fn setpgrp() -> Result<(), Error> {
    setpgid(0, 0)
}

/// Get the process group ID of the calling process.
///
/// This is equivalent to `getpgid(0)`.
///
/// # Returns
/// * On success: the process group ID
/// * On error: `Err(Error)`
#[inline]
pub fn getpgrp() -> Result<Pid, Error> {
    getpgid(0)
}

/// Create a new session and set the calling process as the session leader.
///
/// The calling process becomes:
/// - The session leader of a new session
/// - The process group leader of a new process group
/// - Detached from any controlling terminal
///
/// # Returns
/// * On success: the new session ID (which equals the calling process's PID)
/// * On error: `Err(Error)` (typically EPERM if already a process group leader)
#[inline]
pub fn setsid() -> Result<Pid, Error> {
    let ret = unsafe { raw::syscall0(nr::SETSID) };
    Error::from_syscall(ret as i64).map(Pid::from_raw)
}

/// Get the session ID of a process.
///
/// # Arguments
/// * `pid` - Process ID to query:
///   - `pid == 0`: Get the SID of the current process
///   - `pid > 0`: Get the SID of the specified process
///
/// # Returns
/// * On success: the session ID
/// * On error: `Err(Error)` (typically ESRCH if process not found)
#[inline]
pub fn getsid(pid: i32) -> Result<Pid, Error> {
    let ret = unsafe { raw::syscall1(nr::GETSID, pid as u64) };
    Error::from_syscall(ret as i64).map(Pid::from_raw)
}

/// Get the current working directory.
///
/// Writes the absolute pathname of the current working directory
/// to the provided buffer.
///
/// # Arguments
/// * `buf` - Buffer to store the path
///
/// # Returns
/// * On success: number of bytes written
/// * On error: `Err(Error)`
///
/// # Errors
/// * EFAULT - Invalid buffer pointer
/// * ERANGE - Buffer too small
#[inline]
pub fn getcwd(buf: &mut [u8]) -> Result<usize, Error> {
    let ret = unsafe {
        raw::syscall2(nr::GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64)
    };
    Error::from_syscall(ret as i64).map(|v| v as usize)
}

/// Change the current working directory.
///
/// Changes the current working directory to the specified path.
///
/// # Arguments
/// * `path` - Path to the new working directory (must be null-terminated)
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(Error)` on error
///
/// # Errors
/// * ENOENT - Directory does not exist
/// * ENOTDIR - Path is not a directory
/// * EACCES - Permission denied
#[inline]
pub fn chdir(path: &[u8]) -> Result<(), Error> {
    debug_assert!(path.last() == Some(&0), "chdir path must be null-terminated");
    let ret = unsafe { raw::syscall1(nr::CHDIR, path.as_ptr() as u64) };
    Error::from_syscall(ret as i64).map(|_| ())
}
