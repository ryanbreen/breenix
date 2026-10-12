//! Own descendants while retaining the group leader until cleanup.

use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};

pub struct ChildGroup {
    child: Child,
}

impl ChildGroup {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        command.process_group(0).stdin(Stdio::null());
        Ok(Self {
            child: command.spawn()?,
        })
    }

    pub fn id(&self) -> libc::pid_t {
        self.child.id() as libc::pid_t
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        // WNOWAIT reserves the leader PID even after exit: Drop can safely kill
        // descendants without ever signalling a subsequently reused group ID.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { info.si_pid() } == 0 {
            return Ok(None);
        }
        let status = unsafe { info.si_status() };
        let raw = if info.si_code == libc::CLD_EXITED {
            status << 8
        } else {
            status
        };
        Ok(Some(ExitStatus::from_raw(raw)))
    }
}

impl Drop for ChildGroup {
    fn drop(&mut self) {
        // The unreaped leader still owns this PID, including after try_wait.
        unsafe { libc::kill(-self.id(), libc::SIGKILL) };
        let _ = self.child.wait();
    }
}
