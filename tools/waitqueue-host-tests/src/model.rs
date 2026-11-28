//! A recording scheduler behind the event queue's `Hooks` seam.
//!
//! Each thread is running, published blocked, or runnable with the reason
//! its wait ended. The model is sequentially consistent: a test step is one
//! call into the queue (one queue critical section) or one scheduler event,
//! and a test orders the steps explicitly.

use crate::event_queue::Hooks;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

/// Why a published-blocked thread became runnable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// A task-context wake (`Hooks::wake_task`).
    Woken,
    /// The owner CPU's drain of a lock-free inbox wake.
    Inbox,
    /// Its deadline passed.
    Timeout,
    /// A signal arrived.
    Signal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Blocked { deadline_ns: Option<u64> },
    Runnable(End),
}

#[derive(Clone, Debug)]
pub struct Thread {
    pub state: State,
    pub blocked_in_syscall: bool,
    /// A pending kill: publication is refused (`Thread::must_not_sleep`).
    pub must_not_sleep: bool,
    /// Transitions from blocked to runnable, so a test can see a wait end once.
    pub ends: u32,
}

/// One recorded hook call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Publish { tid: u64, queue: usize, masked: bool },
    WakeTask { tid: u64, masked: bool },
    WakeInbox { tid: u64 },
    EndWait { tid: u64 },
}

#[derive(Default)]
pub struct Model {
    current: Cell<Option<u64>>,
    threads: RefCell<BTreeMap<u64, Thread>>,
    inbox: RefCell<Vec<u64>>,
    masked: Cell<bool>,
    events: RefCell<Vec<Event>>,
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    /// A model with running threads `tids`, the first one current.
    pub fn with_threads(tids: &[u64]) -> Self {
        let model = Self::new();
        for &tid in tids {
            model.threads.borrow_mut().insert(
                tid,
                Thread { state: State::Running, blocked_in_syscall: false, must_not_sleep: false, ends: 0 },
            );
        }
        model.current.set(tids.first().copied());
        model
    }

    pub fn run_as(&self, tid: u64) {
        self.current.set(Some(tid));
    }

    pub fn thread(&self, tid: u64) -> Thread {
        self.threads.borrow()[&tid].clone()
    }

    pub fn state(&self, tid: u64) -> State {
        self.thread(tid).state
    }

    pub fn set_must_not_sleep(&self, tid: u64) {
        self.threads.borrow_mut().get_mut(&tid).unwrap().must_not_sleep = true;
    }

    pub fn events(&self) -> Vec<Event> {
        self.events.borrow().clone()
    }

    pub fn inbox(&self) -> Vec<u64> {
        self.inbox.borrow().clone()
    }

    pub fn masked(&self) -> bool {
        self.masked.get()
    }

    fn end(&self, tid: u64, why: End) {
        if let Some(thread) = self.threads.borrow_mut().get_mut(&tid) {
            if matches!(thread.state, State::Blocked { .. }) {
                thread.state = State::Runnable(why);
                thread.ends += 1;
            }
        }
    }

    /// The scheduler dispatches `tid`, as it does before a woken waiter
    /// returns from its sleep and finishes its wait.
    pub fn run(&self, tid: u64) {
        let mut threads = self.threads.borrow_mut();
        let thread = threads.get_mut(&tid).unwrap();
        if matches!(thread.state, State::Runnable(_)) {
            thread.state = State::Running;
        }
        drop(threads);
        self.run_as(tid);
    }

    /// The owner CPU drains its inbox: each posted wake ends a blocked wait.
    pub fn drain_inbox(&self) {
        let posted: Vec<u64> = self.inbox.borrow_mut().drain(..).collect();
        for tid in posted {
            self.end(tid, End::Inbox);
        }
    }

    /// The timer finds `tid`'s deadline passed.
    pub fn expire(&self, tid: u64) {
        let timed = matches!(self.state(tid), State::Blocked { deadline_ns: Some(_) });
        if timed {
            self.end(tid, End::Timeout);
        }
    }

    /// A signal is delivered to `tid`.
    pub fn signal(&self, tid: u64) {
        self.end(tid, End::Signal);
    }

    /// A multi-queue waiter publishes itself blocked under the scheduler
    /// lock (the queue is not involved).
    pub fn publish_current(&self, deadline_ns: Option<u64>) -> bool {
        let tid = self.current.get().unwrap();
        self.publish(tid, deadline_ns)
    }

    fn publish(&self, tid: u64, deadline_ns: Option<u64>) -> bool {
        let mut threads = self.threads.borrow_mut();
        let thread = threads.get_mut(&tid).unwrap();
        if thread.must_not_sleep {
            return false;
        }
        thread.state = State::Blocked { deadline_ns };
        thread.blocked_in_syscall = true;
        true
    }
}

impl Hooks for Model {
    type Irq = bool;

    fn mask_irqs(&self) -> bool {
        self.masked.replace(true)
    }

    fn restore_irqs(&self, saved: bool) {
        self.masked.set(saved);
    }

    fn current_tid(&self) -> Option<u64> {
        self.current.get()
    }

    fn publish_blocked(&self, tid: u64, deadline_ns: Option<u64>, queue: usize) -> bool {
        self.events.borrow_mut().push(Event::Publish { tid, queue, masked: self.masked.get() });
        self.publish(tid, deadline_ns)
    }

    fn end_wait(&self, tid: u64) {
        self.events.borrow_mut().push(Event::EndWait { tid });
        let mut threads = self.threads.borrow_mut();
        let thread = threads.get_mut(&tid).unwrap();
        thread.blocked_in_syscall = false;
        if matches!(thread.state, State::Blocked { .. }) {
            thread.state = State::Running;
        }
    }

    fn wake_task(&self, tid: u64) {
        self.events.borrow_mut().push(Event::WakeTask { tid, masked: self.masked.get() });
        self.end(tid, End::Woken);
    }

    fn wake_from_interrupt(&self, tid: u64) {
        self.events.borrow_mut().push(Event::WakeInbox { tid });
        let mut inbox = self.inbox.borrow_mut();
        if !inbox.contains(&tid) {
            inbox.push(tid);
        }
    }
}

/// The merges of sequences of the given lengths that keep each sequence's
/// own order, enumerated exhaustively. Each merge lists the sequence index
/// of each step.
pub fn interleavings(lengths: &[usize]) -> Vec<Vec<usize>> {
    fn go(left: &mut Vec<usize>, prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if left.iter().all(|&n| n == 0) {
            out.push(prefix.clone());
            return;
        }
        for which in 0..left.len() {
            if left[which] > 0 {
                left[which] -= 1;
                prefix.push(which);
                go(left, prefix, out);
                prefix.pop();
                left[which] += 1;
            }
        }
    }
    let mut out = Vec::new();
    go(&mut lengths.to_vec(), &mut Vec::new(), &mut out);
    out
}
