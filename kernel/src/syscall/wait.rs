//! wait4 and waitid: report a child's exit, stop or continue to its parent.
//!
//! One implementation for both architectures. A child is selected by PID, by
//! process group or as any child; its state changes are reported in the order
//! Linux's `wait_consider_task` takes them: an exit (the child is reaped unless
//! WNOWAIT), then a stop (WUNTRACED / WSTOPPED), then a continue (WCONTINUED).
//! A stop or continue is reported once, unless WNOWAIT leaves it in place.

use super::errno::{ECHILD, EFAULT, EINVAL, ESRCH};
use super::userptr;
use super::SyscallResult;
use crate::process::process::JobReport;
use crate::process::ProcessId;
use crate::signal::constants::{CLD_CONTINUED, CLD_STOPPED};

/// Ensure TTBR0 is set to the current thread's process page tables.
///
/// After a syscall blocks and resumes (e.g., waitpid blocking until child exits),
/// TTBR0 may have been changed by context switches to other processes. Before
/// accessing user memory, we must restore TTBR0 to the current thread's page tables.
#[cfg(target_arch = "aarch64")]
fn ensure_current_address_space() {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };

    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            if let Some(ref page_table) = process.page_table {
                let ttbr0_value = page_table.level_4_frame().start_address().as_u64();
                crate::arch_impl::aarch64::ttbr0::restore_process_ttbr0(ttbr0_value);
            }
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn ensure_current_address_space() {
    // On x86_64, CR3 handling is done differently
}

/// Return at once if no child has a state change to report.
pub const WNOHANG: u32 = 1;
/// wait4: report stopped children. waitid's WSTOPPED has the same value.
pub const WUNTRACED: u32 = 2;
/// waitid: report children that have exited (wait4 always does).
pub const WEXITED: u32 = 4;
/// Report children continued by SIGCONT.
pub const WCONTINUED: u32 = 8;
/// waitid: leave the reported child waitable.
pub const WNOWAIT: u32 = 0x0100_0000;
/// Linux's thread and clone selection flags. Each row waits for its own
/// children here, so __WNOTHREAD changes nothing. __WCLONE selects only
/// children created by clone (CLONE_VM rows), unless __WALL, which selects
/// every child as a plain wait does.
const WNOTHREAD: u32 = 0x2000_0000;
const WALL: u32 = 0x4000_0000;
const WCLONE: u32 = 0x8000_0000;

/// The option bits wait4 accepts; any other is EINVAL.
const WAIT4_OPTIONS: u32 = WNOHANG | WUNTRACED | WCONTINUED | WNOTHREAD | WALL | WCLONE;
/// The option bits waitid accepts; any other is EINVAL.
const WAITID_OPTIONS: u32 =
    WNOHANG | WUNTRACED | WEXITED | WCONTINUED | WNOWAIT | WNOTHREAD | WALL | WCLONE;

/// waitid idtypes.
const P_ALL: u32 = 0;
const P_PID: u32 = 1;
const P_PGID: u32 = 2;

/// Which children a wait considers.
#[derive(Clone, Copy)]
enum Selector {
    /// The child with this PID.
    Pid(ProcessId),
    /// Children in this process group.
    Group(ProcessId),
    /// Children in the caller's process group, as it was when the wait
    /// began: `bind_caller_group` makes it a `Group` before the first scan.
    CallerGroup,
    /// Any child.
    Any,
}

/// A state change a wait reports.
#[derive(Clone, Copy)]
enum Event {
    /// The child exited with this status (negative: killed by signal -status,
    /// with 0x80 for a core dump).
    Exited(i32),
    /// The child was stopped by this signal.
    Stopped(u32),
    /// The child was continued.
    Continued,
}

/// What a scan found.
struct Found {
    /// The waiting process, the reaper an exit's reap claim records.
    reaper: ProcessId,
    pid: ProcessId,
    uid: u32,
    event: Event,
}

/// Look once through the calling thread's process's children for a state
/// change `options` asks for. A stop or continue it reports is consumed here,
/// unless WNOWAIT; an exit is left for `reap` to claim. ECHILD when no child
/// matches the selector.
fn scan(thread_id: u64, selector: Selector, options: u32) -> Result<Option<Found>, u64> {
    crate::process::with_process_manager(|manager| {
        let (reaper, caller_pgid, children) = match manager.find_process_by_thread(thread_id) {
            Some((pid, caller)) => (pid, caller.pgid, caller.children.clone()),
            None => return Err(EINVAL as u64),
        };
        let clone_only = options & WCLONE != 0 && options & WALL == 0;
        let mut any = false;
        for child_pid in children {
            let Some(child) = manager.get_process_mut(child_pid) else {
                continue;
            };
            let selected = match selector {
                Selector::Pid(pid) => child_pid == pid,
                Selector::Group(pgid) => child.pgid == pgid,
                Selector::CallerGroup => child.pgid == caller_pgid,
                Selector::Any => true,
            } && (!clone_only || child.thread_group_id.is_some());
            if !selected {
                continue;
            }
            any = true;
            let uid = child.cred.uid;
            if let crate::process::ProcessState::Terminated(code) = child.state {
                if options & WEXITED != 0 {
                    return Ok(Some(Found {
                        reaper,
                        pid: child_pid,
                        uid,
                        event: Event::Exited(code),
                    }));
                }
                continue;
            }
            let event = match child.job.report {
                Some(JobReport::Stopped(sig))
                    if options & WUNTRACED != 0 && child.job.stopped.is_some() =>
                {
                    Event::Stopped(sig)
                }
                Some(JobReport::Continued) if options & WCONTINUED != 0 => Event::Continued,
                _ => continue,
            };
            if options & WNOWAIT == 0 {
                child.job.report = None;
            }
            return Ok(Some(Found {
                reaper,
                pid: child_pid,
                uid,
                event,
            }));
        }
        if any {
            Ok(None)
        } else {
            Err(ECHILD as u64)
        }
    })
    .unwrap_or(Err(ECHILD as u64))
}

/// The caller's process group, read once when the wait begins: a later
/// setpgid by the caller does not change which children the wait selects.
fn bind_caller_group(thread_id: u64, selector: Selector) -> Result<Selector, u64> {
    let Selector::CallerGroup = selector else {
        return Ok(selector);
    };
    crate::process::with_process_manager(|manager| {
        manager
            .find_process_by_thread(thread_id)
            .map(|(_, caller)| Selector::Group(caller.pgid))
    })
    .flatten()
    .ok_or(EINVAL as u64)
}

/// Let the calling thread run on after `block_current_for_child_exit`.
fn unblock_self() {
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
            thread.set_ready();
        }
    });
}

/// Wait for a state change `options` asks for, blocking unless WNOHANG.
/// Ok(None) only for WNOHANG with nothing to report. A signal interrupts the
/// wait with EINTR, or ERESTARTSYS when it is to be restarted.
///
/// Blocking marks the thread BlockedOnChildExit first and scans again after,
/// so a change landing between the two is seen by the scan or wakes the thread
/// (`unblock_for_child_exit`); the scheduler lock orders the two.
fn wait_for_child(selector: Selector, options: u32) -> Result<Option<Found>, u64> {
    let thread_id = crate::task::scheduler::current_thread_id().ok_or(EINVAL as u64)?;
    let selector = bind_caller_group(thread_id, selector)?;
    if let Some(found) = scan(thread_id, selector, options)? {
        return Ok(Some(found));
    }
    if options & WNOHANG != 0 {
        return Ok(None);
    }
    loop {
        crate::task::scheduler::with_scheduler(|sched| sched.block_current_for_child_exit());
        crate::tracing::providers::process::trace_waitpid_block(thread_id as u16, 0);
        match scan(thread_id, selector, options) {
            Ok(None) => {}
            done => {
                unblock_self();
                return done;
            }
        }
        if let Some(e) = crate::syscall::check_signals_for_restartable_wait() {
            unblock_self();
            return Err(e as u64);
        }
        crate::per_cpu::preempt_enable();
        crate::task::scheduler::yield_current();
        crate::arch_halt_with_interrupts();
        crate::per_cpu::preempt_disable();
    }
}

/// The wait status wait4 reports for `event`.
fn wait_status(event: Event) -> i32 {
    match event {
        Event::Exited(code) if code < 0 => {
            let signal_number = -code;
            let core_dump = (signal_number & 0x80) != 0;
            (signal_number & 0x7f) | if core_dump { 0x80 } else { 0 }
        }
        Event::Exited(code) => (code & 0xff) << 8,
        Event::Stopped(sig) => ((sig as i32) << 8) | 0x7f,
        Event::Continued => 0xffff,
    }
}

/// sys_waitpid - wait4(pid, status, options): wait for a child to change state
///
/// pid > 0 waits for that child, pid == -1 for any child, pid == 0 for any
/// child in the caller's process group, and pid < -1 for any child in process
/// group -pid. Returns the child's PID, or 0 under WNOHANG when no child has a
/// state change to report.
pub fn sys_waitpid(pid: i64, status_ptr: u64, options: u32) -> SyscallResult {
    if options & !WAIT4_OPTIONS != 0 {
        return SyscallResult::Err(EINVAL as u64);
    }
    let pid = pid as i32;
    let selector = match pid {
        -1 => Selector::Any,
        0 => Selector::CallerGroup,
        i32::MIN => return SyscallResult::Err(ESRCH as u64),
        p if p < 0 => Selector::Group(ProcessId::new((-p) as u64)),
        p => Selector::Pid(ProcessId::new(p as u64)),
    };
    let found = match wait_for_child(selector, options | WEXITED) {
        Ok(Some(found)) => found,
        Ok(None) => return SyscallResult::Ok(0),
        Err(e) => return SyscallResult::Err(e),
    };
    match found.event {
        Event::Exited(code) => complete_wait(found.pid, code, status_ptr, found.reaper),
        event => {
            if status_ptr != 0 {
                ensure_current_address_space();
                if userptr::copy_to_user(status_ptr as *mut i32, &wait_status(event)).is_err() {
                    return SyscallResult::Err(EFAULT as u64);
                }
            }
            SyscallResult::Ok(found.pid.as_u64())
        }
    }
}

/// waitid(idtype, id, infop, options, rusage): wait for a child to change
/// state and describe it in a siginfo.
///
/// idtype P_ALL waits for any child, P_PID for child `id` and P_PGID for any
/// child in process group `id` (0: the caller's). `options` must ask for at
/// least one of WEXITED, WSTOPPED and WCONTINUED. Returns 0; under WNOHANG
/// with nothing to report, the siginfo's si_signo and si_pid are 0.
pub fn sys_waitid(idtype: u32, id: u64, infop: u64, options: u32) -> SyscallResult {
    if options & !WAITID_OPTIONS != 0 || options & (WEXITED | WUNTRACED | WCONTINUED) == 0 {
        return SyscallResult::Err(EINVAL as u64);
    }
    let id = id as i32;
    let selector = match idtype {
        P_ALL => Selector::Any,
        P_PID if id > 0 => Selector::Pid(ProcessId::new(id as u64)),
        P_PGID if id == 0 => Selector::CallerGroup,
        P_PGID if id > 0 => Selector::Group(ProcessId::new(id as u64)),
        _ => return SyscallResult::Err(EINVAL as u64),
    };
    let found = match wait_for_child(selector, options) {
        Ok(found) => found,
        Err(e) => return SyscallResult::Err(e),
    };

    let mut info = [0i32; 32];
    if let Some(found) = &found {
        let (code, status) = match found.event {
            Event::Exited(code) => crate::signal::types::child_exit_code_status(code),
            Event::Stopped(sig) => (CLD_STOPPED, sig as i32),
            Event::Continued => (CLD_CONTINUED, crate::signal::constants::SIGCONT as i32),
        };
        info[0] = crate::signal::constants::SIGCHLD as i32;
        info[2] = code;
        info[4] = found.pid.as_u64() as i32;
        info[5] = found.uid as i32;
        info[6] = status;
    }

    if let Some(Found {
        reaper,
        pid,
        event: Event::Exited(code),
        ..
    }) = found
    {
        if options & WNOWAIT == 0 {
            // The child is reaped as wait4 reaps it; its status goes in the
            // siginfo, not through a status pointer.
            if let SyscallResult::Err(e) = complete_wait(pid, code, 0, reaper) {
                return SyscallResult::Err(e);
            }
        }
    }

    if infop != 0 {
        ensure_current_address_space();
        if userptr::copy_to_user(infop as *mut [i32; 32], &info).is_err() {
            return SyscallResult::Err(EFAULT as u64);
        }
    }
    SyscallResult::Ok(0)
}

/// Helper function to complete a wait operation
///
/// `reaper` is the process calling `waitpid`; it is the identity recorded in the
/// row's reap claim (P6a's reap arm).
fn complete_wait(
    child_pid: crate::process::ProcessId,
    exit_code: i32,
    status_ptr: u64,
    reaper: crate::process::ProcessId,
) -> SyscallResult {
    if exit_code == -14 {
        crate::signal::types::tmpdiag::REAP_US.store(crate::signal::monotonic_micros(), core::sync::atomic::Ordering::Relaxed);
    }
    let wstatus = wait_status(Event::Exited(exit_code));

    // P6a reap arm. Condition C3: the claim is taken under PM *before* any
    // status reaches userspace. Two concurrent waiters can both pass the scan
    // that produced `exit_code`; only the one that installs the claim may report
    // it, and the loser returns ECHILD having copied nothing.
    let mut claim_refused = false;
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        let mut evicted = None;
        {
            let mut manager_guard = crate::process::manager();
            if let Some(ref mut manager) = *manager_guard {
                if let Some((_parent_pid, parent)) = manager.find_process_by_thread_mut(thread_id) {
                    parent.children.retain(|&id| id != child_pid);
                }
                match manager.reap_row(child_pid, reaper, exit_code) {
                    crate::process::manager::ReapOutcome::Claimed(row) => evicted = row,
                    crate::process::manager::ReapOutcome::Refused => claim_refused = true,
                }
            }
        }
        // Condition C8: the row destructor runs after the guard is released, so
        // no `Process` drop happens inside the DAIF-masked PM window.
        drop(evicted);
        log::debug!(
            "complete_wait: reap arm for child {} ({})",
            child_pid.as_u64(),
            if claim_refused { "refused" } else { "claimed" }
        );
    }
    if claim_refused {
        return SyscallResult::Err(ECHILD as u64);
    }
    // Disclosed consequence of C3's ordering, and it is forced: the claim
    // commits the reap before the status is copied, so a `copy_to_user` that
    // faults now loses that status instead of leaving the child reapable. It
    // cannot be avoided while the claim is the arbiter — a row claimed by this
    // caller is a tombstone whether or not the copy lands, and re-opening it on
    // a fault would hand the same status to a second waiter, which is exactly
    // the defect C3 exists to close. Linux has the same wart on the same path.

    if status_ptr != 0 {
        // CRITICAL: Restore TTBR0 to current process's page tables before accessing user memory.
        // After blocking in waitpid and resuming, TTBR0 may have been changed by context
        // switches to other processes. Without this, we'd fault trying to access the
        // parent's user stack through the wrong page tables.
        ensure_current_address_space();

        let user_ptr = status_ptr as *mut i32;
        if userptr::copy_to_user(user_ptr, &wstatus).is_err() {
            log::error!("complete_wait: Failed to write status");
            return SyscallResult::Err(EFAULT as u64);
        }
    }

    SyscallResult::Ok(child_pid.as_u64())
}
