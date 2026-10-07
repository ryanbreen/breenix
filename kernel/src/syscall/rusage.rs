//! getrusage and times: the CPU time of the caller and of the children it has
//! waited for.
//!
//! CPU time is counted in timer ticks (`MS_PER_TICK` ms each) while a thread
//! runs, kernel work on its behalf included; it is not split between user and
//! system mode, so all of it is reported as user time and the system times are
//! zero. A child's time counts towards its parent's children's time when the
//! parent reaps it (`ProcessManager::reap_row`), not before.

use super::errno::{EINVAL, ESRCH};
use super::SyscallResult;
use crate::time::timer::MS_PER_TICK;

const RUSAGE_SELF: i64 = 0;
const RUSAGE_CHILDREN: i64 = -1;
const RUSAGE_THREAD: i64 = 1;

/// Clock ticks per second in `struct tms` and times()'s return: Linux's
/// USER_HZ, which is what sysconf(_SC_CLK_TCK) reports.
const CLK_TCK: u64 = 100;

/// CPU ticks of the calling thread, its process, and the process's waited-for
/// children, with the caller's current interval charged first.
fn cpu_ticks() -> Result<(u64, u64, u64), u64> {
    let thread = crate::task::scheduler::charge_current_cpu();
    crate::arch_without_interrupts(|| {
        let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
        let guard = crate::process::manager();
        let (_, process) = guard
            .as_ref()
            .and_then(|manager| manager.find_process_by_thread(tid))
            .ok_or(ESRCH as u64)?;
        Ok((thread, process.cpu.ticks(), process.cpu.children_ticks()))
    })
}

fn ms(ticks: u64) -> u64 {
    ticks.saturating_mul(MS_PER_TICK)
}

/// getrusage: ru_utime holds the CPU time of `who`; every other field of the
/// struct rusage is zero.
pub fn sys_getrusage(who: u64, usage_ptr: u64) -> SyscallResult {
    let (thread, own, children) = match cpu_ticks() {
        Ok(ticks) => ticks,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let ticks = match who as i64 {
        RUSAGE_SELF => own,
        RUSAGE_CHILDREN => children,
        RUSAGE_THREAD => thread,
        _ => return SyscallResult::Err(EINVAL as u64),
    };
    // ru_utime, ru_stime, then fourteen longs.
    let mut usage = [0i64; 18];
    let ms = ms(ticks);
    usage[0] = (ms / 1000) as i64;
    usage[1] = ((ms % 1000) * 1000) as i64;
    match super::userptr::copy_to_user(usage_ptr as *mut [i64; 18], &usage) {
        Ok(()) => SyscallResult::Ok(0),
        Err(errno) => SyscallResult::Err(errno),
    }
}

/// times: fill struct tms (if `buf` is not null) with the caller's and its
/// waited-for children's CPU time, and return the elapsed time since boot, all
/// in `CLK_TCK` ticks.
pub fn sys_times(buf: u64) -> SyscallResult {
    let (_, own, children) = match cpu_ticks() {
        Ok(ticks) => ticks,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let clock = |ms: u64| (ms * CLK_TCK / 1000) as i64;
    if buf != 0 {
        let tms = [clock(ms(own)), 0, clock(ms(children)), 0];
        if let Err(errno) = super::userptr::copy_to_user(buf as *mut [i64; 4], &tms) {
            return SyscallResult::Err(errno);
        }
    }
    SyscallResult::Ok(clock(crate::time::get_monotonic_time()) as u64)
}
