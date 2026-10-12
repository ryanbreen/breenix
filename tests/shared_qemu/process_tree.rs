//! An owned process group for cargo -> qemu-uefi -> QEMU (and disk builders).

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};

pub struct ChildGroup {
    child: Child,
}

impl ChildGroup {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        // Every descendant inherits this group unless it deliberately detaches.
        command.process_group(0);
        Ok(Self { child: command.spawn()? })
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
}

impl Drop for ChildGroup {
    fn drop(&mut self) {
        // Kill descendants even if the direct child has already exited.
        // SAFETY: the child was made leader of a new process group at spawn.
        unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
        let _ = self.child.wait();
    }
}
