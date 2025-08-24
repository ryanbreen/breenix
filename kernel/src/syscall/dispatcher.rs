//! System call dispatcher
//! 
//! Routes system calls to their appropriate handlers based on the syscall number.

use super::{SyscallNumber, SyscallResult};
use super::handlers;

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
    // Convert syscall number
    let syscall = match SyscallNumber::from_u64(syscall_num) {
        Some(s) => s,
        None => {
            log::warn!("Invalid syscall number: {}", syscall_num);
            return SyscallResult::Err(38); // ENOSYS
        }
    };
    
    // Dispatch to appropriate handler
    match syscall {
        SyscallNumber::Exit => handlers::sys_exit(arg1 as i32),
        SyscallNumber::Write => handlers::sys_write(arg1, arg2, arg3),
        SyscallNumber::Read => handlers::sys_read(arg1, arg2, arg3),
        SyscallNumber::Yield => handlers::sys_yield(),
        SyscallNumber::GetTime => handlers::sys_get_time(),
        SyscallNumber::Fork => handlers::sys_fork(),
        SyscallNumber::Exec => handlers::sys_exec(arg1, arg2),
        SyscallNumber::GetPid => handlers::sys_getpid(),
        SyscallNumber::GetTid => handlers::sys_gettid(),
        SyscallNumber::ClockGetTime => {
            let clock_id = arg1 as u32;
            let user_timespec_ptr = arg2 as *mut super::time::Timespec;
            super::time::sys_clock_gettime(clock_id, user_timespec_ptr)
        }
    }
}