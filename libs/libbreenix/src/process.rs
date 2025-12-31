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
