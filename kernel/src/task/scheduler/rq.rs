//! x86_64 per-CPU run-queue locks (#1649).
//!
//! The scheduler was one lock, and a CPU whose vCPU the host stopped while it
//! held that lock stopped scheduling on every other CPU for as long as it was
//! stopped. Each CPU now has a run-queue lock of its own. What a CPU's lock
//! protects is that CPU's slice of the scheduler -- its queue, its
//! `CpuSchedulerState`, its timer heap -- and the scheduling state of every
//! thread it owns.
//!
//! A thread's owner is the CPU whose queue holds it or that runs it; a
//! blocked thread is owned by the CPU it blocked on. The owner is kept in the
//! thread index below, beside the thread's address, so a CPU that wants to
//! touch another thread reads its owner without dereferencing it, takes that
//! CPU's lock and checks the owner again.
//!
//! `with_scheduler` and every other user of the whole `Scheduler` still hold
//! every lock: the scheduler-wide lock and every run-queue lock. Those holders
//! may move threads anywhere; when they release, the owners and the lock-free
//! per-CPU summaries are recomputed from the queues and the current threads,
//! so a holder of a single run-queue lock always finds them consistent. No
//! frequent path takes the whole scheduler: a vCPU stopped while it held every
//! lock would stop scheduling everywhere.
//!
//! The scheduler-wide lock alone covers the thread list (`Scheduler::threads`)
//! and the reclamation state; reclamation takes it, and the run-queue lock of
//! each thread it frees, and nothing else. A new thread is published without
//! it: its box goes on a lock-free list that the next holder of the
//! scheduler-wide lock moves into the thread list (`splice_spawned`).

use super::{CpuSchedulerState, Scheduler, MAX_CPUS, SCHEDULER_HELD};
use crate::task::thread::{SchedPolicy, Thread, ThreadPrivilege, ThreadState};
use alloc::boxed::Box;
use alloc::collections::{BinaryHeap, VecDeque};
use core::cmp::Reverse;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

/// One CPU's run-queue lock. Held only with interrupts masked, so the CPU
/// that took it releases it.
pub(super) struct RqLock {
    held: AtomicBool,
}

impl RqLock {
    const fn new() -> Self {
        Self { held: AtomicBool::new(false) }
    }

    #[inline(always)]
    pub(super) fn try_lock(&self) -> bool {
        self.held
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline(always)]
    pub(super) fn is_locked(&self) -> bool {
        self.held.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub(super) fn unlock(&self) {
        self.held.store(false, Ordering::Release);
    }
}

pub(super) static RQ_LOCKS: [RqLock; MAX_CPUS] = [const { RqLock::new() }; MAX_CPUS];

/// Take every run-queue lock, or none: a busy one releases those already
/// taken, so a holder whose vCPU has stopped holds up only this caller, never
/// a CPU waiting for one of the locks this caller took first.
pub(super) fn try_lock_all() -> bool {
    for cpu in 0..MAX_CPUS {
        if !RQ_LOCKS[cpu].try_lock() {
            for taken in 0..cpu {
                RQ_LOCKS[taken].unlock();
            }
            return false;
        }
    }
    true
}

/// Take every run-queue lock, waiting with none of them held while one is
/// busy.
pub(super) fn lock_all() {
    while !try_lock_all() {
        while RQ_LOCKS.iter().any(RqLock::is_locked) {
            core::hint::spin_loop();
        }
    }
}

pub(super) fn unlock_all() {
    for lock in RQ_LOCKS.iter() {
        lock.unlock();
    }
}

// ------------------------------------------------------------ thread index

/// Slots of the thread index. Thread IDs are handed out in order, so a tid
/// hashes to its own slot until the IDs wrap the table; probing is linear.
const SLOTS: usize = 4096;

/// A slot's key is the tid plus one, so tid 0 (an idle thread) is a key.
const EMPTY: u64 = 0;
/// A removed entry: lookups probe past it, inserts reuse it.
const REMOVED: u64 = u64::MAX;
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
const USER: u8 = 1;
/// `Slot::flags`: the thread has terminated.
const DEAD: u8 = 2;

struct Slot {
    key: AtomicU64,
    thread: AtomicPtr<Thread>,
    owner: AtomicU32,
    /// The page-table root the thread caches (`Thread::cached_ttbr0`) while
    /// it has not terminated, and 0 once it has: what `any_cached_root` reads,
    /// with no lock.
    root: AtomicU64,
    /// Wakes held for the thread because the lock of the CPU that owns it was
    /// busy when they were made (`hold`): the tid, the home CPU of a held
    /// placement, and the `HELD_*` bits; 0 for none.
    held: AtomicU64,
    /// The process row the thread belongs to (`Thread::owner_pid`), 0 for none.
    pid: AtomicU64,
    /// `USER` and `DEAD`.
    flags: AtomicU8,
    /// The address of the thread's interval timers (`Thread::signal_timers`),
    /// 0 for none: which process's timers it may receive.
    group: AtomicUsize,
    /// The thread's scheduling policy without its run-time fields
    /// (`pack_policy`), so its rank and policy are read without a lock.
    policy: AtomicU64,
}


static INDEX: [Slot; SLOTS] = [const {
    Slot {
        key: AtomicU64::new(EMPTY),
        thread: AtomicPtr::new(core::ptr::null_mut()),
        owner: AtomicU32::new(0),
        root: AtomicU64::new(0),
        held: AtomicU64::new(0),
        pid: AtomicU64::new(0),
        flags: AtomicU8::new(0),
        group: AtomicUsize::new(0),
        policy: AtomicU64::new(0),
    }
}; SLOTS];

fn home(tid: u64) -> usize {
    tid as usize % SLOTS
}

fn find(tid: u64) -> Option<&'static Slot> {
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
fn live_key(key: u64) -> bool {
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

fn group_of(thread: &Thread) -> usize {
    thread.signal_timers.as_ref().map_or(0, |timers| alloc::sync::Arc::as_ptr(timers) as usize)
}

/// Write what the index publishes about `thread` into `slot`.
fn fill(slot: &Slot, thread: &mut Thread, owner: usize) {
    let dead = thread.state == ThreadState::Terminated;
    let flags = if thread.privilege == ThreadPrivilege::User { USER } else { 0 } | if dead { DEAD } else { 0 };
    slot.thread.store(thread, Ordering::SeqCst);
    slot.owner.store(owner as u32, Ordering::Release);
    slot.root.store(if dead { 0 } else { thread.cached_ttbr0 }, Ordering::Release);
    slot.pid.store(thread.owner_pid.unwrap_or(0), Ordering::Release);
    slot.flags.store(flags, Ordering::Release);
    slot.group.store(group_of(thread), Ordering::Release);
    slot.policy.store(pack_policy(&thread.sched), Ordering::Release);
}

/// Index `thread`, owned by `owner`. No lock is needed: each insert claims
/// its slot, and a lookup sees the thread once its key is stored. The thread
/// is not queued yet, so nothing else reaches it meanwhile.
pub(super) fn insert(thread: &mut Thread, owner: usize) {
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
            && slot.key.compare_exchange(key, CLAIMED, Ordering::AcqRel, Ordering::Acquire).is_ok()
        {
            TAKEN.fetch_add(1, Ordering::AcqRel);
            fill(slot, thread, owner);
            slot.key.store(tid.wrapping_add(1), Ordering::Release);
            return;
        }
    }
    panic!("scheduler: thread index full ({} slots) adding thread {}", SLOTS, tid);
}

/// Drop `tid` from the index. The scheduler-wide lock and the run-queue lock
/// of the CPU that owns the thread are held, or every lock: no other CPU then
/// holds a reference to the thread taken under that lock. A reader that took
/// one from the index without a lock holds a `Pin`, and the caller frees the
/// thread only once none does (`unpinned`).
pub(super) fn remove(tid: u64) {
    if let Some(slot) = find(tid) {
        let word = slot.held.swap(0, Ordering::AcqRel);
        if word != 0 {
            HELD_SLOTS.fetch_sub(1, Ordering::AcqRel);
            release_held_place(word);
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
pub(super) fn publish_root(thread: &Thread, root: u64) {
    if let Some(slot) = own_slot(thread) {
        slot.root.store(root, Ordering::Release);
    }
}

/// `thread`'s slot, if the live index holds this thread.
fn own_slot(thread: &Thread) -> Option<&'static Slot> {
    find(thread.id()).filter(|slot| core::ptr::eq(slot.thread.load(Ordering::Acquire), thread))
}

/// Publish that `thread` has terminated: no page-table root, the dead flag.
pub(super) fn publish_terminated(thread: &Thread) {
    if let Some(slot) = own_slot(thread) {
        slot.root.store(0, Ordering::Release);
        slot.flags.fetch_or(DEAD, Ordering::AcqRel);
    }
}

/// Publish `thread`'s scheduling policy, after its owner's lock holder or the
/// whole scheduler changed it.
pub(super) fn publish_policy(thread: &Thread) {
    if let Some(slot) = own_slot(thread) {
        slot.policy.store(pack_policy(&thread.sched), Ordering::Release);
    }
}

/// Publish which interval timers `thread` receives signals of, after exec
/// gave it its new process's.
pub(super) fn publish_group(thread: &Thread) {
    if let Some(slot) = own_slot(thread) {
        slot.group.store(group_of(thread), Ordering::Release);
    }
}

/// Publish the process row `thread` now belongs to (`Thread::owner_pid`).
pub(super) fn publish_owner(thread: &Thread) {
    if let Some(slot) = own_slot(thread) {
        slot.pid.store(thread.owner_pid.unwrap_or(0), Ordering::Release);
    }
}

/// `tid`'s scheduling policy, without its run-time fields, read without a
/// lock.
pub(super) fn policy(tid: u64) -> Option<SchedPolicy> {
    let word = find(tid)?.policy.load(Ordering::Acquire);
    (word != 0).then(|| unpack_policy(word))
}

/// The CPU that owns `tid`, read without any lock.
pub(super) fn owner(tid: u64) -> Option<usize> {
    find(tid).map(|slot| slot.owner.load(Ordering::Acquire) as usize)
}

/// `tid`'s thread, if `cpu` owns it. The caller holds `cpu`'s run-queue lock,
/// so the thread can neither change owner nor be reclaimed while the
/// reference lives: both take that lock.
///
/// # Safety
/// The caller holds `RQ_LOCKS[cpu]` for as long as the reference is used,
/// and holds no other reference to the same thread.
pub(super) unsafe fn owned_thread<'a>(tid: u64, cpu: usize) -> Option<&'a mut Thread> {
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
pub(super) fn set_owner(tid: u64, cpu: usize) {
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
pub(super) struct Pin(());

impl Pin {
    pub(super) fn new() -> Self {
        PINS.fetch_add(1, Ordering::SeqCst);
        Pin(())
    }

    /// `tid`'s thread, which stays allocated while the pin lives.
    pub(super) fn thread(&self, tid: u64) -> Option<&Thread> {
        let thread = find(tid)?.thread.load(Ordering::SeqCst);
        // SAFETY: the pin keeps the thread allocated (`Pin`); the caller reads
        // only its immutable fields and atomics.
        unsafe { thread.as_ref() }
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        PINS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether no pin is held, so a thread already dropped from the index can be
/// freed.
pub(super) fn unpinned() -> bool {
    PINS.load(Ordering::SeqCst) == 0
}

// --------------------------------------------------------- per-CPU summary

/// Per CPU, as its slice stood when its lock was last released: how many
/// threads it has to run (its queue, plus the thread it runs unless that is
/// its idle thread), whether it runs its idle thread, and the tick it last
/// entered the scheduler at. Placement reads them without locks.
pub(super) static CPU_LOAD: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
pub(super) static CPU_IDLE: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
pub(super) static CPU_LAST_SCHEDULE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Per CPU, the thread it ran when its lock was last released, plus one; 0
/// for none.
static CURRENT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Whether `tid` was some CPU's current thread when that CPU's lock was last
/// released. Read without a lock.
pub(super) fn is_current_anywhere(tid: u64) -> bool {
    CURRENT.iter().any(|current| current.load(Ordering::Acquire) == tid + 1)
}

/// The live scheduler, which `SCHEDULER` holds from `init` on.
static LIVE: AtomicPtr<Scheduler> = AtomicPtr::new(core::ptr::null_mut());

/// Record the live scheduler and index its threads. Called once, with every
/// lock held, when `init` installs it.
pub(super) fn publish_live(scheduler: &mut Scheduler) {
    LIVE.store(scheduler, Ordering::Release);
    let idle: [u64; MAX_CPUS] = core::array::from_fn(|cpu| scheduler.cpu_state[cpu].idle_thread);
    for thread in scheduler.threads.iter_mut() {
        let owner = idle.iter().position(|&tid| tid == thread.id()).unwrap_or(0);
        insert(thread, owner);
    }
    refresh(scheduler);
}

/// Whether `scheduler` is the live one, whose threads the index holds. A
/// scheduler a test builds is not.
pub(super) fn is_live(scheduler: &Scheduler) -> bool {
    core::ptr::eq(LIVE.load(Ordering::Acquire), scheduler)
}

/// Recompute what a single run-queue lock holder relies on from the whole
/// scheduler, before its locks are released: every queued or running thread
/// is owned by its CPU, and the per-CPU summaries are current.
pub(super) fn refresh(scheduler: &Scheduler) {
    if !is_live(scheduler) {
        return;
    }
    for cpu in 0..MAX_CPUS {
        for &tid in scheduler.per_cpu_queues[cpu].iter() {
            set_owner(tid, cpu);
        }
        let state = &scheduler.cpu_state[cpu];
        if let Some(tid) = state.current_thread {
            set_owner(tid, cpu);
        }
        let idle = state.pending_next.or(state.current_thread) == Some(state.idle_thread);
        let busy = state.pending_next.or(state.current_thread).is_some() && !idle;
        CPU_LOAD[cpu].store(scheduler.per_cpu_queues[cpu].len() as u32 + busy as u32, Ordering::Relaxed);
        CPU_IDLE[cpu].store(idle, Ordering::Relaxed);
        CPU_LAST_SCHEDULE[cpu].store(state.last_schedule_ticks, Ordering::Relaxed);
        CURRENT[cpu].store(state.current_thread.map_or(0, |tid| tid + 1), Ordering::Release);
        publish_next_check(cpu, scheduler.timer_heap.heaps[cpu].peek().map_or(u64::MAX, |entry| entry.0 .0));
    }
}

// ------------------------------------------------------- one CPU's slice

/// A held run-queue lock: CPU `cpu`'s slice of the live scheduler and the
/// threads it owns, and nothing else. Interrupts are masked for as long as it
/// is held, so the CPU that took it releases it.
pub(super) struct Held {
    cpu: usize,
    sched: *mut Scheduler,
}

impl Held {
    fn take(cpu: usize, sched: *mut Scheduler) -> Self {
        let me = super::current_cpu_id_raw();
        if me < MAX_CPUS {
            SCHEDULER_HELD[me].fetch_add(1, Ordering::Relaxed);
        }
        Self { cpu, sched }
    }

    pub(super) fn queue(&mut self) -> &mut VecDeque<u64> {
        // SAFETY: this CPU's lock is held, and it protects its queue.
        unsafe { &mut *core::ptr::addr_of_mut!((*self.sched).per_cpu_queues[self.cpu]) }
    }

    pub(super) fn state(&mut self) -> &mut CpuSchedulerState {
        // SAFETY: as `queue`.
        unsafe { &mut *core::ptr::addr_of_mut!((*self.sched).cpu_state[self.cpu]) }
    }

    pub(super) fn heap(&mut self) -> &mut BinaryHeap<Reverse<(u64, u64)>> {
        // SAFETY: as `queue`.
        unsafe { &mut *core::ptr::addr_of_mut!((*self.sched).timer_heap.heaps[self.cpu]) }
    }

    /// `tid`, if this CPU owns it.
    pub(super) fn thread(&mut self, tid: u64) -> Option<&mut Thread> {
        // SAFETY: the lock is held for the borrow of `self`.
        unsafe { owned_thread(tid, self.cpu) }
    }

    /// Two different threads this CPU owns.
    pub(super) fn two(&mut self, a: u64, b: u64) -> (Option<&mut Thread>, Option<&mut Thread>) {
        if a == b {
            return (None, None);
        }
        // SAFETY: two different threads, both owned under the held lock.
        unsafe { (owned_thread(a, self.cpu), owned_thread(b, self.cpu)) }
    }

    /// Take `tid` off the ready queue: this CPU's, the only one a thread it
    /// owns can be on.
    pub(super) fn remove_from_ready_queue(&mut self, tid: u64) {
        self.queue().retain(|&queued| queued != tid);
    }

    /// Queue `tid`, a thread this CPU now owns, at the back or the front.
    pub(super) fn enqueue(&mut self, tid: u64, front: bool) {
        set_owner(tid, self.cpu);
        if front {
            self.queue().push_front(tid);
        } else {
            self.queue().push_back(tid);
        }
    }

    /// Publish the summaries placement reads, before the lock is released.
    fn publish(&mut self) {
        let cpu = self.cpu;
        let len = self.queue().len() as u32;
        let state = self.state();
        let idle = state.pending_next.or(state.current_thread) == Some(state.idle_thread);
        let busy = state.pending_next.or(state.current_thread).is_some() && !idle;
        let last = state.last_schedule_ticks;
        let current = state.current_thread.map_or(0, |tid| tid + 1);
        CPU_LOAD[cpu].store(len + busy as u32, Ordering::Relaxed);
        CPU_IDLE[cpu].store(idle, Ordering::Relaxed);
        CPU_LAST_SCHEDULE[cpu].store(last, Ordering::Relaxed);
        CURRENT[cpu].store(current, Ordering::Release);
        let bit = 1u64 << cpu;
        if len != 0 {
            super::QUEUED_CPUS.fetch_or(bit, Ordering::Release);
        } else {
            super::QUEUED_CPUS.fetch_and(!bit, Ordering::Release);
        }
        publish_next_check(cpu, self.heap().peek().map_or(u64::MAX, |entry| entry.0 .0));
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.publish();
        RQ_LOCKS[self.cpu].unlock();
        let me = super::current_cpu_id_raw();
        if me < MAX_CPUS {
            SCHEDULER_HELD[me].fetch_sub(1, Ordering::Relaxed);
        }
        super::send_deferred_resched_ipis();
    }
}

/// This CPU's run-queue lock, waited for. None before the live scheduler
/// exists. Interrupts must be masked.
///
/// The only other holders of this lock are a peer that tried it, which holds
/// it briefly, and a holder of the whole scheduler.
pub(super) fn local() -> Option<Held> {
    let sched = LIVE.load(Ordering::Acquire);
    let cpu = super::current_cpu_id_raw();
    if sched.is_null() || cpu >= MAX_CPUS {
        return None;
    }
    if SCHEDULER_HELD[cpu].load(Ordering::Relaxed) != 0 {
        panic!("scheduler: CPU {} took its run-queue lock while holding a scheduler lock", cpu);
    }
    while !RQ_LOCKS[cpu].try_lock() {
        core::hint::spin_loop();
    }
    Some(Held::take(cpu, sched))
}

/// CPU `cpu`'s run-queue lock, if it is free now. Never waits: a CPU whose
/// vCPU has stopped while it holds its lock is passed over.
pub(super) fn try_remote(cpu: usize) -> Option<Held> {
    let sched = LIVE.load(Ordering::Acquire);
    if sched.is_null() || cpu >= MAX_CPUS || !RQ_LOCKS[cpu].try_lock() {
        return None;
    }
    Some(Held::take(cpu, sched))
}

// ------------------------------------------------------------- liveness

/// Per CPU, the earliest deadline in its timer heap, as of its last release.
static NEXT_CHECK: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(u64::MAX) }; MAX_CPUS];

fn publish_next_check(cpu: usize, deadline: u64) {
    NEXT_CHECK[cpu].store(deadline, Ordering::Release);
}

/// Claim `cpu`'s due deadline for a pass on this CPU: it moves a tick later,
/// and the pass publishes the real next one (`timer_deadline_due`).
pub(super) fn claim_due(cpu: usize, now: u64, retry: u64) -> bool {
    let deadline = NEXT_CHECK[cpu].load(Ordering::Acquire);
    deadline != u64::MAX
        && now >= deadline
        && NEXT_CHECK[cpu]
            .compare_exchange(deadline, retry, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
}

/// Claim a due deadline of a CPU other than `me`, whose sleepers this CPU's
/// pass then wakes (`expire_timers`). With one heap any CPU's tick found every
/// due deadline, so a sleep waited for the next tick of any of them, not of
/// its own CPU alone.
pub(super) fn claim_other_due(me: usize, now: u64, retry: u64) -> bool {
    (0..MAX_CPUS).any(|cpu| {
        cpu != me && NEXT_CHECK[cpu].load(Ordering::Relaxed) <= now && claim_due(cpu, now, retry)
    })
}

/// Whether `cpu` is online and has entered the scheduler, or returned from an
/// interrupt, recently enough to dispatch newly runnable work: the staleness
/// test `Scheduler::cpu_accepts_wakeups` makes, from the published ticks.
pub(super) fn accepts_wakeups(cpu: usize) -> bool {
    let online = (crate::arch_impl::x86_64::smp::cpus_online() as usize).clamp(1, MAX_CPUS);
    if cpu >= online {
        return false;
    }
    if cpu == super::current_cpu_id_raw() {
        return true;
    }
    if spinning(cpu) {
        return false;
    }
    let last = CPU_LAST_SCHEDULE[cpu].load(Ordering::Relaxed).max(super::last_interrupt_return_tick(cpu));
    crate::time::get_ticks().wrapping_sub(last) <= super::CPU_STALL_TICKS
}

/// Whether `cpu` can dispatch a thread queued on it now: it accepts wakeups,
/// and it has entered the scheduler, or returned from an interrupt with
/// preemption on, within `CPU_STALL_TICKS`. A CPU whose interrupts return with
/// preemption held off still runs and still takes the threads pinned to it,
/// but its queue waits for the section to end. In a testing boot under the x86
/// gate, CPU 0's boot thread, which is its idle thread, held preemption off
/// for 11.6 s while it created the boot's processes and waited for their
/// reads, and looked idle throughout; woken threads placed there waited up to
/// 1.5 s for a peer to take them. Placement passes such a CPU over for
/// unpinned threads, and peers take its unpinned queued ones.
pub(super) fn dispatching(cpu: usize) -> bool {
    if !accepts_wakeups(cpu) {
        return false;
    }
    if cpu == super::current_cpu_id_raw() {
        return true;
    }
    let last = CPU_LAST_SCHEDULE[cpu].load(Ordering::Relaxed).max(super::last_dispatchable_return_tick(cpu));
    crate::time::get_ticks().wrapping_sub(last) <= super::CPU_STALL_TICKS
}

/// Whether `cpu` runs its idle thread and no idle housekeeping, as of its last
/// release.
pub(super) fn is_idle(cpu: usize) -> bool {
    CPU_IDLE[cpu].load(Ordering::Relaxed) && !super::IDLE_HOUSEKEEPING[cpu].load(Ordering::Acquire)
}

/// The least loaded CPU that is dispatching, `prefer` among equals; else the
/// least loaded that accepts wakeups; `prefer` when none does.
pub(super) fn least_loaded(prefer: usize) -> usize {
    let online = (crate::arch_impl::x86_64::smp::cpus_online() as usize).clamp(1, MAX_CPUS);
    let pick = |usable: fn(usize) -> bool| {
        (0..online).filter(|&cpu| usable(cpu)).min_by_key(|&cpu| (load(cpu), cpu != prefer))
    };
    pick(dispatching).or_else(|| pick(accepts_wakeups)).unwrap_or(prefer)
}

/// `Scheduler::cpu_load` from the published summaries: the threads `cpu` has
/// to run as of its last release, with a CPU whose idle loop is running
/// housekeeping counted busy. That housekeeping holds preemption off, so a
/// thread queued there waits for all of it; it starts and ends without the
/// run-queue lock, so it is read here rather than published.
fn load(cpu: usize) -> u32 {
    let housekeeping =
        CPU_IDLE[cpu].load(Ordering::Relaxed) && super::IDLE_HOUSEKEEPING[cpu].load(Ordering::Acquire);
    CPU_LOAD[cpu].load(Ordering::Relaxed) + housekeeping as u32
}

// ------------------------------------------------------ the dispatch path

use super::{
    count_pinned_migration_refusal, observe_exit_kick, record_ready_site, request_resched_x86,
    send_reschedule_vector_x86, trace_sched_diag, x86_stack_owner_elsewhere,
    UnblockOutcome, CONTEXT_SWITCH_COUNT, ENQUEUE_ALREADY_QUEUED_OK, ENQUEUE_DEFERRED,
    ENQUEUE_ISR_BUFFER_DRAINED_OK, ENQUEUE_SAME_LOCK_OK, ENQUEUE_STALLED_RECLAIMED,
    ENQUEUE_TIMER_WAKE, ISR_WAKEUP_BUFFERS, LEAVING_STACK, PINNED_HOLD_PEN_MIGRATED,
    PINNED_PUBLISH_DISCARDED, POLICIES_IN_USE, READY_SITE_SCHEDULE, READY_SITE_TIMER,
    READY_SITE_UNBLOCK, READY_SITE_WAKE_IO_ISR_DRAIN, READY_SITE_WAKE_IO_LOCKED,
    TRACE_SCHED_DIAG_WAKE_TIMER_ALREADY_QUEUED, TRACE_SCHED_DIAG_WAKE_TIMER_CURRENT,
    TRACE_SCHED_DIAG_WAKE_TIMER_ENQUEUE, WAKE_SITE_SCHEDULE, WAKE_SITE_TIMER, WAKE_SITE_UNBLOCK,
    WAKE_SITE_WAKE_IO_LOCKED,
};

fn online() -> usize {
    (crate::arch_impl::x86_64::smp::cpus_online() as usize).clamp(1, MAX_CPUS)
}

fn now_ns() -> u64 {
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    secs as u64 * 1_000_000_000 + nanos as u64
}

/// The scheduling rank of `tid`, owned by `held`'s CPU (`SchedPolicy::rank`).
fn rank(held: &mut Held, tid: u64) -> u8 {
    held.thread(tid).map_or(0, |thread| thread.sched.rank())
}

/// `Scheduler::promote_best_of` on `held`'s queue.
fn promote_best_of(held: &mut Held, count: usize) {
    if !POLICIES_IN_USE.load(Ordering::Acquire) {
        return;
    }
    let mut best = 0;
    let mut best_rank = None;
    for index in 0..count.min(held.queue().len()) {
        let tid = held.queue()[index];
        let rank = rank(held, tid);
        if best_rank.map_or(true, |best_rank| rank > best_rank) {
            best = index;
            best_rank = Some(rank);
        }
    }
    if best > 0 {
        held.queue().make_contiguous()[..=best].rotate_right(1);
    }
}

/// `Scheduler::best_queued_rank` on `held`'s queue.
fn best_queued_rank(held: &mut Held, except: Option<u64>) -> u8 {
    let mut best = 0;
    for index in 0..held.queue().len() {
        let tid = held.queue()[index];
        if Some(tid) == except {
            continue;
        }
        if let Some(thread) = held.thread(tid).filter(|thread| thread.state != ThreadState::Terminated) {
            best = best.max(thread.sched.rank());
        }
    }
    best
}

/// `Scheduler::policy_keeps_cpu` on `held`'s CPU.
fn policy_keeps_cpu(held: &mut Held, tid: u64, holds_head: bool) -> bool {
    if !POLICIES_IN_USE.load(Ordering::Acquire) {
        return false;
    }
    let mine = rank(held, tid);
    if mine <= SchedPolicy::DEFAULT.rank() {
        return false;
    }
    let other = best_queued_rank(held, Some(tid));
    mine > other || (holds_head && mine >= other)
}

/// `Scheduler::settle_outgoing` for a thread `held`'s CPU owns.
fn settle_outgoing(held: &mut Held, tid: u64, runnable: bool) -> bool {
    if !POLICIES_IN_USE.load(Ordering::Acquire) {
        return false;
    }
    let Some(thread) = held.thread(tid) else {
        return false;
    };
    if runnable {
        thread.settle_outgoing(crate::time::get_cpu_ticks())
    } else {
        thread.sched.yielded = false;
        thread.sched.head_on_requeue = false;
        false
    }
}

/// `Scheduler::timer_wake_promotion_open` on `held`'s CPU.
fn promotion_open(held: &mut Held) -> bool {
    let promoted = held.state().promoted_wake;
    !held.state().last_dispatch_promoted
        && promoted.map_or(true, |tid| !held.queue().contains(&tid))
}

/// `Scheduler::retain_pinned_worker_on_source_queue`: a per-CPU worker popped
/// from its own home's queue goes back on it.
fn retain_on_home(source: &mut Held, tid: u64) -> bool {
    if crate::task::thread::CPU_PINS_STAMPED.load(Ordering::Relaxed) == 0 || source.cpu >= online() {
        return false;
    }
    let cpu = source.cpu;
    let home = source.thread(tid).and_then(|thread| {
        (thread.state != ThreadState::Terminated).then_some(thread.cpu_affinity).flatten()
    });
    if home.is_some_and(|pin| pin.per_cpu_worker && pin.cpu == cpu) {
        source.queue().push_back(tid);
        return true;
    }
    false
}

/// `Scheduler::retain_cpu_affine_thread`, for a thread `source`'s CPU owns that
/// is about to run on `taking`: a per-CPU worker pinned elsewhere goes to its
/// home instead, and true says it was placed. Its home's lock is only tried;
/// when it is busy the worker stays where it was.
fn retain_affine(source: &mut Held, tid: u64, taking: usize) -> bool {
    if crate::task::thread::CPU_PINS_STAMPED.load(Ordering::Relaxed) == 0 {
        return false;
    }
    let Some(thread) = source.thread(tid) else {
        return false;
    };
    if thread.state == ThreadState::Terminated {
        return false;
    }
    let Some(pin) = thread.cpu_affinity else {
        return false;
    };
    if !pin.per_cpu_worker {
        if pin.cpu != taking {
            PINNED_HOLD_PEN_MIGRATED.fetch_add(1, Ordering::Relaxed);
        }
        return false;
    }
    if pin.cpu == taking {
        return false;
    }
    if pin.cpu >= online() {
        PINNED_PUBLISH_DISCARDED.fetch_add(1, Ordering::Relaxed);
        thread.cpu_affinity = None;
        return false;
    }
    count_pinned_migration_refusal(tid);
    if pin.cpu == source.cpu {
        source.enqueue(tid, false);
        return true;
    }
    if let Some(mut home) = try_home(pin.cpu) {
        home.enqueue(tid, false);
        drop(home);
        request_resched_x86(pin.cpu);
        return true;
    }
    // Its home's lock stayed busy: hold the wake for it rather than queue a
    // pinned thread anywhere else.
    super::note_pinned_hold(tid, pin.cpu);
    request_resched_x86(pin.cpu);
    true
}

/// How many times a pinned thread's placement tries its home CPU's lock
/// before it holds the wake for that CPU instead.
const HOME_TRIES: usize = 256;

/// `cpu`'s lock, tried `HOME_TRIES` times: a pinned thread may run nowhere
/// else, so a briefly held lock is worth waiting out, a stopped holder not.
fn try_home(cpu: usize) -> Option<Held> {
    for _ in 0..HOME_TRIES {
        if let Some(held) = try_remote(cpu) {
            return Some(held);
        }
        core::hint::spin_loop();
    }
    None
}

/// Where a woken thread goes.
enum Target {
    /// This CPU's queue; true when the thread is pinned to it.
    Cpu(usize, bool),
    /// Nowhere yet: it is pinned to this CPU, which does not accept wakeups,
    /// and its wake is held for it.
    Hold(usize),
}

/// Where a woken thread `tid`, owned by `source`'s CPU and current on no CPU,
/// runs: `Scheduler::find_target_cpu_for_wakeup`, from the published
/// summaries.
fn wake_target(source: &mut Held, tid: u64) -> Target {
    let me = super::current_cpu_id_raw();
    if let Some(pin) = source.thread(tid).and_then(|thread| thread.cpu_affinity) {
        if pin.cpu < online() && (pin.cpu == me || accepts_wakeups(pin.cpu)) {
            return Target::Cpu(pin.cpu, pin.per_cpu_worker);
        }
        if pin.per_cpu_worker {
            return Target::Hold(pin.cpu);
        }
        PINNED_HOLD_PEN_MIGRATED.fetch_add(1, Ordering::Relaxed);
    }
    Target::Cpu(least_loaded(me), false)
}

/// Place a woken thread where `target` says (`place`), or hold its wake, and
/// interrupt the CPU it went to. Returns the CPU it was queued on.
fn place_woken(local: &mut Held, source: Option<&mut Held>, tid: u64, target: Target, promote: bool) -> Option<usize> {
    let home = match target {
        Target::Cpu(cpu, pinned) => match place(local, source, tid, cpu, promote, pinned) {
            Some(queued_on) => {
                kick(queued_on);
                return Some(queued_on);
            }
            None => cpu,
        },
        Target::Hold(home) => home,
    };
    super::note_pinned_hold(tid, home);
    None
}

/// Queue the woken thread `tid`, owned by `source`'s CPU (`local`'s when
/// None), on `target`; with `promote`, at the head when `target`'s promotion
/// is open (a late timer wake, #766). An unpinned thread goes to `local`'s
/// queue when `target`'s lock is busy; a `pinned` one runs nowhere but
/// `target`, and None says its lock stayed busy, for the caller to hold the
/// wake. Returns the CPU it was queued on, which the caller interrupts.
fn place(
    local: &mut Held,
    mut source: Option<&mut Held>,
    tid: u64,
    target: usize,
    promote: bool,
    pinned: bool,
) -> Option<usize> {
    let ranked = POLICIES_IN_USE.load(Ordering::Acquire);
    let rank_of = match source.as_deref_mut() {
        Some(source) => rank(source, tid),
        None => rank(local, tid),
    };
    let put = |held: &mut Held| {
        let front = promote
            && !(ranked && rank_of > SchedPolicy::DEFAULT.rank())
            && promotion_open(held);
        held.enqueue(tid, front);
        if front {
            held.state().promoted_wake = Some(tid);
        }
        held.cpu
    };
    if let Some(source) = source {
        if target == source.cpu {
            return Some(put(source));
        }
    }
    if target == local.cpu {
        return Some(put(local));
    }
    let remote = if pinned { try_home(target) } else { try_remote(target) };
    match remote {
        Some(mut remote) => Some(put(&mut remote)),
        None if pinned => None,
        None => Some(put(local)),
    }
}

/// Ask `cpu` to pick up what was queued for it.
fn kick(cpu: usize) {
    if cpu != super::current_cpu_id_raw() {
        request_resched_x86(cpu);
    }
}

/// `Scheduler::note_timer_pop`, for an entry of `held`'s heap.
fn note_timer_pop(held: &mut Held, wake_time: u64, tid: u64) {
    if let Some(thread) = held.thread(tid) {
        let wake_time_set = thread.wake_time_ns == Some(wake_time);
        if let Some(record) = thread.timer_pop.as_mut() {
            if record.deadline_ns == wake_time {
                record.own_entry = if wake_time_set {
                    crate::task::thread::TimerPop::WakeTimeSet
                } else {
                    crate::task::thread::TimerPop::WakeTimeCleared
                };
            } else {
                record.stale_entries = record.stale_entries.saturating_add(1);
            }
        }
    }
}

/// `Scheduler::timer_entry_live`: the entry times out `tid`'s current wait, and
/// `held`'s CPU still owns it. An entry left behind when its thread moved is
/// stale: the thread moves only once it is woken.
fn timer_entry_live(held: &mut Held, wake_time: u64, tid: u64) -> bool {
    held.thread(tid).is_some_and(|thread| {
        (matches!(thread.state, ThreadState::BlockedOnTimer)
            || (thread.state == ThreadState::BlockedOnIO && thread.wake_time_ns.is_some()))
            && thread.wake_time_ns == Some(wake_time)
    })
}

/// Expire the deadlines due in `source`'s heap (`local` itself when `source` is
/// None): `Scheduler::wake_expired_timers` for the threads one CPU owns.
fn expire_heap(local: &mut Held, mut source: Option<&mut Held>, now: u64) {
    loop {
        let held: &mut Held = match source.as_deref_mut() {
            Some(source) => source,
            None => &mut *local,
        };
        let Some(&Reverse((wake_time, tid))) = held.heap().peek() else {
            break;
        };
        if wake_time > now {
            break;
        }
        held.heap().pop();
        note_timer_pop(held, wake_time, tid);
        if !timer_entry_live(held, wake_time, tid) {
            continue;
        }
        let current_here = held.state().current_thread == Some(tid);
        let queued_len = held.queue().len() as u32;
        let Some(thread) = held.thread(tid) else {
            continue;
        };
        let was_blocked_on_io = thread.state == ThreadState::BlockedOnIO;
        thread.state = ThreadState::Ready;
        WAKE_SITE_TIMER.fetch_add(1, Ordering::Relaxed);
        record_ready_site(tid, READY_SITE_TIMER);
        thread.wake_time_ns = None;
        if !was_blocked_on_io {
            thread.blocked_in_syscall = false;
        }
        if current_here {
            // Still running its wait loop there: the state change ends it once
            // that CPU is out of its halt.
            ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
            trace_sched_diag(
                TRACE_SCHED_DIAG_WAKE_TIMER_CURRENT,
                tid,
                tid,
                0,
                ((was_blocked_on_io as u32) << 31) | queued_len,
            );
            let cpu = held.cpu;
            if cpu != super::current_cpu_id_raw() {
                send_reschedule_vector_x86(cpu);
            }
            continue;
        }
        let idle = tid == held.state().idle_thread;
        if held.queue().contains(&tid) {
            trace_sched_diag(
                TRACE_SCHED_DIAG_WAKE_TIMER_ALREADY_QUEUED,
                tid,
                tid,
                0,
                ((was_blocked_on_io as u32) << 31) | queued_len,
            );
            ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if idle {
            continue;
        }
        let target = wake_target(held, tid);
        let target_cpu = match target {
            Target::Cpu(cpu, _) | Target::Hold(cpu) => cpu,
        };
        trace_sched_diag(
            TRACE_SCHED_DIAG_WAKE_TIMER_ENQUEUE,
            tid,
            tid,
            target_cpu as u64,
            ((was_blocked_on_io as u32) << 31) | ((target_cpu as u32 & 0xF) << 16) | queued_len,
        );
        // Discovery must preempt even a busy destination, which placement
        // interrupts; otherwise its remaining quantum delays this deadline.
        let queued = match source.as_deref_mut() {
            Some(source) => place_woken(local, Some(source), tid, target, true),
            None => place_woken(local, None, tid, target, true),
        };
        if queued.is_some() {
            ENQUEUE_TIMER_WAKE.fetch_add(1, Ordering::Relaxed);
            ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Expire this CPU's due deadlines, and those of every other CPU whose lock
/// is free now: a sleep is woken by the first pass on any CPU after its
/// deadline, as with one heap, and a stopped vCPU's sleepers run on a CPU
/// that is running. A deadline claimed by a tick (`claim_due`) reads a tick
/// later, so any CPU with deadlines is looked at, not only one whose
/// published deadline is due.
pub(super) fn expire_timers(local: &mut Held) {
    let now = now_ns();
    expire_heap(local, None, now);
    for cpu in 0..MAX_CPUS {
        if cpu == local.cpu || NEXT_CHECK[cpu].load(Ordering::Acquire) == u64::MAX {
            continue;
        }
        if let Some(mut remote) = try_remote(cpu) {
            let due = remote.heap().peek().is_some_and(|entry| entry.0 .0 <= now);
            if due {
                expire_heap(local, Some(&mut remote), now);
            }
        }
    }
}

/// How a wake applies to the thread's state: `Scheduler::unblock` (Generic) or
/// `Scheduler::wake_io_thread_locked` (Io).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Wake {
    Generic,
    Io { from_isr_buffer: bool },
}

/// What a wake did, for the caller's accounting.
pub(super) enum Woken {
    /// The thread's lock was busy; nothing changed.
    Busy,
    Done(UnblockOutcome),
}

/// Wake `tid` with `kind` under the lock of the CPU that owns it: `local`'s, or
/// a peer's taken without waiting. Busy when the peer's lock is held.
pub(super) fn wake(local: &mut Held, tid: u64, kind: Wake) -> Woken {
    let Some(owner) = owner(tid) else {
        return Woken::Done(UnblockOutcome::NotFound);
    };
    if owner == local.cpu {
        return Woken::Done(wake_owned(local, None, tid, kind));
    }
    let Some(mut remote) = try_remote(owner) else {
        return Woken::Busy;
    };
    // The owner can have changed before its lock was taken.
    if self::owner(tid) != Some(owner) {
        return Woken::Busy;
    }
    Woken::Done(wake_owned(local, Some(&mut remote), tid, kind))
}

fn wake_owned(local: &mut Held, mut source: Option<&mut Held>, tid: u64, kind: Wake) -> UnblockOutcome {
    let held: &mut Held = match source.as_deref_mut() {
        Some(source) => source,
        None => &mut *local,
    };
    let current_here = held.state().current_thread == Some(tid);
    let idle = tid == held.state().idle_thread;
    let Some(thread) = held.thread(tid) else {
        return UnblockOutcome::NotFound;
    };
    let should_queue = match kind {
        Wake::Generic => match thread.state {
            ThreadState::Blocked
            | ThreadState::BlockedOnSignal
            | ThreadState::BlockedOnTimer
            | ThreadState::BlockedOnIO => {
                thread.set_ready();
                WAKE_SITE_UNBLOCK.fetch_add(1, Ordering::Relaxed);
                record_ready_site(tid, READY_SITE_UNBLOCK);
                true
            }
            ThreadState::Ready | ThreadState::Running => return UnblockOutcome::AlreadyRunnable,
            _ => return UnblockOutcome::NotFound,
        },
        Wake::Io { from_isr_buffer } => {
            let site = if from_isr_buffer { READY_SITE_WAKE_IO_ISR_DRAIN } else { READY_SITE_WAKE_IO_LOCKED };
            match thread.state {
                ThreadState::BlockedOnIO => {
                    thread.set_ready();
                    WAKE_SITE_WAKE_IO_LOCKED.fetch_add(1, Ordering::Relaxed);
                    record_ready_site(tid, site);
                    thread.wake_time_ns = None;
                    true
                }
                ThreadState::Blocked => {
                    thread.set_ready();
                    WAKE_SITE_WAKE_IO_LOCKED.fetch_add(1, Ordering::Relaxed);
                    record_ready_site(tid, site);
                    thread.wake_time_ns = None;
                    thread.blocked_in_syscall = false;
                    true
                }
                _ => thread.state == ThreadState::Ready && thread.blocked_in_syscall,
            }
        }
    };
    if !should_queue {
        return UnblockOutcome::NotFound;
    }
    let held_cpu = held.cpu;
    if current_here {
        ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
        if held_cpu != super::current_cpu_id_raw() {
            send_reschedule_vector_x86(held_cpu);
        }
    } else if held.queue().contains(&tid) {
        ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
    } else if !idle {
        let target = wake_target(held, tid);
        let queued = match source.as_deref_mut() {
            Some(source) => place_woken(local, Some(source), tid, target, false),
            None => place_woken(local, None, tid, target, false),
        };
        if queued.is_some() {
            match kind {
                Wake::Io { from_isr_buffer: true } => ENQUEUE_ISR_BUFFER_DRAINED_OK.fetch_add(1, Ordering::Relaxed),
                _ => ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed),
            };
        }
    }
    if let Wake::Io { .. } = kind {
        super::set_need_resched();
    }
    UnblockOutcome::Transitioned
}

/// `Slot::held` bits: an I/O wake (`Wake::Io`), a plain wake
/// (`Wake::Generic`), a wake for a signal or for a child's exit (`Notice`),
/// and a placement held for a pinned thread's home CPU.
pub(super) const HELD_IO: u64 = 1;
pub(super) const HELD_GENERIC: u64 = 2;
pub(super) const HELD_SIGNAL: u64 = 4;
pub(super) const HELD_CHILD_EXIT: u64 = 8;
pub(super) const HELD_PLACE: u64 = 16;
const HELD_BITS: u64 = 0xff;
/// Where a held placement's home CPU sits in `Slot::held`, and the tid.
const HELD_HOME_SHIFT: u32 = 8;
const HELD_TID_SHIFT: u32 = 16;

/// How many index slots hold wakes (`hold`): raised when a slot's word
/// becomes nonzero and lowered when it is taken, so a pass that finds 0 has
/// nothing held to apply.
static HELD_SLOTS: AtomicU32 = AtomicU32::new(0);

/// Hold the wakes `bits` of `tid` for a later pass, marked on its index slot:
/// unlike a wake buffer, which producers can fill meanwhile, the slot always
/// has room for them. `home` is the CPU a held placement waits for. The word
/// names the tid, so a mark left on a slot that a later thread reuses wakes
/// no one: the tid it names is no longer indexed. A tid not indexed has no
/// thread to wake.
///
/// The counts a word carries (`HELD_SLOTS`, and a held placement's home's
/// `PINNED_HOLDS_OUTSTANDING`) are raised before the word is visible, as a
/// taker may lower them the moment it is, and a placement this one replaces
/// gives its home's count back once it is off the slot.
pub(super) fn hold(tid: u64, bits: u64, home: usize) {
    let Some(slot) = find(tid) else {
        return;
    };
    let mut word = slot.held.load(Ordering::Acquire);
    loop {
        if word != 0 && word >> HELD_TID_SHIFT != tid {
            // The slot was reused by another thread meanwhile: this one is gone.
            return;
        }
        let places = bits & HELD_PLACE != 0;
        let home_field = if places {
            (home as u64 & 0xff) << HELD_HOME_SHIFT
        } else {
            word & (0xff << HELD_HOME_SHIFT)
        };
        let next = tid << HELD_TID_SHIFT | home_field | (word & HELD_BITS) | bits;
        let counts_slot = word == 0;
        let counts_home = places && home < MAX_CPUS;
        if counts_slot {
            HELD_SLOTS.fetch_add(1, Ordering::AcqRel);
        }
        if counts_home {
            super::PINNED_HOLDS_OUTSTANDING[home].fetch_add(1, Ordering::AcqRel);
        }
        match slot.held.compare_exchange(word, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                if places {
                    release_held_place(word);
                }
                return;
            }
            Err(now) => {
                if counts_slot {
                    HELD_SLOTS.fetch_sub(1, Ordering::AcqRel);
                }
                if counts_home {
                    release_held_place(next);
                }
                word = now;
            }
        }
    }
}

/// Take every held word and pass its tid, bits and placement home to `apply`.
/// `apply` re-holds what it cannot apply yet.
pub(super) fn take_held(mut apply: impl FnMut(u64, u64, usize)) {
    if HELD_SLOTS.load(Ordering::Acquire) == 0 {
        return;
    }
    for slot in INDEX.iter() {
        if slot.held.load(Ordering::Relaxed) == 0 {
            continue;
        }
        let word = slot.held.swap(0, Ordering::AcqRel);
        if word == 0 {
            continue;
        }
        HELD_SLOTS.fetch_sub(1, Ordering::AcqRel);
        release_held_place(word);
        apply(word >> HELD_TID_SHIFT, word & HELD_BITS, ((word >> HELD_HOME_SHIFT) & 0xff) as usize);
    }
}

/// Uncount a held placement that `word`, just taken off its slot, carries.
/// `hold` counted it for its home CPU, which treats itself as having work
/// until the count is back to zero: a count left behind kept that CPU's idle
/// loop from ever halting, and every sched_yield from taking its shortcut.
fn release_held_place(word: u64) {
    let home = ((word >> HELD_HOME_SHIFT) & 0xff) as usize;
    if word & HELD_PLACE != 0 && home < MAX_CPUS {
        let _ = super::PINNED_HOLDS_OUTSTANDING[home]
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |holds| holds.checked_sub(1));
    }
}

/// Apply the held wakes under `local`, re-holding those whose owner's lock is
/// still busy. A held placement is applied only on its home CPU.
fn apply_held(local: &mut Held) {
    take_held(|tid, bits, home| {
        let mut again = 0;
        if bits & HELD_IO != 0 && matches!(wake(local, tid, Wake::Io { from_isr_buffer: true }), Woken::Busy) {
            again |= HELD_IO;
        }
        if bits & HELD_GENERIC != 0 && matches!(wake(local, tid, Wake::Generic), Woken::Busy) {
            again |= HELD_GENERIC;
        }
        if bits & HELD_SIGNAL != 0 && !notify(local, tid, Notice::Signal) {
            again |= HELD_SIGNAL;
        }
        if bits & HELD_CHILD_EXIT != 0 && !notify(local, tid, Notice::ChildExit) {
            again |= HELD_CHILD_EXIT;
        }
        if bits & HELD_PLACE != 0 && !place_held(local, tid, home) {
            again |= HELD_PLACE;
        }
        if again != 0 {
            hold(tid, again, home);
            if let Some(owner) = owner(tid).filter(|&owner| owner != local.cpu) {
                kick(owner);
            }
        }
    });
}

/// Queue a pinned thread whose wake was held for its home CPU, `home`, once
/// this pass runs there: `Scheduler::deliver_pinned_wakes_for_this_cpu` for
/// one thread. True when nothing is left to do: placed, or no longer waiting
/// for a placement (placed by another path, blocked again, gone). False to
/// keep it held: this is not its home, its owner's lock is busy, or another
/// CPU is still leaving its stack.
fn place_held(local: &mut Held, tid: u64, home: usize) -> bool {
    if home != local.cpu {
        kick(home);
        return false;
    }
    let Some(owner) = owner(tid) else {
        return true;
    };
    if super::x86_stack_owner_elsewhere(tid, local.cpu).is_some() {
        return false;
    }
    let mut remote = None;
    let (current, queued, waiting) = {
        let source: &mut Held = if owner == local.cpu {
            &mut *local
        } else {
            match try_remote(owner) {
                Some(taken) if self::owner(tid) == Some(owner) => remote.insert(taken),
                _ => return false,
            }
        };
        let current = source.state().current_thread == Some(tid);
        let queued = source.queue().contains(&tid);
        let waiting = source.thread(tid).is_some_and(|thread| {
            thread.state == ThreadState::Ready
                && thread.cpu_affinity.is_some_and(|pin| pin.per_cpu_worker && pin.cpu == home)
        });
        (current, queued, waiting)
    };
    if !waiting || current || queued {
        return true;
    }
    // Moved with both locks held: the old owner's (`remote`) and this CPU's.
    local.enqueue(tid, false);
    drop(remote);
    super::PINNED_WAKES_DELIVERED.fetch_add(1, Ordering::Relaxed);
    ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
    true
}

/// Apply the wakes interrupt handlers buffered (`Scheduler::apply_isr_wakeups`),
/// and those held by earlier passes: one whose thread is owned by a CPU whose
/// lock is busy is held for the next pass (`hold`).
pub(super) fn apply_isr_wakeups(local: &mut Held) {
    let mut apply = |tid| {
        if let Woken::Busy = wake(local, tid, Wake::Io { from_isr_buffer: true }) {
            hold(tid, HELD_IO, 0);
        }
    };
    for buffer in ISR_WAKEUP_BUFFERS.iter() {
        buffer.take_each(&mut apply);
    }
    apply_held(local);
}

/// How many times a wake made outside a scheduling pass tries the lock of the
/// CPU that owns its thread before it holds the wake instead (`hold`). A lock
/// held for an ordinary pass is free within this many tries; one held by a
/// CPU whose vCPU has stopped is not, and the waker does not wait for it.
const OWNER_TRIES: usize = 256;

/// Wake `tid` with `kind` from outside a scheduling pass. When the lock of the
/// CPU that owns it stays busy, the wake is held on its index slot and that
/// CPU is interrupted: its next pass, or any CPU's, applies it. None when the
/// wake was held, or the run-queue locks do not exist yet.
pub(super) fn wake_or_hold(tid: u64, kind: Wake) -> Option<UnblockOutcome> {
    let mut local = local()?;
    for _ in 0..OWNER_TRIES {
        match wake(&mut local, tid, kind) {
            Woken::Done(outcome) => return Some(outcome),
            Woken::Busy => core::hint::spin_loop(),
        }
    }
    let bits = match kind {
        Wake::Generic => HELD_GENERIC,
        Wake::Io { .. } => HELD_IO,
    };
    hold(tid, bits, 0);
    drop(local);
    if let Some(owner) = owner(tid) {
        kick(owner);
    }
    None
}

/// Apply `notices` to `tid` from outside a scheduling pass, holding what its
/// owner's lock keeps out (`wake_or_hold`). False before the run-queue locks
/// exist.
pub(super) fn notify_or_hold(tid: u64, notices: &[Notice]) -> bool {
    let Some(mut local) = local() else {
        return false;
    };
    let mut left = 0;
    for &notice in notices {
        let bit = match notice {
            Notice::Signal => HELD_SIGNAL,
            Notice::ChildExit => HELD_CHILD_EXIT,
        };
        let mut done = false;
        for _ in 0..OWNER_TRIES {
            if notify(&mut local, tid, notice) {
                done = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !done {
            left |= bit;
        }
    }
    if left != 0 {
        hold(tid, left, 0);
        drop(local);
        if let Some(owner) = owner(tid) {
            kick(owner);
        }
    }
    true
}

/// Move what is queued on CPUs that have stopped scheduling, or are offline,
/// onto this one (`Scheduler::reclaim_unschedulable_cpu_queues`), and the
/// unpinned threads queued on CPUs that are not dispatching (`dispatching`).
/// A CPU whose lock is held is passed over.
fn reclaim_stalled(local: &mut Held) {
    let online = online();
    for cpu in 0..MAX_CPUS {
        if cpu == local.cpu {
            continue;
        }
        let offline = cpu >= online;
        let accepting = !offline && accepts_wakeups(cpu);
        if accepting && dispatching(cpu) {
            continue;
        }
        if offline && CPU_LOAD[cpu].load(Ordering::Relaxed) == 0 {
            continue;
        }
        let Some(mut remote) = try_remote(cpu) else {
            continue;
        };
        let candidates = remote.queue().len();
        let mut reclaimed = 0u64;
        for _ in 0..candidates {
            let Some(tid) = remote.queue().pop_front() else {
                break;
            };
            if accepting && remote.thread(tid).is_some_and(|thread| thread.cpu_affinity.is_some()) {
                remote.queue().push_back(tid);
                continue;
            }
            if retain_on_home(&mut remote, tid) || retain_affine(&mut remote, tid, local.cpu) {
                continue;
            }
            local.enqueue(tid, false);
            reclaimed += 1;
        }
        if reclaimed != 0 {
            if offline {
                super::ENQUEUE_OFFLINE_RECLAIMED.fetch_add(reclaimed, Ordering::Relaxed);
            } else {
                ENQUEUE_STALLED_RECLAIMED.fetch_add(reclaimed, Ordering::Relaxed);
            }
        }
    }
}

/// Take a runnable thread from `cpu`'s queue to run on `local`'s, if its lock
/// is free: the steal arms of `Scheduler::schedule`. `once` pops at most one
/// candidate, as the second pick does.
fn steal_from(local: &mut Held, cpu: usize, stack_waits: &mut u64, once: bool) -> Option<u64> {
    let mut remote = try_remote(cpu)?;
    let candidates = remote.queue().len();
    for tried in 0..candidates {
        promote_best_of(&mut remote, candidates - tried);
        let tid = remote.queue().pop_front()?;
        if let Some(owner) = x86_stack_owner_elsewhere(tid, local.cpu) {
            remote.queue().push_back(tid);
            *stack_waits |= 1 << owner;
            if once {
                return None;
            }
            continue;
        }
        if retain_on_home(&mut remote, tid) || retain_affine(&mut remote, tid, local.cpu) {
            if once {
                return None;
            }
            continue;
        }
        let (terminated, owner_pid) = remote
            .thread(tid)
            .map(|thread| (thread.state == ThreadState::Terminated, thread.owner_pid))
            .unwrap_or((true, None));
        if terminated {
            if owner_pid.is_some_and(|pid| !observe_exit_kick(pid)) {
                remote.queue().push_back(tid);
            }
            if once {
                return None;
            }
            continue;
        }
        set_owner(tid, local.cpu);
        return Some(tid);
    }
    None
}

/// `Scheduler::steal_higher_ranked` from the CPUs whose locks are free.
fn steal_higher_ranked(local: &mut Held, floor: u8, stack_waits: &mut u64) -> Option<u64> {
    let floor = floor.max(SchedPolicy::DEFAULT.rank());
    let mut best: Option<(u8, usize, u64)> = None;
    for cpu in 0..MAX_CPUS {
        if cpu == local.cpu || is_idle(cpu) {
            continue;
        }
        let Some(mut remote) = try_remote(cpu) else {
            continue;
        };
        for index in 0..remote.queue().len() {
            let tid = remote.queue()[index];
            let Some(thread) = remote.thread(tid) else {
                continue;
            };
            let rank = thread.sched.rank();
            let may_run_here = thread.state != ThreadState::Terminated
                && thread.cpu_affinity.map_or(true, |pin| pin.cpu == local.cpu);
            if may_run_here && rank > floor && best.map_or(true, |(best_rank, _, _)| rank > best_rank) {
                best = Some((rank, cpu, tid));
            }
        }
    }
    let (_, cpu, tid) = best?;
    let mut remote = try_remote(cpu)?;
    let index = remote.queue().iter().position(|&queued| queued == tid)?;
    remote.queue().make_contiguous()[..=index].rotate_right(1);
    let tid = remote.queue().pop_front()?;
    if let Some(owner) = x86_stack_owner_elsewhere(tid, local.cpu) {
        remote.queue().insert(index, tid);
        *stack_waits |= 1 << owner;
        return None;
    }
    if retain_on_home(&mut remote, tid) || retain_affine(&mut remote, tid, local.cpu) {
        return None;
    }
    if remote.thread(tid).map_or(true, |thread| thread.state == ThreadState::Terminated) {
        remote.queue().insert(index, tid);
        return None;
    }
    set_owner(tid, local.cpu);
    Some(tid)
}

/// Pop a runnable thread from `local`'s own queue, passing over threads
/// another CPU is still leaving and terminated ones.
fn pop_local(local: &mut Held, stack_waits: &mut u64) -> Option<u64> {
    let candidates = local.queue().len();
    for tried in 0..candidates {
        promote_best_of(local, candidates - tried);
        let tid = local.queue().pop_front()?;
        if let Some(owner) = x86_stack_owner_elsewhere(tid, local.cpu) {
            local.queue().push_back(tid);
            *stack_waits |= 1 << owner;
            continue;
        }
        let Some((terminated, owner_pid)) = local
            .thread(tid)
            .map(|thread| (thread.state == ThreadState::Terminated, thread.owner_pid))
        else {
            // Not this CPU's: a thread the whole-scheduler path moved without
            // dequeueing; whichever queue owns it now dispatches it.
            continue;
        };
        if terminated {
            if owner_pid.is_some_and(|pid| !observe_exit_kick(pid)) {
                local.queue().push_back(tid);
            }
            continue;
        }
        return Some(tid);
    }
    None
}

/// One quantum in ticks (`interrupts::timer::TIME_QUANTUM`): how long a CPU's
/// idle thread waits for the CPU while it carries boot work.
const BOOT_IDLE_WAIT_TICKS: u64 = 10;

/// The tick at which each CPU last switched away from its idle thread.
static IDLE_LEFT_TICK: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// `Scheduler::schedule` on x86_64 under this CPU's run-queue lock alone
/// (#1649). Returns the outgoing and the incoming thread, both owned here.
pub(super) fn schedule(local: &mut Held) -> Option<(u64, u64)> {
    let cpu = local.cpu;
    let now = crate::time::get_ticks();
    local.state().last_schedule_ticks = now;
    reclaim_stalled(local);
    apply_isr_wakeups(local);
    if crate::net::loopback_queue_has_work() {
        let pump = crate::net::loopback_pump_tid();
        if pump != 0 {
            match wake(local, pump, Wake::Generic) {
                Woken::Done(UnblockOutcome::Transitioned) => crate::net::record_loopback_pump_rearm_from_sched(),
                Woken::Busy => super::set_need_resched(),
                Woken::Done(_) => {}
            }
        }
    }
    // Waking first: see `Scheduler::schedule` (#568).
    expire_timers(local);

    let mut outgoing_holds_head = false;
    if let Some(current_id) = local.state().current_thread {
        if current_id != local.state().idle_thread {
            let (is_terminated, is_blocked, published_ready) = match local.thread(current_id) {
                Some(current) => {
                    let was_terminated = current.state == ThreadState::Terminated;
                    let was_blocked = current.state.is_blocked();
                    let published_ready = if !was_blocked && !was_terminated {
                        current.charge_cpu(crate::time::get_cpu_ticks());
                        current.set_ready();
                        WAKE_SITE_SCHEDULE.fetch_add(1, Ordering::Relaxed);
                        record_ready_site(current_id, READY_SITE_SCHEDULE);
                        true
                    } else {
                        current.run_start_ticks = crate::time::get_cpu_ticks();
                        false
                    };
                    (was_terminated, was_blocked, published_ready)
                }
                None => (true, false, false),
            };
            let in_queue = local.queue().contains(&current_id);
            let will_add = !is_terminated && !is_blocked && !in_queue;
            outgoing_holds_head = settle_outgoing(local, current_id, published_ready);
            if will_add {
                // `retain_affine` may queue it on its home CPU, whose lock is
                // released at once: that CPU must find this one still on its
                // stack, or it dispatches the thread from a context not yet
                // saved while this CPU runs on its kernel stack.
                LEAVING_STACK[cpu].store(current_id, Ordering::Release);
                if !retain_affine(local, current_id, cpu) {
                    local.enqueue(current_id, outgoing_holds_head);
                }
                if published_ready {
                    ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
                }
            } else if published_ready && in_queue {
                ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    // Until a thread reaches user mode, a CPU's idle thread resumes its saved
    // context when dispatched (`switch_to_thread`), and on the boot CPU that
    // context is the boot coordinator, which the boot tests' kernel threads
    // wait on. Idle is kept out of the run queues, so the picks below reach
    // it only when every queue is empty or through the lone-thread handoff
    // further down. A worker spinning for the coordinator's flag while
    // fbcon's render thread woke every 20 ms kept one or the other queued at
    // every pass, each switch began a fresh quantum, and the coordinator was
    // not picked again until something else set the flag (#1848: 5 of 5
    // forced preemptions). Here it gets the CPU once it has waited a quantum,
    // until its next tick.
    let idle_thread = local.state().idle_thread;
    let boot_idle_due = local.state().current_thread.is_some_and(|tid| tid != idle_thread)
        && !crate::syscall::handler::is_ring3_confirmed()
        && now.wrapping_sub(IDLE_LEFT_TICK[cpu].load(Ordering::Relaxed)) >= BOOT_IDLE_WAIT_TICKS;
    let mut stack_waits = 0u64;
    let stolen = if !boot_idle_due && POLICIES_IN_USE.load(Ordering::Acquire) {
        let floor = best_queued_rank(local, None);
        steal_higher_ranked(local, floor, &mut stack_waits)
    } else {
        None
    };
    let mut next = match stolen {
        Some(tid) => tid,
        None if boot_idle_due => idle_thread,
        None => match pop_local(local, &mut stack_waits) {
            Some(tid) => tid,
            None => (0..MAX_CPUS)
                .filter(|&other| other != cpu)
                .find_map(|other| steal_from(local, other, &mut stack_waits, false))
                .unwrap_or(idle_thread),
        },
    };
    super::wait_for_stacks_x86(core::mem::take(&mut stack_waits), cpu);

    let others_queued = super::QUEUED_CPUS.load(Ordering::Acquire) & !(1u64 << cpu) != 0;
    let any_queued = !local.queue().is_empty() || others_queued;
    let current = local.state().current_thread;
    let idle = local.state().idle_thread;
    if Some(next) == current && next != idle && any_queued && !policy_keeps_cpu(local, next, outgoing_holds_head) {
        local.enqueue(next, outgoing_holds_head);
        let found = pop_local(local, &mut stack_waits).or_else(|| {
            (0..MAX_CPUS)
                .filter(|&other| other != cpu && !is_idle(other))
                .find_map(|other| steal_from(local, other, &mut stack_waits, true))
        });
        super::wait_for_stacks_x86(core::mem::take(&mut stack_waits), cpu);
        match found {
            Some(tid) => next = tid,
            None => {
                // Everything queued was declined: the current thread keeps
                // this CPU, and leaves the queue it was put back on.
                if let Some(position) = local.queue().iter().position(|&tid| tid == next) {
                    local.queue().remove(position);
                }
                if let Some(thread) = local.thread(next) {
                    thread.set_running();
                }
                return None;
            }
        }
    } else if Some(next) == current {
        if next == idle {
            return None;
        }
        let user = local
            .thread(next)
            .is_some_and(|thread| thread.privilege == crate::task::thread::ThreadPrivilege::User);
        if online() > 1 && user {
            if let Some(thread) = local.thread(next) {
                thread.set_running();
            }
            return None;
        }
        local.enqueue(next, false);
        next = idle;
        crate::per_cpu::set_need_resched(true);
    }

    let old = current.unwrap_or(idle);
    if old == idle {
        IDLE_LEFT_TICK[cpu].store(now, Ordering::Relaxed);
    }
    local.state().current_thread = Some(next);
    set_owner(next, cpu);
    LEAVING_STACK[cpu].store(old, Ordering::Release);
    CONTEXT_SWITCH_COUNT.fetch_add(1, Ordering::Relaxed);
    if let Some(thread) = local.thread(next) {
        Scheduler::set_selected_running(thread, cpu);
    }
    let state = local.state();
    let promoted = state.promoted_wake == Some(next);
    if promoted {
        state.promoted_wake = None;
    }
    state.last_dispatch_promoted = promoted;
    if find(old).is_none() || find(next).is_none() {
        return None;
    }
    Some((old, next))
}

/// `x86_idle_pass` under this CPU's lock: entry stamp, buffered wakes and due
/// deadlines; whether a full pass has work to find.
pub(super) fn idle_pass(local: &mut Held) -> bool {
    let cpu = local.cpu;
    local.state().last_schedule_ticks = crate::time::get_ticks();
    apply_isr_wakeups(local);
    expire_timers(local);
    super::PINNED_HOLDS_OUTSTANDING[cpu].load(Ordering::Relaxed) != 0
        || !local.queue().is_empty()
        || super::QUEUED_CPUS.load(Ordering::Acquire) & !(1u64 << cpu) != 0
}

// ------------------------------------------------- single-thread operations

/// Run `f` on `tid` under the lock of the CPU that owns it, waiting for that
/// lock and following the thread when it changes owner meanwhile; never the
/// whole scheduler. None when the thread is not indexed, or before the live
/// scheduler exists. The caller holds no run-queue lock and has interrupts
/// masked.
///
/// For changes to one thread's own scheduling state (its policy, its pin,
/// its termination): a CPU stopped while it holds its own lock delays only
/// callers that change a thread it owns. Wakes do not wait (`wake_or_hold`).
pub(super) fn with_thread<R>(tid: u64, f: impl FnOnce(&mut Held, &mut Thread) -> R) -> Option<R> {
    let me = super::current_cpu_id_raw();
    if me < MAX_CPUS && SCHEDULER_HELD[me].load(Ordering::Relaxed) != 0 {
        panic!("scheduler: CPU {} waited for a thread's run-queue lock while holding a scheduler lock", me);
    }
    loop {
        let owner = owner(tid)?;
        let mut held = if owner == me {
            local()?
        } else {
            match try_remote(owner) {
                Some(held) => held,
                None if LIVE.load(Ordering::Acquire).is_null() => return None,
                None => {
                    // Waited for with this CPU's work handed to its peers
                    // meanwhile (`super::LockSpin`).
                    let mut spin = super::LockSpin::new();
                    let taken = loop {
                        if let Some(held) = try_remote(owner) {
                            break Some(held);
                        }
                        if self::owner(tid) != Some(owner) {
                            break None;
                        }
                        spin.tick();
                        core::hint::spin_loop();
                    };
                    match taken {
                        Some(held) => held,
                        None => continue,
                    }
                }
            }
        };
        if self::owner(tid) != Some(owner) {
            continue;
        }
        let thread: *mut Thread = held.thread(tid)?;
        // SAFETY: `held` covers the thread for this call; `f` gets the only
        // reference to it.
        return Some(f(&mut held, unsafe { &mut *thread }));
    }
}

/// `switch_to_idle` under this CPU's lock. False when its idle thread is not
/// its own, for the caller to take the whole scheduler.
pub(super) fn switch_to_idle(local: &mut Held) -> bool {
    let idle = local.state().idle_thread;
    let Some(thread) = local.thread(idle) else {
        return false;
    };
    let ptr = thread as *mut Thread;
    local.state().current_thread = Some(idle);
    crate::per_cpu::set_current_thread(ptr);
    true
}

/// `requeue_refused_dispatch` under this CPU's lock. False when the thread is
/// not this CPU's.
pub(super) fn requeue_refused_dispatch(local: &mut Held, tid: u64) -> bool {
    if local.state().idle_thread == tid || local.state().current_thread == Some(tid) {
        return true;
    }
    let Some(thread) = local.thread(tid) else {
        return false;
    };
    if thread.state != ThreadState::Ready || local.queue().contains(&tid) {
        return true;
    }
    let cpu = local.cpu;
    if !retain_affine(local, tid, cpu) {
        local.enqueue(tid, false);
    }
    true
}

/// `abort_dispatch_and_resume` under this CPU's lock. False, changing
/// nothing, when either thread is not this CPU's.
pub(super) fn abort_dispatch_and_resume(local: &mut Held, aborted: u64, resume: u64) -> bool {
    let cpu = local.cpu;
    {
        let (a, r) = local.two(aborted, resume);
        if a.is_none() || r.is_none() {
            return false;
        }
    }
    let should_queue = match local.thread(aborted) {
        Some(thread) if thread.state != ThreadState::Terminated => {
            thread.set_ready();
            true
        }
        _ => false,
    };
    if should_queue && !local.queue().contains(&aborted) && !retain_affine(local, aborted, cpu) {
        local.enqueue(aborted, false);
    }
    let Some(thread) = local.thread(resume) else {
        return true;
    };
    let resume_runnable = matches!(thread.state, ThreadState::Ready | ThreadState::Running);
    if resume_runnable {
        thread.set_running();
    }
    let kernel_stack_top = thread.kernel_stack_top;
    let ptr = thread as *mut Thread;
    if resume_runnable {
        if let Some(position) = local.queue().iter().position(|&tid| tid == resume) {
            local.queue().remove(position);
        }
    }
    local.state().current_thread = Some(resume);
    crate::arch_impl::x86_64::fpu::hand_over_with(None, resume, |tid| {
        local.thread(tid).map(|thread| thread as *mut Thread)
    });
    crate::per_cpu::set_current_thread(ptr);
    if let Some(top) = kernel_stack_top {
        crate::per_cpu::update_tss_rsp0(top.as_u64());
    }
    true
}

/// The switch's hand-over to `incoming` under this CPU's lock: the x87/SSE
/// registers, the per-CPU current thread and TSS.RSP0, and `f` reading how
/// the thread resumes. None when `incoming` is not this CPU's.
pub(super) fn switch_in<R>(
    local: &mut Held,
    outgoing: Option<u64>,
    incoming: u64,
    f: impl FnOnce(&Thread, bool) -> R,
) -> Option<R> {
    let is_idle = local.state().idle_thread == incoming;
    local.thread(incoming)?;
    let cpu = local.cpu;
    crate::arch_impl::x86_64::fpu::hand_over_with(outgoing, incoming, |tid| {
        local.thread(tid).map(|thread| thread as *mut Thread).or_else(|| leaving_thread(cpu, tid))
    });
    let thread = local.thread(incoming)?;
    crate::per_cpu::set_current_thread(thread as *mut Thread);
    if let Some(top) = thread.kernel_stack_top {
        crate::per_cpu::update_tss_rsp0(top.as_u64());
    }
    Some(f(thread, is_idle))
}

/// `tid`, the thread `cpu` is switching away from, when another CPU owns it
/// already: the scheduling pass queued it on its home CPU (`retain_affine`),
/// or a wake moved it once the pass released this CPU's lock. Its x87/SSE
/// registers are still this CPU's to save. Only its `fpu` image may be
/// written through the pointer: no other CPU dispatches the thread while
/// `LEAVING_STACK` names it here, and every other reader of that image holds
/// the whole scheduler, as does reclamation; this CPU's lock, which the
/// caller holds, is part of that.
fn leaving_thread(cpu: usize, tid: u64) -> Option<*mut Thread> {
    if tid == 0 || LEAVING_STACK[cpu].load(Ordering::Acquire) != tid {
        return None;
    }
    let thread = find(tid)?.thread.load(Ordering::Acquire);
    (!thread.is_null()).then_some(thread)
}

// ------------------------------------------- the current thread's waits

/// Block the thread this CPU runs, as `Scheduler::block_current_*` do, under
/// this CPU's lock: it is this CPU's, as is any deadline it sleeps to.
pub(super) enum Block {
    /// `block_current` / `block_current_in_syscall`.
    Plain { in_syscall: bool },
    /// `block_current_for_timer`.
    Timer { wake_time_ns: u64 },
    /// `block_current_for_io_with_timeout`.
    Io { wake_time_ns: Option<u64> },
    /// `block_current_for_signal_with_context`.
    Signal { context: Option<crate::task::thread::CpuContext> },
    /// `block_current_for_child_exit`.
    ChildExit,
}

/// Whether the current thread published the blocked state.
pub(super) fn block_current(local: &mut Held, block: Block) -> bool {
    let Some(tid) = local.state().current_thread else {
        return false;
    };
    let mut push_deadline = None;
    let entry_here = match block {
        Block::Timer { wake_time_ns } => local.heap().iter().any(|&Reverse(entry)| entry == (wake_time_ns, tid)),
        _ => false,
    };
    {
        let Some(thread) = local.thread(tid) else {
            return false;
        };
        if thread.must_not_sleep() {
            return false;
        }
        thread.charge_cpu(crate::time::get_cpu_ticks());
        match block {
            Block::Plain { in_syscall } => {
                thread.stop_timer_cpu();
                thread.state = ThreadState::Blocked;
                if in_syscall {
                    thread.blocked_in_syscall = true;
                }
            }
            Block::Timer { wake_time_ns } => {
                // The entry an earlier block pushed counts only if it is in
                // this CPU's heap: a sleep stopped and continued before its
                // deadline can resume on another CPU, whose expiry would
                // never reach the entry the old CPU's heap keeps.
                let already_armed = thread.timer_pop.is_some_and(|record| {
                    entry_here
                        && record.deadline_ns == wake_time_ns
                        && record.own_entry == crate::task::thread::TimerPop::NotPopped
                });
                thread.stop_timer_cpu();
                thread.state = ThreadState::BlockedOnTimer;
                thread.wake_time_ns = Some(wake_time_ns);
                if !already_armed {
                    thread.timer_pop = Some(crate::task::thread::TimerPopRecord::armed(wake_time_ns));
                    push_deadline = Some(wake_time_ns);
                }
                thread.blocked_in_syscall = thread.owner_pid.is_some();
            }
            Block::Io { wake_time_ns } => {
                thread.stop_timer_cpu();
                thread.state = ThreadState::BlockedOnIO;
                thread.wake_time_ns = wake_time_ns;
                thread.timer_pop = wake_time_ns.map(crate::task::thread::TimerPopRecord::armed);
                thread.blocked_in_syscall = thread.owner_pid.is_some();
                push_deadline = wake_time_ns;
            }
            Block::Signal { context } => {
                if let Some(context) = context {
                    thread.saved_userspace_context = Some(context);
                }
                thread.stop_timer_cpu();
                thread.state = ThreadState::BlockedOnSignal;
                thread.blocked_in_syscall = true;
            }
            Block::ChildExit => {
                thread.stop_timer_cpu();
                thread.state = ThreadState::BlockedOnChildExit;
                thread.blocked_in_syscall = true;
            }
        }
    }
    if let Some(deadline) = push_deadline {
        local.heap().push(Reverse((deadline, tid)));
    }
    local.remove_from_ready_queue(tid);
    // Publish the sleep state before later condition checks or schedule entry
    // observe wakeups (`Scheduler::block_current_for_io_publish`).
    core::sync::atomic::fence(Ordering::SeqCst);
    true
}

// --------------------------------------------- signal and child-exit wakes

/// What a wake for a signal or a child's exit does to `tid`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Notice {
    /// `Scheduler::unblock_for_signal`.
    Signal,
    /// `Scheduler::unblock_for_child_exit`.
    ChildExit,
}

/// Apply `notice` to `tid` under the lock of the CPU that owns it: `local`'s,
/// or a peer's taken without waiting. False when that lock is busy.
pub(super) fn notify(local: &mut Held, tid: u64, notice: Notice) -> bool {
    let Some(owner) = owner(tid) else {
        return true;
    };
    if owner == local.cpu {
        notify_owned(local, None, tid, notice);
        return true;
    }
    let Some(mut remote) = try_remote(owner) else {
        return false;
    };
    if self::owner(tid) != Some(owner) {
        return false;
    }
    notify_owned(local, Some(&mut remote), tid, notice);
    true
}

fn notify_owned(local: &mut Held, mut source: Option<&mut Held>, tid: u64, notice: Notice) {
    let held: &mut Held = match source.as_deref_mut() {
        Some(source) => source,
        None => &mut *local,
    };
    let Some(state) = held.thread(tid).map(|thread| (thread.state, thread.blocked_in_syscall)) else {
        return;
    };
    match (notice, state) {
        (Notice::Signal, (ThreadState::BlockedOnIO, _)) => {
            wake_owned(local, source.as_deref_mut(), tid, Wake::Io { from_isr_buffer: false });
            expedite(local, source, tid);
        }
        (Notice::Signal, (ThreadState::Blocked, true)) | (Notice::Signal, (ThreadState::BlockedOnTimer, _)) => {
            wake_owned(local, source.as_deref_mut(), tid, Wake::Generic);
            expedite(local, source, tid);
        }
        (Notice::Signal, (ThreadState::BlockedOnSignal, _)) => {
            mark_ready_and_place(local, source.as_deref_mut(), tid, &super::WAKE_SITE_SIGNAL, super::READY_SITE_SIGNAL);
            super::set_need_resched();
            expedite(local, source, tid);
        }
        (Notice::Signal, _) => expedite(local, source, tid),
        (Notice::ChildExit, (ThreadState::BlockedOnChildExit, _)) => {
            mark_ready_and_place(
                local,
                source.as_deref_mut(),
                tid,
                &super::WAKE_SITE_CHILD_EXIT,
                super::READY_SITE_CHILD_EXIT,
            );
            super::set_need_resched();
        }
        (Notice::ChildExit, _) => {}
    }
}

/// Make `tid`, blocked and owned by `source`'s CPU (`local`'s when None),
/// Ready, and queue it unless it is still current there or already queued:
/// the shared arm of the signal and child-exit wakes.
fn mark_ready_and_place(
    local: &mut Held,
    mut source: Option<&mut Held>,
    tid: u64,
    site: &AtomicU64,
    ready_site: u64,
) {
    let held: &mut Held = match source.as_deref_mut() {
        Some(source) => source,
        None => &mut *local,
    };
    let current_here = held.state().current_thread == Some(tid);
    let idle = held.state().idle_thread == tid;
    let Some(thread) = held.thread(tid) else {
        return;
    };
    thread.set_ready();
    site.fetch_add(1, Ordering::Relaxed);
    record_ready_site(tid, ready_site);
    if current_here {
        ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
    } else if held.queue().contains(&tid) {
        ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
    } else if !idle {
        let target = wake_target(held, tid);
        let queued = match source.as_deref_mut() {
            Some(source) => place_woken(local, Some(source), tid, target, false),
            None => place_woken(local, None, tid, target, false),
        };
        if queued.is_some() {
            ENQUEUE_SAME_LOCK_OK.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// `Scheduler::expedite_signal_recipient` for `tid`, on the CPU that owns it
/// now: the wake before this can have queued it on another CPU than
/// `source`'s (`local`'s when None), `local`'s included. A third CPU's lock is
/// only tried; when it is busy that CPU is asked to reschedule.
fn expedite(local: &mut Held, source: Option<&mut Held>, tid: u64) {
    let Some(cpu) = owner(tid) else {
        return;
    };
    let mut remote = None;
    let held: &mut Held = match source {
        Some(source) if source.cpu == cpu => source,
        _ if local.cpu == cpu => local,
        _ => match try_remote(cpu) {
            Some(taken) if owner(tid) == Some(cpu) => remote.insert(taken),
            _ => {
                kick(cpu);
                return;
            }
        },
    };
    let me = super::current_cpu_id_raw();
    if held.state().current_thread == Some(tid) {
        if cpu != me {
            send_reschedule_vector_x86(cpu);
        }
        return;
    }
    if !held.queue().contains(&tid) {
        return;
    }
    if !retain_affine(held, tid, cpu) && promotion_open(held) {
        held.queue().retain(|&queued| queued != tid);
        held.queue().push_front(tid);
        held.state().promoted_wake = Some(tid);
    }
    if cpu == me {
        super::set_need_resched();
    } else {
        request_resched_x86(cpu);
    }
}

// ------------------------------------------------------- whole-index scans

/// Whether a thread that has not terminated caches a page-table root
/// `matches` accepts (`Scheduler::any_cached_ttbr0_matches`), from the roots
/// the thread index publishes (`publish_root`), without any lock. None before
/// the live scheduler exists. A dead process's roots are cached by fewer
/// threads over time, never more, and retirement asks again until none is.
pub(super) fn any_cached_root(mut matches: impl FnMut(u64) -> bool) -> Option<bool> {
    if LIVE.load(Ordering::Acquire).is_null() {
        return None;
    }
    Some(INDEX.iter().any(|slot| {
        let key = slot.key.load(Ordering::Acquire);
        let root = slot.root.load(Ordering::Acquire);
        key != EMPTY && key != REMOVED && root != 0 && matches(root)
    }))
}

// ------------------------------------------------------------ new threads

/// A published thread whose box the thread list does not hold yet.
struct Spawned {
    thread: Box<Thread>,
    next: *mut Spawned,
}

/// Threads published since the scheduler-wide lock was last taken, newest
/// first (`splice_spawned`).
static SPAWNED: AtomicPtr<Spawned> = AtomicPtr::new(core::ptr::null_mut());

/// Move the boxes of the threads published since into `scheduler`'s thread
/// list, oldest first. The scheduler-wide lock is held.
pub(super) fn splice_spawned(scheduler: &mut Scheduler) {
    let mut node = SPAWNED.swap(core::ptr::null_mut(), Ordering::AcqRel);
    let start = scheduler.threads.len();
    while !node.is_null() {
        // SAFETY: each node was leaked by `spawn` and is taken off the list
        // once, here, by the swap above.
        let spawned = unsafe { Box::from_raw(node) };
        node = spawned.next;
        scheduler.admit_thread(spawned.thread);
    }
    scheduler.threads[start..].reverse();
}

/// Publish `thread` and queue it: on the CPU its pin names, or on the least
/// loaded CPU. Only that CPU's lock is taken, or this CPU's when the target's
/// stays busy; a pinned thread whose home's lock stays busy is held for it.
/// Returns the CPU it went to; the thread back before the live scheduler
/// exists. Interrupts are masked.
pub(super) fn spawn(thread: Box<Thread>, front: bool) -> Result<usize, Box<Thread>> {
    if LIVE.load(Ordering::Acquire).is_null() {
        return Err(thread);
    }
    let me = super::current_cpu_id_raw();
    let online = online();
    let pin = thread.cpu_affinity;
    let pinned = pin.map(|pin| pin.cpu).filter(|&cpu| cpu < online);
    let target = pinned.unwrap_or_else(|| least_loaded(me));
    let node = Box::into_raw(Box::new(Spawned { thread, next: core::ptr::null_mut() }));
    // SAFETY: the node is ours until it is pushed below.
    let thread: &mut Thread = unsafe { &mut *(*node).thread };
    if pin.is_some() && pinned.is_none() {
        // An out-of-range pin is counted and dropped (`Scheduler::add_thread`).
        super::PINNED_PUBLISH_DISCARDED.fetch_add(1, Ordering::Relaxed);
        thread.cpu_affinity = None;
    }
    let tid = thread.id();
    let held = if target == me {
        local()
    } else {
        let mut taken = None;
        for _ in 0..OWNER_TRIES {
            taken = try_remote(target);
            if taken.is_some() {
                break;
            }
            core::hint::spin_loop();
        }
        match taken {
            Some(held) => Some(held),
            None => local(),
        }
    };
    let Some(mut held) = held else {
        // SAFETY: not pushed; the node is still ours.
        return Err(unsafe { Box::from_raw(node) }.thread);
    };
    insert(thread, held.cpu);
    let queued_on = held.cpu;
    let held_for_home = pinned.is_some_and(|home| home != queued_on);
    if !held_for_home {
        held.enqueue(tid, front);
    }
    // The box goes on the list before this lock is released, so a holder of
    // the whole scheduler, which takes this lock too, finds it there.
    let mut head = SPAWNED.load(Ordering::Acquire);
    loop {
        // SAFETY: the node is not yet on the list.
        unsafe { (*node).next = head };
        match SPAWNED.compare_exchange_weak(head, node, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(now) => head = now,
        }
    }
    drop(held);
    if let Some(home) = pinned.filter(|_| held_for_home) {
        super::note_pinned_hold(tid, home);
        kick(home);
        return Ok(home);
    }
    Ok(queued_on)
}

// ------------------------------------------------------------- reclamation

/// Terminated threads one reclamation pass takes. An x86 idle dispatch
/// restarts the idle loop rather than resuming it, so a pass's
/// non-preemptible release stays bounded; taking more than one lets their
/// kernel stacks share one flush of the kernel's translations
/// (`kernel_stack::release_batch`), most of what freeing a thread costs. One
/// a pass freed about 150 threads a second on the x86 gate, and a process
/// creating threads faster than that ran out of thread slots (#1712).
const RECLAIM_BATCH: usize = 16;

/// Free up to `RECLAIM_BATCH` terminated threads whose stacks a CPU can no
/// longer be on: the scheduler-wide lock is held (`scheduler`), and only the
/// run-queue lock of the CPU that owns each thread is taken, without waiting.
/// A thread dropped from the index while a pin is held is kept until the last
/// pin is released (`Scheduler::unpinned_free`). Returns the threads to free.
pub(super) fn reclaim(scheduler: &mut Scheduler) -> alloc::vec::Vec<Box<Thread>> {
    splice_spawned(scheduler);
    let mut freed = alloc::vec::Vec::new();
    if !scheduler.unpinned_free.is_empty() && unpinned() {
        freed.append(&mut scheduler.unpinned_free);
    }
    let dead: alloc::vec::Vec<u64> = INDEX
        .iter()
        .filter_map(|slot| {
            let key = slot.key.load(Ordering::Acquire);
            (live_key(key) && slot.flags.load(Ordering::Acquire) & DEAD != 0).then(|| key - 1)
        })
        .collect();
    scheduler.retirement_grace.retain(|grace| dead.contains(&grace.thread_id));
    for &tid in dead.iter() {
        if !scheduler.retirement_grace.iter().any(|grace| grace.thread_id == tid) {
            scheduler.retirement_grace.push(super::RetirementGrace {
                thread_id: tid,
                after_epoch: super::retirement_grace_target(),
            });
        }
    }
    let me = super::current_cpu_id_raw();
    let mut reclaimed = alloc::vec::Vec::with_capacity(RECLAIM_BATCH);
    for &tid in dead.iter() {
        if reclaimed.len() == RECLAIM_BATCH {
            break;
        }
        let elapsed = scheduler
            .retirement_grace
            .iter()
            .find(|grace| grace.thread_id == tid)
            .is_some_and(|grace| super::retirement_grace_elapsed(&grace.after_epoch));
        if !elapsed || LEAVING_STACK.iter().any(|leaving| leaving.load(Ordering::Acquire) == tid) {
            continue;
        }
        let Some(owner) = owner(tid) else {
            continue;
        };
        let held = if owner == me { local() } else { try_remote(owner) };
        let Some(mut held) = held else {
            continue;
        };
        if self::owner(tid) != Some(owner)
            || held.state().current_thread == Some(tid)
            || held.state().idle_thread == tid
        {
            continue;
        }
        let Some(thread) = held.thread(tid) else {
            continue;
        };
        let stack_live = thread
            .kernel_stack_top
            .is_some_and(|top| crate::memory::kernel_stack::is_kernel_stack_slot_live(top.as_u64()));
        if thread.state != ThreadState::Terminated || stack_live {
            continue;
        }
        let address = thread as *const Thread;
        let Some(at) = scheduler.threads.iter().position(|boxed| core::ptr::eq(&**boxed, address)) else {
            continue;
        };
        held.remove_from_ready_queue(tid);
        remove(tid);
        drop(held);
        scheduler.retirement_grace.retain(|grace| grace.thread_id != tid);
        reclaimed.push(scheduler.threads.remove(at));
    }
    let reclaimed_count = reclaimed.len();
    if unpinned() {
        freed.append(&mut reclaimed);
    } else {
        scheduler.unpinned_free.append(&mut reclaimed);
    }
    // What is left to do: terminated threads still indexed, and threads
    // waiting for the last pin to go.
    let left = dead.len() - reclaimed_count + scheduler.unpinned_free.len();
    super::TERMINATED_UNRECLAIMED.store(left as u64, Ordering::Release);
    freed
}

// ------------------------------------------------------- lock-free scans

/// Whether a thread that runs user code has not terminated.
pub(super) fn any_live_user_thread() -> bool {
    INDEX.iter().any(|slot| {
        live_key(slot.key.load(Ordering::Acquire)) && slot.flags.load(Ordering::Acquire) & (USER | DEAD) == USER
    })
}

/// The threads of process row `pid`, as their tids, with whether each has
/// terminated, passed to `f`.
fn each_of_pid(pid: u64, mut f: impl FnMut(u64, bool)) {
    if pid == 0 {
        return;
    }
    for slot in INDEX.iter() {
        let key = slot.key.load(Ordering::Acquire);
        if live_key(key) && slot.pid.load(Ordering::Acquire) == pid {
            f(key - 1, slot.flags.load(Ordering::Acquire) & DEAD != 0);
        }
    }
}

/// `f` on `tid` under its owner's lock, tried `OWNER_TRIES` times: None when
/// that lock stays busy, the thread is not indexed, or before the live
/// scheduler exists. Interrupts masked.
fn try_with_thread<R>(tid: u64, f: impl FnOnce(&mut Held, &mut Thread) -> R) -> Option<R> {
    let me = super::current_cpu_id_raw();
    for _ in 0..OWNER_TRIES {
        let owner = owner(tid)?;
        let held = if owner == me { local() } else { try_remote(owner) };
        let Some(mut held) = held else {
            core::hint::spin_loop();
            continue;
        };
        if self::owner(tid) != Some(owner) {
            continue;
        }
        let thread: *mut Thread = held.thread(tid)?;
        // SAFETY: as `with_thread`.
        return Some(f(&mut held, unsafe { &mut *thread }));
    }
    None
}

/// The display state of process row `pid` from its threads
/// (`get_process_display_state`): None when it has none, or a thread's
/// owner's lock stayed busy.
pub(super) fn display_state(pid: u64) -> Option<&'static str> {
    let (mut found, mut running, mut ready, mut disk_sleep, mut blocked, mut terminated, mut unknown) =
        (false, false, false, false, false, false, false);
    each_of_pid(pid, |tid, dead| {
        found = true;
        if dead {
            terminated = true;
            return;
        }
        match try_with_thread(tid, |_, thread| (thread.state, thread.uninterruptible_io)) {
            Some((ThreadState::Running, _)) => running = true,
            Some((ThreadState::Ready, _)) => ready = true,
            Some((ThreadState::Terminated, _)) => terminated = true,
            Some((ThreadState::BlockedOnIO, true)) => disk_sleep = true,
            Some(_) => blocked = true,
            None => unknown = true,
        }
    });
    if !found || unknown {
        None
    } else if running {
        Some("Running")
    } else if ready {
        Some("Ready")
    } else if disk_sleep {
        Some("Disk sleep")
    } else if blocked {
        Some("Blocked")
    } else if terminated {
        Some("Terminated")
    } else {
        None
    }
}

/// Run `f` on the thread each online CPU runs and on those queued there,
/// under that CPU's lock; a CPU whose lock stays busy for `OWNER_TRIES` tries
/// is passed over, and its threads are charged at their own next charge.
/// Interrupts masked.
fn each_runnable(mut f: impl FnMut(&mut Thread)) {
    let me = super::current_cpu_id_raw();
    for cpu in 0..online() {
        let held = if cpu == me {
            local()
        } else {
            (0..OWNER_TRIES).find_map(|_| {
                let taken = try_remote(cpu);
                if taken.is_none() {
                    core::hint::spin_loop();
                }
                taken
            })
        };
        let Some(mut held) = held else {
            continue;
        };
        if let Some(tid) = held.state().current_thread {
            if let Some(thread) = held.thread(tid) {
                f(thread);
            }
        }
        for index in 0..held.queue().len() {
            let tid = held.queue()[index];
            if let Some(thread) = held.thread(tid) {
                f(thread);
            }
        }
    }
}

/// Charge the open run interval of every runnable thread charged to
/// `account` (`Scheduler::charge_account_threads`).
pub(super) fn charge_account(account: &alloc::sync::Arc<crate::task::thread::CpuAccount>) {
    each_runnable(|thread| {
        if thread.cpu_account.as_ref().is_some_and(|own| alloc::sync::Arc::ptr_eq(own, account)) {
            thread.charge_timer_cpu();
        }
    });
}

/// Charge the open run interval of every runnable thread that receives the
/// timers at `group` (`group_of`).
pub(super) fn charge_group(group: usize) {
    each_runnable(|thread| {
        if group_of(thread) == group {
            thread.charge_timer_cpu();
        }
    });
}

/// Charge the running threads of process row `pid` (`charge_process_cpu`).
pub(super) fn charge_pid(pid: u64) {
    let now = crate::time::get_cpu_ticks();
    each_runnable(|thread| {
        if thread.owner_pid == Some(pid) {
            thread.charge_cpu_if_running(now);
        }
    });
}

/// Whether a thread that receives the timers at `group` has not terminated.
pub(super) fn group_has_live_thread(group: usize) -> bool {
    INDEX.iter().any(|slot| {
        live_key(slot.key.load(Ordering::Acquire))
            && slot.group.load(Ordering::Acquire) == group
            && slot.flags.load(Ordering::Acquire) & DEAD == 0
    })
}

/// The thread that receives a timer signal `bit` sent to the process whose
/// timers are at `group`: the first that does not block it or waits for it,
/// else the first (`Scheduler::wake_signal_timers`). Read through `pin`.
pub(super) fn group_recipient(pin: &Pin, group: usize, bit: u64) -> Option<u64> {
    let mut first = None;
    for slot in INDEX.iter() {
        let key = slot.key.load(Ordering::Acquire);
        if !live_key(key) || slot.group.load(Ordering::Acquire) != group || slot.flags.load(Ordering::Acquire) & DEAD != 0 {
            continue;
        }
        let tid = key - 1;
        let Some(thread) = pin.thread(tid) else {
            continue;
        };
        let signals = &thread.signals;
        if (!signals.blocked.load(Ordering::Relaxed) | signals.wait_set.load(Ordering::Acquire)) & bit != 0 {
            return Some(tid);
        }
        first.get_or_insert(tid);
    }
    first
}

/// Each thread that receives the timers at `group`, with its signal state,
/// read through `pin`.
pub(super) fn each_of_group<'a>(pin: &'a Pin, group: usize, mut f: impl FnMut(u64, &'a crate::signal::ThreadSignals)) {
    for slot in INDEX.iter() {
        let key = slot.key.load(Ordering::Acquire);
        if !live_key(key) || slot.group.load(Ordering::Acquire) != group || slot.flags.load(Ordering::Acquire) & DEAD != 0 {
            continue;
        }
        if let Some(thread) = pin.thread(key - 1) {
            f(key - 1, &thread.signals);
        }
    }
}

/// The threads of process row `pid` that have not terminated.
pub(super) fn each_live_of_pid(pid: u64, mut f: impl FnMut(u64)) {
    each_of_pid(pid, |tid, dead| {
        if !dead {
            f(tid);
        }
    });
}

// ------------------------------------------------------------ exit kicks

/// Per kick bucket, the victim a kill quarantined whose exit kick no peer has
/// observed yet, plus one (0 for none), and the bucket's generation when it
/// was quarantined: `Scheduler::exit_kick_watch`, read and released without a
/// lock.
static EXIT_WATCH: [AtomicU64; crate::tracing::providers::teardown::EXIT_KICK_BUCKETS] =
    [const { AtomicU64::new(0) }; crate::tracing::providers::teardown::EXIT_KICK_BUCKETS];
static EXIT_SINCE: [AtomicU64; crate::tracing::providers::teardown::EXIT_KICK_BUCKETS] =
    [const { AtomicU64::new(0) }; crate::tracing::providers::teardown::EXIT_KICK_BUCKETS];
/// How many buckets watch a victim.
static EXIT_WATCHED: AtomicU32 = AtomicU32::new(0);

/// Watch victim `pid`'s exit kick in its bucket, as of generation `since`.
pub(super) fn watch_exit_kick(bucket: usize, pid: u64, since: u64) {
    EXIT_SINCE[bucket].store(since, Ordering::Release);
    if EXIT_WATCH[bucket].swap(pid + 1, Ordering::AcqRel) == 0 {
        EXIT_WATCHED.fetch_add(1, Ordering::AcqRel);
    }
}

/// Whether any exit kick is watched.
pub(super) fn exit_kicks_watched() -> bool {
    EXIT_WATCHED.load(Ordering::Acquire) != 0
}

/// `Scheduler::observe_watched_exit_kicks`, from any CPU, with no lock: a
/// bucket is released once its kick is observed or displaced, unless a newer
/// watch replaced it meanwhile.
pub(super) fn observe_exit_kicks() {
    if !exit_kicks_watched() {
        return;
    }
    for bucket in 0..EXIT_WATCH.len() {
        let watched = EXIT_WATCH[bucket].load(Ordering::Acquire);
        if watched == 0 {
            continue;
        }
        let since = EXIT_SINCE[bucket].load(Ordering::Acquire);
        if EXIT_WATCH[bucket].load(Ordering::Acquire) != watched {
            continue;
        }
        let pid = watched - 1;
        if super::observe_exit_kick(pid)
            || crate::tracing::providers::teardown::EXIT_KICK_SLOTS[bucket].superseded_for(pid, since)
        {
            if EXIT_WATCH[bucket]
                .compare_exchange(watched, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                EXIT_WATCHED.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}

// ------------------------------------------------- one thread's settings

/// Whether the live scheduler exists, so the run-queue locks do.
pub(super) fn live() -> bool {
    !LIVE.load(Ordering::Acquire).is_null()
}

/// After `tid`'s policy changed under `held`, the lock of the CPU that owns
/// it: that CPU chooses again if the thread runs or is queued there.
pub(super) fn choose_again(held: &mut Held, tid: u64) {
    if held.state().current_thread == Some(tid) || held.queue().contains(&tid) {
        if held.cpu == super::current_cpu_id_raw() {
            super::set_need_resched();
        } else {
            request_resched_x86(held.cpu);
        }
    }
}

/// After `tid`'s pin changed under `held`, the lock of the CPU that owns it
/// (`set_user_affinity`): a thread queued, or woken and held, for a CPU the
/// pin no longer allows goes to its new home; one running elsewhere is moved
/// when it is next switched out (`retain_affine`), which that CPU is asked
/// for now.
pub(super) fn repin(held: &mut Held, tid: u64) {
    let cpu = held.cpu;
    let Some(thread) = held.thread(tid) else {
        return;
    };
    let pin = thread.cpu_affinity.filter(|pin| pin.cpu < online());
    let ready = thread.state == ThreadState::Ready;
    let current = held.state().current_thread == Some(tid);
    let queued = held.queue().contains(&tid);
    if current {
        if pin.is_some_and(|pin| pin.cpu != cpu) {
            if cpu == super::current_cpu_id_raw() {
                super::set_need_resched();
            } else {
                request_resched_x86(cpu);
            }
        }
        return;
    }
    // Queued here, or Ready on no queue: a wake held for the old pin's CPU,
    // which only that CPU would have placed.
    if !ready || (queued && pin.map_or(true, |pin| pin.cpu == cpu)) {
        return;
    }
    if queued {
        held.remove_from_ready_queue(tid);
    }
    match pin {
        None => {
            held.enqueue(tid, false);
            kick(cpu);
        }
        Some(pin) if pin.cpu == cpu => {
            held.enqueue(tid, false);
            kick(cpu);
        }
        Some(pin) => match try_home(pin.cpu) {
            Some(mut home) => {
                home.enqueue(tid, false);
                drop(home);
                kick(pin.cpu);
            }
            None => {
                super::note_pinned_hold(tid, pin.cpu);
                kick(pin.cpu);
            }
        },
    }
}

/// Whether `tid` is indexed and has terminated.
pub(super) fn is_dead(tid: u64) -> Option<bool> {
    find(tid).map(|slot| slot.flags.load(Ordering::Acquire) & DEAD != 0)
}

/// `Scheduler::expedite_signal_recipient` for `tid`, from outside a
/// scheduling pass: under this CPU's lock and, tried, the lock of the CPU
/// that owns the thread (`expedite`).
pub(super) fn expedite_thread(tid: u64) {
    if let Some(mut local) = local() {
        expedite(&mut local, None, tid);
    }
}

/// Every indexed thread that has not terminated, by tid.
pub(super) fn each_live(mut f: impl FnMut(u64)) {
    for slot in INDEX.iter() {
        let key = slot.key.load(Ordering::Acquire);
        if live_key(key) && slot.flags.load(Ordering::Acquire) & DEAD == 0 {
            f(key - 1);
        }
    }
}

/// Every indexed thread of process row `pid`, terminated or not, by tid.
pub(super) fn each_tid_of_pid(pid: u64, mut f: impl FnMut(u64)) {
    each_of_pid(pid, |tid, _| f(tid));
}

/// Keep one scheduler-visible token for `tid`, a thread a kill has just
/// terminated, on the queue of `held`'s CPU, which owns it, when no queue
/// holds it yet and that queue has room without allocating
/// (`Scheduler::terminate_process_threads`): a pass that pops it declines it
/// until a peer has observed the kill's exit kick.
pub(super) fn quarantine_token(held: &mut Held, tid: u64) {
    let queue = held.queue();
    if !queue.contains(&tid) && queue.len() < queue.capacity() {
        queue.push_back(tid);
    }
}

/// The signal state of `tid`, if it is indexed and has not terminated, with
/// the process row it belongs to, read through `pin`.
pub(super) fn live_signals(pin: &Pin, tid: u64) -> Option<(u64, alloc::sync::Arc<crate::signal::ThreadSignals>)> {
    let slot = find(tid)?;
    if slot.flags.load(Ordering::Acquire) & DEAD != 0 {
        return None;
    }
    let pid = slot.pid.load(Ordering::Acquire);
    pin.thread(tid).map(|thread| (pid, thread.signals.clone()))
}

/// The live thread of process row `pid`, by tid.
pub(super) fn row_thread(pid: u64) -> Option<u64> {
    let mut found = None;
    each_live_of_pid(pid, |tid| {
        found.get_or_insert(tid);
    });
    found
}

/// The live threads that receive the same interval timers as `tid` (its
/// thread group), `tid` included, by tid.
pub(super) fn group_threads(tid: u64) -> alloc::vec::Vec<u64> {
    let mut out = alloc::vec::Vec::new();
    let Some(group) = find(tid).map(|slot| slot.group.load(Ordering::Acquire)) else {
        return out;
    };
    if group == 0 {
        out.push(tid);
        return out;
    }
    for slot in INDEX.iter() {
        let key = slot.key.load(Ordering::Acquire);
        if live_key(key) && slot.group.load(Ordering::Acquire) == group && slot.flags.load(Ordering::Acquire) & DEAD == 0 {
            out.push(key - 1);
        }
    }
    out
}

/// `Scheduler::block_ready_user_thread` for `tid`, owned by `held`'s CPU: a
/// job stop parks a thread that is Ready to resume user code, Blocked and off
/// the queue, and returns true; one running, in a syscall or not Ready is
/// left alone.
pub(super) fn block_ready_user_thread(held: &mut Held, tid: u64) -> bool {
    if held.state().current_thread == Some(tid) {
        return false;
    }
    let Some(thread) = held.thread(tid) else {
        return false;
    };
    let user_context = thread.context.cs & 3 == 3;
    if thread.state != ThreadState::Ready
        || thread.privilege != ThreadPrivilege::User
        || thread.blocked_in_syscall
        || !(user_context || !thread.has_started)
    {
        return false;
    }
    thread.stop_timer_cpu();
    thread.state = ThreadState::Blocked;
    held.remove_from_ready_queue(tid);
    true
}

// ------------------------------------------------- CPUs spinning for a lock

/// Per CPU: spinning for a lock another CPU holds (`LockSpin`), and so
/// dispatching nothing until it gets it.
static SPINNING: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// Whether `cpu` is spinning for a lock (`SPINNING`).
fn spinning(cpu: usize) -> bool {
    SPINNING[cpu].load(Ordering::Acquire)
}

/// This CPU spins for a lock from now on: placement passes it over
/// (`accepts_wakeups`), and an idle peer is asked to take what is queued on
/// it (`reclaim_stalled`).
pub(super) fn spinning_begin() {
    let me = super::current_cpu_id_raw();
    if me >= MAX_CPUS || LIVE.load(Ordering::Acquire).is_null() {
        return;
    }
    SPINNING[me].store(true, Ordering::Release);
    if super::QUEUED_CPUS.load(Ordering::Acquire) & 1 << me == 0 {
        return;
    }
    if let Some(peer) = (0..online()).find(|&cpu| cpu != me && is_idle(cpu) && accepts_wakeups(cpu)) {
        request_resched_x86(peer);
    }
}

/// This CPU has the lock it spun for.
pub(super) fn spinning_end() {
    let me = super::current_cpu_id_raw();
    if me < MAX_CPUS {
        SPINNING[me].store(false, Ordering::Release);
    }
}
