//! System call dispatcher
//!
//! Routes system calls to their appropriate handlers based on the syscall number.

use super::handlers;
use super::{SyscallNumber, SyscallResult};

/// Dispatch a system call to the appropriate handler
#[allow(dead_code)]
pub fn dispatch_syscall(
    syscall_num: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    arg6: u64,
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
        SyscallNumber::Brk => super::memory::sys_brk(arg1),
        SyscallNumber::Mmap => super::mmap::sys_mmap(arg1, arg2, arg3 as u32, arg4 as u32, arg5 as i64, arg6),
        SyscallNumber::Munmap => super::mmap::sys_munmap(arg1, arg2),
        SyscallNumber::Mprotect => super::mmap::sys_mprotect(arg1, arg2, arg3 as u32),
        SyscallNumber::Kill => super::signal::sys_kill(arg1 as i64, arg2 as i32),
        SyscallNumber::Sigaction => super::signal::sys_sigaction(arg1 as i32, arg2, arg3, arg4),
        SyscallNumber::Sigprocmask => super::signal::sys_sigprocmask(arg1 as i32, arg2, arg3, arg4),
        SyscallNumber::Sigreturn => super::signal::sys_sigreturn(),
        SyscallNumber::Ioctl => super::ioctl::sys_ioctl(arg1, arg2, arg3),
        SyscallNumber::Socket => super::socket::sys_socket(arg1, arg2, arg3),
        SyscallNumber::Bind => super::socket::sys_bind(arg1, arg2, arg3),
        SyscallNumber::SendTo => super::socket::sys_sendto(arg1, arg2, arg3, arg4, arg5, arg6),
        SyscallNumber::RecvFrom => super::socket::sys_recvfrom(arg1, arg2, arg3, arg4, arg5, arg6),
        SyscallNumber::Poll => handlers::sys_poll(arg1, arg2, arg3 as i32),
        SyscallNumber::Select => handlers::sys_select(arg1 as i32, arg2, arg3, arg4, arg5),
        SyscallNumber::Pipe => super::pipe::sys_pipe(arg1),
        SyscallNumber::Pipe2 => super::pipe::sys_pipe2(arg1, arg2),
        SyscallNumber::Close => super::pipe::sys_close(arg1 as i32),
        SyscallNumber::Dup => handlers::sys_dup(arg1),
        SyscallNumber::Dup2 => handlers::sys_dup2(arg1, arg2),
        SyscallNumber::Fcntl => handlers::sys_fcntl(arg1, arg2, arg3),
        SyscallNumber::Pause => super::signal::sys_pause(),
        SyscallNumber::Wait4 => handlers::sys_waitpid(arg1 as i64, arg2, arg3 as u32),
        SyscallNumber::SetPgid => super::session::sys_setpgid(arg1 as i32, arg2 as i32),
        SyscallNumber::SetSid => super::session::sys_setsid(),
        SyscallNumber::GetPgid => super::session::sys_getpgid(arg1 as i32),
        SyscallNumber::GetSid => super::session::sys_getsid(arg1 as i32),
    }
}
