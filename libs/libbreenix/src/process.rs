//! Process management syscall wrappers

use crate::syscall::{nr, raw};
use crate::types::{Pid, Tid};

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
/// - In parent: child's PID (positive)
/// - In child: 0
/// - On error: negative errno
#[inline]
pub fn fork() -> i64 {
    unsafe { raw::syscall0(nr::FORK) as i64 }
}

/// Replace the current process image with a new program.
///
/// Note: Currently only supports embedded binaries, not filesystem loading.
/// IMPORTANT: path must be a null-terminated C string slice (ending with \0)
///
/// # Arguments
/// * `path` - Path to the program (must end with \0 byte). Use b"program_name\0"
///
/// # Returns
/// This function should not return on success. On error, returns negative errno.
///
/// # Safety
/// The path MUST be a null-terminated byte slice. Rust &str is NOT null-terminated.
/// Use: `exec(b"program_name\0")` instead of `exec("program_name")`
#[inline]
pub fn exec(path: &[u8]) -> i64 {
    // Verify path is null-terminated
    debug_assert!(path.last() == Some(&0), "exec path must be null-terminated");
    unsafe {
        raw::syscall2(nr::EXEC, path.as_ptr() as u64, 0) as i64
    }
}

/// Get the current process ID.
#[inline]
pub fn getpid() -> Pid {
    unsafe { raw::syscall0(nr::GETPID) }
}

/// Get the current thread ID.
#[inline]
pub fn gettid() -> Tid {
    unsafe { raw::syscall0(nr::GETTID) }
}

/// Yield the CPU to the scheduler.
///
/// This is a hint to the scheduler that the current thread is willing
/// to give up its time slice. The scheduler may or may not honor this.
#[inline]
pub fn yield_now() {
    unsafe {
        raw::syscall0(nr::YIELD);
    }
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
/// * On success: PID of the terminated child
/// * If WNOHANG and no child terminated: 0
/// * On error: negative errno
#[inline]
pub fn waitpid(pid: i32, status: *mut i32, options: i32) -> i64 {
    unsafe {
        raw::syscall3(nr::WAIT4, pid as u64, status as u64, options as u64) as i64
    }
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
/// * On success: 0
/// * On error: negative errno
#[inline]
pub fn setpgid(pid: i32, pgid: i32) -> i32 {
    unsafe { raw::syscall2(nr::SETPGID, pid as u64, pgid as u64) as i32 }
}

/// Get the process group ID for a process.
///
/// # Arguments
/// * `pid` - Process ID to query:
///   - `pid == 0`: Get the PGID of the current process
///   - `pid > 0`: Get the PGID of the specified process
///
/// # Returns
/// * On success: the process group ID (positive)
/// * On error: negative errno
#[inline]
pub fn getpgid(pid: i32) -> i32 {
    unsafe { raw::syscall1(nr::GETPGID, pid as u64) as i32 }
}

/// Set the calling process as a new process group leader.
///
/// This is equivalent to `setpgid(0, 0)`, which sets the current process's
/// PGID to its own PID, making it a process group leader.
///
/// # Returns
/// * On success: 0
/// * On error: negative errno
#[inline]
pub fn setpgrp() -> i32 {
    setpgid(0, 0)
}

/// Get the process group ID of the calling process.
///
/// This is equivalent to `getpgid(0)`.
///
/// # Returns
/// * On success: the process group ID (positive)
/// * On error: negative errno
#[inline]
pub fn getpgrp() -> i32 {
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
/// * On error: negative errno (typically EPERM if already a process group leader)
#[inline]
pub fn setsid() -> i32 {
    unsafe { raw::syscall0(nr::SETSID) as i32 }
}

/// Get the session ID of a process.
///
/// # Arguments
/// * `pid` - Process ID to query:
///   - `pid == 0`: Get the SID of the current process
///   - `pid > 0`: Get the SID of the specified process
///
/// # Returns
/// * On success: the session ID (positive)
/// * On error: negative errno (typically ESRCH if process not found)
#[inline]
pub fn getsid(pid: i32) -> i32 {
    unsafe { raw::syscall1(nr::GETSID, pid as u64) as i32 }
}
