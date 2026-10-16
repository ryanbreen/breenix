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
//! so a holder of a single run-queue lock always finds them consistent.

use super::{CpuSchedulerState, Scheduler, MAX_CPUS, SCHEDULER_HELD};
use crate::task::thread::{Thread, ThreadState};
use alloc::collections::{BinaryHeap, VecDeque};
use core::cmp::Reverse;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};

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

struct Slot {
    key: AtomicU64,
    thread: AtomicPtr<Thread>,
    owner: AtomicU32,
    /// The page-table root the thread caches (`Thread::cached_ttbr0`) while
    /// it has not terminated, and 0 once it has: what `any_cached_root` reads,
    /// with no lock.
    root: AtomicU64,
    /// A buffered I/O wake for the thread is held for a later pass: its CPU
    /// had its lock held when the wake was applied (`hold_io_wake`).
    io_wake: AtomicBool,
}

static INDEX: [Slot; SLOTS] = [const {
    Slot {
        key: AtomicU64::new(EMPTY),
        thread: AtomicPtr::new(core::ptr::null_mut()),
        owner: AtomicU32::new(0),
        root: AtomicU64::new(0),
        io_wake: AtomicBool::new(false),
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

/// Index `thread`, owned by `owner`. Every run-queue lock held, or the thread
/// not yet visible to any other CPU.
pub(super) fn insert(thread: &mut Thread, owner: usize) {
    let tid = thread.id();
    let root = if thread.state == ThreadState::Terminated { 0 } else { thread.cached_ttbr0 };
    if let Some(slot) = find(tid) {
        slot.thread.store(thread, Ordering::Release);
        slot.owner.store(owner as u32, Ordering::Release);
        slot.root.store(root, Ordering::Release);
        return;
    }
    let start = home(tid);
    for probe in 0..SLOTS {
        let slot = &INDEX[(start + probe) % SLOTS];
        let key = slot.key.load(Ordering::Acquire);
        if key == EMPTY || key == REMOVED {
            slot.thread.store(thread, Ordering::Relaxed);
            slot.owner.store(owner as u32, Ordering::Relaxed);
            slot.root.store(root, Ordering::Relaxed);
            slot.key.store(tid.wrapping_add(1), Ordering::Release);
            return;
        }
    }
    panic!("scheduler: thread index full ({} slots) adding thread {}", SLOTS, tid);
}

/// Drop `tid` from the index. Every run-queue lock held.
pub(super) fn remove(tid: u64) {
    if let Some(slot) = find(tid) {
        slot.key.store(REMOVED, Ordering::Release);
        slot.thread.store(core::ptr::null_mut(), Ordering::Release);
        slot.root.store(0, Ordering::Release);
        if slot.io_wake.swap(false, Ordering::AcqRel) {
            HELD_IO_WAKES.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Publish `root` as the page-table root `thread` caches, 0 once it has
/// terminated. The caller writes the thread under its owner's lock, or the
/// whole scheduler. A thread not in the live index (not yet added, or one a
/// test builds) publishes nothing: `insert` reads its root.
pub(super) fn publish_root(thread: &Thread, root: u64) {
    if let Some(slot) = find(thread.id()) {
        if core::ptr::eq(slot.thread.load(Ordering::Acquire), thread) {
            slot.root.store(root, Ordering::Release);
        }
    }
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

// --------------------------------------------------------- per-CPU summary

/// Per CPU, as its slice stood when its lock was last released: how many
/// threads it has to run (its queue, plus the thread it runs unless that is
/// its idle thread), whether it runs its idle thread, and the tick it last
/// entered the scheduler at. Placement reads them without locks.
pub(super) static CPU_LOAD: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
pub(super) static CPU_IDLE: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
pub(super) static CPU_LAST_SCHEDULE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

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
        publish_next_check(cpu, scheduler.timer_heap.heaps[cpu].peek().map_or(u64::MAX, |entry| entry.0 .0));
    }
    super::EXIT_KICKS_WATCHED.store(scheduler.exit_kick_watched != 0, Ordering::Release);
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
        CPU_LOAD[cpu].store(len + busy as u32, Ordering::Relaxed);
        CPU_IDLE[cpu].store(idle, Ordering::Relaxed);
        CPU_LAST_SCHEDULE[cpu].store(last, Ordering::Relaxed);
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
    let last = CPU_LAST_SCHEDULE[cpu].load(Ordering::Relaxed).max(super::last_interrupt_return_tick(cpu));
    crate::time::get_ticks().wrapping_sub(last) <= super::CPU_STALL_TICKS
}

/// Whether `cpu` runs its idle thread and no idle housekeeping, as of its last
/// release.
pub(super) fn is_idle(cpu: usize) -> bool {
    CPU_IDLE[cpu].load(Ordering::Relaxed) && !super::IDLE_HOUSEKEEPING[cpu].load(Ordering::Acquire)
}

/// The least loaded CPU that accepts wakeups, `prefer` among equals; `prefer`
/// when none does.
pub(super) fn least_loaded(prefer: usize) -> usize {
    let online = (crate::arch_impl::x86_64::smp::cpus_online() as usize).clamp(1, MAX_CPUS);
    (0..online)
        .filter(|&cpu| accepts_wakeups(cpu))
        .min_by_key(|&cpu| (CPU_LOAD[cpu].load(Ordering::Relaxed), cpu != prefer))
        .unwrap_or(prefer)
}

// ------------------------------------------------------ the dispatch path

use super::{
    count_pinned_migration_refusal, observe_exit_kick, record_ready_site, request_resched_x86,
    send_reschedule_vector_x86, trace_sched_diag, x86_stack_owner_elsewhere, SchedPolicy,
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

/// How many index slots hold an I/O wake (`io_wake`), counted before the
/// mark is set and after it is cleared, so it is never below their number.
static HELD_IO_WAKES: AtomicU32 = AtomicU32::new(0);

/// Hold the buffered I/O wake of `tid` for a later pass, marked on its index
/// slot: unlike a wake buffer, which producers can fill meanwhile, the slot
/// always has room for it. A tid not indexed has no thread to wake.
fn hold_io_wake(tid: u64) {
    let Some(slot) = find(tid) else {
        return;
    };
    HELD_IO_WAKES.fetch_add(1, Ordering::AcqRel);
    if slot.io_wake.swap(true, Ordering::AcqRel) {
        HELD_IO_WAKES.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Take each held I/O wake (`hold_io_wake`) and pass its tid to `apply`. The
/// caller holds a run-queue lock, so no slot is removed or reused meanwhile:
/// removal takes every lock.
pub(super) fn take_held_io_wakes(mut apply: impl FnMut(u64)) {
    if HELD_IO_WAKES.load(Ordering::Acquire) == 0 {
        return;
    }
    for slot in INDEX.iter() {
        if !slot.io_wake.load(Ordering::Relaxed) || !slot.io_wake.swap(false, Ordering::AcqRel) {
            continue;
        }
        HELD_IO_WAKES.fetch_sub(1, Ordering::AcqRel);
        let key = slot.key.load(Ordering::Acquire);
        if key != EMPTY && key != REMOVED {
            apply(key.wrapping_sub(1));
        }
    }
}

/// Apply the wakes interrupt handlers buffered (`Scheduler::apply_isr_wakeups`),
/// and those held by earlier passes: one whose thread is owned by a CPU whose
/// lock is busy is held for the next pass (`hold_io_wake`).
pub(super) fn apply_isr_wakeups(local: &mut Held) {
    let mut apply = |tid| {
        if let Woken::Busy = wake(local, tid, Wake::Io { from_isr_buffer: true }) {
            hold_io_wake(tid);
        }
    };
    for buffer in ISR_WAKEUP_BUFFERS.iter() {
        buffer.take_each(&mut apply);
    }
    take_held_io_wakes(apply);
}

/// Move what is queued on CPUs that have stopped scheduling, or are offline,
/// onto this one (`Scheduler::reclaim_unschedulable_cpu_queues`). A CPU whose
/// lock is held is passed over.
fn reclaim_stalled(local: &mut Held) {
    let online = online();
    for cpu in 0..MAX_CPUS {
        if cpu == local.cpu {
            continue;
        }
        let offline = cpu >= online;
        if !offline && accepts_wakeups(cpu) {
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

/// `Scheduler::schedule` on x86_64 under this CPU's run-queue lock alone
/// (#1649). Returns the outgoing and the incoming thread, both owned here.
pub(super) fn schedule(local: &mut Held) -> Option<(u64, u64)> {
    let cpu = local.cpu;
    local.state().last_schedule_ticks = crate::time::get_ticks();
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

    let mut stack_waits = 0u64;
    let stolen = if POLICIES_IN_USE.load(Ordering::Acquire) {
        let floor = best_queued_rank(local, None);
        steal_higher_ranked(local, floor, &mut stack_waits)
    } else {
        None
    };
    let mut next = match stolen {
        Some(tid) => tid,
        None => match pop_local(local, &mut stack_waits) {
            Some(tid) => tid,
            None => (0..MAX_CPUS)
                .filter(|&other| other != cpu)
                .find_map(|other| steal_from(local, other, &mut stack_waits, false))
                .unwrap_or(local.state().idle_thread),
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

/// Run `f` on `tid` under the lock of the CPU that owns it: this CPU's, or a
/// peer's taken without waiting. None when the thread is not indexed, its
/// owner's lock is busy, or it moved before the lock was taken; the caller
/// then takes the whole scheduler.
pub(super) fn with_owner<R>(tid: u64, f: impl FnOnce(&mut Held, &mut Thread) -> R) -> Option<R> {
    let owner = owner(tid)?;
    let mut held = if owner == super::current_cpu_id_raw() { local()? } else { try_remote(owner)? };
    let thread: *mut Thread = held.thread(tid)?;
    // SAFETY: `held` covers the thread for this call; `f` gets the only
    // reference to it.
    Some(f(&mut held, unsafe { &mut *thread }))
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
                let already_armed = thread.timer_pop.is_some_and(|record| {
                    record.deadline_ns == wake_time_ns
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
