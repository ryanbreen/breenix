//! Process management for Breenix userspace programs.
//!
//! This module re-exports process-related functions from libbreenix
//! and provides std-like APIs for process control.

pub use libbreenix::process::{exit, fork, getpid, gettid, waitpid, yield_now};

/// Exit the current process with a status code.
///
/// This is equivalent to `std::process::exit()`.
///
/// # Arguments
/// * `code` - Exit status code (0 for success, non-zero for error)
///
/// # Example
/// ```ignore
/// use breenix_std::process;
/// process::exit(0); // Success
/// ```
#[inline]
pub fn exit_process(code: i32) -> ! {
    exit(code)
}
