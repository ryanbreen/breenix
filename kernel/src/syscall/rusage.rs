//! getrusage and times: the CPU time of the caller and of the children it has
//! waited for.
//!
//! The total is the CPU time the scheduler charges in ticks while a thread
//! runs, the same account /proc and the CPU-time clocks read, so they agree.
//! It is split between user and system time in the proportion the nanosecond
//! counters kept at kernel entry and exit give (`Thread::switch_timer_mode`):
//! a system call's time, from its entry to the end of its return path, and an
//! interrupt or fault taken from user mode, is system time; the rest is user
//! time. A child's time counts towards its parent's children's time when the
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

/// User and system nanoseconds.
type Split = (u64, u64);

/// `ticks` of CPU time, in nanoseconds, split between user and system time
/// in the proportion of the user and system nanoseconds counted; all user
/// time when the counters have recorded none. System time is a whole number
/// of microseconds, so the two timevals add up to the total exactly.
fn split(ticks: u64, (user, system): Split) -> Split {
    let total = ticks.saturating_mul(MS_PER_TICK).saturating_mul(1_000_000);
    let counted = user.saturating_add(system);
    if counted == 0 {
        return (total, 0);
    }
    let system = (u128::from(total) * u128::from(system) / u128::from(counted)) as u64 / 1000 * 1000;
    (total - system, system)
}

/// The CPU time of the calling thread, of its process and of the process's
/// waited-for children, with every running thread of the process charged up
/// to now first.
fn cpu_split() -> Result<(Split, Split, Split), u64> {
    let thread_ticks = crate::task::scheduler::charge_current_cpu();
    let process_ticks = crate::task::scheduler::process_cpu_ticks().ok_or(ESRCH as u64)?;
    let account = crate::arch_without_interrupts(|| -> Result<_, u64> {
        let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
        let guard = crate::process::manager();
        let (_, process) = guard
            .as_ref()
            .and_then(|manager| manager.find_process_by_thread(tid))
            .ok_or(ESRCH as u64)?;
        Ok(process.cpu.clone())
    })?;
    crate::task::scheduler::charge_account_cpu(&account);
    let thread = crate::task::thread::current_thread_cpu_split_ns().ok_or(ESRCH as u64)?;
    Ok((
        split(thread_ticks, thread),
        split(process_ticks, account.split_ns()),
        split(account.children_ticks(), account.children_split_ns()),
    ))
}

/// A struct timeval's seconds and microseconds.
fn timeval(ns: u64) -> [i64; 2] {
    [(ns / 1_000_000_000) as i64, ((ns % 1_000_000_000) / 1000) as i64]
}

/// getrusage: ru_utime and ru_stime hold the user and system time of `who`;
/// every other field of the struct rusage is zero.
pub fn sys_getrusage(who: u64, usage_ptr: u64) -> SyscallResult {
    let (thread, own, children) = match cpu_split() {
        Ok(split) => split,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let (user, system) = match who as i64 {
        RUSAGE_SELF => own,
        RUSAGE_CHILDREN => children,
        RUSAGE_THREAD => thread,
        _ => return SyscallResult::Err(EINVAL as u64),
    };
    // ru_utime, ru_stime, then fourteen longs.
    let mut usage = [0i64; 18];
    usage[..2].copy_from_slice(&timeval(user));
    usage[2..4].copy_from_slice(&timeval(system));
    match super::userptr::copy_to_user(usage_ptr as *mut [i64; 18], &usage) {
        Ok(()) => SyscallResult::Ok(0),
        Err(errno) => SyscallResult::Err(errno),
    }
}

/// times: fill struct tms (if `buf` is not null) with the caller's and its
/// waited-for children's user and system time, and return the elapsed time
/// since boot, all in `CLK_TCK` ticks.
pub fn sys_times(buf: u64) -> SyscallResult {
    let (_, (user, system), (c_user, c_system)) = match cpu_split() {
        Ok(split) => split,
        Err(errno) => return SyscallResult::Err(errno),
    };
    // User time in whole ticks, and system time as what is left of the
    // total in whole ticks: truncating each on its own could make the two
    // add up to almost two ticks less than the CPU time getrusage reports.
    let clock = |user: u64, system: u64| {
        let tick = 1_000_000_000 / CLK_TCK;
        let user_ticks = user / tick;
        [user_ticks as i64, (user.saturating_add(system) / tick - user_ticks) as i64]
    };
    if buf != 0 {
        let (own, children) = (clock(user, system), clock(c_user, c_system));
        let tms = [own[0], own[1], children[0], children[1]];
        if let Err(errno) = super::userptr::copy_to_user(buf as *mut [i64; 4], &tms) {
            return SyscallResult::Err(errno);
        }
    }
    SyscallResult::Ok((crate::time::get_monotonic_time() * CLK_TCK / 1000) as u64)
}
