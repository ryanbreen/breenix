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

use super::{Scheduler, MAX_CPUS};
use crate::task::thread::Thread;
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
}

static INDEX: [Slot; SLOTS] = [const {
    Slot {
        key: AtomicU64::new(EMPTY),
        thread: AtomicPtr::new(core::ptr::null_mut()),
        owner: AtomicU32::new(0),
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
    if let Some(slot) = find(tid) {
        slot.thread.store(thread, Ordering::Release);
        slot.owner.store(owner as u32, Ordering::Release);
        return;
    }
    let start = home(tid);
    for probe in 0..SLOTS {
        let slot = &INDEX[(start + probe) % SLOTS];
        let key = slot.key.load(Ordering::Acquire);
        if key == EMPTY || key == REMOVED {
            slot.thread.store(thread, Ordering::Relaxed);
            slot.owner.store(owner as u32, Ordering::Relaxed);
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
    }
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
    }
}
