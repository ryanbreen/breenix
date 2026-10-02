//! POSIX advisory record locks (fcntl `F_GETLK`, `F_SETLK`, `F_SETLKW`).
//!
//! Locks are owned by a process and name a byte range of a file, identified by
//! its mount and inode, so every descriptor a process has for the file sees the
//! same locks. Read locks are shared and write locks are exclusive between
//! owners; an owner's own locks never conflict with each other, and a new lock
//! replaces whatever the owner held over its range. All of an owner's locks on
//! a file go when it closes any descriptor for that file, and all of its locks
//! go when it exits.
//!
//! The table is one spin mutex taken with local interrupts masked. A blocked
//! `F_SETLKW` publishes itself on `WAITERS` while holding it, so a release,
//! which changes the table under the same mutex before it wakes, cannot be
//! lost between the waiter's conflict check and its sleep.

use alloc::vec::Vec;

use crate::syscall::errno;
use crate::task::thread::ThreadState;
use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};

/// `l_type` values of `struct flock`.
pub const F_RDLCK: i16 = 0;
pub const F_WRLCK: i16 = 1;
pub const F_UNLCK: i16 = 2;

/// The last byte offset a lock can cover; a range ending here runs to the end
/// of the file however far it grows (`l_len == 0`).
pub const OFFSET_MAX: i64 = i64::MAX;

/// Deadlock detection follows at most this many owner-waits-for-owner edges.
const MAX_DEADLOCK_STEPS: usize = 10;

/// The file a lock is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileKey {
    pub mount_id: usize,
    pub inode: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    Read,
    Write,
}

impl LockKind {
    pub fn l_type(self) -> i16 {
        match self {
            LockKind::Read => F_RDLCK,
            LockKind::Write => F_WRLCK,
        }
    }
}

/// An inclusive byte range, `start <= end <= OFFSET_MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: i64,
    pub end: i64,
}

impl Range {
    fn overlaps(&self, other: &Range) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// A lock another owner holds that blocks the requested one.
#[derive(Clone, Copy, Debug)]
pub struct Conflict {
    pub kind: LockKind,
    pub range: Range,
    pub owner: u64,
}

#[derive(Clone, Copy, Debug)]
struct RecordLock {
    key: FileKey,
    owner: u64,
    kind: LockKind,
    range: Range,
}

/// A thread sleeping in `F_SETLKW`: its owner waits for `blocker`.
#[derive(Clone, Copy, Debug)]
struct Blocked {
    tid: u64,
    owner: u64,
    blocker: u64,
}

struct LockTable {
    locks: Vec<RecordLock>,
    blocked: Vec<Blocked>,
}

impl LockTable {
    fn conflict(&self, key: FileKey, owner: u64, kind: LockKind, range: Range) -> Option<Conflict> {
        self.locks
            .iter()
            .find(|l| {
                l.key == key
                    && l.owner != owner
                    && l.range.overlaps(&range)
                    && (l.kind == LockKind::Write || kind == LockKind::Write)
            })
            .map(|l| Conflict {
                kind: l.kind,
                range: l.range,
                owner: l.owner,
            })
    }

    /// Replace `owner`'s locks over `range` with `kind` (`None` unlocks).
    /// Returns whether any lock the owner held was removed or changed.
    fn apply(&mut self, key: FileKey, owner: u64, kind: Option<LockKind>, range: Range) -> bool {
        let mut changed = false;
        let mut kept = Vec::with_capacity(self.locks.len() + 2);
        for l in self.locks.drain(..) {
            if l.key != key || l.owner != owner || !l.range.overlaps(&range) {
                kept.push(l);
                continue;
            }
            changed = true;
            if l.range.start < range.start {
                kept.push(RecordLock {
                    range: Range {
                        start: l.range.start,
                        end: range.start - 1,
                    },
                    ..l
                });
            }
            if l.range.end > range.end {
                kept.push(RecordLock {
                    range: Range {
                        start: range.end + 1,
                        end: l.range.end,
                    },
                    ..l
                });
            }
        }
        self.locks = kept;

        if let Some(kind) = kind {
            // Coalesce with the owner's same-kind locks that touch the range,
            // so F_GETLK reports the region as one lock, as it was requested.
            let mut merged = range;
            self.locks.retain(|l| {
                let touches = l.range.start <= merged.end.saturating_add(1)
                    && merged.start <= l.range.end.saturating_add(1);
                if l.key == key && l.owner == owner && l.kind == kind && touches {
                    merged.start = merged.start.min(l.range.start);
                    merged.end = merged.end.max(l.range.end);
                    false
                } else {
                    true
                }
            });
            self.locks.push(RecordLock {
                key,
                owner,
                kind,
                range: merged,
            });
        }
        changed
    }

    /// Whether `owner` waiting for `blocker` closes a cycle of waiting owners.
    fn would_deadlock(&self, owner: u64, mut blocker: u64) -> bool {
        for _ in 0..MAX_DEADLOCK_STEPS {
            if blocker == owner {
                return true;
            }
            match self.blocked.iter().find(|b| b.owner == blocker) {
                Some(b) => blocker = b.blocker,
                None => return false,
            }
        }
        false
    }
}

static TABLE: spin::Mutex<LockTable> = spin::Mutex::new(LockTable {
    locks: Vec::new(),
    blocked: Vec::new(),
});

/// Threads sleeping in `F_SETLKW`. Every release wakes them all; each re-checks.
static WAITERS: WaitQueueHead = WaitQueueHead::new();

fn with_table<R>(f: impl FnOnce(&mut LockTable) -> R) -> R {
    crate::arch_without_interrupts(|| f(&mut TABLE.lock()))
}

/// Wake `F_SETLKW` sleepers after the table changed. A caller holding the
/// process-manager lock must not take the scheduler lock, so it wakes through
/// the deferred path.
fn wake_waiters() {
    if !WAITERS.has_waiters() {
        return;
    }
    if crate::process::process_manager_held_on_current_cpu() {
        WAITERS.wake_up_deferred();
    } else {
        WAITERS.wake_up();
    }
}

/// `F_GETLK`: the first lock of another owner that would block `kind` over
/// `range`, if any.
pub fn get(key: FileKey, owner: u64, kind: LockKind, range: Range) -> Option<Conflict> {
    with_table(|t| t.conflict(key, owner, kind, range))
}

/// What one pass over the table decided for `set`.
enum Step {
    /// The request was applied; whether it removed or changed a held lock.
    Applied(bool),
    /// The caller was published on `WAITERS` and must sleep, then retry.
    Wait(PrepareOutcome),
}

/// `F_SETLK` (`wait == false`) and `F_SETLKW` (`wait == true`). `kind` of
/// `None` unlocks. Returns an errno: `EAGAIN` for a conflict without waiting,
/// `EDEADLK` when waiting would deadlock, `EINTR` when a signal ends the wait.
pub fn set(
    key: FileKey,
    owner: u64,
    kind: Option<LockKind>,
    range: Range,
    wait: bool,
) -> Result<(), i32> {
    let tid = crate::task::scheduler::current_thread_id().unwrap_or(0);
    loop {
        let step = with_table(|t| {
            let conflict = kind.and_then(|k| t.conflict(key, owner, k, range));
            match conflict {
                None => Ok(Step::Applied(t.apply(key, owner, kind, range))),
                Some(_) if !wait => Err(errno::EAGAIN),
                Some(c) if t.would_deadlock(owner, c.owner) => Err(errno::EDEADLK),
                Some(c) => {
                    t.blocked.push(Blocked {
                        tid,
                        owner,
                        blocker: c.owner,
                    });
                    Ok(Step::Wait(WAITERS.prepare_to_wait_checked(
                        ThreadState::BlockedOnIO,
                        None,
                        || true,
                    )))
                }
            }
        })?;
        match step {
            Step::Applied(changed) => {
                if changed {
                    wake_waiters();
                }
                return Ok(());
            }
            Step::Wait(outcome) => {
                // The table guard is gone before preemption, signals or sleeping.
                let woke = crate::syscall::blocking_io::wait_prepared(&WAITERS, outcome);
                with_table(|t| t.blocked.retain(|b| b.tid != tid));
                woke?;
            }
        }
    }
}

/// Drop every lock `owner` holds on `key`: it closed a descriptor for the file.
pub fn release_file(owner: u64, key: FileKey) {
    let changed = with_table(|t| {
        let before = t.locks.len();
        t.locks.retain(|l| !(l.owner == owner && l.key == key));
        t.locks.len() != before
    });
    if changed {
        wake_waiters();
    }
}

/// Drop every lock `owner` holds, and any wait it was in: it exited.
pub fn release_owner(owner: u64) {
    let changed = with_table(|t| {
        let before = t.locks.len();
        t.locks.retain(|l| l.owner != owner);
        t.blocked.retain(|b| b.owner != owner);
        t.locks.len() != before
    });
    if changed {
        wake_waiters();
    }
}

/// Drop `owner`'s locks on the file a descriptor it just closed referred to.
pub fn release_closed(owner: u64, kind: &crate::ipc::FdKind) {
    if let crate::ipc::FdKind::RegularFile(file) = kind {
        let key = {
            let file = file.lock();
            FileKey {
                mount_id: file.mount_id,
                inode: file.inode_num,
            }
        };
        release_file(owner, key);
    }
}
