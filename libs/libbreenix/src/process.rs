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
///
/// # Arguments
/// * `path` - Path to the program (for embedded binaries, this is the binary name)
/// * `args` - Arguments string (currently unused)
///
/// # Returns
/// This function should not return on success. On error, returns negative errno.
#[inline]
pub fn exec(path: &str, args: &str) -> i64 {
    unsafe {
        raw::syscall2(nr::EXEC, path.as_ptr() as u64, args.as_ptr() as u64) as i64
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
