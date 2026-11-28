use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering},
    Mutex,
};

use super::{QUEUED_CPUS, SCHEDULER_HELD};
use crate::wake_inbox::{WakeInbox, WakeNode};

pub(super) const MAX_CPUS: usize = 4;
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
static WAKE_INBOXES: [WakeInbox; MAX_CPUS] = [const { WakeInbox::new() }; MAX_CPUS];
static WAKE_INBOX_CPUS: AtomicU64 = AtomicU64::new(0);
static RESCHED_REQUESTED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static NODE: WakeNode = WakeNode::new(TID);
static ENQUEUE_SAME_LOCK_OK: AtomicU64 = AtomicU64::new(0);
static ENQUEUE_DEFERRED: AtomicU64 = AtomicU64::new(0);
static ENQUEUE_ALREADY_QUEUED_OK: AtomicU64 = AtomicU64::new(0);
static ENQUEUE_TIMER_WAKE: AtomicU64 = AtomicU64::new(0);
static WAKE_SITE_TIMER: AtomicU64 = AtomicU64::new(0);
static PINNED_HOLD_PEN_MIGRATED: AtomicU64 = AtomicU64::new(0);
const READY_SITE_TIMER: u8 = 0;
const TRACE_SCHED_DIAG_WAKE_TIMER_CURRENT: u16 = 0;
const TRACE_SCHED_DIAG_WAKE_TIMER_ALREADY_QUEUED: u16 = 1;
const TRACE_SCHED_DIAG_WAKE_TIMER_ENQUEUE: u16 = 2;
use crate::task::thread::TimerPop;

thread_local! {
    static CPU: Cell<usize> = const { Cell::new(0) };
    static MIGRATE_ON_LOOKUP: Cell<Option<usize>> = const { Cell::new(None) };
    static KICKS: Cell<u64> = const { Cell::new(0) };
    static SGIS_TO_A: Cell<usize> = const { Cell::new(0) };
    static DISPATCH_DURING_REMOTE_LOCK: Cell<bool> = const { Cell::new(false) };
    static DISPATCHES_OWED_ON_A: Cell<usize> = const { Cell::new(0) };
    static NOW_NS: Cell<u64> = const { Cell::new(0) };
    static ACQUISITIONS: Cell<usize> = const { Cell::new(0) };
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
    BlockedOnTimer,
    BlockedOnIO,
}

#[derive(Clone, Copy)]

struct Pin {
    per_cpu_worker: bool,
    cpu: usize,
}

pub(super) struct Thread {
    state: ThreadState,
    cpu_affinity: Option<Pin>,
    owner_pid: Option<u64>,
    wake_time_ns: Option<u64>,
    blocked_in_syscall: bool,
    timer_pop: Option<TimerPopRecord>,
}

struct TimerPopRecord {
    deadline_ns: u64,
    own_entry: TimerPop,
    stale_entries: u64,
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
    use super::*;

    pub(super) fn note_lock_interval(acquired: bool, _: u64) {
        if acquired {
            ACQUISITIONS.set(ACQUISITIONS.get() + 1);
        }
        if acquired && RQ_LOCKS[0].is_locked() && DISPATCH_DURING_REMOTE_LOCK.replace(false) {
            // CPU A reaches its return-path dispatch while the idle peer
            // holds A's queue. Resume the peer after the acquisition attempt.
            let peer = CPU.replace(0);
            assert!(Dispatch::try_local(0).is_none());
            DISPATCHES_OWED_ON_A.set(
                DISPATCHES_OWED_ON_A.get() + usize::from(DISPATCH_OWED[0].load(Ordering::SeqCst)),
            );
            CPU.set(peer);
        }
    }
}

fn live() -> bool {
    !LIVE.load(Ordering::Acquire).is_null()
}

fn is_live(sched: &Scheduler) -> bool {
    std::ptr::eq(LIVE.load(Ordering::Acquire), sched)
}

fn now_ns() -> u64 {
    NOW_NS.get()
}

fn record_ready_site(_: u64, _: u8) {}

fn trace_sched_diag(_: u16, _: u64, _: u64, _: u64, _: u32) {}

fn online() -> usize {
    MAX_CPUS
}

fn accepts_wakeups(_: usize) -> bool {
    true
}

fn least_loaded(prefer: usize) -> usize {
    prefer
}

fn place(_: &mut Held, _: Option<&mut Held>, _: u64, _: usize, _: bool, _: bool) -> Option<usize> {
    panic!("timer subject is still current on its owner");
}

fn request_resched(cpu: usize) {
    request_resched_arm64(cpu);
}

fn online_cpus() -> usize {
    MAX_CPUS
}

fn set_need_resched() {
    post_resched_arm64(current_cpu_id_raw());
}

fn send_sgi_outside_scheduler_lock(sgi: u32, cpu: usize) {
    assert_eq!(sgi, crate::arch_impl::aarch64::constants::SGI_RESCHEDULE);
    KICKS.set(KICKS.get() | 1 << cpu);
    if cpu == 0 {
        SGIS_TO_A.set(SGIS_TO_A.get() + 1);
    }
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

mod wake_index {
    use super::*;

    pub(super) struct Pin;

    impl Pin {
        pub(super) fn new() -> Self {
            Self
        }

        pub(super) fn lookup(&self, tid: u64) -> Option<(&'static WakeNode, usize)> {
            assert_eq!(tid, NODE.tid());
            Some((&NODE, owner(tid)?))
        }
    }
}

fn drain_wake_inbox(cpu: usize, apply: impl FnMut(u64, u64)) {
    // SAFETY: the fixture is the sole consumer and keeps NODE alive.
    unsafe { WAKE_INBOXES[cpu].drain(apply) };
}

mod rq {
    pub(super) use super::local;
}

fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}

impl Scheduler {
    fn current_cpu_id() -> usize {
        current_cpu_id_raw()
    }
}

/// The whole scheduler as `lock_scheduler` takes it on ARM64: `lock_all`,
/// then `refresh` and `unlock_all` when the guard drops.
struct WholeScheduler(Option<&'static Scheduler>);

impl std::ops::Deref for WholeScheduler {
    type Target = Option<&'static Scheduler>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for WholeScheduler {
    fn drop(&mut self) {
        if let Some(scheduler) = self.0 {
            refresh(scheduler);
        }
        unlock_all();
    }
}

fn with_scheduler_lock<R>(f: impl FnOnce(&mut WholeScheduler) -> R) -> R {
    without_interrupts(|| {
        lock_all();
        // SAFETY: the fixture's scheduler outlives the guard.
        let mut whole = WholeScheduler(unsafe { LIVE.load(Ordering::Acquire).as_ref() });
        f(&mut whole)
    })
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
            wake_time_ns: None,
            blocked_in_syscall: false,
            timer_pop: None,
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
    SGIS_TO_A.set(0);
    DISPATCH_DURING_REMOTE_LOCK.set(false);
    DISPATCHES_OWED_ON_A.set(0);
    NOW_NS.set(0);
    ACQUISITIONS.set(0);
    for cpu in 0..MAX_CPUS {
        publish_next_check(cpu, u64::MAX);
    }
    WAKE_INBOX_CPUS.store(0, Ordering::Relaxed);
    for requested in &RESCHED_REQUESTED {
        requested.store(false, Ordering::Relaxed);
    }
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
fn idle_steal_leaves_lone_runner_alone_and_real_wake_kicks_once() {
    fixture(|| {
        for cpu in 0..MAX_CPUS {
            CPU.set(cpu);
            let mut held = try_remote(cpu).unwrap();
            let state = held.state();
            state.idle_thread = 100 + cpu as u64;
            state.current_thread = Some(if cpu == 0 { OTHER_TID } else { state.idle_thread });
        }

        for _ in 0..8 {
            for peer in 1..MAX_CPUS {
                CPU.set(peer);
                let mut idle = Dispatch::try_local(peer).unwrap();
                DISPATCH_DURING_REMOTE_LOCK.set(true);
                let stolen = steal_from(&mut idle.0, 0, &mut 0, false, &mut |_| false);
                DISPATCH_DURING_REMOTE_LOCK.set(false);
                assert_eq!(stolen, None);
                drop(idle);

                // A can finish a dispatch without changing its running thread.
                CPU.set(0);
                let mut runner = Dispatch::try_local(0).unwrap();
                assert_eq!(runner.0.state().current_thread, Some(OTHER_TID));
            }
        }
        let idle_traffic = (SGIS_TO_A.get(), DISPATCHES_OWED_ON_A.get());

        CPU.set(1);
        hold(TID, WAKE_GENERIC, 0);
        assert_eq!(SGIS_TO_A.get() - idle_traffic.0, 1);
        assert!(RESCHED_REQUESTED[0].load(Ordering::Acquire));
        CPU.set(0);
        let mut runner = Dispatch::try_local(0).unwrap();
        apply_isr_wakeups(&mut runner.0);
        assert_eq!(runner.0.queue().front(), Some(&TID));
        assert_eq!(idle_traffic, (0, 0), "(reschedule SGIs, owed dispatches) on CPU A");
    });
}

#[test]
fn peer_expiry_skips_future_timer_and_wakes_claimed_due_timer() {
    fixture(|| {
        const DEADLINE: u64 = 2_000_000;
        let mut runner = Dispatch::try_local(0).unwrap();
        runner.0.state().current_thread = Some(TID);
        let thread = runner.0.thread(TID).unwrap();
        thread.state = ThreadState::BlockedOnTimer;
        thread.wake_time_ns = Some(DEADLINE);
        thread.blocked_in_syscall = true;
        runner.0.heap().push(Reverse((DEADLINE, TID)));
        drop(runner);

        CPU.set(1);
        let mut idle = Dispatch::try_local(1).unwrap();
        NOW_NS.set(DEADLINE - 1);
        DISPATCH_DURING_REMOTE_LOCK.set(true);
        expire_timers(&mut idle.0);
        DISPATCH_DURING_REMOTE_LOCK.set(false);
        drop(idle);
        assert_eq!(
            (SGIS_TO_A.get(), DISPATCHES_OWED_ON_A.get()),
            (0, 0),
            "(reschedule SGIs, owed dispatches) for a future timer"
        );

        CPU.set(0);
        let mut runner = Dispatch::try_local(0).unwrap();
        assert_eq!(runner.0.thread(TID).unwrap().wake_time_ns, Some(DEADLINE));
        drop(runner);
        NOW_NS.set(DEADLINE);
        assert!(claim_due(0, DEADLINE, DEADLINE + 1_000_000));
        CPU.set(1);
        let mut idle = Dispatch::try_local(1).unwrap();
        expire_timers(&mut idle.0);
        drop(idle);
        assert_eq!(SGIS_TO_A.get(), 1);

        CPU.set(0);
        let mut runner = Dispatch::try_local(0).unwrap();
        assert!(runner.0.heap().is_empty());
        let thread = runner.0.thread(TID).unwrap();
        assert!(thread.state == ThreadState::Ready);
        assert_eq!(thread.wake_time_ns, None);
        assert!(!thread.blocked_in_syscall);
    });
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

#[test]
fn return_to_user_reads_its_current_thread_under_its_own_queue_lock_alone() {
    fixture(|| {
        // SAFETY: the fixture's scheduler is live for the closure.
        let sched = unsafe { &mut *LIVE.load(Ordering::Acquire) };
        sched.cpu_state[1].current_thread = Some(TID);
        set_owner(TID, 1);
        // CPU 2's last dispatch found its own lock busy and is owed a pass.
        DISPATCH_OWED[2].store(true, Ordering::SeqCst);
        CPU.set(1);
        // The syscall return to EL0 reads this (context_switch.rs,
        // check_and_deliver_signals_for_current_thread_arm64).
        assert_eq!(current_thread_id(), Some(TID));
        assert_eq!(ACQUISITIONS.get(), 1, "the read took peers' run-queue locks");
        assert_eq!(KICKS.get(), 0, "the read sent a peer a reschedule SGI");
        DISPATCH_OWED[2].store(false, Ordering::SeqCst);
    });
}
