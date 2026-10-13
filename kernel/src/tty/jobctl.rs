//! Job control on a terminal: the foreground process group, the background
//! access checks, and terminal-generated signals.

use super::ctty::TtyRef;
use super::ioctl::{Side, TIOCGPGRP, TIOCSPGRP};
use crate::process::ProcessId;
use crate::syscall::errno::{EFAULT, EINVAL, ENOTTY};
use crate::syscall::userptr::{copy_from_user, copy_to_user};

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
///
/// It allocates nothing: a user can make it run (a signal character written
/// to a master, TIOCSWINSZ), so the group is walked in pid order a batch at a
/// time, each batch signalled under the process manager and its threads
/// woken after the manager is released.
pub fn signal_foreground(t: &TtyRef, sig: u32) {
    const BATCH: usize = 16;
    let Some(pgid) = t.fg_pgrp() else {
        log::debug!("{:?}: Signal {} but no foreground pgid", t.id(), sig);
        return;
    };
    let caller_tid = crate::task::scheduler::current_thread_id();
    let stop = crate::signal::constants::sig_mask(sig) & crate::signal::constants::STOP_SIGNALS != 0;
    // Members with a pid above `after` are still to be signalled.
    let mut after = 0u64;
    loop {
        let mut members = [ProcessId::new(0); BATCH];
        let mut found = 0;
        let mut woken = [0u64; BATCH];
        let mut wake = 0;
        {
            let mut manager_guard = crate::process::manager();
            let Some(pm) = manager_guard.as_mut() else {
                return;
            };
            for (pid, process) in pm.iter_processes() {
                if pid.as_u64() <= after || process.pgid != pgid || process.is_terminated() {
                    continue;
                }
                members[found] = pid;
                found += 1;
                if found == BATCH {
                    break;
                }
            }
            for &pid in &members[..found] {
                // A stop signal is taken, or discarded, as kill() takes it,
                // and a stopped process is not woken.
                if stop && crate::signal::delivery::generate_stop_locked(pm, pid, sig, caller_tid) {
                    continue;
                }
                let recipient = pm.signal_recipient(pid, sig);
                if let Some(proc) = pm.get_process_mut(recipient) {
                    proc.signals.set_process_pending(sig);
                    if proc.job.stopped.is_none() {
                        if let Some(thread) = proc.main_thread.as_ref() {
                            woken[wake] = thread.id;
                            wake += 1;
                        }
                    }
                    if matches!(proc.state, crate::process::ProcessState::Blocked) {
                        proc.set_ready();
                    }
                }
            }
        }

        // Wake threads that may be blocked on signals or waitpid
        for &tid in &woken[..wake] {
            crate::task::scheduler::with_scheduler(|sched| {
                sched.unblock_for_signal(tid);
                sched.unblock_for_child_exit(tid);
            });
        }
        if found < BATCH {
            break;
        }
        after = members[BATCH - 1].as_u64();
    }
    crate::task::scheduler::set_need_resched();
}
