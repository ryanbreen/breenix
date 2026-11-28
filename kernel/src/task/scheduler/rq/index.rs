use super::*;
use crate::task::thread::{ThreadPrivilege, ThreadWake};
use crate::task::wake_inbox::WakeNode;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU8, AtomicUsize};

// ------------------------------------------------------------ thread index

/// Slots of the thread index. Thread IDs are handed out in order, so a tid
/// hashes to its own slot until the IDs wrap the table; probing is linear.
const SLOTS: usize = 4096;

/// A slot's key is the tid plus one, so tid 0 (an idle thread) is a key.
pub(super) const EMPTY: u64 = 0;
/// A removed entry: lookups probe past it, inserts reuse it.
pub(super) const REMOVED: u64 = u64::MAX;
/// An entry an insert is filling in: lookups probe past it.
const CLAIMED: u64 = u64::MAX - 1;

/// Slots kept for kernel threads, which are indexed without a reservation:
/// a fork or clone is refused once only these are left (`reserve`).
const KERNEL_RESERVE: u32 = 256;

/// Slots indexed, plus those reserved for threads a fork or clone is still
/// creating (`reserve`).
static TAKEN: AtomicU32 = AtomicU32::new(0);

/// Reserve a slot for a user thread about to be created, as Linux's
/// `copy_process` refuses a task past `max_threads` with EAGAIN, so creating
/// threads cannot fill the index. False when only the kernel's reserve is
/// left. The caller gives the reservation back (`unreserve`) once the thread
/// is indexed, or once it is not created: the slot counts twice meanwhile.
pub(crate) fn reserve() -> bool {
    TAKEN
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |taken| {
            (taken < SLOTS as u32 - KERNEL_RESERVE).then_some(taken + 1)
        })
        .is_ok()
}

/// Give back a reservation `reserve` made.
pub(crate) fn unreserve() {
    TAKEN.fetch_sub(1, Ordering::AcqRel);
}

/// `Slot::flags`: the thread runs user code.
pub(super) const USER: u8 = 1;
/// `Slot::flags`: the thread has terminated.
pub(super) const DEAD: u8 = 2;

pub(super) struct Slot {
    pub(super) key: AtomicU64,
    pub(super) thread: AtomicPtr<Thread>,
    pub(super) wake: AtomicPtr<ThreadWake>,
    pub(super) owner: AtomicU32,
    /// The page-table root the thread caches (`Thread::cached_ttbr0`) while
    /// it has not terminated, and 0 once it has: what `any_cached_root` reads,
    /// with no lock.
    pub(super) root: AtomicU64,
    /// The process row the thread belongs to (`Thread::owner_pid`), 0 for none.
    pub(super) pid: AtomicU64,
    /// `USER` and `DEAD`.
    pub(super) flags: AtomicU8,
    /// The address of the thread's interval timers (`Thread::signal_timers`),
    /// 0 for none: which process's timers it may receive.
    pub(super) group: AtomicUsize,
    /// The thread's scheduling policy without its run-time fields
    /// (`pack_policy`), so its rank and policy are read without a lock.
    pub(super) policy: AtomicU64,
}

pub(super) static INDEX: [Slot; SLOTS] = [const {
    Slot {
        key: AtomicU64::new(EMPTY),
        thread: AtomicPtr::new(core::ptr::null_mut()),
        wake: AtomicPtr::new(core::ptr::null_mut()),
        owner: AtomicU32::new(0),
        root: AtomicU64::new(0),
        pid: AtomicU64::new(0),
        flags: AtomicU8::new(0),
        group: AtomicUsize::new(0),
        policy: AtomicU64::new(0),
    }
}; SLOTS];

fn home(tid: u64) -> usize {
    tid as usize % SLOTS
}

pub(super) fn find(tid: u64) -> Option<&'static Slot> {
    let key = tid.wrapping_add(1);
    let start = home(tid);
    for probe in 0..SLOTS {
        let slot = &INDEX[(start + probe) % SLOTS];
        match slot.key.load(Ordering::Acquire) {
            found if found == key => return Some(slot),
            EMPTY => return None,
            _ => {}
        }
    }
    None
}

/// Whether a slot's key names a thread.
pub(super) fn live_key(key: u64) -> bool {
    key != EMPTY && key != REMOVED && key != CLAIMED
}

/// `thread`'s policy, packed: policy, priority, inherited rank, ceiling rank,
/// and bit 32 `reset_on_fork`, bit 33 `pi_exited`, bit 63 set.
fn pack_policy(sched: &SchedPolicy) -> u64 {
    sched.policy as u64
        | (sched.priority as u64) << 8
        | (sched.inherited as u64) << 16
        | (sched.ceiling as u64) << 24
        | (sched.reset_on_fork as u64) << 32
        | (sched.pi_exited as u64) << 33
        | 1 << 63
}

fn unpack_policy(word: u64) -> SchedPolicy {
    SchedPolicy {
        policy: word as u8,
        priority: (word >> 8) as u8,
        inherited: (word >> 16) as u8,
        ceiling: (word >> 24) as u8,
        reset_on_fork: word & 1 << 32 != 0,
        pi_exited: word & 1 << 33 != 0,
        ..SchedPolicy::DEFAULT
    }
}

pub(super) fn group_of(thread: &Thread) -> usize {
    thread
        .signal_timers
        .as_ref()
        .map_or(0, |timers| alloc::sync::Arc::as_ptr(timers) as usize)
}

/// Write what the index publishes about `thread` into `slot`.
fn fill(slot: &Slot, thread: &mut Thread, owner: usize) {
    let dead = thread.state == ThreadState::Terminated;
    let flags = if thread.privilege == ThreadPrivilege::User {
        USER
    } else {
        0
    } | if dead { DEAD } else { 0 };
    slot.thread.store(thread, Ordering::SeqCst);
    slot.owner.store(owner as u32, Ordering::Release);
    slot.root.store(
        if dead { 0 } else { thread.cached_ttbr0 },
        Ordering::Release,
    );
    slot.pid
        .store(thread.owner_pid.unwrap_or(0), Ordering::Release);
    slot.flags.store(flags, Ordering::Release);
    slot.group.store(group_of(thread), Ordering::Release);
    slot.policy
        .store(pack_policy(&thread.sched), Ordering::Release);
}

/// Index `thread`, owned by `owner`. No lock is needed: each insert claims
/// its slot, and a lookup sees the thread once its key is stored. The thread
/// is not queued yet, so nothing else reaches it meanwhile.
pub(in crate::task::scheduler) fn insert(thread: &mut Thread, owner: usize) {
    let tid = thread.id();
    if let Some(slot) = find(tid) {
        fill(slot, thread, owner);
        return;
    }
    let start = home(tid);
    for probe in 0..SLOTS {
        let slot = &INDEX[(start + probe) % SLOTS];
        let key = slot.key.load(Ordering::Acquire);
        if (key == EMPTY || key == REMOVED)
            && slot
                .key
                .compare_exchange(key, CLAIMED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            TAKEN.fetch_add(1, Ordering::AcqRel);
            let wake = Arc::new(ThreadWake::new(tid));
            slot.wake
                .store(Arc::as_ptr(&wake).cast_mut(), Ordering::SeqCst);
            thread.wake = Some(wake);
            fill(slot, thread, owner);
            slot.key.store(tid.wrapping_add(1), Ordering::Release);
            return;
        }
    }
    panic!(
        "scheduler: thread index full ({} slots) adding thread {}",
        SLOTS, tid
    );
}

/// Drop `tid` from the index. The scheduler-wide lock and the run-queue lock
/// of the CPU that owns the thread are held, or every lock: no other CPU then
/// holds a reference to the thread taken under that lock. A reader that took
/// one from the index without a lock holds a `Pin`, and the caller frees the
/// thread only once none does (`unpinned`).
pub(in crate::task::scheduler) fn remove(tid: u64) {
    if let Some(slot) = find(tid) {
        let wake = slot.wake.swap(core::ptr::null_mut(), Ordering::SeqCst);
        // SAFETY: this caller still owns the indexed thread under its lock.
        if let Some(wake) = unsafe { wake.as_ref() } {
            release_held_place(wake.node.close());
        }
        slot.root.store(0, Ordering::Release);
        slot.group.store(0, Ordering::Release);
        slot.pid.store(0, Ordering::Release);
        slot.flags.store(0, Ordering::Release);
        slot.thread.store(core::ptr::null_mut(), Ordering::SeqCst);
        slot.key.store(REMOVED, Ordering::Release);
        TAKEN.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Publish `root` as the page-table root `thread` caches, 0 once it has
/// terminated. The caller writes the thread under its owner's lock, or the
/// whole scheduler. A thread not in the live index (not yet added, or one a
/// test builds) publishes nothing: `insert` reads its root.
pub(in crate::task::scheduler) fn publish_root(thread: &Thread, root: u64) {
    if let Some(slot) = own_slot(thread) {
        slot.root.store(root, Ordering::Release);
    }
}

/// `thread`'s slot, if the live index holds this thread.
fn own_slot(thread: &Thread) -> Option<&'static Slot> {
    find(thread.id()).filter(|slot| core::ptr::eq(slot.thread.load(Ordering::Acquire), thread))
}

/// Publish that `thread` has terminated: no page-table root, the dead flag.
pub(in crate::task::scheduler) fn publish_terminated(thread: &Thread) {
    if let Some(slot) = own_slot(thread) {
        slot.root.store(0, Ordering::Release);
        slot.flags.fetch_or(DEAD, Ordering::AcqRel);
    }
}

/// Publish `thread`'s scheduling policy, after its owner's lock holder or the
/// whole scheduler changed it.
pub(in crate::task::scheduler) fn publish_policy(thread: &Thread) {
    if let Some(slot) = own_slot(thread) {
        slot.policy
            .store(pack_policy(&thread.sched), Ordering::Release);
    }
}

/// Publish which interval timers `thread` receives signals of, after exec
/// gave it its new process's.
pub(in crate::task::scheduler) fn publish_group(thread: &Thread) {
    if let Some(slot) = own_slot(thread) {
        slot.group.store(group_of(thread), Ordering::Release);
    }
}

/// Publish the process row `thread` now belongs to (`Thread::owner_pid`).
pub(in crate::task::scheduler) fn publish_owner(thread: &Thread) {
    if let Some(slot) = own_slot(thread) {
        slot.pid
            .store(thread.owner_pid.unwrap_or(0), Ordering::Release);
    }
}

/// `tid`'s scheduling policy, without its run-time fields, read without a
/// lock.
pub(in crate::task::scheduler) fn policy(tid: u64) -> Option<SchedPolicy> {
    let word = find(tid)?.policy.load(Ordering::Acquire);
    (word != 0).then(|| unpack_policy(word))
}

/// The CPU that owns `tid`, read without any lock.
pub(in crate::task::scheduler) fn owner(tid: u64) -> Option<usize> {
    find(tid).map(|slot| slot.owner.load(Ordering::Acquire) as usize)
}

/// `tid`'s thread, if `cpu` owns it. The caller holds `cpu`'s run-queue lock,
/// so the thread can neither change owner nor be reclaimed while the
/// reference lives: both take that lock.
///
/// # Safety
/// The caller holds `RQ_LOCKS[cpu]` for as long as the reference is used,
/// and holds no other reference to the same thread.
pub(in crate::task::scheduler) unsafe fn owned_thread<'a>(
    tid: u64,
    cpu: usize,
) -> Option<&'a mut Thread> {
    let slot = find(tid)?;
    if slot.owner.load(Ordering::Acquire) as usize != cpu {
        return None;
    }
    let thread = slot.thread.load(Ordering::Acquire);
    // SAFETY: the slot names the thread while `cpu` owns it, and the caller
    // holds `cpu`'s lock, which reclamation also takes.
    unsafe { thread.as_mut() }
}

/// Record `cpu` as `tid`'s owner. The caller holds the run-queue locks of the
/// old and the new owner, or every one.
pub(in crate::task::scheduler) fn set_owner(tid: u64, cpu: usize) {
    if let Some(slot) = find(tid) {
        slot.owner.store(cpu as u32, Ordering::Release);
    }
}

// ------------------------------------------------------------------- pins

/// Readers holding a thread they took from the index without its owner's lock
/// (`Pin`).
static PINS: AtomicU64 = AtomicU64::new(0);

/// While a pin lives, no thread it reached through the index is freed:
/// reclamation drops a thread from the index first and frees it only once no
/// pin is held (`unpinned`), keeping it for a later pass otherwise. Only what
/// never changes after a thread is published, and atomics, may be read
/// through a pin: its owner's lock holder writes the rest.
pub(in crate::task::scheduler) struct Pin(());

impl Pin {
    pub(in crate::task::scheduler) fn new() -> Self {
        PINS.fetch_add(1, Ordering::SeqCst);
        Pin(())
    }

    /// `tid`'s thread, which stays allocated while the pin lives.
    pub(in crate::task::scheduler) fn thread(&self, tid: u64) -> Option<&Thread> {
        let thread = find(tid)?.thread.load(Ordering::SeqCst);
        // SAFETY: the pin keeps the thread allocated (`Pin`); the caller reads
        // only its immutable fields and atomics.
        unsafe { thread.as_ref() }
    }

    pub(in crate::task::scheduler) fn lookup(&self, tid: u64) -> Option<(&WakeNode, usize)> {
        let slot = find(tid)?;
        let wake = slot.wake.load(Ordering::SeqCst);
        // SAFETY: the pin keeps the wake record allocated if the slot was
        // reused after find(). Check the record's TID before publishing.
        let wake = unsafe { wake.as_ref() }.filter(|wake| wake.node.tid() == tid)?;
        Some((&wake.node, slot.owner.load(Ordering::Acquire) as usize))
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        PINS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether no pin is held, so a thread already dropped from the index can be
/// freed.
pub(in crate::task::scheduler) fn unpinned() -> bool {
    PINS.load(Ordering::SeqCst) == 0
}
