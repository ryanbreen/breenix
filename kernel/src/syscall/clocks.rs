//! Clock queries outside the precision syscall path.
use super::{time::Timespec, userptr, ErrorCode, SyscallResult};

/// CLOCK_THREAD_CPUTIME_ID (3) and CLOCK_PROCESS_CPUTIME_ID (2): the user
/// and system nanoseconds kept at kernel entry and exit, the counters the
/// CPU-time POSIX timers expire on, so a timer and its clock agree. The
/// process clock charges every running thread of the process first, the
/// caller included.
pub fn cpu_time(clock: u32) -> Result<Timespec, ErrorCode> {
    let ns = if clock == 3 {
        let (user, system) = crate::task::thread::current_thread_cpu_split_ns().unwrap_or((0, 0));
        user.saturating_add(system)
    } else {
        // CLONE_THREAD rows have distinct owner PIDs but share this account.
        // Charge the account's running members, including non-leader rows.
        crate::task::scheduler::process_cpu_ns().ok_or(ErrorCode::InvalidArgument)?
    };
    Ok(Timespec {
        tv_sec: (ns / 1_000_000_000) as i64,
        tv_nsec: (ns % 1_000_000_000) as i64,
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
        // The CPU-time counters advance in microseconds.
        2 | 3 => 1_000,
        5 | 6 => crate::time::timer::MS_PER_TICK * 1_000_000,
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
