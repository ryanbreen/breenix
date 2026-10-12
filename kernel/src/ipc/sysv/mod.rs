//! System V IPC (XSI): message queues, semaphore sets and shared memory
//! segments, named by keys and identifiers.
//!
//! Each class keeps its objects in one table behind an `IrqSafeMutex`
//! (`msg::SYSV_MSG`, `sem::SYSV_SEM`). A table is a leaf: no
//! code holding one takes PROCESS_MANAGER, the scheduler or user memory, so
//! callers read their credentials and copy user data in before locking, copy
//! out after unlocking, and wake sleepers after unlocking. A sleeper waits on
//! its object's `Waiters`, which outlives the object's removal.

pub mod msg;
pub mod sem;
pub mod state;

use crate::fs::permissions::Credentials;
use crate::ipc::wait::{IpcDeadline, WaitEnd};
use crate::process::ProcessId;
use crate::syscall::SyscallResult;
use crate::task::waitqueue::WaitQueueHead;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub use state::IpcPerm;

/// Whether `cred` is granted every bit of `wanted` (state::READ, state::WRITE)
/// on an object with `perm`: POSIX's owner, group and other classes, where
/// the creator counts as an owner and the creator's group as the group, and
/// an effective user of 0 is always granted.
pub(crate) fn ipcperms(perm: &IpcPerm, cred: &Credentials, wanted: u32) -> bool {
    perm.permits(cred.euid, |gid| cred.in_group(gid), wanted)
}

/// The calling process, read before any table is locked: credentials come
/// from PROCESS_MANAGER.
pub(crate) struct Caller {
    pub cred: Credentials,
    /// The process ID (thread group ID) that getpid returns.
    pub pid: i32,
}

pub(crate) fn caller() -> Caller {
    let cred = Credentials::current(false);
    let pid = match crate::syscall::handlers::sys_getpid() {
        SyscallResult::Ok(pid) => pid as i32,
        SyscallResult::Err(_) => 0,
    };
    Caller { cred, pid }
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
}

impl Waiters {
    pub fn new() -> Arc<Waiters> {
        Arc::new(Waiters { queue: WaitQueueHead::new(), changes: AtomicU64::new(0), removed: AtomicBool::new(false) })
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

    /// Wake every sleeper. Called with no table locked.
    pub fn wake(&self) {
        self.queue.wake_up();
    }

    /// Sleep until a change after `seen`, the object's removal, `deadline`
    /// or a signal.
    pub fn wait(&self, seen: u64, deadline: &IpcDeadline) -> WaitEnd {
        crate::ipc::wait::wait_on(&self.queue, deadline, || self.seen() == seen && !self.is_removed())
    }
}

/// Wake each object's sleepers, with no table locked.
pub(crate) fn wake_all(woken: Vec<Arc<Waiters>>) {
    for w in woken {
        w.wake();
    }
}

/// Process `pid` has exited: apply its SEM_UNDO adjustments, held to the
/// semaphores' range, wake the processes waiting on those sets, and drop its
/// adjustments. Called with no lock held.
pub fn process_exit(pid: ProcessId) {
    sem::process_exit(pid.as_u64());
}

/// The row `find` picks, its thread group and the group's rows that have
/// not ended, or None when the row is missing or has already ended.
fn group_rows(
    find: impl FnOnce(&crate::process::ProcessManager) -> Option<ProcessId>,
) -> Option<(ProcessId, u64, Vec<u64>)> {
    crate::process::with_process_manager(|manager| {
        let pid = find(manager)?;
        let row = manager.get_process(pid)?;
        if row.is_terminated() {
            return None;
        }
        let group = row.thread_group_id.unwrap_or(pid.as_u64());
        let live = manager
            .iter_processes()
            .filter(|(peer, p)| !p.is_terminated() && p.thread_group_id.unwrap_or(peer.as_u64()) == group)
            .map(|(peer, _)| peer.as_u64())
            .collect();
        Some((pid, group, live))
    })
    .flatten()
}

/// A row is about to end, and nothing holds PROCESS_MANAGER. When it is the
/// last row of its thread group the process is exiting, and its SEM_UNDO
/// adjustments are applied here, before the row becomes a zombie its parent
/// can reap, as on Linux, where exit_sem runs before exit_notify.
fn row_ending(found: Option<(ProcessId, u64, Vec<u64>)>) {
    let Some((pid, group, live)) = found else { return };
    if sem::row_ending(group, pid.as_u64(), &live) {
        process_exit(ProcessId::new(group));
    }
}

/// Row `pid` is about to end: see `row_ending`.
pub fn row_exit(pid: ProcessId) {
    row_ending(group_rows(|_| Some(pid)));
}

/// Row `pid` has ended. When no row of its thread group still runs, apply
/// any SEM_UNDO adjustments made after `row_exit` ran: a process killed from
/// outside could still be inside a semop when its exit began.
pub fn row_exited(pid: ProcessId) {
    let group = crate::process::with_process_manager(|manager| {
        let row = manager.get_process(pid)?;
        let group = row.thread_group_id.unwrap_or(pid.as_u64());
        let running = manager
            .iter_processes()
            .any(|(peer, p)| !p.is_terminated() && p.thread_group_id.unwrap_or(peer.as_u64()) == group);
        (!running).then_some(group)
    })
    .flatten();
    if let Some(group) = group {
        process_exit(ProcessId::new(group));
    }
}

/// Thread `thread_id` is about to exit: see `row_ending`, for its row.
pub fn thread_exit(thread_id: u64) {
    row_ending(group_rows(|manager| manager.find_process_by_thread(thread_id).map(|(pid, _)| pid)));
}
