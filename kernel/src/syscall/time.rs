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

/// Syscall #35 — nanosleep(req, rem)
///
/// Suspends the calling thread for the time specified in `req`.
/// If interrupted by a signal, writes the remaining time to `rem` (if non-null)
/// and returns -EINTR.
///
/// Note: This is NOT a hot path (called once per sleep), so brief logging is acceptable.
pub fn sys_nanosleep(req_ptr: u64, _rem_ptr: u64) -> SyscallResult {
    // Read the requested sleep duration from userspace
    let req: Timespec = match crate::syscall::userptr::copy_from_user(req_ptr as *const Timespec) {
        Ok(ts) => ts,
        Err(_) => return SyscallResult::Err(ErrorCode::Fault as u64),
    };

    // Validate the timespec
    if req.tv_nsec < 0 || req.tv_nsec >= 1_000_000_000 || req.tv_sec < 0 {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Calculate absolute wake time
    let (cur_secs, cur_nanos) = get_monotonic_time_ns();
    let now_ns = cur_secs as u64 * 1_000_000_000 + cur_nanos as u64;
    let sleep_ns = req.tv_sec as u64 * 1_000_000_000 + req.tv_nsec as u64;
    let wake_time_ns = now_ns.saturating_add(sleep_ns);

    // Zero-length sleep returns immediately
    if sleep_ns == 0 {
        return SyscallResult::Ok(0);
    }

    // Busy-wait until the wake time.
    //
    // ARCHITECTURAL NOTE: We cannot use block_current_for_timer + yield_current here
    // because syscall handlers run with preempt_count > 0. yield_current() only sets
    // the need_resched flag, but timer interrupts check preempt_count and skip context
    // switches when it's > 0 (to avoid preempting kernel code). This means the thread
    // can never actually be switched out, and the yield loop would spin forever.
    //
    // A proper implementation would require voluntary context switching from within
    // syscall handlers (like Linux's schedule()), which Breenix doesn't support yet.
    // For now, busy-wait is correct behavior: the thread sleeps for the requested
    // duration. On a single-CPU system this blocks other threads during the sleep,
    // but for typical userspace sleeps (tens of ms) this is acceptable.
    //
    // TODO: Implement voluntary preemption from syscall handlers to allow true blocking.
    loop {
        let (cur_secs, cur_nanos) = get_monotonic_time_ns();
        let now = cur_secs as u64 * 1_000_000_000 + cur_nanos as u64;
        if now >= wake_time_ns {
            break;
        }
        core::hint::spin_loop();
    }

    SyscallResult::Ok(0)
}
