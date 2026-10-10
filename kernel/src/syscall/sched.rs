//! sched_setscheduler, sched_getscheduler, sched_setparam, sched_getparam,
//! sched_get_priority_max, sched_get_priority_min and sched_rr_get_interval
//! (#1320).
//!
//! A policy belongs to a thread: `pid` names a thread ID, 0 the caller, as on
//! Linux. The scheduler honours it (`SchedPolicy`). A caller may change
//! another thread's policy when it is root or its effective user ID is that
//! thread's real or effective user ID; an unprivileged caller may take
//! SCHED_FIFO or SCHED_RR only up to its RLIMIT_RTPRIO, and may always lower
//! a priority or return to a normal policy. SCHED_RESET_ON_FORK stays with the
//! policy: a thread or process its holder creates starts with SCHED_OTHER in
//! place of a real-time policy, and an unprivileged caller cannot clear it.

use super::errno::{EFAULT, EINVAL, EPERM, ESRCH};
use super::userptr::{copy_from_user, copy_to_user};
use super::SyscallResult;
use crate::task::thread::SchedPolicy;

/// SCHED_RESET_ON_FORK, which sched_setscheduler accepts ORed into a policy.
const SCHED_RESET_ON_FORK: u64 = 0x4000_0000;
/// The real-time priorities.
const RT_PRIORITY_MIN: i32 = 1;
const RT_PRIORITY_MAX: i32 = 99;
/// The quantum a SCHED_RR thread runs for: the scheduler's, ten timer ticks
/// (1000 Hz on ARM64, 200 Hz on x86-64).
#[cfg(target_arch = "aarch64")]
const RR_INTERVAL_NS: i64 = 10_000_000;
#[cfg(target_arch = "x86_64")]
const RR_INTERVAL_NS: i64 = 50_000_000;

fn result(r: Result<u64, u64>) -> SyscallResult {
    match r {
        Ok(v) => SyscallResult::Ok(v),
        Err(e) => SyscallResult::Err(e),
    }
}

/// A policy number sched_setscheduler accepts.
fn valid_policy(policy: u64) -> Option<u8> {
    match policy as u32 {
        p @ (0 | 1 | 2 | 3 | 5) => Some(p as u8),
        _ => None,
    }
}

/// The thread `pid` names (0 for the caller), and whether the caller is
/// allowed to change its policy, with the caller's RLIMIT_RTPRIO soft limit
/// (None for a privileged caller).
fn target(pid: u64) -> Result<(u64, bool, Option<u64>), u64> {
    let me = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    if (pid as i64) < 0 {
        return Err(EINVAL as u64);
    }
    let tid = if pid == 0 { me } else { pid };
    let guard = crate::process::manager();
    let manager = guard.as_ref().ok_or(ESRCH as u64)?;
    let (_, target) = manager.find_process_by_thread(tid).ok_or(ESRCH as u64)?;
    if target.is_terminated() {
        return Err(ESRCH as u64);
    }
    let (_, caller) = manager.find_process_by_thread(me).ok_or(ESRCH as u64)?;
    let euid = caller.cred.euid;
    let privileged = euid == 0;
    let may = privileged || euid == target.cred.uid || euid == target.cred.euid;
    let rtprio = (!privileged).then(|| caller.limits.get(crate::process::limits::RTPRIO).soft);
    Ok((tid, may, rtprio))
}

/// Serializes every change of a thread's policy. The process row's copy of
/// the thread, which fork and clone inherit, and the scheduler's, which it
/// runs by, change together, and each change starts from the policy the one
/// before it left: two setters cannot leave contradictory copies, and
/// sched_setparam never acts on a stale policy.
static POLICY_CHANGE: spin::Mutex<()> = spin::Mutex::new(());

/// Set thread `pid`'s policy (`None`: keep it) and priority, checking both and
/// the caller's permission as Linux does. `reset_on_fork` is
/// SCHED_RESET_ON_FORK as sched_setscheduler gives it; sched_setparam keeps
/// the thread's.
fn set(pid: u64, policy: Option<u8>, reset_on_fork: Option<bool>, param: u64) -> Result<u64, u64> {
    if param == 0 {
        return Err(EINVAL as u64);
    }
    let priority: i32 = copy_from_user(param as *const i32).map_err(|_| EFAULT as u64)?;
    let (tid, may, rtprio) = target(pid)?;
    let _serial = POLICY_CHANGE.lock();
    let old = crate::task::scheduler::sched_policy(tid).ok_or(ESRCH as u64)?;
    let policy = policy.unwrap_or(old.policy);
    let reset_on_fork = reset_on_fork.unwrap_or(old.reset_on_fork);
    let realtime = matches!(policy, SchedPolicy::FIFO | SchedPolicy::RR);
    let valid = if realtime {
        (RT_PRIORITY_MIN..=RT_PRIORITY_MAX).contains(&priority)
    } else {
        priority == 0
    };
    if !valid {
        return Err(EINVAL as u64);
    }
    if !may {
        return Err(EPERM as u64);
    }
    if let Some(limit) = rtprio {
        // Unprivileged: a real-time priority up to RLIMIT_RTPRIO, or no
        // higher than the thread already has.
        let within = (priority as u64) <= limit
            || (old.is_realtime() && priority <= i32::from(old.priority));
        // Nor may it clear SCHED_RESET_ON_FORK.
        if (realtime && !within) || (old.reset_on_fork && !reset_on_fork) {
            return Err(EPERM as u64);
        }
    }
    let sched = SchedPolicy {
        policy,
        priority: priority as u8,
        reset_on_fork,
        ..SchedPolicy::DEFAULT
    };
    // The process row's copy of the thread is what fork and clone inherit.
    {
        let mut guard = crate::process::manager();
        if let Some((_, process)) = guard.as_mut().and_then(|m| m.find_process_by_thread_mut(tid)) {
            if let Some(thread) = process.main_thread.as_mut().filter(|t| t.id == tid) {
                thread.sched = sched;
            }
        }
    }
    if !crate::task::scheduler::set_sched_policy(tid, sched) {
        return Err(ESRCH as u64);
    }
    Ok(0)
}

/// sched_setscheduler(pid, policy, param). SCHED_RESET_ON_FORK may be ORed
/// into the policy.
pub fn sys_sched_setscheduler(pid: u64, policy: u64, param: u64) -> SyscallResult {
    let Some(policy_number) = valid_policy(policy & !SCHED_RESET_ON_FORK) else {
        return SyscallResult::Err(EINVAL as u64);
    };
    let reset_on_fork = policy & SCHED_RESET_ON_FORK != 0;
    result(set(pid, Some(policy_number), Some(reset_on_fork), param))
}

/// sched_setparam(pid, param): the priority, under the thread's policy.
pub fn sys_sched_setparam(pid: u64, param: u64) -> SyscallResult {
    result(set(pid, None, None, param))
}

/// sched_getscheduler(pid): the policy, with SCHED_RESET_ON_FORK ORed in
/// when the thread has it.
pub fn sys_sched_getscheduler(pid: u64) -> SyscallResult {
    result(target(pid).and_then(|(tid, _, _)| {
        crate::task::scheduler::sched_policy(tid)
            .map(|sched| {
                let reset = if sched.reset_on_fork { SCHED_RESET_ON_FORK } else { 0 };
                u64::from(sched.policy) | reset
            })
            .ok_or(ESRCH as u64)
    }))
}

/// sched_getparam(pid, param).
pub fn sys_sched_getparam(pid: u64, param: u64) -> SyscallResult {
    if param == 0 {
        return SyscallResult::Err(EINVAL as u64);
    }
    result(target(pid).and_then(|(tid, _, _)| {
        let sched = crate::task::scheduler::sched_policy(tid).ok_or(ESRCH as u64)?;
        let priority = i32::from(sched.priority);
        copy_to_user(param as *mut i32, &priority).map_err(|_| EFAULT as u64)?;
        Ok(0)
    }))
}

/// sched_get_priority_max(policy).
pub fn sys_sched_get_priority_max(policy: u64) -> SyscallResult {
    match valid_policy(policy) {
        Some(SchedPolicy::FIFO | SchedPolicy::RR) => SyscallResult::Ok(RT_PRIORITY_MAX as u64),
        Some(_) => SyscallResult::Ok(0),
        None => SyscallResult::Err(EINVAL as u64),
    }
}

/// sched_get_priority_min(policy).
pub fn sys_sched_get_priority_min(policy: u64) -> SyscallResult {
    match valid_policy(policy) {
        Some(SchedPolicy::FIFO | SchedPolicy::RR) => SyscallResult::Ok(RT_PRIORITY_MIN as u64),
        Some(_) => SyscallResult::Ok(0),
        None => SyscallResult::Err(EINVAL as u64),
    }
}

/// sched_rr_get_interval(pid, interval): the SCHED_RR quantum, and 0 for a
/// SCHED_FIFO thread, which has none.
pub fn sys_sched_rr_get_interval(pid: u64, interval: u64) -> SyscallResult {
    result(target(pid).and_then(|(tid, _, _)| {
        let sched = crate::task::scheduler::sched_policy(tid).ok_or(ESRCH as u64)?;
        let ns = if sched.policy == SchedPolicy::FIFO { 0 } else { RR_INTERVAL_NS };
        let ts = super::time::Timespec {
            tv_sec: ns / 1_000_000_000,
            tv_nsec: ns % 1_000_000_000,
        };
        copy_to_user(interval as *mut super::time::Timespec, &ts).map_err(|_| EFAULT as u64)?;
        Ok(0)
    }))
}
