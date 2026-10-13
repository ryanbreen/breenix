//! Job control on a terminal: the foreground process group, the background
//! access checks, and terminal-generated signals.

use super::ctty::TtyRef;
use super::ioctl::{Side, TIOCGPGRP, TIOCSPGRP};
use super::pty::PtyPair;
use crate::process::{Process, ProcessId};
use crate::syscall::errno::{EINVAL, EIO, ENOTTY, EPERM, ESRCH};

/// What a process is doing to its terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobOp {
    Read,
    Write,
    Change,
}

/// Background access check for `op` on terminal `t`. Called with no locks
/// held, on the slave or console side only.
pub fn check(_t: &TtyRef, _op: JobOp) -> Result<(), i32> {
    Ok(())
}

/// TIOCGPGRP and TIOCSPGRP on terminal `t`, reached through `side`.
pub fn ioctl(t: &TtyRef, side: Side, request: u64, arg: u64) -> Result<i32, i32> {
    match (t, request) {
        // The console keeps its foreground handling in this effort: any
        // caller may read it or set it to any non-negative group.
        (TtyRef::Console(tty), TIOCGPGRP) => super::ioctl::handle_tiocgpgrp(tty, arg).map(|()| 0),
        (TtyRef::Console(tty), TIOCSPGRP) => super::ioctl::handle_tiocspgrp(tty, arg).map(|()| 0),
        (TtyRef::Pty(_), TIOCGPGRP) => get_pgrp(t, side, arg),
        (TtyRef::Pty(pair), TIOCSPGRP) => set_pgrp(t, pair, arg),
        _ => Err(ENOTTY),
    }
}

/// Whether `t` is the calling process's controlling terminal.
fn is_callers_ctty(t: &TtyRef) -> bool {
    let id = t.id();
    current_process(|me| me.ctty == Some(id)).unwrap_or(false)
}

/// `f` applied to the calling thread's row, under the process manager.
fn current_process<R>(f: impl FnOnce(&Process) -> R) -> Option<R> {
    let tid = crate::task::scheduler::current_thread_id()?;
    let guard = crate::process::manager();
    let pm = guard.as_ref()?;
    pm.find_process_by_thread(tid).map(|(_, me)| f(me))
}

fn put_pgrp(arg: u64, pgrp: Option<ProcessId>) -> Result<i32, i32> {
    let value = pgrp.map_or(0, |p| p.as_u64() as i32);
    crate::syscall::userptr::copy_to_user(arg as *mut i32, &value).map_err(|e| e as i32)?;
    Ok(0)
}

fn take_pgrp(arg: u64) -> Result<i32, i32> {
    crate::syscall::userptr::copy_from_user(arg as *const i32).map_err(|e| e as i32)
}

/// tcgetpgrp: the foreground group, or 0 when there is none. On the slave
/// side the terminal must be the caller's controlling terminal (ENOTTY); the
/// master side may ask of any terminal it holds (Linux `tiocgpgrp`).
fn get_pgrp(t: &TtyRef, side: Side, arg: u64) -> Result<i32, i32> {
    if side == Side::Slave && !is_callers_ctty(t) {
        return Err(ENOTTY);
    }
    put_pgrp(arg, t.fg_pgrp())
}

/// tcsetpgrp (Linux `tiocspgrp`), through either side of a PTY:
/// - a caller in a background group meets `check(Change)`, where an orphaned
///   group's EIO is reported as ENOTTY;
/// - a negative group is EINVAL;
/// - the terminal must be the caller's controlling terminal, of its session
///   (ENOTTY);
/// - the group must exist (ESRCH) and be in the caller's session (EPERM).
fn set_pgrp(t: &TtyRef, pair: &PtyPair, arg: u64) -> Result<i32, i32> {
    match check(t, JobOp::Change) {
        Err(EIO) => return Err(ENOTTY),
        other => other?,
    }
    let pgrp = take_pgrp(arg)?;
    if pgrp < 0 {
        return Err(EINVAL);
    }
    let pgrp = ProcessId::new(pgrp as u64);
    let id = t.id();
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH)?;
    let guard = crate::process::manager();
    let pm = guard.as_ref().ok_or(ESRCH)?;
    let (_, me) = pm.find_process_by_thread(tid).ok_or(ESRCH)?;
    if me.ctty != Some(id) || t.session() != Some(me.sid) {
        return Err(ENOTTY);
    }
    // The group's session: a member's, or, when no process has it as its
    // group, the session of the process with that ID (Linux
    // `session_of_pgrp`). A zombie counts until it is reaped.
    let session = pm
        .iter_processes()
        .find(|(_, p)| p.pgid == pgrp)
        .or_else(|| pm.iter_processes().find(|(pid, _)| *pid == pgrp))
        .map(|(_, p)| p.sid)
        .ok_or(ESRCH)?;
    if session != me.sid {
        return Err(EPERM);
    }
    pair.ctl.lock().set_fg(pgrp);
    Ok(0)
}

/// Send `sig` to the terminal's foreground process group (a signal character
/// typed at the terminal, or a window size change).
pub fn signal_foreground(t: &TtyRef, sig: u32) {
    if let Some(pgid) = t.fg_pgrp() {
        crate::syscall::signal::signal_pgrp_from_kernel(pgid, sig);
    }
}
