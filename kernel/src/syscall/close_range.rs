//! Inclusive close_range(first, last, 0), shared by both architectures.
//! Designed from close_range(2), DESCRIPTION and ERRORS. Nonzero flags are
//! unsupported and refused. Endpoint cleanup uses the ordinary close path.
use alloc::vec::Vec;
use super::{errno, pipe, SyscallResult};

pub fn sys_close_range(first: u32, last: u32, flags: u32) -> SyscallResult {
    if first > last || flags != 0 {
        return SyscallResult::Err(errno::EINVAL as u64);
    }
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(errno::ESRCH as u64);
    };
    let victims: Vec<_> = {
        let guard = crate::process::manager();
        let Some(manager) = guard.as_ref() else {
            return SyscallResult::Err(errno::ESRCH as u64);
        };
        let Some((_, process)) = manager.find_process_by_thread(tid) else {
            return SyscallResult::Err(errno::ESRCH as u64);
        };
        (first..=last.min(crate::ipc::fd::MAX_FDS as u32 - 1))
            .filter(|&fd| process.fd_table.get(fd as i32).is_some()).collect()
    };
    // No process-manager guard crosses endpoint cleanup. Holes and individual
    // close errors are ignored, as close_range(2) specifies.
    for fd in victims { let _ = pipe::sys_close(fd as i32); }
    SyscallResult::Ok(0)
}
