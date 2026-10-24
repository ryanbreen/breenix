//! The operations only a privileged process may perform, in one list, and the
//! rule for inspecting another process (`docs/users/design.md`, section 2).
//!
//! Privilege is an effective user ID of 0; there is no finer split. The checks
//! for these operations name their `Operation` (through `require`, `allowed`
//! or its refusal errno), so this list is the audit of what root alone may do. Checks that belong to a
//! file's or an object's own permission bits (open, unlink, chmod, kill, IPC
//! modes) are not here: they follow `fs::permissions::Credentials::may` and
//! the rules beside each object.
//!
//! Two privileged operations are checked where they live rather than through
//! `require`:
//! - setting the realtime clock, in `syscall/time.rs` (a hot-path file whose
//!   check reads the caller's effective user ID through `sys_geteuid`, which
//!   refuses a user thread without a process);
//! - the display's diagnostic pixel read-back, in `syscall/graphics.rs`, which
//!   also requires the caller to own the display.
//!
//! sethostname joins this list when it lands.

use crate::fs::permissions::Credentials;
use crate::syscall::errno::{EACCES, EPERM};

/// An operation reserved to root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Final system reset or power-off, coordinated by PID 1.
    Reboot,
    /// chroot.
    Chroot,
    /// Raising a hard resource limit (setrlimit, prlimit).
    RaiseHardLimit,
    /// Binding a TCP or UDP port below 1024.
    LowPort,
    /// Creating a character or block device node (mknod).
    DeviceNode,
    /// Reading the kernel's retained output: `/proc/kmsg` and the console's
    /// retained text.
    KernelLog,
    /// Reading or controlling the tracing framework (`/proc/trace`).
    Trace,
    /// The sound device. Until a login session owns its devices (the main
    /// path's graphical login), only root plays sound.
    Audio,
    /// Revoking the controlling terminal from the descriptions open on it
    /// (vhangup), which getty does before each login.
    TerminalHangup,
}

impl Operation {
    /// The refusal: EACCES for a port or a file under `/proc`, EPERM otherwise.
    pub fn errno(self) -> u64 {
        match self {
            Operation::LowPort | Operation::KernelLog | Operation::Trace => EACCES as u64,
            _ => EPERM as u64,
        }
    }
}

/// Whether credentials `cred` may perform `operation`.
pub fn allowed(cred: &Credentials, _operation: Operation) -> bool {
    cred.privileged()
}

/// Refuse `operation` unless the caller is privileged. A kernel thread acts
/// with kernel credentials; a user thread without a process is refused.
pub fn require(operation: Operation) -> Result<(), u64> {
    match Credentials::current(false) {
        Ok(cred) if allowed(&cred, operation) => Ok(()),
        Ok(_) => Err(operation.errno()),
        Err(errno) => Err(errno),
    }
}

/// Whether a process with credentials `inspector` may read another process's
/// private state (`/proc/<pid>/exe`, `kernel_output`, and later `fd`,
/// `environ`, `mem` and ptrace attach) with credentials `target`. Root may.
/// Otherwise the inspector's real user and group must equal each of the
/// target's real, effective and saved IDs, and the target must not have
/// gained an identity at exec: a set-ID program's memory may hold what its
/// identity let it read, which its invoker may not.
pub fn may_inspect(
    inspector: &super::credentials::ProcessCredentials,
    target: &super::credentials::ProcessCredentials,
) -> bool {
    inspector.may_inspect(target)
}

/// The calling process's credentials: root's for a kernel thread, or for boot
/// code before its thread is registered; ESRCH for a user thread without a
/// process.
pub fn current_identity() -> Result<super::credentials::ProcessCredentials, u64> {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return Ok(super::credentials::ProcessCredentials::root());
    };
    {
        let guard = super::manager();
        if let Some((_, process)) = guard.as_ref().and_then(|manager| manager.find_process_by_thread(tid)) {
            return Ok(process.cred.clone());
        }
    }
    if crate::task::scheduler::thread_is_user(tid) {
        Err(crate::fs::permissions::NO_PROCESS)
    } else {
        Ok(super::credentials::ProcessCredentials::root())
    }
}

/// Refuse unless the caller may inspect process `pid` (`may_inspect`): ENOENT
/// when it does not exist or has exited, EACCES when the caller may not.
pub fn require_inspect(pid: u64) -> Result<(), u64> {
    use crate::syscall::errno::ENOENT;
    let inspector = current_identity()?;
    let guard = super::manager();
    let target = guard
        .as_ref()
        .and_then(|manager| manager.get_process(super::ProcessId::new(pid)))
        .filter(|process| !process.is_terminated())
        .ok_or(ENOENT as u64)?;
    if may_inspect(&inspector, &target.cred) {
        Ok(())
    } else {
        Err(EACCES as u64)
    }
}
