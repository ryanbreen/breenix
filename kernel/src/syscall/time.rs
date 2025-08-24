// ─── File: kernel/src/syscall/time.rs ──────────────────────────────
use crate::syscall::SyscallResult;
use crate::time::{get_monotonic_time, get_real_time};

/// POSIX clock identifiers
pub const CLOCK_REALTIME:   u32 = 0;
pub const CLOCK_MONOTONIC:  u32 = 1;

/// Kernel‑internal representation of `struct timespec`
/// Matches the POSIX ABI layout exactly.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timespec {
    pub tv_sec:  i64, // seconds since Unix epoch
    pub tv_nsec: i64, // nanoseconds [0, 999 999 999]
}

/// Syscall #228 — clock_gettime(clock_id, *timespec)
///
/// Granularity: 1 ms until TSC‑deadline fast‑path is enabled.
pub fn sys_clock_gettime(clock_id: u32, user_ptr: *mut Timespec) -> SyscallResult {
    let ts = match clock_id {
        CLOCK_REALTIME => {
            let dt = get_real_time();
            Timespec {
                tv_sec:  dt.to_unix_timestamp() as i64,
                tv_nsec: 0,
            }
        }
        CLOCK_MONOTONIC => {
            let ms = get_monotonic_time();
            Timespec {
                tv_sec:  (ms / 1_000) as i64,
                tv_nsec: ((ms % 1_000) * 1_000_000) as i64,
            }
        }
        _ => return SyscallResult::Err(crate::syscall::ErrorCode::InvalidArgument as u64),
    };

    // Safe copy‑out to userspace
    // Check if we're in kernel mode (for testing) or user mode
    use x86_64::registers::segmentation::{Segment, CS};
    let cs = CS::get_reg();
    if cs.index() == 1 {  // Kernel code segment (GDT index 1)
        // Direct copy for kernel-mode testing
        unsafe {
            *user_ptr = ts;
        }
    } else {
        // Use copy_to_user for real userspace calls
        if let Err(e) = crate::syscall::handlers::copy_to_user(user_ptr as u64, &ts as *const _ as u64, core::mem::size_of::<Timespec>()) {
            log::error!("sys_clock_gettime: Failed to copy to user: {}", e);
            return SyscallResult::Err(crate::syscall::ErrorCode::Fault as u64);
        }
    }
    
    SyscallResult::Ok(0)
}