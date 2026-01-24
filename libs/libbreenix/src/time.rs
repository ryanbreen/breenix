//! Time-related syscall wrappers

use crate::syscall::{nr, raw};
use crate::types::{clock, Timespec};

/// Get the current time from a clock.
///
/// # Arguments
/// * `clock_id` - Which clock to query (CLOCK_REALTIME or CLOCK_MONOTONIC)
/// * `ts` - Timespec struct to fill with the result
///
/// # Returns
/// 0 on success, negative errno on error.
///
/// # Example
/// ```rust,ignore
/// use libbreenix::time::{clock_gettime, CLOCK_MONOTONIC};
/// use libbreenix::Timespec;
///
/// let mut ts = Timespec::new();
/// if clock_gettime(CLOCK_MONOTONIC, &mut ts) == 0 {
///     // ts now contains the monotonic time
/// }
/// ```
#[inline]
pub fn clock_gettime(clock_id: u32, ts: &mut Timespec) -> i64 {
    unsafe { raw::syscall2(nr::CLOCK_GETTIME, clock_id as u64, ts as *mut Timespec as u64) as i64 }
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
/// Timespec with current time, or Timespec::new() on error.
#[inline]
pub fn now_realtime() -> Timespec {
    let mut ts = Timespec::new();
    clock_gettime(clock::REALTIME, &mut ts);
    ts
}

/// Get current monotonic time (time since boot).
///
/// # Returns
/// Timespec with monotonic time, or Timespec::new() on error.
#[inline]
pub fn now_monotonic() -> Timespec {
    let mut ts = Timespec::new();
    clock_gettime(clock::MONOTONIC, &mut ts);
    ts
}

/// Sleep for the specified number of milliseconds.
///
/// This is a busy-wait implementation since we don't have nanosleep yet.
/// It uses clock_gettime(CLOCK_MONOTONIC) for timing.
///
/// # Arguments
/// * `ms` - Number of milliseconds to sleep
#[inline]
pub fn sleep_ms(ms: u64) {
    let start = now_monotonic();
    let target_ns = ms * 1_000_000;

    loop {
        let now = now_monotonic();
        let elapsed_sec = now.tv_sec - start.tv_sec;
        let elapsed_nsec = if now.tv_nsec >= start.tv_nsec {
            now.tv_nsec - start.tv_nsec
        } else {
            // Handle nanosecond underflow
            1_000_000_000 - (start.tv_nsec - now.tv_nsec)
        };

        let elapsed_ns = (elapsed_sec as u64) * 1_000_000_000 + (elapsed_nsec as u64);
        if elapsed_ns >= target_ns {
            break;
        }

        // Yield to other processes while waiting
        crate::process::yield_now();
    }
}

// Re-export clock constants for convenience
pub use crate::types::clock::{MONOTONIC as CLOCK_MONOTONIC, REALTIME as CLOCK_REALTIME};
