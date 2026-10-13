//! Controlling terminals: terminal identity, the session a terminal belongs
//! to, and the controlling terminal each process records.

use super::driver::TtyDevice;
use super::ioctl::{Side, TIOCGSID, TIOCNOTTY, TIOCSCTTY};
use super::pty::PtyPair;
use super::termios::Termios;
use crate::process::{Process, ProcessId, ProcessManager};
use crate::syscall::errno::{ENOTTY, EPERM, ESRCH};
use alloc::sync::Arc;

/// A terminal, by identity. Stored on processes as their controlling terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TtyId {
    Console,
    Pty(u32),
}

/// A terminal, by reference.
#[derive(Clone)]
pub enum TtyRef {
    Console(Arc<TtyDevice>),
    Pty(Arc<PtyPair>),
}

impl TtyRef {
    pub fn id(&self) -> TtyId {
        match self {
            TtyRef::Console(_) => TtyId::Console,
            TtyRef::Pty(pair) => TtyId::Pty(pair.pty_num),
        }
    }

    pub fn resolve(id: TtyId) -> Option<TtyRef> {
        match id {
            TtyId::Console => super::console().map(TtyRef::Console),
            TtyId::Pty(n) => super::pty::get(n).map(TtyRef::Pty),
        }
    }

    /// The session the terminal is the controlling terminal of.
    pub fn session(&self) -> Option<ProcessId> {
        match self {
            TtyRef::Console(tty) => tty.get_session(),
            TtyRef::Pty(pair) => pair.ctl.lock().session,
        }
    }

    /// The terminal's foreground process group.
    pub fn fg_pgrp(&self) -> Option<ProcessId> {
        match self {
            TtyRef::Console(tty) => tty.get_foreground_pgrp().map(ProcessId::new),
            TtyRef::Pty(pair) => pair.ctl.lock().fg_pgrp,
        }
    }

    pub fn termios(&self) -> Termios {
        match self {
            TtyRef::Console(tty) => tty.get_termios(),
            TtyRef::Pty(pair) => pair.termios_snapshot(),
        }
    }
}

/// A pseudo-terminal's session and foreground process group.
#[derive(Clone, Copy, Debug)]
pub struct TermCtl {
    pub session: Option<ProcessId>,
    pub fg_pgrp: Option<ProcessId>,
}

impl TermCtl {
    pub const fn new() -> Self {
        Self {
            session: None,
            fg_pgrp: None,
        }
    }

    /// Make the terminal the controlling terminal of `sid`, with `pgid` in the
    /// foreground.
    pub fn attach(&mut self, sid: ProcessId, pgid: ProcessId) {
        self.session = Some(sid);
        self.fg_pgrp = Some(pgid);
    }

    /// Detach the terminal from its session, returning the old foreground group.
    pub fn detach(&mut self) -> Option<ProcessId> {
        self.session = None;
        self.fg_pgrp.take()
    }

    pub fn set_fg(&mut self, pgid: ProcessId) {
        self.fg_pgrp = Some(pgid);
    }
}

/// A new process takes its parent's session and controlling terminal.
pub fn inherit(child: &mut Process, parent: &Process) {
    child.sid = parent.sid;
    child.ctty = parent.ctty;
}

/// Set the controlling terminal on every row of thread group `group`.
pub fn set_group_ctty(pm: &mut ProcessManager, group: u64, t: Option<TtyId>) {
    for row in pm.group_rows_mut(group) {
        row.ctty = t;
    }
}

/// The terminal's master side went away. Must be called with no process
/// manager and no pair lock held.
pub fn hangup(_t: TtyId) {}

/// A terminal was opened. Called with no locks held, after the fd is installed.
pub fn acquire_on_open(_t: TtyId, _open_flags: u32) {}

/// The caller's process id, session, process group and thread group.
struct Caller {
    sid: ProcessId,
    pgid: ProcessId,
    group: u64,
}

fn caller(pm: &ProcessManager) -> Result<Caller, i32> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH)?;
    let (pid, process) = pm.find_process_by_thread(tid).ok_or(ESRCH)?;
    Ok(Caller {
        sid: process.sid,
        pgid: process.pgid,
        group: process.thread_group_id.unwrap_or(pid.as_u64()),
    })
}

/// TIOCSCTTY, TIOCNOTTY and TIOCGSID.
pub fn ioctl(t: &TtyRef, _side: Side, request: u64, arg: u64) -> Result<i32, i32> {
    let TtyRef::Pty(pair) = t else {
        return Err(ENOTTY);
    };
    match request {
        TIOCSCTTY => {
            let mut guard = crate::process::manager();
            let pm = guard.as_mut().ok_or(ESRCH)?;
            let me = caller(pm)?;
            {
                let mut ctl = pair.ctl.lock();
                if ctl.session.is_some_and(|sid| sid != me.sid) && arg == 0 {
                    return Err(EPERM);
                }
                ctl.attach(me.sid, me.pgid);
            }
            set_group_ctty(pm, me.group, Some(t.id()));
            Ok(0)
        }
        TIOCNOTTY => {
            let mut guard = crate::process::manager();
            let pm = guard.as_mut().ok_or(ESRCH)?;
            let me = caller(pm)?;
            {
                let mut ctl = pair.ctl.lock();
                if ctl.session.is_some_and(|sid| sid != me.sid) {
                    return Err(EPERM);
                }
                ctl.session = None;
            }
            set_group_ctty(pm, me.group, None);
            Ok(0)
        }
        TIOCGSID => Err(ENOTTY),
        _ => Err(ENOTTY),
    }
}
