//! Shared process-control extensions in the C library's reserved BR namespace.

use super::errno::{ENOSYS, ESRCH};
use super::{userptr::copy_to_user, SyscallResult};

const PR_SET_CHILD_SUBREAPER: u64 = 0x42520007;
const PR_GET_CHILD_SUBREAPER: u64 = 0x42520008;

pub fn sys_prctl(option: u64, arg: u64) -> SyscallResult {
    if option != PR_SET_CHILD_SUBREAPER && option != PR_GET_CHILD_SUBREAPER {
        return SyscallResult::Err(ENOSYS as u64);
    }
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(ESRCH as u64);
    };
    // Drop PM before copying to userspace: a copy may fault and resolve CoW.
    let state = {
        let guard = crate::process::manager();
        guard.as_ref().and_then(|manager| manager.find_process_by_thread(tid))
            .map(|(_, row)| row.reaper.share_thread())
    };
    let Some(state) = state else { return SyscallResult::Err(ESRCH as u64) };
    if option == PR_SET_CHILD_SUBREAPER {
        state.set_subreaper(arg != 0);
        SyscallResult::Ok(0)
    } else {
        let value = i32::from(state.is_subreaper());
        match copy_to_user(arg as *mut i32, &value) {
            Ok(()) => SyscallResult::Ok(0),
            Err(errno) => SyscallResult::Err(errno),
        }
    }
}
