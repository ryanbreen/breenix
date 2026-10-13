//! Job control on a terminal: its foreground process group, and what a process
//! in a background group meets when it uses its controlling terminal
//! (POSIX.1-2024 11.1.4, "Terminal Access Control"; tcgetpgrp, tcsetpgrp).
//!
//! The access rules are Linux's `__tty_check_change`: they apply only to the
//! caller's own controlling terminal, and only while the terminal has a
//! foreground group other than the caller's.
//!
//! | op | signal | blocked or ignored | orphaned group | otherwise |
//! |---|---|---|---|---|
//! | read | SIGTTIN | EIO | EIO | signal own group, restart |
//! | write with TOSTOP, attribute changes, tcsetpgrp | SIGTTOU | allowed | EIO | signal own group, restart |

use crate::process::{Process, ProcessId};
use crate::signal::constants::{SIGTTIN, SIGTTOU};
use crate::syscall::errno::{EINVAL, EIO, ENOTTY, EPERM, ERESTARTSYS, ESRCH};
use super::ctty::TtyRef;
use super::ioctl::{Side, TIOCGPGRP, TIOCSPGRP};
use super::pty::PtyPair;
use super::termios::TOSTOP;

/// What a process does with a terminal, for `check`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobOp {
    /// Read input.
    Read,
    /// Write output; checked only while TOSTOP is set.
    Write,
    /// Change the terminal's state: its attributes, its queues, its flow, a
    /// break, or its foreground group.
    Change,
}

/// Whether the calling process may do `op` on terminal `t` now (POSIX 11.1.4).
///
/// Ok when `t` is not the caller's controlling terminal, when it has no
/// foreground group, and when the caller's group is the foreground group.
/// Otherwise, for a process in a background group:
/// - with the signal (SIGTTIN for a read, SIGTTOU for the rest) blocked or
///   ignored, a read fails with EIO and the rest are allowed;
/// - in an orphaned process group, it fails with EIO;
/// - else the signal goes to the caller's process group and the call is to be
///   restarted once the group continues (ERESTARTSYS), or fails with EINTR
///   when a handler installed without SA_RESTART runs first.
///
/// Called with no locks held: for reads (before every attempt, so a sleeping
/// reader moved to the background is caught), writes, flushes, flow control
/// and breaks on the slave side of a PTY, and for attribute changes and
/// tcsetpgrp on either side, which Linux checks against the slave behind a
/// master (`real_tty`). The console keeps its own foreground handling in this
/// effort and is always allowed.
pub fn check(t: &TtyRef, op: JobOp) -> Result<(), i32> {
    if let TtyRef::Console(_) = t {
        return Ok(());
    }
    let sig = match op {
        JobOp::Read => SIGTTIN,
        JobOp::Write if t.termios().c_lflag & TOSTOP == 0 => return Ok(()),
        JobOp::Write | JobOp::Change => SIGTTOU,
    };
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return Ok(());
    };
    let pgid = {
        let guard = crate::process::manager();
        let Some(pm) = guard.as_ref() else { return Ok(()) };
        let Some((_, me)) = pm.find_process_by_thread(tid) else { return Ok(()) };
        if me.ctty != Some(t.id()) {
            return Ok(());
        }
        match t.fg_pgrp() {
            Some(fg) if fg != me.pgid => {}
            _ => return Ok(()),
        }
        if blocks_or_ignores(me, sig) {
            return if op == JobOp::Read { Err(EIO) } else { Ok(()) };
        }
        if pm.pgrp_is_orphaned(me.pgid, None) {
            return Err(EIO);
        }
        me.pgid
    };
    crate::syscall::signal::signal_pgrp_from_kernel(pgid, sig);
    Err(crate::syscall::check_signals_for_restartable_wait().unwrap_or(ERESTARTSYS))
}

/// Whether the calling thread's row blocks `sig` or its process ignores it
/// explicitly (Linux `is_ignored`).
fn blocks_or_ignores(me: &Process, sig: u32) -> bool {
    me.signals.is_blocked(sig) || me.signals.get_handler(sig).is_ignore()
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

/// Send `sig` to the foreground process group of `t`, if it has one: the
/// signals the line discipline generates (VINTR, VQUIT, VSUSP) and SIGWINCH.
/// A stop signal is discarded for an orphaned group, as kill() takes it.
/// Called with no locks held.
pub fn signal_foreground(t: &TtyRef, sig: u32) {
    if let Some(pgid) = t.fg_pgrp() {
        crate::syscall::signal::signal_pgrp_from_kernel(pgid, sig);
    }
}
