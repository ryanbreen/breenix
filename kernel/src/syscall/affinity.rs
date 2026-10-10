//! sched_setaffinity, sched_getaffinity and getcpu: which processors a thread
//! may run on, and which one it is on.
//!
//! An affinity is kept as the thread's CPU pin (`CpuPin::user_affinity`),
//! which every placement and migration site of the scheduler honours. A mask
//! of every online processor clears the pin. A mask of some of them pins the
//! thread to one: the processor it runs on when that is in the mask, else the
//! lowest in it. The mask itself is what sched_getaffinity reports.

use super::errno::{EFAULT, EINVAL, EPERM, ESRCH};
use super::userptr::{copy_from_user, copy_to_user};
use super::SyscallResult;
use crate::task::thread::CpuPin;

/// Bytes of a CPU mask: one long, for up to 64 processors.
const MASK_BYTES: u64 = 8;

/// The processors online, as a mask.
fn online_mask() -> u64 {
    let cpus = crate::task::scheduler::online_cpus().clamp(1, 64);
    if cpus == 64 { u64::MAX } else { (1u64 << cpus) - 1 }
}

/// The thread `pid` names (0 for the caller), checked as Linux checks it:
/// the caller may set another thread's affinity when it is root or has the
/// thread's user ID as its effective user ID.
fn target(pid: i64, setting: bool) -> Result<u64, u64> {
    let me = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    if pid < 0 {
        return Err(EINVAL as u64);
    }
    let tid = if pid == 0 { me } else { pid as u64 };
    let guard = crate::process::manager();
    let manager = guard.as_ref().ok_or(ESRCH as u64)?;
    let (_, target) = manager.find_process_by_thread(tid).ok_or(ESRCH as u64)?;
    if setting && tid != me {
        let (_, caller) = manager.find_process_by_thread(me).ok_or(ESRCH as u64)?;
        let euid = caller.cred.euid;
        if euid != 0 && euid != target.cred.uid && euid != target.cred.euid {
            return Err(EPERM as u64);
        }
    }
    Ok(tid)
}

/// Set thread `tid`'s pin in the process table, which fork and clone copy and
/// a later publication of the row carries, and in the scheduler, which
/// places it and which sched_getaffinity reads. Both are written under one
/// hold of the process manager, so that concurrent calls cannot leave the
/// two holding different pins: fork and clone read the row under it too.
/// False when the scheduler has no such thread.
fn apply_pin(tid: u64, pin: Option<CpuPin>) -> bool {
    let mut guard = crate::process::manager();
    if let Some((_, process)) = guard.as_mut().and_then(|m| m.find_process_by_thread_mut(tid)) {
        if let Some(thread) = process.main_thread.as_mut().filter(|t| t.id == tid) {
            thread.cpu_affinity = pin;
        }
    }
    let placed = crate::task::scheduler::set_user_affinity(tid, pin);
    drop(guard);
    placed
}

/// The CPUs thread `tid` may run on.
fn allowed(tid: u64) -> Option<u64> {
    crate::task::scheduler::with_scheduler(|s| s.get_thread(tid).map(|t| t.cpu_affinity))
        .flatten()
        .map(|pin| pin.map_or_else(online_mask, |pin| pin.allowed & online_mask()))
}

/// sched_setaffinity(pid, cpusetsize, mask).
pub fn sys_sched_setaffinity(pid: i64, len: u64, mask_ptr: u64) -> SyscallResult {
    match set_affinity(pid, len, mask_ptr) {
        Ok(()) => SyscallResult::Ok(0),
        Err(errno) => SyscallResult::Err(errno),
    }
}

fn set_affinity(pid: i64, len: u64, mask_ptr: u64) -> Result<(), u64> {
    // A mask shorter than a long is read as far as it goes, the rest zero; a
    // longer one is read to its first long, the processors there are.
    let mut bytes = [0u8; MASK_BYTES as usize];
    let take = len.min(MASK_BYTES) as usize;
    if take != 0 {
        let whole: [u8; MASK_BYTES as usize] = if take == MASK_BYTES as usize {
            copy_from_user(mask_ptr as *const [u8; MASK_BYTES as usize])?
        } else {
            let mut partial = [0u8; MASK_BYTES as usize];
            for (i, byte) in partial.iter_mut().enumerate().take(take) {
                *byte = copy_from_user((mask_ptr + i as u64) as *const u8)?;
            }
            partial
        };
        bytes[..take].copy_from_slice(&whole[..take]);
    }
    let online = online_mask();
    let allowed = u64::from_le_bytes(bytes) & online;
    if allowed == 0 {
        return Err(EINVAL as u64);
    }
    let tid = target(pid, true)?;
    let pin = if allowed == online {
        None
    } else {
        let here = crate::task::scheduler::current_cpu();
        let on_here = pid_is_caller(tid) && allowed & (1 << here) != 0;
        let cpu = if on_here { here } else { allowed.trailing_zeros() as usize };
        Some(CpuPin::user_affinity(cpu, allowed))
    };
    if !apply_pin(tid, pin) {
        return Err(ESRCH as u64);
    }
    Ok(())
}

fn pid_is_caller(tid: u64) -> bool {
    crate::task::scheduler::current_thread_id() == Some(tid)
}

/// sched_getaffinity(pid, cpusetsize, mask): the bytes of the mask written.
pub fn sys_sched_getaffinity(pid: i64, len: u64, mask_ptr: u64) -> SyscallResult {
    let cpus = crate::task::scheduler::online_cpus() as u64;
    if len * 8 < cpus || len % MASK_BYTES != 0 {
        return SyscallResult::Err(EINVAL as u64);
    }
    let tid = match target(pid, false) {
        Ok(tid) => tid,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let Some(mask) = allowed(tid) else {
        return SyscallResult::Err(ESRCH as u64);
    };
    match copy_to_user(mask_ptr as *mut u64, &mask) {
        Ok(()) => SyscallResult::Ok(MASK_BYTES),
        Err(errno) => SyscallResult::Err(errno),
    }
}

/// getcpu(cpu, node, tcache): the processor the caller runs on and its NUMA
/// node, which is 0; either pointer may be null, and tcache is unused.
pub fn sys_getcpu(cpu_ptr: u64, node_ptr: u64, _tcache: u64) -> SyscallResult {
    let cpu = crate::task::scheduler::current_cpu() as u32;
    for (ptr, value) in [(cpu_ptr, cpu), (node_ptr, 0)] {
        if ptr != 0 && copy_to_user(ptr as *mut u32, &value).is_err() {
            return SyscallResult::Err(EFAULT as u64);
        }
    }
    SyscallResult::Ok(0)
}
