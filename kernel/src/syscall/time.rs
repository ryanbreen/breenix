// ─── File: kernel/src/syscall/time.rs ──────────────────────────────
use crate::syscall::{ErrorCode, SyscallResult};
use crate::time::{get_monotonic_time, get_real_time};

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
/// Granularity: 1 ms until TSC‑deadline fast‑path is enabled.
pub fn clock_gettime(clock_id: u32) -> Result<Timespec, ErrorCode> {
    match clock_id {
        CLOCK_REALTIME => {
            let dt = get_real_time();
            Ok(Timespec {
                tv_sec: dt.to_unix_timestamp() as i64,
                tv_nsec: 0,
            })
        }
        CLOCK_MONOTONIC => {
            let ms = get_monotonic_time();
            Ok(Timespec {
                tv_sec: (ms / 1_000) as i64,
                tv_nsec: ((ms % 1_000) * 1_000_000) as i64,
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
pub fn sys_clock_gettime(clock_id: u32, user_ptr: *mut Timespec) -> SyscallResult {
    log::debug!("sys_clock_gettime: clock_id={}, user_ptr={:#x}", clock_id, user_ptr as u64);

    // Get the time using internal implementation
    let ts = match clock_gettime(clock_id) {
        Ok(ts) => ts,
        Err(e) => {
            log::debug!("sys_clock_gettime: clock_gettime failed with error {:?}", e);
            return SyscallResult::Err(e as u64);
        }
    };

    log::debug!("sys_clock_gettime: got time tv_sec={}, tv_nsec={}", ts.tv_sec, ts.tv_nsec);

    // Copy result to userspace
    log::debug!("sys_clock_gettime: calling copy_to_user to {:#x}", user_ptr as u64);
    if let Err(e) = crate::syscall::handlers::copy_to_user(
        user_ptr as u64,
        &ts as *const _ as u64,
        core::mem::size_of::<Timespec>(),
    ) {
        log::error!("sys_clock_gettime: Failed to copy to user: {}", e);
        return SyscallResult::Err(ErrorCode::Fault as u64);
    }

    log::debug!("sys_clock_gettime: copy_to_user succeeded, returning 0");
    SyscallResult::Ok(0)
}
