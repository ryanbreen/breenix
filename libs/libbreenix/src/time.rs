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
/// Uses the nanosleep syscall which blocks the thread in the kernel scheduler,
/// allowing other threads to run. The kernel wakes us via timer expiry.
///
/// # Arguments
/// * `ms` - Number of milliseconds to sleep
///
/// # Returns
/// `Ok(())` on success, `Err(Error)` on error.
#[inline]
pub fn sleep_ms(ms: u64) -> Result<(), Error> {
    let req = Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    };
    nanosleep(&req)
}

/// Sleep for the specified duration using the nanosleep syscall.
///
/// Unlike `sleep_ms` which busy-waits, this suspends the process until
/// the kernel timer expires.
///
/// # Arguments
/// * `req` - Requested sleep duration
///
/// # Returns
/// `Ok(())` on success, `Err(Error)` on error (typically EINTR if interrupted by a signal).
#[inline]
pub fn nanosleep(req: &Timespec) -> Result<(), Error> {
    let ret = unsafe {
        raw::syscall2(nr::NANOSLEEP, req as *const Timespec as u64, 0)
    };
    Error::from_syscall(ret as i64).map(|_| ())
}

// Re-export clock constants for convenience
pub use crate::types::clock::{MONOTONIC as CLOCK_MONOTONIC, REALTIME as CLOCK_REALTIME};
