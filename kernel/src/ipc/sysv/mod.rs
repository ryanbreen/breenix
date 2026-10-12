//! System V IPC (XSI): message queues, semaphore sets and shared memory
//! segments, named by keys and identifiers.
//!
//! Each class keeps its objects in one table behind an `IrqSafeMutex`
//! (`msg::SYSV_MSG`). A table is a leaf: no
//! code holding one takes PROCESS_MANAGER, the scheduler or user memory, so
//! callers read their credentials and copy user data in before locking, copy
//! out after unlocking, and wake sleepers after unlocking. A sleeper waits on
//! its object's `Waiters`, which outlives the object's removal.

pub mod msg;
pub mod state;

use crate::fs::permissions::Credentials;
use crate::ipc::wait::{IpcDeadline, WaitEnd};
use crate::syscall::SyscallResult;
use crate::task::waitqueue::WaitQueueHead;
use alloc::sync::Arc;
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
