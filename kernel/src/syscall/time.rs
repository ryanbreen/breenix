// ─── File: kernel/src/syscall/time.rs ──────────────────────────────
use crate::syscall::SyscallResult;
use crate::time::{get_monotonic_time, get_real_time};

/// POSIX clock identifiers
pub const CLOCK_REALTIME: u32 = 0;
pub const CLOCK_MONOTONIC: u32 = 1;

/// Kernel‑internal representation of `struct timespec`
/// Matches the POSIX ABI layout exactly.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timespec {
    pub tv_sec: i64,  // seconds since Unix epoch
    pub tv_nsec: i64, // nanoseconds [0, 999 999 999]
}

/// Syscall #228 — clock_gettime(clock_id, *timespec)
///
/// Granularity: 1 ms until TSC‑deadline fast‑path is enabled.
pub fn sys_clock_gettime(clock_id: u32, user_ptr: *mut Timespec) -> SyscallResult {
    log::debug!("sys_clock_gettime: clock_id={}, user_ptr={:#x}", clock_id, user_ptr as u64);

    let ts = match clock_id {
        CLOCK_REALTIME => {
            let dt = get_real_time();
            Timespec {
                tv_sec: dt.to_unix_timestamp() as i64,
                tv_nsec: 0,
            }
        }
        CLOCK_MONOTONIC => {
            let ms = get_monotonic_time();
            Timespec {
                tv_sec: (ms / 1_000) as i64,
                tv_nsec: ((ms % 1_000) * 1_000_000) as i64,
            }
        }
        _ => {
            log::debug!("sys_clock_gettime: invalid clock_id={}", clock_id);
            return SyscallResult::Err(crate::syscall::ErrorCode::InvalidArgument as u64);
        }
    };

    log::debug!("sys_clock_gettime: returning tv_sec={}, tv_nsec={}", ts.tv_sec, ts.tv_nsec);

    // Copy timespec to userspace
    // During syscall handling, we're in kernel mode but using the process's
    // page table, so copy_to_user can directly access userspace memory.
    log::debug!("sys_clock_gettime: copy_to_user to {:#x}", user_ptr as u64);
    if let Err(e) = crate::syscall::handlers::copy_to_user(
        user_ptr as u64,
        &ts as *const _ as u64,
        core::mem::size_of::<Timespec>(),
    ) {
        log::error!("sys_clock_gettime: Failed to copy to user: {}", e);
        return SyscallResult::Err(crate::syscall::ErrorCode::Fault as u64);
    }
    log::debug!("sys_clock_gettime: copy_to_user succeeded");

    log::debug!("sys_clock_gettime: returning success (0)");
    SyscallResult::Ok(0)
}
