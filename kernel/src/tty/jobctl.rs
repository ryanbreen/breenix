//! Job control on a terminal: the foreground process group, the background
//! access checks, and terminal-generated signals.

use super::ctty::TtyRef;
use super::ioctl::{Side, TIOCGPGRP, TIOCSPGRP};
use crate::process::ProcessId;
use crate::syscall::errno::{EFAULT, EINVAL, ENOTTY};
use crate::syscall::userptr::{copy_from_user, copy_to_user};
use alloc::vec::Vec;

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

/// TIOCGPGRP and TIOCSPGRP.
pub fn ioctl(t: &TtyRef, side: Side, request: u64, arg: u64) -> Result<i32, i32> {
    match (t, request) {
        (TtyRef::Console(tty), TIOCGPGRP) => super::ioctl::handle_tiocgpgrp(tty, arg).map(|()| 0),
        (TtyRef::Console(tty), TIOCSPGRP) => {
            check(t, JobOp::Change)?;
            super::ioctl::handle_tiocspgrp(tty, arg).map(|()| 0)
        }
        (TtyRef::Pty(pair), TIOCGPGRP) => {
            if arg == 0 {
                return Err(EFAULT);
            }
            let pgrp = pair.ctl.lock().fg_pgrp.map_or(0, |pgid| pgid.as_u64() as i32);
            copy_to_user(arg as *mut i32, &pgrp).map_err(|_| EFAULT)?;
            Ok(0)
        }
        (TtyRef::Pty(pair), TIOCSPGRP) => {
            if arg == 0 {
                return Err(EFAULT);
            }
            let pgrp: i32 = copy_from_user(arg as *const i32).map_err(|_| EFAULT)?;
            if pgrp < 0 {
                return Err(EINVAL);
            }
            if side != Side::Master {
                check(t, JobOp::Change)?;
            }
            pair.ctl.lock().set_fg(ProcessId::new(pgrp as u64));
            Ok(0)
        }
        _ => Err(ENOTTY),
    }
}

/// Send `sig` to the terminal's foreground process group (a signal character
/// typed at the terminal, or a window size change).
pub fn signal_foreground(t: &TtyRef, sig: u32) {
    let pgid_as_pid = match t.fg_pgrp() {
        Some(pgid) => pgid,
        None => {
            log::debug!("{:?}: Signal {} but no foreground pgid", t.id(), sig);
            return;
        }
    };

    // Collect target PIDs and thread IDs for all non-terminated processes
    // in the foreground group (collect first, then deliver, to avoid
    // holding the manager lock while waking threads)
    let targets: Vec<(crate::process::ProcessId, Option<u64>)> = {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            manager
                .all_processes()
                .iter()
                .filter(|p| p.pgid == pgid_as_pid && !p.is_terminated())
                .map(|p| (p.id, p.main_thread.as_ref().map(|t| t.id)))
                .collect()
        } else {
            return;
        }
    };

    if targets.is_empty() {
        log::debug!(
            "{:?}: No processes in foreground group {} for signal {}",
            t.id(),
            pgid_as_pid.as_u64(),
            sig
        );
        return;
    }

    // Set signal pending on each process. A stop signal is taken, or
    // discarded, as kill() takes it, and a stopped process is not woken.
    let mut woken: Vec<u64> = Vec::new();
    {
        let caller_tid = crate::task::scheduler::current_thread_id();
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut pm) = *manager_guard {
            for &(pid, _) in &targets {
                if crate::signal::constants::sig_mask(sig) & crate::signal::constants::STOP_SIGNALS
                    != 0
                    && crate::signal::delivery::generate_stop_locked(pm, pid, sig, caller_tid)
                {
                    continue;
                }
                let recipient = pm.signal_recipient(pid, sig);
                if let Some(proc) = pm.get_process_mut(recipient) {
                    proc.signals.set_process_pending(sig);
                    if proc.job.stopped.is_none() {
                        woken.extend(proc.main_thread.as_ref().map(|thread| thread.id));
                    }
                    if matches!(proc.state, crate::process::ProcessState::Blocked) {
                        proc.set_ready();
                    }
                    log::info!(
                        "{:?}: Sent signal {} to process {} (PID {})",
                        t.id(),
                        sig,
                        proc.name,
                        pid.as_u64()
                    );
                }
            }
        }
    }

    // Wake threads that may be blocked on signals or waitpid
    for tid in woken {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.unblock_for_signal(tid);
            sched.unblock_for_child_exit(tid);
        });
    }
    crate::task::scheduler::set_need_resched();
}
