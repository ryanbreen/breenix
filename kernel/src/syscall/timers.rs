//! POSIX per-process timers: timer_create, timer_settime, timer_gettime,
//! timer_getoverrun and timer_delete. The timers themselves, and how they
//! expire and signal, are in `crate::signal::timers`.

use super::errno::{EFAULT, EINVAL, EOPNOTSUPP, ESRCH};
use super::userptr::{copy_from_user, copy_to_user};
use super::SyscallResult;
use crate::signal::timers::{clock, Notify, Now, SIGEV_NONE, SIGEV_SIGNAL, SIGEV_THREAD, SIGEV_THREAD_ID};
use crate::signal::IntervalTimers;
use crate::task::thread::CpuAccount;
use alloc::sync::Arc;

const NS_PER_SEC: u64 = 1_000_000_000;

/// struct sigevent as the Linux ABI lays it out: sigev_value, sigev_signo,
/// sigev_notify, then a union padding it to 64 bytes whose first int is the
/// thread ID SIGEV_THREAD_ID names.
#[repr(C)]
#[derive(Clone, Copy)]
struct SigEvent {
    value: u64,
    signo: i32,
    notify: i32,
    thread: i32,
    pad: [i32; 11],
}

/// struct itimerspec: the interval's seconds and nanoseconds, then the value's.
type Itimerspec = [i64; 4];

/// What a timer syscall needs of the calling process, read under the process
/// manager and used after it is released.
struct Caller {
    timers: Arc<IntervalTimers>,
    cpu: Arc<CpuAccount>,
}

fn caller() -> Result<Caller, u64> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    let guard = crate::process::manager();
    let (_, process) = guard
        .as_ref()
        .and_then(|manager| manager.find_process_by_thread(tid))
        .ok_or(ESRCH as u64)?;
    Ok(Caller { timers: process.itimers.clone(), cpu: process.cpu.clone() })
}

/// The clocks the caller's timers count, with the process's running threads
/// charged up to now.
fn now(cpu: &Arc<CpuAccount>) -> Now {
    crate::task::scheduler::charge_account_cpu(cpu);
    let (user, system) = cpu.split_ns();
    Now::read(user.saturating_add(system))
}

fn result(r: Result<u64, u64>) -> SyscallResult {
    match r {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno),
    }
}

/// timer_create(clockid, sevp, timerid): a timer on `clock_id`, notifying as
/// `sevp` says, or with SIGALRM and the timer's ID as its value when `sevp`
/// is null; its ID is stored at `timerid`.
pub fn sys_timer_create(clock_id: i32, sevp: u64, timerid: u64) -> SyscallResult {
    result(timer_create(clock_id, sevp, timerid))
}

fn timer_create(clock_id: i32, sevp: u64, timerid: u64) -> Result<u64, u64> {
    match clock_id {
        clock::REALTIME | clock::MONOTONIC | clock::PROCESS_CPUTIME | clock::THREAD_CPUTIME | clock::BOOTTIME => {}
        // CLOCK_MONOTONIC_RAW and the coarse clocks can be read but not
        // armed, as on Linux.
        4..=6 => return Err(EOPNOTSUPP as u64),
        _ => return Err(EINVAL as u64),
    }
    let event = if sevp == 0 {
        None
    } else {
        Some(copy_from_user(sevp as *const SigEvent)?)
    };
    let (notify, signo, value) = match event {
        None => (Notify::Process, crate::signal::constants::SIGALRM, None),
        Some(event) => {
            let notify = match event.notify {
                SIGEV_NONE => Notify::Nothing,
                SIGEV_SIGNAL | SIGEV_THREAD => Notify::Process,
                SIGEV_THREAD_ID => Notify::Thread(event.thread as u32 as u64),
                _ => return Err(EINVAL as u64),
            };
            if notify != Notify::Nothing && !(1..=crate::signal::constants::NSIG as i32).contains(&event.signo) {
                return Err(EINVAL as u64);
            }
            (notify, event.signo as u32, Some(event.value))
        }
    };

    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    let (timers, thread_clock, limit) = {
        let guard = crate::process::manager();
        let manager = guard.as_ref().ok_or(ESRCH as u64)?;
        let (pid, process) = manager.find_process_by_thread(tid).ok_or(ESRCH as u64)?;
        // SIGEV_THREAD_ID must name a thread of the caller's process.
        if let Notify::Thread(target) = notify {
            let group = manager.thread_group_of(pid);
            let same = manager
                .find_process_by_thread(target)
                .is_some_and(|(other, _)| manager.thread_group_of(other) == group);
            if !same {
                return Err(EINVAL as u64);
            }
        }
        let thread_clock = (clock_id == clock::THREAD_CPUTIME).then(|| process.signals.thread.clone());
        let limit = process.limits.get(crate::process::limits::SIGPENDING).soft;
        (process.itimers.clone(), thread_clock, usize::try_from(limit).unwrap_or(usize::MAX))
    };
    let id = timers.posix.create(clock_id, notify, signo, value, thread_clock, limit)?;
    if copy_to_user(timerid as *mut i32, &id).is_err() {
        let _ = timers.posix.delete(id);
        return Err(EFAULT as u64);
    }
    Ok(0)
}

/// A timespec's nanoseconds, or EINVAL for one out of range.
fn nanos(sec: i64, nsec: i64) -> Result<u64, u64> {
    if sec < 0 || !(0..NS_PER_SEC as i64).contains(&nsec) {
        return Err(EINVAL as u64);
    }
    Ok((sec as u64).saturating_mul(NS_PER_SEC).saturating_add(nsec as u64))
}

fn itimerspec(interval: u64, value: u64) -> Itimerspec {
    [
        (interval / NS_PER_SEC) as i64,
        (interval % NS_PER_SEC) as i64,
        (value / NS_PER_SEC) as i64,
        (value % NS_PER_SEC) as i64,
    ]
}

/// timer_settime(timerid, flags, new_value, old_value).
pub fn sys_timer_settime(id: i32, flags: u64, new_value: u64, old_value: u64) -> SyscallResult {
    result(timer_settime(id, flags, new_value, old_value))
}

fn timer_settime(id: i32, flags: u64, new_value: u64, old_value: u64) -> Result<u64, u64> {
    if new_value == 0 {
        return Err(EINVAL as u64);
    }
    let new: Itimerspec = copy_from_user(new_value as *const Itimerspec)?;
    let interval = nanos(new[0], new[1])?;
    let value = nanos(new[2], new[3])?;
    let caller = caller()?;
    let (old_value_ns, old_interval) =
        caller.timers.posix.settime(id, flags, interval, value, &now(&caller.cpu))?;
    if value != 0 {
        crate::task::scheduler::with_scheduler(|s| s.register_signal_timers(&caller.timers, &caller.cpu));
    }
    if old_value != 0 {
        copy_to_user(old_value as *mut Itimerspec, &itimerspec(old_interval, old_value_ns))?;
    }
    Ok(0)
}

/// timer_gettime(timerid, curr_value).
pub fn sys_timer_gettime(id: i32, curr_value: u64) -> SyscallResult {
    result(timer_gettime(id, curr_value))
}

fn timer_gettime(id: i32, curr_value: u64) -> Result<u64, u64> {
    let caller = caller()?;
    let (value, interval) = caller.timers.posix.gettime(id, &now(&caller.cpu))?;
    copy_to_user(curr_value as *mut Itimerspec, &itimerspec(interval, value))?;
    Ok(0)
}

/// timer_getoverrun(timerid): the overrun count of the timer's last
/// delivered signal.
pub fn sys_timer_getoverrun(id: i32) -> SyscallResult {
    result(caller().and_then(|caller| caller.timers.posix.getoverrun(id)).map(|n| n as u64))
}

/// timer_delete(timerid).
pub fn sys_timer_delete(id: i32) -> SyscallResult {
    result(caller().and_then(|caller| caller.timers.posix.delete(id)).map(|()| 0))
}
