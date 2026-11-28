//! A recording scheduler and CPU behind `park_core::ParkHooks` and the event
//! queue's `Hooks`.
//!
//! One waiting thread runs on one CPU. A test scripts arrivals at the wait's
//! `Point`s or while it is switched out, and the model applies each as the
//! kernel does: a queue wake (task context or interrupt) ends the wait word's
//! current wait and makes the thread runnable; the inbox drain and a signal
//! make it runnable without touching the word; an expired live deadline ends
//! the word's wait with `Timeout`, as the timer heap does.

use crate::event_queue::{EventQueue, Hooks};
use crate::park_core::{
    classify, exit_switch, Arrival, ContextInputs, ExitSwitch, ExitThread, ParkContext, ParkHooks, Phase, Point, SignalEnd,
    WaitMode, WaitWord,
};
use std::cell::{Cell, RefCell};

pub const TID: u64 = 7;
pub const EINTR: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Blocked { deadline_ns: Option<u64> },
    Runnable,
}

/// One scripted arrival.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A task-context wake of one waiter on queue `i`.
    Wake(usize),
    /// An interrupt-context wake of one waiter on queue `i` (lock-free inbox).
    IrqWake(usize),
    /// The inbox drain applies a wake published during an earlier wait.
    StaleInbox,
    /// An unblocked signal that is not a kill.
    Signal,
    /// SIGKILL.
    Kill,
    /// Time passes the wait's deadline; the timer expires its live entry.
    Deadline,
    /// An interrupt exit with a reschedule pending: it samples the depth and,
    /// at depth 0, makes the production exit decision (`exit_switch`).
    InterruptExit,
}

/// Where an action happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    Point(Point),
    /// During the thread's n-th switch out (1-based).
    Asleep(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Point(Point, u32),
    InterruptExitDepth(u32),
    /// An interrupt exit switched the thread out as blocked, with its wait
    /// word in this phase. Until something makes it runnable it does not run.
    SwitchedOut(Phase),
    /// An interrupt exit requeued the thread: its wait was published and not
    /// committed (`ExitSwitch::Requeue`).
    Requeued,
    Sleep { depth: u32, irqs: bool },
    Receipt(ParkContext),
}

pub struct Model {
    pub queues: Vec<EventQueue>,
    pub word: WaitWord,
    state: Cell<State>,
    depth: Cell<u32>,
    baseline: Cell<u32>,
    irqs: Cell<bool>,
    baseline_irqs_masked: Cell<bool>,
    pub deadline_ns: u64,
    now: Cell<u64>,
    signal: Cell<bool>,
    kill: Cell<bool>,
    must_not_sleep: Cell<bool>,
    inbox: RefCell<Vec<u64>>,
    script: RefCell<Vec<(At, Action)>>,
    events: RefCell<Vec<Event>>,
    sleeps: Cell<u32>,
    /// Transitions from blocked to runnable.
    pub becomes_runnable: Cell<u32>,
    pub inputs: Cell<Option<ContextInputs>>,
}

impl Model {
    /// One thread at `baseline` (and the same depth), with `queues` queues.
    pub fn new(queues: usize, baseline: u32) -> Self {
        Self {
            queues: (0..queues).map(|_| EventQueue::new()).collect(),
            word: WaitWord::new(),
            state: Cell::new(State::Running),
            depth: Cell::new(baseline),
            baseline: Cell::new(baseline),
            irqs: Cell::new(true),
            baseline_irqs_masked: Cell::new(false),
            deadline_ns: 1_000,
            now: Cell::new(0),
            signal: Cell::new(false),
            kill: Cell::new(false),
            must_not_sleep: Cell::new(false),
            inbox: RefCell::new(Vec::new()),
            script: RefCell::new(Vec::new()),
            events: RefCell::new(Vec::new()),
            sleeps: Cell::new(0),
            becomes_runnable: Cell::new(0),
            inputs: Cell::new(None),
        }
    }

    /// An x86 syscall: baseline 1, entered with interrupts masked.
    pub fn x86_syscall(queues: usize) -> Self {
        let model = Self::new(queues, 1);
        model.irqs.set(false);
        model.baseline_irqs_masked.set(true);
        model
    }

    pub fn at(&self, at: At, action: Action) -> &Self {
        self.script.borrow_mut().push((at, action));
        self
    }

    pub fn depth(&self) -> u32 {
        self.depth.get()
    }

    pub fn set_depth(&self, depth: u32) {
        self.depth.set(depth);
    }

    pub fn irqs(&self) -> bool {
        self.irqs.get()
    }

    pub fn state(&self) -> State {
        self.state.get()
    }

    pub fn events(&self) -> Vec<Event> {
        self.events.borrow().clone()
    }

    pub fn sleeps(&self) -> u32 {
        self.sleeps.get()
    }

    pub fn set_must_not_sleep(&self) {
        self.must_not_sleep.set(true);
    }

    /// Post a wake for this thread to the inbox now, as an interrupt waker
    /// of an earlier wait did; the drain applies it later.
    pub fn post_inbox(&self) {
        self.inbox.borrow_mut().push(TID);
    }

    pub fn inbox_len(&self) -> usize {
        self.inbox.borrow().len()
    }

    /// Whether the thread is still linked on any queue.
    pub fn linked(&self) -> bool {
        self.queues.iter().any(|queue| queue.contains(self, TID))
    }

    fn make_runnable(&self) {
        if matches!(self.state.get(), State::Blocked { .. }) {
            self.state.set(State::Runnable);
            self.becomes_runnable.set(self.becomes_runnable.get() + 1);
        }
    }

    fn apply(&self, action: Action) {
        match action {
            Action::Wake(i) => {
                self.queues[i].wake_n(self, 1);
            }
            Action::IrqWake(i) => {
                self.queues[i].wake_n_from_irq(self, 1);
            }
            Action::StaleInbox => {
                self.post_inbox();
                self.drain_inbox();
            }
            Action::Signal => {
                self.signal.set(true);
                self.make_runnable();
            }
            Action::Kill => {
                self.kill.set(true);
                self.make_runnable();
            }
            Action::Deadline => {
                self.now.set(self.deadline_ns);
                if let State::Blocked { deadline_ns: Some(deadline) } = self.state.get() {
                    if deadline <= self.now.get() {
                        self.word.arrive_current(Arrival::Timeout);
                        self.make_runnable();
                    }
                }
            }
            Action::InterruptExit => {
                let depth = self.depth.get();
                self.events.borrow_mut().push(Event::InterruptExitDepth(depth));
                if depth == 0 {
                    self.involuntary_switch();
                }
            }
        }
    }

    fn run_script(&self, at: At) {
        loop {
            let next = {
                let mut script = self.script.borrow_mut();
                script.iter().position(|(when, _)| *when == at).map(|i| script.remove(i).1)
            };
            match next {
                Some(action) => self.apply(action),
                None => break,
            }
        }
    }

    /// An interrupt exit at depth 0 with a reschedule pending, decided by the
    /// production `exit_switch` from the thread's state and wait word.
    fn involuntary_switch(&self) {
        let thread = match self.state.get() {
            State::Blocked { .. } => ExitThread::BlockedOnIo(self.word.state().1),
            State::Running | State::Runnable => ExitThread::Runnable,
        };
        match exit_switch(thread, true) {
            ExitSwitch::SwitchOut => self.events.borrow_mut().push(Event::SwitchedOut(self.word.state().1)),
            // The kernel's `apply_exit_switch`: back to running, so the
            // switch requeues it; its queue entry and wait word stay.
            ExitSwitch::Requeue => {
                self.state.set(State::Running);
                self.events.borrow_mut().push(Event::Requeued);
            }
            ExitSwitch::Preempt | ExitSwitch::Resume => {}
        }
    }

    /// The owner CPU's inbox drain: each posted wake makes a blocked thread
    /// runnable (`unblock_for_io`), without touching its wait word.
    pub fn drain_inbox(&self) {
        let posted: Vec<u64> = self.inbox.borrow_mut().drain(..).collect();
        for _ in posted {
            self.make_runnable();
        }
    }
}

impl Hooks for Model {
    type Irq = bool;

    fn mask_irqs(&self) -> bool {
        self.irqs.replace(false)
    }

    fn restore_irqs(&self, saved: bool) {
        self.irqs.set(saved);
    }

    fn current_tid(&self) -> Option<u64> {
        Some(TID)
    }

    fn publish_blocked(&self, _tid: u64, deadline_ns: Option<u64>, _queue: usize) -> bool {
        self.republish(TID, deadline_ns)
    }

    fn end_wait(&self, _tid: u64) {
        self.state.set(State::Running);
    }

    fn wake_task(&self, _tid: u64) {
        self.word.arrive_current(Arrival::Woken);
        self.make_runnable();
    }

    fn wake_from_interrupt(&self, tid: u64) {
        self.word.arrive_current(Arrival::Woken);
        self.inbox.borrow_mut().push(tid);
    }
}

impl ParkHooks for Model {
    fn context(&self) -> ParkContext {
        classify(self.inputs.get().unwrap_or(ContextInputs {
            dispatching: true,
            has_thread: true,
            depth: self.depth.get(),
            baseline: self.baseline.get(),
            irqs_masked: !self.irqs.get(),
            baseline_irqs_masked: self.baseline_irqs_masked.get(),
            ..ContextInputs::default()
        }))
    }

    fn receipt(&self, context: ParkContext, _site: u64) {
        self.events.borrow_mut().push(Event::Receipt(context));
    }

    fn word(&self) -> Option<&WaitWord> {
        Some(&self.word)
    }

    fn depth(&self) -> u32 {
        self.depth.get()
    }

    fn baseline(&self) -> u32 {
        self.baseline.get()
    }

    fn set_baseline(&self, baseline: u32) {
        self.baseline.set(baseline);
    }

    fn raise(&self) {
        self.depth.set(self.depth.get() + 1);
    }

    fn lower(&self) {
        assert!(self.depth.get() > 0, "a wait released a level it does not hold");
        self.depth.set(self.depth.get() - 1);
    }

    fn irqs_enabled(&self) -> bool {
        self.irqs.get()
    }

    fn set_irqs(&self, enabled: bool) {
        self.irqs.set(enabled);
    }

    fn now_ns(&self) -> u64 {
        self.now.get()
    }

    fn pending_signal(&self, mode: WaitMode) -> Option<SignalEnd> {
        match mode {
            WaitMode::Interruptible if self.kill.get() => Some(SignalEnd::Killed),
            WaitMode::Interruptible if self.signal.get() => Some(SignalEnd::Interrupted(EINTR)),
            WaitMode::Killable if self.kill.get() => Some(SignalEnd::Killed),
            _ => None,
        }
    }

    fn republish(&self, _tid: u64, deadline_ns: Option<u64>) -> bool {
        if self.must_not_sleep.get() {
            return false;
        }
        self.state.set(State::Blocked { deadline_ns });
        true
    }

    /// The switch: interrupts on, as the halt or the dispatch leaves them;
    /// the script's sleeping arrivals happen, the inbox drains, and the
    /// thread runs again once it is runnable.
    fn sleep(&self) {
        self.events.borrow_mut().push(Event::Sleep { depth: self.depth.get(), irqs: self.irqs.get() });
        self.irqs.set(true);
        self.sleeps.set(self.sleeps.get() + 1);
        assert!(self.sleeps.get() < 16, "the wait slept with nothing left to end it");
        let published = matches!(self.state.get(), State::Blocked { .. });
        self.run_script(At::Asleep(self.sleeps.get()));
        self.drain_inbox();
        if !published {
            // An interrupt exit requeued the thread before its commit, so it
            // is running, not published: the kernel's switch returns at once
            // (`waitqueue::switch_while_blocked`).
            self.state.set(State::Running);
            return;
        }
        // A thread still blocked stays switched out in the kernel (its sleep
        // hook loops while blocked). With no arrival left, that is a hang.
        assert_eq!(self.state.get(), State::Runnable, "the wait slept with nothing left to end it");
        self.state.set(State::Running);
    }

    fn point(&self, point: Point) {
        self.events.borrow_mut().push(Event::Point(point, self.depth.get()));
        self.run_script(At::Point(point));
    }
}
