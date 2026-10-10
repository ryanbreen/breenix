//! Clock queries outside the precision syscall path.
use super::{time::Timespec, userptr, ErrorCode, SyscallResult};

/// CPU accounting uses scheduler ticks, including the interval currently running.
pub fn cpu_time(clock: u32) -> Result<Timespec, ErrorCode> {
    let ticks = if clock == 3 {
        crate::task::scheduler::charge_current_cpu()
    } else {
        // CLONE_THREAD rows have distinct owner PIDs but share this account.
        // Charge every running member, rather than only the leader's row.
        crate::task::scheduler::process_cpu_ticks().ok_or(ErrorCode::InvalidArgument)?
    };
    let ms = ticks.saturating_mul(crate::time::timer::MS_PER_TICK);
    Ok(Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    })
}

pub fn sys_clock_getres(clock: u32, ptr: u64) -> SyscallResult {
    let nanos = match clock {
        0 | 1 | 4 | 7 => {
            let hz = crate::time::tsc::frequency_hz();
            if hz == 0 {
                crate::time::timer::MS_PER_TICK * 1_000_000
            } else {
                1_000_000_000u64.div_ceil(hz).max(1)
            }
        }
        2 | 3 | 5 | 6 => crate::time::timer::MS_PER_TICK * 1_000_000,
        _ => return SyscallResult::Err(22),
    };
    if ptr != 0 {
        let ts = Timespec {
            tv_sec: (nanos / 1_000_000_000) as i64,
            tv_nsec: (nanos % 1_000_000_000) as i64,
        };
        if let Err(e) = userptr::copy_to_user(ptr as *mut Timespec, &ts) {
            return SyscallResult::Err(e);
        }
    }
    SyscallResult::Ok(0)
}

pub fn sys_gettimeofday(tv: u64, tz: u64) -> SyscallResult {
    let (secs, nanos) = crate::time::get_real_time_ns();
    if tv != 0 {
        if let Err(e) = userptr::copy_to_user(tv as *mut [i64; 2], &[secs, nanos / 1000]) {
            return SyscallResult::Err(e);
        }
    }
    if tz != 0 {
        if let Err(e) = userptr::copy_to_user(tz as *mut [i32; 2], &[0, 0]) {
            return SyscallResult::Err(e);
        }
    }
    SyscallResult::Ok(0)
}

pub fn sys_time(ptr: u64) -> SyscallResult {
    let secs = crate::time::get_real_time_ns().0;
    if ptr != 0 {
        if let Err(e) = userptr::copy_to_user(ptr as *mut i64, &secs) {
            return SyscallResult::Err(e);
        }
    }
    SyscallResult::Ok(secs as u64)
}
