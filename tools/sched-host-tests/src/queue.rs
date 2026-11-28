use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Mutex,
};

use super::{QUEUED_CPUS, SCHEDULER_HELD};
use crate::wake_inbox::{WakeInbox, WakeNode};

const MAX_CPUS: usize = 3;
const TID: u64 = 1;
const OTHER_TID: u64 = 2;
const WAKE_IO: u64 = 1;
const WAKE_GENERIC: u64 = 2;
const WAKE_SIGNAL: u64 = 4;
const WAKE_CHILD_EXIT: u64 = 8;
const WAKE_PLACE: u64 = 16;
const WAKE_HOME_SHIFT: u32 = 5;
const WAKE_HOME_MASK: u64 = 3 << WAKE_HOME_SHIFT;

static SERIAL: Mutex<()> = Mutex::new(());
static LIVE: AtomicPtr<Scheduler> = AtomicPtr::new(std::ptr::null_mut());
static RQ_LOCKS: [RqLock; MAX_CPUS] = [const { RqLock::new() }; MAX_CPUS];
static DISPATCH_OWED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static CPU_LOAD: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
static CPU_QUEUE_LEN: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];
static CPU_IDLE: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static CPU_LAST_SCHEDULE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static CURRENT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static OWNERS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
static INBOXES: [WakeInbox; MAX_CPUS] = [const { WakeInbox::new() }; MAX_CPUS];
static NODE: WakeNode = WakeNode::new(TID);
static ENQUEUE_SAME_LOCK_OK: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static CPU: Cell<usize> = const { Cell::new(0) };
    static MIGRATE_ON_LOOKUP: Cell<Option<usize>> = const { Cell::new(None) };
    static KICKS: Cell<u64> = const { Cell::new(0) };
}
pub(super) use super::{PINNED_HOLDS_OUTSTANDING, SAVING_CPU};

#[derive(Default)]
pub(super) struct CpuSchedulerState {
    current_thread: Option<u64>,
    pending_next: Option<u64>,
    idle_thread: u64,
    last_schedule_ticks: u64,
}

#[derive(Default)]
struct TimerHeap {
    heaps: [BinaryHeap<Reverse<(u64, u64)>>; MAX_CPUS],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThreadState {
    Ready,
    Terminated,
}

#[derive(Clone, Copy)]

struct Pin {
    per_cpu_worker: bool,
    cpu: usize,
}

struct Thread {
    state: ThreadState,
    cpu_affinity: Option<Pin>,
    owner_pid: Option<u64>,
}

#[derive(Default)]
pub(super) struct Scheduler {
    per_cpu_queues: [VecDeque<u64>; MAX_CPUS],
    cpu_state: [CpuSchedulerState; MAX_CPUS],
    timer_heap: TimerHeap,
    subject: Option<Thread>,
}

mod scheduler {
    pub(super) use super::super::{current_cpu_id_raw, QUEUED_CPUS};
}

mod arm64 {
    pub(super) fn note_lock_interval(_: bool, _: u64) {}
}

fn live() -> bool {
    !LIVE.load(Ordering::Acquire).is_null()
}

fn is_live(sched: &Scheduler) -> bool {
    std::ptr::eq(LIVE.load(Ordering::Acquire), sched)
}

fn publish_next_check(_: usize, _: u64) {}

fn request_resched(cpu: usize) {
    KICKS.set(KICKS.get() | 1 << cpu);
}

fn set_owner(tid: u64, cpu: usize) {
    OWNERS[tid as usize].store(cpu, Ordering::Release);
}

fn owner(tid: u64) -> Option<usize> {
    let cpu = OWNERS[tid as usize].load(Ordering::Acquire);
    if let Some(destination) = MIGRATE_ON_LOOKUP.take() {
        // Hold both owners in the fixture before completing the move and
        // returning the now stale owner observation to the caller.
        let (_old, _new) = (try_remote(cpu).unwrap(), try_remote(destination).unwrap());
        set_owner(tid, destination);
    }
    Some(cpu)
}

fn stack_owner_elsewhere(tid: u64, _: usize) -> Option<usize> {
    (tid == TID).then_some(SAVING_CPU.get()).flatten()
}

fn stack_home(_: &Held, _: u64) -> Option<usize> {
    None
}

fn promote_best_of(_: &mut Held, _: usize) {}
fn retain_on_home(_: &mut Held, _: u64) -> bool {
    false
}
fn retain_affine(_: &mut Held, _: u64, _: usize) -> bool {
    false
}
fn observe_exit_kick(_: u64) -> bool {
    panic!("fixture has no exits")
}

type UnblockOutcome = WakeResult;
#[derive(Debug, PartialEq, Eq)]
pub(super) enum WakeResult {
    NotFound,
    Transitioned,
}

fn wake_owned(local: &mut Held, source: Option<&mut Held>, tid: u64, kind: Wake) -> WakeResult {
    assert!(!matches!(
        kind,
        Wake::Io {
            from_isr_buffer: false
        }
    ));
    let source = source.unwrap_or(local);
    assert_eq!(owner(tid), Some(source.cpu));
    assert!(!source.queue().contains(&tid), "duplicate wake application");
    source.enqueue(tid, false);
    WakeResult::Transitioned
}

enum Notice {
    Signal,
    ChildExit,
}

fn notify(_: &mut Held, _: u64, _: Notice) -> bool {
    panic!("fixture has no signal or exit notices")
}

fn publish_wake(tid: u64, bits: u64, home: Option<usize>) -> Option<u64> {
    assert_eq!(tid, NODE.tid());
    let cpu = home.unwrap_or_else(|| owner(tid).unwrap());
    // SAFETY: NODE and the inboxes have static storage. fixture() holds
    // SERIAL through publication and drain.
    unsafe { INBOXES[cpu].publish(&NODE, bits, WAKE_HOME_MASK | WAKE_PLACE) }
}

fn drain_wake_inbox(cpu: usize, apply: impl FnMut(u64, u64)) {
    // SAFETY: the fixture is the sole consumer and keeps NODE alive.
    unsafe { INBOXES[cpu].drain(apply) };
}

include!(concat!(env!("OUT_DIR"), "/queue_operations.rs"));

impl Held {
    fn thread(&mut self, tid: u64) -> Option<&mut Thread> {
        assert!(RQ_LOCKS[self.cpu].is_locked());
        if tid != TID || owner(tid) != Some(self.cpu) {
            return None;
        }
        // SAFETY: the fixture has one thread row, accessed under its owner.
        unsafe { (*self.sched).subject.as_mut() }
    }
}

fn fixture(run: impl FnOnce()) {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut sched = Box::new(Scheduler {
        subject: Some(Thread {
            state: ThreadState::Ready,
            cpu_affinity: None,
            owner_pid: None,
        }),
        per_cpu_queues: std::array::from_fn(|_| VecDeque::with_capacity(2)),
        ..Scheduler::default()
    });
    assert!(RQ_LOCKS.iter().all(|lock| !lock.is_locked()));
    for owed in &DISPATCH_OWED {
        owed.store(false, Ordering::SeqCst);
    }
    CPU.set(0);
    KICKS.set(0);
    QUEUED_CPUS.store(0, Ordering::Release);
    SAVING_CPU.set(None);
    set_owner(TID, 0);
    set_owner(OTHER_TID, 0);
    LIVE.store(sched.as_mut(), Ordering::Release);
    run();
    LIVE.store(std::ptr::null_mut(), Ordering::Release);
    assert!(RQ_LOCKS.iter().all(|lock| !lock.is_locked()));
}

pub(super) fn current_cpu_id_raw() -> usize {
    CPU.get()
}

#[test]
fn independent_dispatch_selects_work_during_a_peer_transaction() {
    fixture(|| {
        CPU.set(1);
        let peer = Dispatch::try_local(1).unwrap();
        CPU.set(2);
        assert!(!try_lock_all());
        let mut local = Dispatch::try_local(2).expect("peer serialized this dispatch");
        let mut source = try_remote(0).unwrap();
        source.enqueue(TID, false);
        drop(source);
        assert_eq!(
            steal_from(&mut local.0, 0, &mut 0, false, &mut |_| false),
            Some(TID)
        );
        drop(local);
        CPU.set(1);
        drop(peer);
    });
}

#[test]
fn stopped_remote_holder_owes_a_dispatch_and_kicks_after_unlock() {
    fixture(|| {
        CPU.set(2);
        let remote = try_remote(1).unwrap();
        CPU.set(1);
        // This is the interrupt-return entry point's acquisition. Busy
        // returns to the interrupted context and leaves a dispatch owed.
        assert!(Dispatch::try_local(1).is_none());
        assert!(DISPATCH_OWED[1].load(Ordering::SeqCst));
        assert_eq!(KICKS.get(), 0);
        CPU.set(2);
        drop(remote);
        assert_eq!(KICKS.get(), 1 << 1);
        CPU.set(1);
        let _dispatch = Dispatch::try_local(1).unwrap();
        assert!(!DISPATCH_OWED[1].load(Ordering::SeqCst));
    });
}

#[test]
fn pivot_token_releases_acquired_queue_even_if_cpu_identity_changes() {
    fixture(|| {
        CPU.set(1);
        let guard = Dispatch::try_local(1).unwrap();
        let token = guard.into_pivot();
        CPU.set(2);
        let unrelated = try_remote(2).unwrap();
        assert!(RQ_LOCKS[1].is_locked());
        let reacquired = unsafe { Dispatch::from_pivot(token) };
        assert_eq!(reacquired.cpu(), 1);
        drop(reacquired);
        assert!(!RQ_LOCKS[1].is_locked());
        assert!(RQ_LOCKS[2].is_locked());
        drop(unrelated);
        // Reconstructing a token in the fallback releases the same lock. The
        // fixture switched its observation between acquisition and Drop.
        SCHEDULER_HELD[1].store(0, Ordering::Relaxed);
        SCHEDULER_HELD[2].store(0, Ordering::Relaxed);
    });
}

#[test]
fn idle_and_steal_skip_stack_custody_then_see_the_runnable_thread() {
    fixture(|| {
        let mut source = try_remote(0).unwrap();
        source.enqueue(TID, false);
        drop(source);
        SAVING_CPU.set(Some(2));
        assert!(!Dispatch::has_ready_work());
        CPU.set(1);
        let mut local = try_remote(1).unwrap();
        let mut waiting = 0;
        assert_eq!(
            steal_from(&mut local, 0, &mut waiting, false, &mut |_| false),
            None
        );
        assert_eq!(waiting, 1 << 2);
        SAVING_CPU.set(None);
        CPU.set(0);
        assert!(Dispatch::has_ready_work());
        CPU.set(1);
        assert_eq!(
            steal_from(&mut local, 0, &mut waiting, false, &mut |_| false),
            Some(TID)
        );
    });
}

#[test]
fn wake_revalidates_owner_and_drains_busy_owner_once() {
    fixture(|| {
        let mut local = try_remote(0).unwrap();
        set_owner(TID, 1);
        MIGRATE_ON_LOOKUP.set(Some(2));
        assert!(matches!(wake(&mut local, TID, Wake::Generic), Woken::Busy));
        let busy_owner = try_remote(2).unwrap();
        assert!(matches!(wake(&mut local, TID, Wake::Generic), Woken::Busy));
        hold(TID, WAKE_GENERIC, 0);
        drop(busy_owner);
        drop(local);
        CPU.set(2);
        let mut owner = try_remote(2).unwrap();
        apply_isr_wakeups(&mut owner);
        apply_isr_wakeups(&mut owner);
        assert_eq!(owner.queue().iter().copied().collect::<Vec<_>>(), [TID]);
        owner.queue().clear();
        assert!(matches!(
            wake(&mut owner, TID, Wake::Generic),
            Woken::Done(WakeResult::Transitioned)
        ));
    });
}

#[test]
fn pinned_placement_touches_only_home_and_owner_and_refreshes_ownership() {
    fixture(|| {
        CPU.set(1);
        let mut unrelated = try_remote(1).unwrap();
        unrelated.enqueue(OTHER_TID, false);
        CPU.set(2);
        let mut source = try_remote(2).unwrap();
        set_owner(TID, 2);
        source.thread(TID).unwrap().cpu_affinity = Some(Pin {
            cpu: 0,
            per_cpu_worker: true,
        });
        hold(TID, WAKE_PLACE, 0);
        CPU.set(0);
        let mut home = try_remote(0).unwrap();
        apply_isr_wakeups(&mut home);
        assert!(home.queue().is_empty());
        CPU.set(2);
        drop(source);
        CPU.set(0);
        apply_isr_wakeups(&mut home);
        apply_isr_wakeups(&mut home);
        assert_eq!(home.queue().iter().copied().collect::<Vec<_>>(), [TID]);
        assert_eq!(PINNED_HOLDS_OUTSTANDING[0].load(Ordering::Acquire), 0);
        drop(home);
        CPU.set(1);
        assert_eq!(unrelated.queue().front(), Some(&OTHER_TID));
        drop(unrelated);
        assert!(try_lock_all());
        // SAFETY: try_lock_all succeeded. These stale owners model a
        // whole-scheduler caller moving queue and current rows.
        unsafe {
            let sched = &mut *LIVE.load(Ordering::Acquire);
            sched.cpu_state[2].current_thread = Some(OTHER_TID);
            sched.per_cpu_queues[1].clear();
            set_owner(TID, 2);
            refresh(sched);
        }
        assert_eq!(owner(TID), Some(0));
        assert_eq!(owner(OTHER_TID), Some(2));
        for lock in &RQ_LOCKS {
            lock.unlock();
        }
    });
}
