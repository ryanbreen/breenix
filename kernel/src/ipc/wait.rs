//! Interruptible IPC waits. Callers retry their operation after `Woken`.
use crate::syscall::{errno, userptr::copy_from_user};
use crate::task::{thread::ThreadState, waitqueue::WaitQueueHead};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcDeadline {
    Never,
    Monotonic(u64),
    Realtime(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitEnd {
    Woken,
    TimedOut,
    Interrupted,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

fn nanoseconds(ts: Timespec, relative: bool) -> Result<u64, u64> {
    if !(0..1_000_000_000).contains(&ts.nsec) || (relative && ts.sec < 0) {
        return Err(errno::EINVAL as u64);
    }
    // An absolute date before the epoch has already expired. Large dates
    // saturate rather than wrapping into an early deadline.
    Ok(
        (i128::from(ts.sec) * 1_000_000_000 + i128::from(ts.nsec)).clamp(0, i128::from(u64::MAX))
            as u64,
    )
}

fn monotonic_ns() -> u64 {
    let (sec, nsec) = crate::time::get_monotonic_time_ns();
    sec.saturating_mul(1_000_000_000).saturating_add(nsec)
}

fn realtime_ns() -> u64 {
    let (sec, nsec) = crate::time::get_real_time_ns();
    (i128::from(sec) * 1_000_000_000 + i128::from(nsec)).clamp(0, i128::from(u64::MAX)) as u64
}

pub fn abs_realtime(ts: u64) -> Result<IpcDeadline, u64> {
    if ts == 0 {
        return Ok(IpcDeadline::Never);
    }
    let value = copy_from_user(ts as *const Timespec)?;
    nanoseconds(value, false).map(IpcDeadline::Realtime)
}

pub fn relative(ts: u64) -> Result<IpcDeadline, u64> {
    if ts == 0 {
        return Ok(IpcDeadline::Never);
    }
    let value = copy_from_user(ts as *const Timespec)?;
    nanoseconds(value, true).map(|ns| IpcDeadline::Monotonic(monotonic_ns().saturating_add(ns)))
}

impl IpcDeadline {
    fn reached(&self) -> bool {
        match *self {
            Self::Never => false,
            Self::Monotonic(at) => monotonic_ns() >= at,
            Self::Realtime(at) => realtime_ns() >= at,
        }
    }

    fn wake_ns(&self) -> Option<u64> {
        match *self {
            Self::Never => None,
            Self::Monotonic(at) => Some(at),
            Self::Realtime(at) => {
                // Recheck the wall clock after a bounded sleep: clock_settime
                // can move a realtime deadline while the scheduler uses monotonic time.
                let remaining = at.saturating_sub(realtime_ns()).min(10_000_000);
                Some(monotonic_ns().saturating_add(remaining))
            }
        }
    }
}

/// `cond` checks whether the operation still needs to wait under the queue
/// lock. Producers release their object lock before waking this queue.
/// Syscall entry already disables preemption; wait_prepared restores that
/// invariant after sleeping. No caller guard may survive entry here; the caller retries after a wake,
/// including a realtime recheck, and checks timeout validity only if blocked.
pub fn wait_on(q: &WaitQueueHead, d: &IpcDeadline, cond: impl FnOnce() -> bool) -> WaitEnd {
    let mut expired = false;
    let mut needed = false;
    let outcome = q.prepare_to_wait_checked(ThreadState::BlockedOnIO, d.wake_ns(), || {
        if !cond() {
            return false;
        }
        needed = true;
        expired = d.reached();
        !expired
    });
    let result = crate::syscall::blocking_io::wait_prepared(q, outcome);
    match result {
        Err(_) => WaitEnd::Interrupted,
        Ok(()) if needed && (expired || d.reached()) => WaitEnd::TimedOut,
        Ok(()) => WaitEnd::Woken,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timespec_validation_and_saturation() {
        assert_eq!(
            nanoseconds(
                Timespec {
                    sec: 1,
                    nsec: 999_999_999
                },
                true
            ),
            Ok(1_999_999_999)
        );
        for nsec in [-1, 1_000_000_000] {
            assert_eq!(
                nanoseconds(Timespec { sec: 0, nsec }, false),
                Err(errno::EINVAL as u64)
            );
        }
        assert_eq!(
            nanoseconds(Timespec { sec: -1, nsec: 0 }, true),
            Err(errno::EINVAL as u64)
        );
        assert_eq!(nanoseconds(Timespec { sec: -1, nsec: 0 }, false), Ok(0));
        assert_eq!(
            nanoseconds(
                Timespec {
                    sec: i64::MAX,
                    nsec: 0
                },
                false
            ),
            Ok(u64::MAX)
        );
    }

    #[test]
    fn null_timeout_means_no_deadline() {
        assert_eq!(abs_realtime(0), Ok(IpcDeadline::Never));
        assert_eq!(relative(0), Ok(IpcDeadline::Never));
    }
}
