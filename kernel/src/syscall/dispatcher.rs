//! System call dispatcher
//! 
//! Routes system calls to their appropriate handlers based on the syscall number.

use super::SyscallResult;
use super::handlers;
use super::syscall_consts::*;

/// Dispatch a system call to the appropriate handler
#[allow(dead_code)]
pub fn dispatch_syscall(
    syscall_num: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    _arg4: u64,
    _arg5: u64,
    _arg6: u64,
) -> SyscallResult {
    // Dispatch to appropriate handler using constants
    match syscall_num {
        SYS_READ => handlers::sys_read(arg1, arg2, arg3),
        SYS_WRITE => handlers::sys_write(arg1, arg2, arg3),
        SYS_GET_TIME => handlers::sys_get_time(),
        SYS_YIELD => handlers::sys_yield(),
        SYS_GETPID => handlers::sys_getpid(),
        SYS_FORK => handlers::sys_fork(),
        SYS_EXEC => handlers::sys_exec(arg1, arg2),
        SYS_EXIT => handlers::sys_exit(arg1 as i32),
        #[cfg(feature = "testing")]
        SYS_SHARE_TEST_PAGE => handlers::sys_share_test_page(arg1),
        #[cfg(feature = "testing")]
        SYS_GET_SHARED_TEST_PAGE => handlers::sys_get_shared_test_page(),
        _ => {
            log::warn!("Unknown syscall number: {}", syscall_num);
            SyscallResult::Err(38) // ENOSYS
        }
    }
}