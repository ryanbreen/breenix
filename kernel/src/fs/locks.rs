//! POSIX advisory record locks (fcntl `F_GETLK`, `F_SETLK`, `F_SETLKW`).
//!
//! Locks are owned by a process and name a byte range of a file, identified by
//! its mount and inode, so every descriptor a process has for the file sees the
//! same locks. A process is a thread group: every row of the group shares one
//! `LockOwner`, whose id is the group's id, so sibling threads share their
//! locks. Read locks are shared and write locks are exclusive between owners;
//! an owner's own locks never conflict with each other, and a new lock
//! replaces whatever the owner held over its range. All of an owner's locks on
//! a file go when any of its rows closes a descriptor for that file, and all
//! of its locks go when its last row terminates.
//!
//! The table is one spin mutex taken with local interrupts masked. A blocked
//! `F_SETLKW` publishes itself on `WAITERS` while holding it, so a release,
//! which changes the table under the same mutex before it wakes, cannot be
//! lost between the waiter's conflict check and its sleep. Every release that
//! wakes the waiters also drops their wait-for edges in the same critical
//! section: a woken waiter no longer depends on anyone until it re-checks and,
//! still blocked, records its edge again.
//!
//! The table is bounded: it holds at most `MAX_RECORDS` locks, an owner holds
//! at most `MAX_RECORDS_PER_OWNER`, and its vectors only grow through fallible
//! reservations. A request that would pass a bound or cannot get memory fails
//! with `ENOLCK`. A request that does not add records never allocates, so
//! unlocking (other than splitting a lock in two) and every release on close
//! or exit always succeed.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

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

/// Most locks the table holds across all owners.
const MAX_RECORDS: usize = 4096;

/// Most locks one owner holds across all files.
const MAX_RECORDS_PER_OWNER: usize = 1024;

/// The lock owner of a thread group, shared by every row of the group.
///
/// `members` counts the group's rows that have not terminated. The last one to
/// terminate releases the owner's locks.
pub struct LockOwner {
    id: u64,
    members: AtomicUsize,
}

impl LockOwner {
    /// The owner of a new process, whose one row is `id`.
    pub fn new(id: u64) -> Arc<Self> {
        Arc::new(Self {
            id,
            members: AtomicUsize::new(1),
        })
    }

    /// The id locks are recorded under and `F_GETLK` reports as `l_pid`.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// How many of the group's rows have not terminated. Read under
    /// PROCESS_MANAGER, which every change to the count is made under.
    pub fn live_rows(&self) -> usize {
        self.members.load(Ordering::Acquire)
    }

    /// A new row joined the group (a thread was created).
    pub fn join(&self) {
        self.members.fetch_add(1, Ordering::AcqRel);
    }

    /// A row of the group terminated. The last one releases the owner's
    /// locks. Called exactly once per joined row, under PROCESS_MANAGER like
    /// every other change to `members`.
    pub fn leave(&self) {
        if self.members.fetch_sub(1, Ordering::AcqRel) == 1 {
            release_owner(self.id);
        }
    }

    /// A row left the group to become a process of its own, now owner `id`
    /// (exec). If it was the group's last live row, the group's locks are its
    /// own and move to `id`; otherwise this is an ordinary `leave`.
    pub fn hand_over(&self, id: u64) {
        if self
            .members
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            rekey(self.id, id);
        } else {
            self.leave();
        }
    }
}

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
    /// Returns whether any lock the owner held was removed or changed, or
    /// `ENOLCK` when the result would pass a bound or memory is short; the
    /// table is unchanged on error.
    ///
    /// An owner's locks on one file never overlap, so at most one of them
    /// reaches below `range` and one above it; those leave a remnant each.
    fn apply(
        &mut self,
        key: FileKey,
        owner: u64,
        kind: Option<LockKind>,
        range: Range,
    ) -> Result<bool, i32> {
        let mine = |l: &RecordLock| l.key == key && l.owner == owner;

        let mut removed = 0;
        let mut below: Option<RecordLock> = None;
        let mut above: Option<RecordLock> = None;
        for l in self.locks.iter().filter(|l| mine(l) && l.range.overlaps(&range)) {
            removed += 1;
            if l.range.start < range.start {
                below = Some(RecordLock {
                    range: Range {
                        start: l.range.start,
                        end: range.start - 1,
                    },
                    ..*l
                });
            }
            if l.range.end > range.end {
                above = Some(RecordLock {
                    range: Range {
                        start: range.end + 1,
                        end: l.range.end,
                    },
                    ..*l
                });
            }
        }

        // Coalesce the new lock with the owner's same-kind locks that touch
        // it, so F_GETLK reports the region as one lock, as it was requested.
        // A touching lock is a remnant or an untouched lock that ends just
        // below the range or starts just above it.
        let mut new = kind.map(|kind| RecordLock {
            key,
            owner,
            kind,
            range,
        });
        let mut absorbed_below: Option<Range> = None;
        let mut absorbed_above: Option<Range> = None;
        if let Some(n) = new.as_mut() {
            match below {
                Some(b) if b.kind == n.kind => {
                    n.range.start = b.range.start;
                    below = None;
                }
                Some(_) => {}
                None if range.start > 0 => {
                    if let Some(l) = self.locks.iter().find(|l| {
                        mine(l) && l.kind == n.kind && l.range.end == range.start - 1
                    }) {
                        n.range.start = l.range.start;
                        absorbed_below = Some(l.range);
                    }
                }
                None => {}
            }
            match above {
                Some(a) if a.kind == n.kind => {
                    n.range.end = a.range.end;
                    above = None;
                }
                Some(_) => {}
                None if range.end < OFFSET_MAX => {
                    if let Some(l) = self.locks.iter().find(|l| {
                        mine(l) && l.kind == n.kind && l.range.start == range.end + 1
                    }) {
                        n.range.end = l.range.end;
                        absorbed_above = Some(l.range);
                    }
                }
                None => {}
            }
        }

        let gone =
            removed + usize::from(absorbed_below.is_some()) + usize::from(absorbed_above.is_some());
        let added =
            usize::from(below.is_some()) + usize::from(above.is_some()) + usize::from(new.is_some());
        if added > gone {
            let growth = added - gone;
            let held = self.locks.iter().filter(|l| l.owner == owner).count();
            if self.locks.len() + growth > MAX_RECORDS || held + growth > MAX_RECORDS_PER_OWNER {
                return Err(errno::ENOLCK);
            }
            self.locks
                .try_reserve(growth)
                .map_err(|_| errno::ENOLCK)?;
        }

        // In place: when nothing grows, the pushes below reuse the slots the
        // retain freed, so this never allocates.
        if gone > 0 {
            self.locks.retain(|l| {
                !(mine(l)
                    && (l.range.overlaps(&range)
                        || Some(l.range) == absorbed_below
                        || Some(l.range) == absorbed_above))
            });
        }
        self.locks.extend(below);
        self.locks.extend(above);
        self.locks.extend(new);
        Ok(removed > 0)
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

    /// The table changed in a way that may unblock a waiter. Every waiter is
    /// about to be woken and re-check, so none of them waits for anyone now;
    /// the caller wakes them once the table guard is gone.
    fn released(&mut self) {
        self.blocked.clear();
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

/// Whether thread `tid` has been terminated (killed). Its process's teardown
/// may already have released the owner's locks, so it must not take new ones.
fn terminated(tid: u64) -> bool {
    crate::task::scheduler::with_thread_mut(tid, |thread| {
        thread.state == ThreadState::Terminated
    })
    .unwrap_or(true)
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
/// `EDEADLK` when waiting would deadlock, `EINTR` when a signal ends the wait
/// or the caller has been killed, `ENOLCK` when the table is full.
pub fn set(
    key: FileKey,
    owner: u64,
    kind: Option<LockKind>,
    range: Range,
    wait: bool,
) -> Result<(), i32> {
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return Err(errno::ESRCH);
    };
    loop {
        let step = with_table(|t| {
            // Checked under the table lock: a kill marks the thread terminated
            // before its process's teardown releases the owner's locks under
            // this lock, so either that release runs after this request and
            // removes what it installs, or this request sees the kill.
            if terminated(tid) {
                return Err(errno::EINTR);
            }
            let conflict = kind.and_then(|k| t.conflict(key, owner, k, range));
            match conflict {
                None => {
                    let changed = t.apply(key, owner, kind, range)?;
                    if changed {
                        t.released();
                    }
                    Ok(Step::Applied(changed))
                }
                Some(_) if !wait => Err(errno::EAGAIN),
                Some(c) if t.would_deadlock(owner, c.owner) => Err(errno::EDEADLK),
                Some(c) => {
                    t.blocked.try_reserve(1).map_err(|_| errno::ENOLCK)?;
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

/// Drop every lock `owner` holds on `key`: one of its rows closed a
/// descriptor for the file.
pub fn release_file(owner: u64, key: FileKey) {
    let changed = with_table(|t| {
        let before = t.locks.len();
        t.locks.retain(|l| !(l.owner == owner && l.key == key));
        let changed = t.locks.len() != before;
        if changed {
            t.released();
        }
        changed
    });
    if changed {
        wake_waiters();
    }
}

/// Drop every lock `owner` holds, and any wait it was in: its last row
/// terminated.
fn release_owner(owner: u64) {
    let changed = with_table(|t| {
        let before = t.locks.len();
        t.locks.retain(|l| l.owner != owner);
        t.blocked.retain(|b| b.owner != owner);
        let changed = t.locks.len() != before;
        if changed {
            t.released();
        }
        changed
    });
    if changed {
        wake_waiters();
    }
}

/// Move every lock and wait recorded under `from` to `to`.
fn rekey(from: u64, to: u64) {
    with_table(|t| {
        for l in t.locks.iter_mut().filter(|l| l.owner == from) {
            l.owner = to;
        }
        for b in t.blocked.iter_mut() {
            if b.owner == from {
                b.owner = to;
            }
            if b.blocker == from {
                b.blocker = to;
            }
        }
    });
}

/// Thread `tid` terminated. If it was killed in `F_SETLKW` it never runs the
/// wait's own cleanup, so drop its wait-for edge and its wait-queue entry.
pub fn release_thread(tid: u64) {
    with_table(|t| t.blocked.retain(|b| b.tid != tid));
    WAITERS.take_waiter(tid);
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
