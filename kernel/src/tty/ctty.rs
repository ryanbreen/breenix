//! Controlling-terminal ownership. PM serializes associations with process
//! rows; terminal control locks are leaves below PM. Signals are sent only
//! after releasing PM and all terminal locks.
use super::driver::TtyDevice;
use super::ioctl::Side;
use super::pty::PtyPair;
use crate::process::{Process, ProcessId, ProcessManager};
use crate::syscall::errno::{EFAULT, EIO, ENOTTY, EPERM, ESRCH};
use alloc::sync::Arc;

/// A terminal a session can control: a virtual terminal (1-based; tty1 is
/// the console) or a pseudo-terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TtyId {
    Vt(u8),
    Pty(u32),
}

impl TtyId {
    /// The console, tty1.
    pub const CONSOLE: TtyId = TtyId::Vt(1);
}

#[derive(Clone)]
pub enum TtyRef {
    Vt(Arc<TtyDevice>),
    Pty(Arc<PtyPair>),
}

#[derive(Clone, Copy, Debug)]
pub struct TermCtl {
    pub session: Option<ProcessId>,
    pub fg_pgrp: Option<ProcessId>,
    last_hangup_fg: Option<ProcessId>,
}
impl TermCtl {
    pub const fn new() -> Self {
        Self {
            session: None,
            fg_pgrp: None,
            last_hangup_fg: None,
        }
    }
    pub fn attach(&mut self, sid: ProcessId, pgid: ProcessId) {
        self.session = Some(sid);
        self.fg_pgrp = Some(pgid);
        self.last_hangup_fg = None;
    }
    pub fn detach(&mut self) -> Option<ProcessId> {
        self.session = None;
        self.fg_pgrp.take()
    }
    pub fn set_fg(&mut self, pgid: ProcessId) {
        self.fg_pgrp = Some(pgid);
    }
}
impl TtyRef {
    pub fn id(&self) -> TtyId {
        match self {
            Self::Vt(t) => TtyId::Vt(t.vt() as u8),
            Self::Pty(p) => TtyId::Pty(p.pty_num),
        }
    }
    pub fn resolve(id: TtyId) -> Option<TtyRef> {
        match id {
            TtyId::Vt(n) => super::driver::vt(usize::from(n)).map(Self::Vt),
            TtyId::Pty(n) => super::pty::get(n).map(Self::Pty),
        }
    }
    pub fn session(&self) -> Option<ProcessId> {
        match self {
            Self::Vt(t) => t.get_session(),
            Self::Pty(p) => p.ctl.lock().session,
        }
    }
    pub fn fg_pgrp(&self) -> Option<ProcessId> {
        match self {
            Self::Vt(t) => t.get_foreground_pgrp().map(ProcessId::new),
            Self::Pty(p) => p.ctl.lock().fg_pgrp,
        }
    }
    pub fn termios(&self) -> super::termios::Termios {
        match self {
            Self::Vt(t) => t.get_termios(),
            Self::Pty(p) => p.termios_snapshot(),
        }
    }
    fn attach(&self, sid: ProcessId, pgid: ProcessId) {
        match self {
            Self::Vt(t) => {
                t.set_session(Some(sid));
                t.set_foreground_pgrp(pgid.as_u64());
            }
            Self::Pty(p) => p.ctl.lock().attach(sid, pgid),
        }
    }
    fn detach(&self) -> Option<ProcessId> {
        match self {
            Self::Vt(t) => {
                t.set_session(None);
                t.clear_foreground_pgrp()
            }
            Self::Pty(p) => p.ctl.lock().detach(),
        }
    }
}

pub fn inherit(child: &mut Process, parent: &Process) {
    child.sid = parent.sid;
    child.ctty = parent.ctty;
}

pub fn set_group_ctty(pm: &mut ProcessManager, group: u64, t: Option<TtyId>) {
    // Retained exited rows include a leader whose other threads still run.
    // They carry the same process metadata until the group is reaped.
    for (_, row) in pm.iter_processes_mut() {
        if row.thread_group_id.unwrap_or(row.id.as_u64()) == group {
            row.ctty = t;
        }
    }
}

fn clear_session(pm: &mut ProcessManager, sid: ProcessId, t: TtyId) {
    for (_, row) in pm.iter_processes_mut() {
        if row.sid == sid && row.ctty == Some(t) {
            row.ctty = None;
        }
    }
}

pub fn acquire_on_open(t: TtyId, open_flags: u32) {
    const O_NOCTTY: u32 = 0x100;
    if open_flags & O_NOCTTY != 0 || open_flags & 3 == 1 {
        return;
    }
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return;
    };
    let mut guard = crate::process::manager();
    let Some(pm) = guard.as_mut() else {
        return;
    };
    let Some((group, sid, pgid, ctty)) = pm
        .find_process_by_thread(tid)
        .and_then(|(pid, row)| Some((pm.thread_group_of(pid)?, row.sid, row.pgid, row.ctty)))
    else {
        return;
    };
    if sid.as_u64() != group || ctty.is_some() {
        return;
    }
    let Some(tty) = TtyRef::resolve(t) else {
        return;
    };
    // The master may have hung up after the fd was installed and before
    // this hook acquired PM. Never restore ownership after that hangup.
    if tty.session().is_some() || matches!(&tty, TtyRef::Pty(pair) if pair.master_closed()) {
        return;
    }
    tty.attach(sid, pgid);
    set_group_ctty(pm, group, Some(t));
}

pub fn ioctl(t: &TtyRef, side: Side, request: u64, arg: u64) -> Result<i32, i32> {
    ioctl_with_access(t, side, request, arg, true)
}

pub fn ioctl_with_access(t: &TtyRef, side: Side, request: u64, arg: u64, readable: bool) -> Result<i32, i32> {
    const TIOCSCTTY: u64 = 0x540e;
    const TIOCNOTTY: u64 = 0x5422;
    const TIOCGSID: u64 = 0x5429;
    // Linux's hung-up slave file operations return EIO before job-control
    // dispatch. Healthy terminals detached with TIOCNOTTY still use ENOTTY.
    if matches!(side, Side::Slave) && matches!(t, TtyRef::Pty(pair) if pair.master_closed()) {
        return Err(EIO);
    }
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH)?;
    let mut guard = crate::process::manager();
    let pm = guard.as_mut().ok_or(ESRCH)?;
    let (group, sid, pgid, ctty, privileged) = pm
        .find_process_by_thread(tid)
        .and_then(|(pid, row)| {
            Some((
                pm.thread_group_of(pid)?,
                row.sid,
                row.pgid,
                row.ctty,
                row.cred.privileged(),
            ))
        })
        .ok_or(ESRCH)?;
    let id = t.id();
    match request {
        TIOCSCTTY => {
            if matches!(t, TtyRef::Pty(pair) if pair.master_closed()) {
                return Err(EIO);
            }
            if sid.as_u64() == group && t.session() == Some(sid) {
                return Ok(0);
            }
            if sid.as_u64() != group || ctty.is_some() {
                return Err(EPERM);
            }
            // Linux permits effective-root acquisition on a write-only fd,
            // but ordinary callers must have read access to the terminal.
            if !readable && !privileged {
                return Err(EPERM);
            }
            if let Some(old_sid) = t.session() {
                if arg != 1 || !privileged {
                    return Err(EPERM);
                }
                clear_session(pm, old_sid, id);
            }
            t.attach(sid, pgid);
            set_group_ctty(pm, group, Some(id));
            Ok(0)
        }
        TIOCGSID => {
            // Linux permits this query on a PTY master regardless of the
            // caller's ctty, but the slave/console must be its own terminal.
            if !matches!(side, Side::Master) && ctty != Some(id) {
                return Err(ENOTTY);
            }
            let session = t.session().ok_or(ENOTTY)?;
            drop(guard);
            crate::syscall::userptr::copy_to_user(arg as *mut i32, &(session.as_u64() as i32))
                .map_err(|_| EFAULT)?;
            Ok(0)
        }
        TIOCNOTTY => {
            if matches!(side, Side::Master) || ctty != Some(id) {
                return Err(ENOTTY);
            }
            let fg = if sid.as_u64() == group {
                let fg = t.detach();
                clear_session(pm, sid, id);
                fg
            } else {
                set_group_ctty(pm, group, None);
                None
            };
            drop(guard);
            if let Some(fg) = fg {
                crate::syscall::signal::signal_pgrp_from_kernel(
                    fg,
                    crate::signal::constants::SIGHUP,
                );
                crate::syscall::signal::signal_pgrp_from_kernel(
                    fg,
                    crate::signal::constants::SIGCONT,
                );
            }
            Ok(0)
        }
        _ => Err(ENOTTY),
    }
}

/// What an exit leaves for after descriptor retirement: the session it
/// controlled, and, when the process's last row exits, its group, whose
/// virtual-terminal modes and display the console resets.
pub struct LeaderExit {
    session: Option<SessionExit>,
    group: Option<u64>,
}

struct SessionExit {
    tty: TtyRef,
    sid: ProcessId,
    fg: Option<ProcessId>,
}

/// Called before descriptor retirement in both exit paths, while PM is held.
/// Snapshot only: descriptors must close before terminal disassociation.
pub fn on_exit_locked(pm: &mut ProcessManager, pid: ProcessId) -> Option<LeaderExit> {
    let group = pm.thread_group_of(pid)?;
    // A leader's row can exit while its other threads still run. The process
    // exits only with its last live row.
    if pm.group_rows(group).any(|row| row.id != pid) {
        return None;
    }
    let session = leader_exit_locked(pm, group);
    Some(LeaderExit { session, group: Some(group) })
}

/// The controlling process's exit: the session it leads and its terminal.
fn leader_exit_locked(pm: &mut ProcessManager, group: u64) -> Option<SessionExit> {
    let leader = pm.get_process(ProcessId::new(group))?;
    if leader.sid.as_u64() != group {
        return None;
    }
    let sid = leader.sid;
    let id = leader.ctty?;
    let tty = TtyRef::resolve(id)?;
    if tty.session() != Some(sid) {
        return None;
    }
    let fg = tty.fg_pgrp();
    // Claim the exiting process once without detaching the terminal before
    // fd close. Retain the object so numeric PTY reuse cannot change it.
    set_group_ctty(pm, group, None);
    Some(SessionExit { tty, sid, fg })
}

pub fn finish_exit(exit: LeaderExit) {
    if let Some(session) = exit.session {
        finish_session_exit(session);
    }
    if let Some(group) = exit.group {
        crate::syscall::handlers::console_owner_exited(group);
    }
}

fn finish_session_exit(exit: SessionExit) {
    // Descriptor retirement may have hung up the master. Complete a deferred
    // hangup too, before deciding whether Linux's saved-pgrp path applies.
    if matches!(&exit.tty, TtyRef::Pty(pair) if pair.master_closed()) {
        hangup_ref(&exit.tty);
    }
    let (detached, fg) = {
        let mut guard = crate::process::manager();
        if let Some(pm) = guard.as_mut() {
            if exit.tty.session() == Some(exit.sid) {
                let fg = exit.tty.detach();
                clear_session(pm, exit.sid, exit.tty.id());
                (true, fg)
            } else {
                let saved = match &exit.tty {
                    TtyRef::Pty(pair) => pair.ctl.lock().last_hangup_fg,
                    TtyRef::Vt(_) => exit.fg,
                };
                (false, saved)
            }
        } else {
            (false, exit.fg)
        }
    };
    if detached && matches!(exit.tty.id(), TtyId::Vt(_)) {
        // Linux's non-PTY exit hangs up the session: clear every association,
        // signal the session leader, then SIGHUP the foreground group.
        crate::syscall::signal::signal_process_from_kernel(exit.sid, crate::signal::constants::SIGHUP);
        crate::syscall::signal::signal_process_from_kernel(exit.sid, crate::signal::constants::SIGCONT);
    }
    if let Some(fg) = fg {
        crate::syscall::signal::signal_pgrp_from_kernel(fg, crate::signal::constants::SIGHUP);
        // Linux resumes the saved foreground group when fd close already
        // hung up the controlling terminal; a live PTY disassociation does not.
        if !detached {
            crate::syscall::signal::signal_pgrp_from_kernel(fg, crate::signal::constants::SIGCONT);
        }
    }
}

/// Called with neither PM nor a pair lock held, before removing the pair
/// from the allocator on the last master close.
pub fn hangup(t: TtyId) {
    let Some(tty) = TtyRef::resolve(t) else {
        return;
    };
    hangup_ref(&tty);
}

fn hangup_ref(tty: &TtyRef) {
    let mut guard = crate::process::manager();
    let Some(pm) = guard.as_mut() else {
        return;
    };
    let Some(sid) = tty.session() else {
        return;
    };
    match tty {
        TtyRef::Pty(pair) => {
            let mut ctl = pair.ctl.lock();
            let fg = ctl.detach();
            ctl.last_hangup_fg = fg;
        }
        TtyRef::Vt(_) => { tty.detach(); }
    }
    clear_session(pm, sid, tty.id());
    drop(guard);
    crate::syscall::signal::signal_process_from_kernel(sid, crate::signal::constants::SIGHUP);
    crate::syscall::signal::signal_process_from_kernel(sid, crate::signal::constants::SIGCONT);
}
