// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║                         🚨 CRITICAL HOT PATH 🚨                               ║
// ║                                                                              ║
// ║  THIS FILE IS ON THE PROHIBITED MODIFICATIONS LIST.                          ║
// ║  sys_clock_gettime is called repeatedly in tight loops for precision timing. ║
// ║                                                                              ║
// ║  DO NOT ADD ANY LOGGING. See kernel/src/syscall/handler.rs for full rules.   ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

use crate::syscall::{ErrorCode, SyscallResult};
use crate::time::{get_monotonic_time_ns, get_real_time_ns};

/// POSIX clock identifiers
pub const CLOCK_REALTIME: u32 = 0;
pub const CLOCK_MONOTONIC: u32 = 1;

/// Kernel‑internal representation of `struct timespec`
/// Matches the POSIX ABI layout exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Timespec {
    pub tv_sec: i64,  // seconds since Unix epoch
    pub tv_nsec: i64, // nanoseconds [0, 999 999 999]
}

/// Internal clock_gettime implementation for kernel use.
///
/// Returns the current time for the specified clock without any
/// userspace memory operations. Use this from kernel code that
/// needs to read the system time directly.
///
/// Granularity: nanosecond precision via TSC (falls back to 1ms via PIT).
pub fn clock_gettime(clock_id: u32) -> Result<Timespec, ErrorCode> {
    match clock_id {
        CLOCK_REALTIME => {
            let (secs, nanos) = get_real_time_ns();
            Ok(Timespec {
                tv_sec: secs,
                tv_nsec: nanos,
            })
        }
        CLOCK_MONOTONIC => {
            let (secs, nanos) = get_monotonic_time_ns();
            Ok(Timespec {
                tv_sec: secs as i64,
                tv_nsec: nanos as i64,
            })
        }
        2 | 3 => super::clocks::cpu_time(clock_id),
        4 | 7 => clock_gettime(CLOCK_MONOTONIC),
        5 => {
            let (secs, nanos) = get_real_time_ns();
            let tick = crate::time::timer::MS_PER_TICK as i64 * 1_000_000;
            Ok(Timespec { tv_sec: secs, tv_nsec: nanos / tick * tick })
        }
        6 => {
            let ms = crate::time::get_monotonic_time();
            Ok(Timespec { tv_sec: (ms / 1000) as i64, tv_nsec: ((ms % 1000) * 1_000_000) as i64 })
        }
        _ => Err(ErrorCode::InvalidArgument),
    }
}

/// Syscall #228 — clock_gettime(clock_id, *timespec)
///
/// This is the userspace syscall entry point. It gets the time via
/// `clock_gettime()` and copies the result to userspace memory.
///
/// For kernel code that needs to read the time, use `clock_gettime()`
/// directly instead of this syscall wrapper.
///
/// NOTE: No logging in this hot path! Serial I/O takes thousands of cycles
/// and would cause the sub-millisecond precision test to fail.
pub fn sys_clock_gettime(clock_id: u32, user_ptr: *mut Timespec) -> SyscallResult {
    // Get the time using internal implementation
    let ts = match clock_gettime(clock_id) {
        Ok(ts) => ts,
        Err(e) => {
            return SyscallResult::Err(e as u64);
        }
    };

    // Copy result to userspace using architecture-independent userptr module
    if let Err(_e) = crate::syscall::userptr::copy_to_user(user_ptr, &ts) {
        return SyscallResult::Err(ErrorCode::Fault as u64);
    }

    SyscallResult::Ok(0)
}

/// Syscall #227 (x86_64) / #112 (ARM64) — clock_settime(clock_id, *timespec)
///
/// Sets the system clock. Only CLOCK_REALTIME is supported.
/// Adjusts the realtime offset without changing the monotonic clock.
///
/// The wall-clock offset is published atomically, with no hot-path logging.
pub fn sys_clock_settime(clock_id: u32, user_ptr: *const Timespec) -> SyscallResult {
    if clock_id != CLOCK_REALTIME {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    let ts: Timespec = match crate::syscall::userptr::copy_from_user(user_ptr) {
        Ok(ts) => ts,
        Err(_) => return SyscallResult::Err(ErrorCode::Fault as u64),
    };

    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    if !matches!(super::handlers::sys_geteuid(), SyscallResult::Ok(0)) {
        return SyscallResult::Err(1); // EPERM
    }
    crate::time::set_real_time_ns(ts.tv_sec, ts.tv_nsec);
    super::sleep::realtime_changed();

    SyscallResult::Ok(0)
}

/// Suspend until the requested duration expires or a caught signal interrupts.
pub fn sys_nanosleep(req_ptr: u64, rem_ptr: u64) -> SyscallResult {
    super::sleep::nanosleep(req_ptr, rem_ptr)
}
