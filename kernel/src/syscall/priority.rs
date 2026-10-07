//! getpriority and setpriority: nice values by process, process group or user.
//!
//! A nice value is stored per process, inherited across fork and kept across
//! exec. The scheduler does not weigh it yet: every runnable thread gets the
//! same share of the processor whatever its nice value.

use super::errno::{EACCES, EINVAL, EPERM, ESRCH};
use super::SyscallResult;
use crate::process::credentials::ProcessCredentials;
use crate::process::{Process, ProcessId};

const PRIO_PROCESS: u64 = 0;
const PRIO_PGRP: u64 = 1;
const PRIO_USER: u64 = 2;

/// The nice values a process may hold.
const NICE_MIN: i32 = -20;
const NICE_MAX: i32 = 19;

/// The processes a `which`/`who` pair names.
enum Selector {
    Process(ProcessId),
    Group(ProcessId),
    User(u32),
    Nothing,
}

impl Selector {
    /// `who` 0 names the caller, its process group, or its real user ID.
    fn new(which: u64, who: u64, caller_pid: ProcessId, caller: &Process) -> Result<Self, u64> {
        let id = who as u32;
        let pid = |own: ProcessId| match id as i32 {
            0 => Selector::Process(own),
            n if n < 0 => Selector::Nothing,
            n => Selector::Process(ProcessId::new(n as u64)),
        };
        match which {
            PRIO_PROCESS => Ok(pid(caller_pid)),
            PRIO_PGRP => Ok(match pid(caller.pgid) {
                Selector::Process(group) => Selector::Group(group),
                other => other,
            }),
            PRIO_USER => Ok(Selector::User(if id == 0 { caller.cred.uid } else { id })),
            _ => Err(EINVAL as u64),
        }
    }

    fn matches(&self, pid: ProcessId, process: &Process) -> bool {
        match *self {
            Selector::Process(target) => pid == target,
            Selector::Group(group) => process.pgid == group,
            Selector::User(uid) => process.cred.uid == uid,
            Selector::Nothing => false,
        }
    }
}

/// The caller's PID and credentials, then `f` with the manager.
fn with_caller<R>(
    f: impl FnOnce(&mut crate::process::ProcessManager, ProcessId, ProcessCredentials) -> Result<R, u64>,
) -> Result<R, u64> {
    crate::arch_without_interrupts(|| {
        let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
        let mut guard = crate::process::manager();
        let manager = guard.as_mut().ok_or(ESRCH as u64)?;
        let (pid, cred) = manager
            .find_process_by_thread(tid)
            .map(|(pid, process)| (pid, process.cred.clone()))
            .ok_or(ESRCH as u64)?;
        f(manager, pid, cred)
    })
}

/// getpriority: 20 minus the lowest nice value among the processes named, as
/// the system call reports it (a C library turns it back into a nice value).
pub fn sys_getpriority(which: u64, who: u64) -> SyscallResult {
    let result = with_caller(|manager, caller_pid, _| {
        let caller = manager.get_process(caller_pid).ok_or(ESRCH as u64)?;
        let selector = Selector::new(which, who, caller_pid, caller)?;
        manager
            .iter_processes()
            .filter(|(pid, process)| selector.matches(*pid, process))
            .map(|(_, process)| process.nice)
            .min()
            .map(|nice| (20 - nice as i64) as u64)
            .ok_or(ESRCH as u64)
    });
    match result {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno),
    }
}

/// setpriority: give every process named `nice`, clamped to -20..=19.
///
/// As Linux does: ESRCH when nothing matches; for each match, EPERM unless the
/// caller is privileged or its effective user ID is the target's real or
/// effective user ID, and EACCES for lowering a nice value unprivileged. The
/// call reports the last such refusal, or success when every match changed.
pub fn sys_setpriority(which: u64, who: u64, nice: u64) -> SyscallResult {
    let nice = (nice as i32).clamp(NICE_MIN, NICE_MAX) as i8;
    let result = with_caller(|manager, caller_pid, cred| {
        let caller = manager.get_process(caller_pid).ok_or(ESRCH as u64)?;
        let selector = Selector::new(which, who, caller_pid, caller)?;
        let mut outcome = Err(ESRCH as u64);
        for (pid, process) in manager.iter_processes_mut() {
            if !selector.matches(pid, process) {
                continue;
            }
            if !cred.may_renice(&process.cred) {
                outcome = Err(EPERM as u64);
            } else if nice < process.nice && !cred.privileged() {
                outcome = Err(EACCES as u64);
            } else {
                process.nice = nice;
                if outcome == Err(ESRCH as u64) {
                    outcome = Ok(0);
                }
            }
        }
        outcome
    });
    match result {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno),
    }
}
