//! System V IPC (XSI): message queues, semaphore sets and shared memory
//! segments, named by keys and identifiers.
//!
//! Each class keeps its objects in one table behind an `IrqSafeMutex`
//! (`msg::SYSV_MSG`, `sem::SYSV_SEM`, `shm::SYSV_SHM`). A table is a leaf: no
//! code holding one takes PROCESS_MANAGER, the scheduler or user memory, so
//! callers read their credentials and copy user data in before locking, copy
//! out after unlocking, and wake sleepers after unlocking. A sleeper waits on
//! its object's `Waiters`, which outlives the object's removal. Two paths
//! take the SEM table inside PROCESS_MANAGER: clone with CLONE_SYSVSEM, and
//! a fatal signal's default action that ends the row under the lock (always
//! on ARM64, on x86-64 when the deferred-exit queue is full). The second
//! applies the row's SEM_UNDO adjustments there but wakes no one, since a
//! wake can take the scheduler lock, which ranks above PROCESS_MANAGER: it
//! marks the sets' sleepers, and `wake_deferred` wakes them from the exit
//! hooks that run with no lock held, or from the next semaphore call.

pub mod msg;
pub mod sem;
pub mod shm;
pub mod state;

use crate::fs::permissions::Credentials;
use crate::ipc::wait::{IpcDeadline, WaitEnd};
use crate::process::process::Process;
use crate::process::ProcessId;
use crate::signal::constants::{NSIG, SIG_DFL};
use crate::syscall::errno::{EINTR, ENOMEM, ERESTARTSYS};
use crate::syscall::SyscallResult;
use crate::task::waitqueue::WaitQueueHead;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub use state::IpcPerm;

/// Whether `cred` is granted every bit of `wanted` (state::READ, state::WRITE)
/// on an object with `perm`: POSIX's owner, group and other classes, where
/// the creator counts as an owner and the creator's group as the group, and
/// an effective user of 0 is always granted. Supplementary groups count for
/// the group class, as on Linux, where POSIX names only the effective group.
pub(crate) fn ipcperms(perm: &IpcPerm, cred: &Credentials, wanted: u32) -> bool {
    perm.permits(cred.euid, |gid| cred.in_group(gid), wanted)
}

/// The calling process, read before any table is locked: credentials and
/// the row come from PROCESS_MANAGER.
pub(crate) struct Caller {
    pub cred: Credentials,
    /// The process ID (thread group ID) that getpid returns.
    pub pid: i32,
    /// The calling task's row, which SEM_UNDO lists are kept by.
    pub row: u64,
}

pub(crate) fn caller() -> Result<Caller, u64> {
    let cred = Credentials::current(false)?;
    let pid = match crate::syscall::handlers::sys_getpid() {
        SyscallResult::Ok(pid) => pid as i32,
        SyscallResult::Err(_) => 0,
    };
    let row = crate::task::scheduler::current_thread_id()
        .and_then(|tid| {
            crate::process::with_process_manager(|manager| manager.find_process_by_thread(tid).map(|(row, _)| row.as_u64()))
                .flatten()
        })
        .unwrap_or(0);
    Ok(Caller { cred, pid, row })
}

/// The time the structures report, in seconds since the epoch.
pub(crate) fn now() -> i64 {
    crate::time::current_unix_time()
}

/// What a process sleeping on an object waits for: any change to the
/// object, or its removal.
pub(crate) struct Waiters {
    queue: WaitQueueHead,
    changes: AtomicU64,
    removed: AtomicBool,
    /// A change made under PROCESS_MANAGER that these sleepers have not
    /// been woken for yet.
    wake_owed: AtomicBool,
}

/// Whether some set's sleepers are owed a wake (`Waiters::owe_wake`).
static WAKES_OWED: AtomicBool = AtomicBool::new(false);

impl Waiters {
    /// A new object's sleepers, or ENOMEM.
    pub fn new() -> Result<Arc<Waiters>, i32> {
        Arc::try_new(Waiters {
            queue: WaitQueueHead::new(),
            changes: AtomicU64::new(0),
            removed: AtomicBool::new(false),
            wake_owed: AtomicBool::new(false),
        })
            .map_err(|_| ENOMEM)
    }

    /// The count of changes so far, read under the table lock by a caller
    /// that is about to sleep.
    pub fn seen(&self) -> u64 {
        self.changes.load(Ordering::SeqCst)
    }

    /// Note a change, under the table lock; `wake` it once the lock is gone.
    pub fn note_change(&self) {
        self.changes.fetch_add(1, Ordering::SeqCst);
    }

    /// Note the object's removal, under the table lock.
    pub fn note_removed(&self) {
        self.removed.store(true, Ordering::SeqCst);
        self.note_change();
    }

    pub fn is_removed(&self) -> bool {
        self.removed.load(Ordering::SeqCst)
    }

    /// Whether a call that slept on `slept` must fail with EIDRM on finding
    /// `found` by the same identifier: the object it slept on was removed,
    /// or the identifier now names another object, which it does once the
    /// sequence numbers wrap. Linux checks the object it slept on in the
    /// same way, not the identifier.
    pub fn gone(slept: &Option<Arc<Waiters>>, found: Option<&Arc<Waiters>>) -> bool {
        slept.as_ref().is_some_and(|w| w.is_removed() || found.is_none_or(|f| !Arc::ptr_eq(w, f)))
    }

    /// Wake every sleeper. Called with no table locked.
    pub fn wake(&self) {
        self.queue.wake_up();
    }

    /// Owe the sleepers a wake, for a caller that holds PROCESS_MANAGER and
    /// so may not wake them: `wake_deferred` does, later.
    fn owe_wake(&self) {
        self.wake_owed.store(true, Ordering::SeqCst);
        WAKES_OWED.store(true, Ordering::SeqCst);
    }

    fn take_owed_wake(&self) -> bool {
        self.wake_owed.swap(false, Ordering::SeqCst)
    }

    /// Sleep until a change after `seen`, the object's removal, `deadline`
    /// or a signal.
    pub fn wait(&self, seen: u64, deadline: &IpcDeadline) -> WaitEnd {
        crate::ipc::wait::wait_on(&self.queue, deadline, || self.seen() == seen && !self.is_removed())
    }
}

/// The errno of msgsnd and msgrcv when their wait ends with `error`: ENOMEM
/// when the wait could not reserve its place on the queue; otherwise as
/// Linux's ERESTARTNOHAND gives it: EINTR when a pending signal will run a
/// handler or kill the process, whatever SA_RESTART says, and ERESTARTSYS
/// for anything else, which the return path turns into the call made again
/// (after a job-control stop, once the process continues). The answer rests
/// on the pending signals, not on the stop state, which a SIGCONT's default
/// action can clear before this runs.
pub(crate) fn interrupted_unless_handled(error: u64) -> i32 {
    if error == ENOMEM as u64 {
        return ENOMEM;
    }
    let handled = crate::task::scheduler::current_thread_id().is_some_and(|tid| {
        crate::process::with_process_manager(|manager| {
            manager.find_process_by_thread(tid).is_some_and(|(_, p)| {
                let deliverable = p.signals.pending_set() & !p.signals.blocked();
                (1..=NSIG).filter(|&sig| deliverable & (1u64 << (sig - 1)) != 0).any(|sig| {
                    let action = p.signals.get_handler(sig);
                    action.is_handler()
                        || (action.handler == SIG_DFL && crate::signal::delivery::fatal_exit_code(sig).is_some())
                })
            })
        })
        .unwrap_or(false)
    });
    if handled {
        EINTR
    } else {
        ERESTARTSYS
    }
}

/// Wake the sleepers of each semaphore set whose undo adjustments were
/// applied under PROCESS_MANAGER. Called with no lock held; allocates
/// nothing.
pub(crate) fn wake_deferred() {
    if !WAKES_OWED.swap(false, Ordering::SeqCst) {
        return;
    }
    loop {
        let owed = {
            let table = sem::SYSV_SEM.lock();
            let found = table.ids.iter().find_map(|(_, e)| e.obj.waiters.take_owed_wake().then(|| e.obj.waiters.clone()));
            found
        };
        match owed {
            Some(w) => w.wake(),
            None => return,
        }
    }
}

/// Task `row`, of process `pid`, drops its SEM_UNDO list; when it held the
/// last reference, the list's adjustments are applied, held to the
/// semaphores' range, and the processes waiting on those sets are woken.
/// `under_pm` when the caller holds PROCESS_MANAGER, which the SEM table
/// nests inside: nothing holding the SEM table takes PROCESS_MANAGER, and
/// the wakes are then owed rather than made.
fn task_exit(row: u64, pid: i32, under_pm: bool) {
    let Some(list) = sem::SYSV_SEM.lock().undo.leave(row) else { return };
    // One set at a time, waking its sleepers with the table unlocked, so
    // that exit allocates nothing.
    loop {
        let t = now();
        let woken = {
            let mut guard = sem::SYSV_SEM.lock();
            let sem::SemTable { ids, undo } = &mut *guard;
            let Some((semid, adj)) = undo.take(list) else { return };
            ids.get_mut(semid).map(|entry| {
                entry.obj.set.apply_undo(&adj, pid, t);
                entry.obj.waiters.note_change();
                entry.obj.waiters.clone()
            })
        };
        if let Some(w) = woken {
            if under_pm {
                w.owe_wake();
            } else {
                w.wake();
            }
        }
    }
}

/// Row `row`'s task and its process, read from PROCESS_MANAGER.
fn task_of(row: ProcessId) -> Option<(u64, i32)> {
    crate::process::with_process_manager(|manager| {
        let p = manager.get_process(row)?;
        Some((row.as_u64(), p.thread_group_id.unwrap_or(row.as_u64()) as i32))
    })
    .flatten()
}

/// Row `pid` is about to end: its task leaves its SEM_UNDO list, before the
/// commit that makes the row a zombie its parent can reap, as on Linux,
/// where exit_sem runs before exit_notify. Called with no lock held.
pub fn row_exit(pid: ProcessId) {
    wake_deferred();
    if let Some((row, tgid)) = task_of(pid) {
        task_exit(row, tgid, false);
    }
}

/// Row `pid` has ended: a SEM_UNDO list its task joined after `row_exit` ran
/// (a task killed from outside can still be inside a semop when its exit
/// begins) is left now. Called with no lock held.
pub fn row_exited(pid: ProcessId) {
    row_exit(pid);
}

/// Thread `thread_id` is about to exit: its row's task leaves its SEM_UNDO
/// list, as `row_exit`. The row may already be marked terminated, as a fatal
/// signal on ARM64 leaves it. Called with no lock held.
pub fn thread_exit(thread_id: u64) {
    wake_deferred();
    let task = crate::process::with_process_manager(|manager| {
        manager
            .find_process_by_thread(thread_id)
            .map(|(row, p)| (row.as_u64(), p.thread_group_id.unwrap_or(row.as_u64()) as i32))
    })
    .flatten();
    if let Some((row, tgid)) = task {
        task_exit(row, tgid, false);
    }
}

/// A fatal signal is about to mark `process` terminated, with
/// PROCESS_MANAGER held, which makes the row a zombie its parent can reap
/// as soon as the lock goes: its task leaves its SEM_UNDO list first. The
/// sets' sleepers are woken once the lock is gone, by the exit hook the
/// deferred-exit drain runs for this row.
pub fn row_dying_locked(process: &Process) {
    let row = process.id.as_u64();
    task_exit(row, process.thread_group_id.unwrap_or(row) as i32, true);
}

/// A semop with SEM_UNDO by task `row` of process `pid` has finished: if
/// the row ended while the call ran, after its exit hooks, the list it
/// rejoined is left now.
pub(crate) fn semop_finished(row: u64, pid: i32) {
    let ended = crate::process::with_process_manager(|manager| {
        manager.get_process(ProcessId::new(row)).is_none_or(|p| p.is_terminated())
    })
    .unwrap_or(true);
    if ended {
        task_exit(row, pid, false);
    }
}

/// clone with CLONE_SYSVSEM, with PROCESS_MANAGER held and before `child`
/// is published: the new task shares its creator's SEM_UNDO list. ENOMEM.
pub fn share_undo(parent: ProcessId, child: ProcessId) -> Result<(), i32> {
    sem::SYSV_SEM.lock().undo.share(parent.as_u64(), child.as_u64())
}

/// An exec moved task `old`'s row to process ID `new`
/// (`ProcessManager::exec_take_over_group`), with PROCESS_MANAGER held: the
/// task keeps its SEM_UNDO list under its new ID.
pub fn rename_undo(old: ProcessId, new: ProcessId) {
    sem::SYSV_SEM.lock().undo.rename(old.as_u64(), new.as_u64());
}

/// Undo `share_undo` for a clone that failed before publishing `child`; the
/// creator still refers to the list, so nothing is applied.
pub fn unshare_undo(child: ProcessId) {
    let _ = sem::SYSV_SEM.lock().undo.leave(child.as_u64());
}
