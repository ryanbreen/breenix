//! Time-related syscall wrappers
//!
//! This module provides both POSIX-named syscall wrappers (clock_gettime) and
//! Rust-idiomatic convenience functions (now_monotonic, now_realtime, sleep_ms).
//! Both layers coexist for flexibility.

use crate::error::Error;
use crate::syscall::{nr, raw};
use crate::types::{clock, Timespec};

/// Get the current time from a clock.
///
/// # Arguments
/// * `clock_id` - Which clock to query (CLOCK_REALTIME or CLOCK_MONOTONIC)
/// * `ts` - Timespec struct to fill with the result
///
/// # Returns
/// `Ok(())` on success, `Err(Error)` on error.
///
/// # Example
/// ```rust,ignore
/// use libbreenix::time::{clock_gettime, CLOCK_MONOTONIC};
/// use libbreenix::Timespec;
///
/// let mut ts = Timespec::new();
/// clock_gettime(CLOCK_MONOTONIC, &mut ts).unwrap();
/// // ts now contains the monotonic time
/// ```
#[inline]
pub fn clock_gettime(clock_id: u32, ts: &mut Timespec) -> Result<(), Error> {
    let ret = unsafe {
        raw::syscall2(nr::CLOCK_GETTIME, clock_id as u64, ts as *mut Timespec as u64)
    };
    Error::from_syscall(ret as i64).map(|_| ())
}

/// Get the monotonic time since boot (deprecated, use clock_gettime).
///
/// Returns time in milliseconds.
#[inline]
#[deprecated(note = "Use clock_gettime with CLOCK_MONOTONIC for better precision")]
pub fn get_time_ms() -> u64 {
    unsafe { raw::syscall0(nr::GET_TIME) }
}

/// Get current wall-clock (real) time.
///
/// # Returns
/// `Ok(Timespec)` with current time, or `Err(Error)` on failure.
#[inline]
pub fn now_realtime() -> Result<Timespec, Error> {
    let mut ts = Timespec::new();
    clock_gettime(clock::REALTIME, &mut ts)?;
    Ok(ts)
}

/// Get current monotonic time (time since boot).
///
/// # Returns
/// `Ok(Timespec)` with monotonic time, or `Err(Error)` on failure.
#[inline]
pub fn now_monotonic() -> Result<Timespec, Error> {
    let mut ts = Timespec::new();
    clock_gettime(clock::MONOTONIC, &mut ts)?;
    Ok(ts)
}

/// Sleep for the specified number of milliseconds.
///
/// This is a busy-wait implementation since we don't have nanosleep yet.
/// It uses clock_gettime(CLOCK_MONOTONIC) for timing.
///
/// # Arguments
/// * `ms` - Number of milliseconds to sleep
///
/// # Returns
/// `Ok(())` on success, `Err(Error)` if clock_gettime fails.
#[inline]
pub fn sleep_ms(ms: u64) -> Result<(), Error> {
    let start = now_monotonic()?;
    let target_ns = ms * 1_000_000;

    loop {
        let now = now_monotonic()?;

        // Calculate elapsed time in nanoseconds using signed arithmetic.
        // This handles all edge cases including timer jitter where nanoseconds
        // might appear to briefly go backwards within the same second.
        let total_start_ns = (start.tv_sec as i128) * 1_000_000_000 + (start.tv_nsec as i128);
        let total_now_ns = (now.tv_sec as i128) * 1_000_000_000 + (now.tv_nsec as i128);
        let elapsed_ns = total_now_ns - total_start_ns;

        // If elapsed time is negative (shouldn't happen with monotonic clock,
        // but could due to jitter or bugs), treat as 0 and keep waiting
        if elapsed_ns < 0 {
            // yield_now can fail, but we ignore the error during sleep
            let _ = crate::process::yield_now();
            continue;
        }

        if elapsed_ns as u64 >= target_ns {
            break;
        }

        // Yield to other processes while waiting
        let _ = crate::process::yield_now();
    }
    Ok(())
}

// Re-export clock constants for convenience
pub use crate::types::clock::{MONOTONIC as CLOCK_MONOTONIC, REALTIME as CLOCK_REALTIME};
