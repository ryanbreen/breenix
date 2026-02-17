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
        SyscallNumber::Fork => handlers::sys_fork(),
        SyscallNumber::Exec => handlers::sys_exec(arg1, arg2),
        SyscallNumber::GetPid => handlers::sys_getpid(),
        SyscallNumber::Getppid => handlers::sys_getppid(),
        SyscallNumber::GetTid => handlers::sys_gettid(),
        SyscallNumber::SetTidAddress => handlers::sys_set_tid_address(arg1),
        SyscallNumber::ExitGroup => handlers::sys_exit(arg1 as i32),
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
        SyscallNumber::Sigpending => super::signal::sys_sigpending(arg1, arg2),
        SyscallNumber::Sigsuspend => {
            // sigsuspend requires frame access - must use handler.rs path
            log::warn!("sigsuspend called without frame access - use handler.rs path");
            SyscallResult::Err(38) // ENOSYS
        }
        SyscallNumber::Sigaltstack => super::signal::sys_sigaltstack(arg1, arg2),
        SyscallNumber::Sigreturn => super::signal::sys_sigreturn(),
        SyscallNumber::Ioctl => super::ioctl::sys_ioctl(arg1, arg2, arg3),
        SyscallNumber::Socket => super::socket::sys_socket(arg1, arg2, arg3),
        SyscallNumber::Bind => super::socket::sys_bind(arg1, arg2, arg3),
        SyscallNumber::SendTo => super::socket::sys_sendto(arg1, arg2, arg3, arg4, arg5, arg6),
        SyscallNumber::RecvFrom => super::socket::sys_recvfrom(arg1, arg2, arg3, arg4, arg5, arg6),
        SyscallNumber::Connect => super::socket::sys_connect(arg1, arg2, arg3),
        SyscallNumber::Accept => super::socket::sys_accept(arg1, arg2, arg3),
        SyscallNumber::Listen => super::socket::sys_listen(arg1, arg2),
        SyscallNumber::Shutdown => super::socket::sys_shutdown(arg1, arg2),
        SyscallNumber::Getsockname => super::socket::sys_getsockname(arg1, arg2, arg3),
        SyscallNumber::Getpeername => super::socket::sys_getpeername(arg1, arg2, arg3),
        SyscallNumber::Socketpair => super::socket::sys_socketpair(arg1, arg2, arg3, arg4),
        SyscallNumber::Setsockopt => super::socket::sys_setsockopt(arg1, arg2, arg3, arg4, arg5),
        SyscallNumber::Getsockopt => super::socket::sys_getsockopt(arg1, arg2, arg3, arg4, arg5),
        SyscallNumber::Poll => handlers::sys_poll(arg1, arg2, arg3 as i32),
        SyscallNumber::Select => handlers::sys_select(arg1 as i32, arg2, arg3, arg4, arg5),
        SyscallNumber::Pipe => super::pipe::sys_pipe(arg1),
        SyscallNumber::Pipe2 => super::pipe::sys_pipe2(arg1, arg2),
        SyscallNumber::Close => super::pipe::sys_close(arg1 as i32),
        SyscallNumber::Dup => handlers::sys_dup(arg1),
        SyscallNumber::Dup2 => handlers::sys_dup2(arg1, arg2),
        SyscallNumber::Fcntl => handlers::sys_fcntl(arg1, arg2, arg3),
        SyscallNumber::Pause => super::signal::sys_pause(),
        SyscallNumber::Nanosleep => super::time::sys_nanosleep(arg1, arg2),
        SyscallNumber::Getitimer => super::signal::sys_getitimer(arg1 as i32, arg2),
        SyscallNumber::Alarm => super::signal::sys_alarm(arg1),
        SyscallNumber::Setitimer => super::signal::sys_setitimer(arg1 as i32, arg2, arg3),
        SyscallNumber::Wait4 => handlers::sys_waitpid(arg1 as i64, arg2, arg3 as u32),
        SyscallNumber::SetPgid => super::session::sys_setpgid(arg1 as i32, arg2 as i32),
        SyscallNumber::SetSid => super::session::sys_setsid(),
        SyscallNumber::GetPgid => super::session::sys_getpgid(arg1 as i32),
        SyscallNumber::GetSid => super::session::sys_getsid(arg1 as i32),
        // Filesystem syscalls
        SyscallNumber::Access => super::fs::sys_access(arg1, arg2 as u32),
        SyscallNumber::Getcwd => super::fs::sys_getcwd(arg1, arg2),
        SyscallNumber::Chdir => super::fs::sys_chdir(arg1),
        SyscallNumber::Open => super::fs::sys_open(arg1, arg2 as u32, arg3 as u32),
        SyscallNumber::Lseek => super::fs::sys_lseek(arg1 as i32, arg2 as i64, arg3 as i32),
        SyscallNumber::Fstat => super::fs::sys_fstat(arg1 as i32, arg2),
        SyscallNumber::Getdents64 => super::fs::sys_getdents64(arg1 as i32, arg2, arg3),
        SyscallNumber::Rename => super::fs::sys_rename(arg1, arg2),
        SyscallNumber::Mkdir => super::fs::sys_mkdir(arg1, arg2 as u32),
        SyscallNumber::Rmdir => super::fs::sys_rmdir(arg1),
        SyscallNumber::Link => super::fs::sys_link(arg1, arg2),
        SyscallNumber::Unlink => super::fs::sys_unlink(arg1),
        SyscallNumber::Symlink => super::fs::sys_symlink(arg1, arg2),
        SyscallNumber::Readlink => super::fs::sys_readlink(arg1, arg2, arg3),
        SyscallNumber::Mknod => super::fifo::sys_mknod(arg1, arg2 as u32, arg3),
        // *at variants (Linux ARM64 uses these, x86_64 also supports them)
        SyscallNumber::Openat => super::fs::sys_openat(arg1 as i32, arg2, arg3 as u32, arg4 as u32),
        SyscallNumber::Faccessat => super::fs::sys_faccessat(arg1 as i32, arg2, arg3 as u32, arg4 as u32),
        SyscallNumber::Mkdirat => super::fs::sys_mkdirat(arg1 as i32, arg2, arg3 as u32),
        SyscallNumber::Mknodat => super::fs::sys_mknodat(arg1 as i32, arg2, arg3 as u32, arg4),
        SyscallNumber::Unlinkat => super::fs::sys_unlinkat(arg1 as i32, arg2, arg3 as i32),
        SyscallNumber::Symlinkat => super::fs::sys_symlinkat(arg1, arg2 as i32, arg3),
        SyscallNumber::Linkat => super::fs::sys_linkat(arg1 as i32, arg2, arg3 as i32, arg4, arg5 as i32),
        SyscallNumber::Renameat => super::fs::sys_renameat(arg1 as i32, arg2, arg3 as i32, arg4),
        SyscallNumber::Readlinkat => super::fs::sys_readlinkat(arg1 as i32, arg2, arg3, arg4),
        SyscallNumber::Dup3 => handlers::sys_dup2(arg1, arg2), // dup3 with flags=0 is dup2
        SyscallNumber::Pselect6 => handlers::sys_select(arg1 as i32, arg2, arg3, arg4, arg5), // simplified
        // PTY syscalls
        SyscallNumber::PosixOpenpt => super::pty::sys_posix_openpt(arg1),
        SyscallNumber::Grantpt => super::pty::sys_grantpt(arg1),
        SyscallNumber::Unlockpt => super::pty::sys_unlockpt(arg1),
        SyscallNumber::Ptsname => super::pty::sys_ptsname(arg1, arg2, arg3),
        SyscallNumber::GetRandom => super::random::sys_getrandom(arg1, arg2, arg3 as u32),
        SyscallNumber::Clone => super::clone::sys_clone(arg1, arg2, arg3, arg4, arg5),
        SyscallNumber::Futex => super::futex::sys_futex(arg1, arg2 as u32, arg3 as u32, arg4, arg5, arg6 as u32),
        // Graphics syscalls (Breenix-specific)
        SyscallNumber::FbInfo => super::graphics::sys_fbinfo(arg1),
        SyscallNumber::FbDraw => super::graphics::sys_fbdraw(arg1),
        SyscallNumber::FbMmap => super::graphics::sys_fbmmap(),
        SyscallNumber::GetMousePos => super::graphics::sys_get_mouse_pos(arg1),
        // Audio syscalls (Breenix-specific)
        SyscallNumber::AudioInit => super::audio::sys_audio_init(),
        SyscallNumber::AudioWrite => super::audio::sys_audio_write(arg1, arg2),
        // Display takeover (Breenix-specific)
        SyscallNumber::TakeOverDisplay => super::handlers::sys_take_over_display(),
        SyscallNumber::GiveBackDisplay => super::handlers::sys_give_back_display(),
        // Vectored I/O
        SyscallNumber::Readv => super::iovec::sys_readv(arg1, arg2, arg3),
        SyscallNumber::Writev => super::iovec::sys_writev(arg1, arg2, arg3),
        // Stubs for musl libc compatibility
        SyscallNumber::Mremap => SyscallResult::Err(super::errno::ENOMEM as u64),
        SyscallNumber::Madvise => SyscallResult::Ok(0),
        SyscallNumber::Ppoll => SyscallResult::Err(super::errno::ENOSYS as u64),
        SyscallNumber::SetRobustList => SyscallResult::Ok(0),
        // arch_prctl (x86_64 only)
        SyscallNumber::ArchPrctl => {
            const ARCH_SET_FS: u64 = 0x1002;
            const ARCH_GET_FS: u64 = 0x1003;
            match arg1 {
                ARCH_SET_FS => {
                    x86_64::registers::model_specific::FsBase::write(
                        x86_64::VirtAddr::new(arg2),
                    );
                    SyscallResult::Ok(0)
                }
                ARCH_GET_FS => {
                    let fs_base = x86_64::registers::model_specific::FsBase::read().as_u64();
                    match super::userptr::copy_to_user(arg2 as *mut u64, &fs_base) {
                        Ok(()) => SyscallResult::Ok(0),
                        Err(e) => SyscallResult::Err(e),
                    }
                }
                _ => SyscallResult::Err(super::errno::EINVAL as u64),
            }
        }
        // Filesystem: newfstatat
        SyscallNumber::Newfstatat => super::fs::sys_newfstatat(arg1 as i32, arg2, arg3, arg4 as u32),
        // GetTime is not mapped on x86_64 (kept for ARM64 compat)
        SyscallNumber::GetTime => SyscallResult::Err(38), // ENOSYS
        // Testing/diagnostic syscalls (Breenix-specific)
        SyscallNumber::CowStats => super::handlers::sys_cow_stats(arg1),
        SyscallNumber::SimulateOom => super::handlers::sys_simulate_oom(arg1),
    }
}
