//! Process and I/O support for Breenix userland commands.

use std::io::{self, Write};
use std::process::ExitCode;

const SIGPIPE: i32 = 13;
const SIG_DFL: usize = 0;
const SIG_ERR: usize = usize::MAX;
const STDOUT_FILENO: i32 = 1;

extern "C" {
    fn signal(number: i32, handler: usize) -> usize;
    fn write(fd: i32, buffer: *const u8, count: usize) -> isize;
}

/// Use the default SIGPIPE action so an output pipeline can stop its producer.
pub fn default_sigpipe() -> io::Result<()> {
    // SAFETY: SIGPIPE and SIG_DFL are valid for the supported Unix ABIs,
    // which pass the disposition and return value as pointer-sized values.
    if unsafe { signal(SIGPIPE, SIG_DFL) } == SIG_ERR {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Write to fd 1 and propagate descriptor errors to the command.
pub struct Stdout;

impl Write for Stdout {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // SAFETY: bytes is readable for count bytes for the duration of write.
        let count = unsafe { write(STDOUT_FILENO, bytes.as_ptr(), bytes.len()) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(count as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Finish a command, reporting an I/O error on stderr if possible.
pub fn finish(name: &str, result: io::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{name}: {error}");
            ExitCode::FAILURE
        }
    }
}

extern "C" {
    fn getuid() -> u32;
    fn geteuid() -> u32;
    fn kill(pid: i32, signal: i32) -> i32;
}

/// The real and effective user IDs of the caller.
pub fn user_ids() -> (u32, u32) {
    // SAFETY: these calls take no arguments and return the caller's IDs.
    unsafe { (getuid(), geteuid()) }
}

/// Kill a process group created for a child by the caller.
pub fn kill_process_group(leader: u32) -> io::Result<()> {
    let group = i32::try_from(leader)
        .ok()
        .filter(|&id| id > 1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid process group ID"))?;
    // SAFETY: a negative ID addresses the child's group and SIGKILL takes no pointer.
    if unsafe { kill(-group, 9) } == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        // ESRCH means the group already exited.
        if error.raw_os_error() == Some(3) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

extern "C" {
    fn fcntl(fd: i32, command: i32, ...) -> i32;
}

/// Make reads on a case's capture pipe nonblocking.
pub fn nonblocking(fd: std::os::fd::BorrowedFd<'_>) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const O_NONBLOCK: i32 = 0x800;
    let fd = fd.as_raw_fd();
    // SAFETY: the descriptor is borrowed and these commands use no pointer.
    let flags = unsafe { fcntl(fd, F_GETFL) };
    if flags < 0 || unsafe { fcntl(fd, F_SETFL, flags | O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

/// Acquire a nonblocking exclusive lock until the caller closes this file.
pub fn lock_exclusive(fd: std::os::fd::BorrowedFd<'_>) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    // SAFETY: the descriptor is borrowed and flock takes no pointer.
    if unsafe { flock(fd.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Breenix resource IDs for limits inherited by children.
#[repr(i32)]
#[derive(Clone, Copy)]
pub enum Resource {
    CpuSeconds = 0,
    FileSize = 1,
    ProcessCount = 6,
    AddressSpace = 9,
}

extern "C" {
    fn prlimit(pid: i32, resource: i32, new: *const [u64; 2], old: *mut [u64; 2]) -> i32;
}

/// Cap the child's soft and hard limits before exec, preserving stricter limits.
/// Failure to apply any cap fails the spawn.
pub fn cap_child_resources(command: &mut std::process::Command, caps: &'static [(Resource, u64)]) {
    use std::os::unix::process::CommandExt;
    use std::ptr::{null, null_mut};

    // SAFETY: the callback uses stack storage, the Breenix prlimit syscall
    // wrapper and raw OS errors only, with no allocation or locks after fork.
    unsafe {
        command.pre_exec(move || {
            for &(resource, cap) in caps {
                let mut inherited = [0; 2];
                if prlimit(0, resource as i32, null(), &mut inherited) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let capped = inherited.map(|limit| limit.min(cap));
                if prlimit(0, resource as i32, &capped, null_mut()) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}
