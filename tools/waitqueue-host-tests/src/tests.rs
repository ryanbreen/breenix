//! Brief B1's deterministic tests against the production event queue.

use crate::budget::with_budget;
use crate::event_queue::{EventQueue, PrepareOutcome};
use crate::model::{interleavings, End, Event, Model, State};
use std::cell::Cell;

const W: u64 = 10;
const V: u64 = 11;
const K: u64 = 20;

/// The ways a waker can wake a queue.
#[derive(Clone, Copy, Debug)]
enum Wake {
    All,
    One,
    Deferred,
    Irq,
    /// From hard interrupt context while another CPU's task holds the lock.
    IrqUnderHeldLock,
}

const WAKES: [Wake; 5] = [Wake::All, Wake::One, Wake::Deferred, Wake::Irq, Wake::IrqUnderHeldLock];

fn wake(queue: &EventQueue, model: &Model, how: Wake) {
    match how {
        Wake::All => {
            queue.wake_all(model);
        }
        Wake::One => {
            queue.wake_n(model, 1);
        }
        Wake::Deferred => {
            queue.wake_all_deferred(model);
        }
        Wake::Irq => {
            queue.wake_n_from_irq(model, 1);
        }
        Wake::IrqUnderHeldLock => {
            let woken = queue.with_lock_held(model, || queue.wake_n_from_irq(model, 1));
            assert_eq!(woken, 0, "a busy queue lock refuses the interrupt");
        }
    }
}

/// B1 requirement 6: the wait is over, the thread runs, it is not registered,
/// it is not marked blocked in a syscall, and a late wake leaves it running.
fn assert_wait_over(queue: &EventQueue, model: &Model, tid: u64) {
    assert!(!queue.contains(model, tid), "thread {tid} still registered");
    let thread = model.thread(tid);
    assert_eq!(thread.state, State::Running, "thread {tid} not running");
    assert!(!thread.blocked_in_syscall);
    crate::event_queue::Hooks::wake_task(model, tid);
    model.drain_inbox();
    assert_eq!(model.state(tid), State::Running, "a late wake changed thread {tid}");
    assert!(!model.masked(), "the queue left interrupts masked");
}

/// B1 requirement 1 for a checked wait, against each kind of waker: in each
/// of the 3 orders of the waiter's registration against the waker's
/// condition store and wake, the waiter either sees the condition or is
/// made runnable.
#[test]
fn checked_wait_loses_no_wake() {
    for how in WAKES {
        for order in interleavings(&[1, 2]) {
            let model = Model::with_threads(&[W, K]);
            let queue = EventQueue::new();
            let ready = Cell::new(false);
            let mut outcome = None;
            let mut waker_step = 0;
            for side in order.iter().copied() {
                if side == 0 {
                    model.run_as(W);
                    outcome = Some(queue.prepare_checked(&model, None, || !ready.get()));
                } else {
                    model.run_as(K);
                    if waker_step == 0 {
                        ready.set(true);
                    } else {
                        wake(&queue, &model, how);
                    }
                    waker_step += 1;
                }
            }
            model.drain_inbox();
            match outcome.unwrap() {
                PrepareOutcome::Mismatch => assert!(ready.get()),
                PrepareOutcome::Queued => {
                    assert!(
                        matches!(model.state(W), State::Runnable(_)),
                        "{how:?} {order:?}: lost wake, waiter left {:?}",
                        model.state(W)
                    );
                    // B1 requirement 2: published inside the queue's critical section.
                    assert!(model.events().iter().any(|e| matches!(e, Event::Publish { tid: W, masked: true, .. })));
                }
                other => panic!("{how:?} {order:?}: unexpected {other:?}"),
            }
            model.run(W);
            queue.finish(&model);
            assert_wait_over(&queue, &model, W);
        }
    }
}

/// B1 requirement 1 for the multi-queue shape: register without publishing,
/// publish once under the scheduler lock, then recheck.
#[test]
fn multi_queue_wait_loses_no_wake() {
    for how in WAKES {
        for order in interleavings(&[3, 2]) {
            let model = Model::with_threads(&[W, K]);
            let queue = EventQueue::new();
            let ready = Cell::new(false);
            let (mut waiter_step, mut waker_step) = (0, 0);
            let mut saw_ready = false;
            for side in order.iter().copied() {
                if side == 0 {
                    model.run_as(W);
                    match waiter_step {
                        0 => assert!(queue.add(&model, W)),
                        1 => assert!(model.publish_current(None)),
                        _ => saw_ready = ready.get(),
                    }
                    waiter_step += 1;
                } else {
                    model.run_as(K);
                    if waker_step == 0 {
                        ready.set(true);
                    } else {
                        wake(&queue, &model, how);
                    }
                    waker_step += 1;
                }
            }
            model.drain_inbox();
            if !saw_ready {
                assert!(
                    matches!(model.state(W), State::Runnable(_)),
                    "{how:?} {order:?}: lost wake, waiter left {:?}",
                    model.state(W)
                );
            }
            model.run(W);
            queue.take(&model, W);
            queue.finish_for(&model, W);
            assert_wait_over(&queue, &model, W);
        }
    }
}

/// An interrupt that finds the lock held leaves the queue's entries in place
/// while the lock is held, and the holder pays the wake when it releases the lock.
/// Against a queue whose try-lock only returns 0 (B1's contract for the old
/// one), the waiter stays blocked: this is the owed-wake case E2.1 adds.
#[test]
fn interrupt_wake_under_a_held_lock_is_paid_by_the_holder() {
    let model = Model::with_threads(&[W, K]);
    let queue = EventQueue::new();
    model.run_as(W);
    assert_eq!(queue.prepare_checked(&model, None, || true), PrepareOutcome::Queued);
    model.run_as(K);
    queue.with_lock_held(&model, || {
        assert_eq!(queue.wake_n_from_irq(&model, 1), 0);
        assert_eq!(queue.irq_wakes_owed(), 1);
        assert!(model.inbox().is_empty(), "nothing is woken while the lock is held");
        assert!(matches!(model.state(W), State::Blocked { .. }));
    });
    assert_eq!(queue.irq_wakes_owed(), 0);
    assert_eq!(model.inbox(), vec![W]);
    assert!(!queue.has_waiters(&model));
    assert!(!model.events().iter().any(|e| matches!(e, Event::WakeTask { .. })), "the payer never enters the scheduler");
    model.drain_inbox();
    assert_eq!(model.state(W), State::Runnable(End::Inbox));
}

/// An interrupt on another CPU can land inside the waiter's own critical
/// section, before or after its condition check. Before, the waiter sees
/// the condition. After, the waiter registers and publishes, and its own
/// unlock pays the wake the interrupt could not make.
#[test]
fn interrupt_inside_the_waiters_critical_section_is_not_lost() {
    for irq_after_check in [false, true] {
        let model = Model::with_threads(&[W]);
        let queue = EventQueue::new();
        let ready = Cell::new(false);
        let irq = || {
            ready.set(true);
            assert_eq!(queue.wake_n_from_irq(&model, 1), 0);
        };
        let outcome = queue.prepare_checked(&model, None, || {
            if !irq_after_check {
                irq();
            }
            let wait = !ready.get();
            if irq_after_check {
                irq();
            }
            wait
        });
        model.drain_inbox();
        if irq_after_check {
            assert_eq!(outcome, PrepareOutcome::Queued);
            assert_eq!(model.state(W), State::Runnable(End::Inbox));
        } else {
            assert_eq!(outcome, PrepareOutcome::Mismatch);
            assert_eq!(model.state(W), State::Running);
        }
        assert_eq!(queue.irq_wakes_owed(), 0);
        model.run(W);
        queue.finish(&model);
        assert_wait_over(&queue, &model, W);
    }
}

/// An owed wake wakes at most the owed count, oldest first, and a waiter that
/// registers after it is paid is not woken by it.
#[test]
fn owed_wakes_are_bounded_and_not_kept() {
    let model = Model::with_threads(&[1, 2, 3, 4, K]);
    let queue = EventQueue::new();
    for tid in [1, 2, 3] {
        model.run_as(tid);
        assert_eq!(queue.prepare_checked(&model, None, || true), PrepareOutcome::Queued);
    }
    queue.with_lock_held(&model, || {
        assert_eq!(queue.wake_n_from_irq(&model, 2), 0);
    });
    assert_eq!(model.inbox(), vec![1, 2]);
    assert!(queue.contains(&model, 3));
    assert_eq!(queue.irq_wakes_owed(), 0);
    model.run_as(4);
    assert_eq!(queue.prepare_checked(&model, None, || true), PrepareOutcome::Queued);
    assert_eq!(model.inbox(), vec![1, 2]);
    // More owed than registered: the 4 waiters are woken, and no owed count
    // is kept.
    queue.with_lock_held(&model, || {
        queue.wake_n_from_irq(&model, 9);
    });
    assert_eq!(model.inbox(), vec![1, 2, 3, 4]);
    assert_eq!(queue.irq_wakes_owed(), 0);
    assert!(!queue.has_waiters(&model));
}

/// A deferred wake does not enter the scheduler lock; the owner CPU's inbox
/// drain makes the waiter runnable.
#[test]
fn deferred_wake_goes_through_the_inbox_only() {
    let model = Model::with_threads(&[W, V, K]);
    let queue = EventQueue::new();
    for tid in [W, V] {
        model.run_as(tid);
        assert_eq!(queue.prepare_checked(&model, None, || true), PrepareOutcome::Queued);
    }
    model.run_as(K);
    assert_eq!(queue.wake_all_deferred(&model), 2);
    assert!(!model.events().iter().any(|e| matches!(e, Event::WakeTask { .. })));
    assert!(matches!(model.state(W), State::Blocked { .. }));
    model.drain_inbox();
    assert_eq!(model.state(W), State::Runnable(End::Inbox));
    assert_eq!(model.state(V), State::Runnable(End::Inbox));
    assert!(!queue.has_waiters(&model));
}

/// B1 requirement 8: a wake, a timeout and a signal racing to end one timed
/// wait end it exactly once, whatever their order, and the wait ends cleanly.
#[test]
fn wake_timeout_and_signal_end_a_wait_once() {
    for how in [Wake::All, Wake::Deferred, Wake::IrqUnderHeldLock] {
        for order in interleavings(&[1, 1, 1]) {
            let model = Model::with_threads(&[W, K]);
            let queue = EventQueue::new();
            model.run_as(W);
            assert_eq!(queue.prepare_checked(&model, Some(1_000), || true), PrepareOutcome::Queued);
            model.run_as(K);
            for side in order.iter().copied() {
                match side {
                    0 => wake(&queue, &model, how),
                    1 => model.expire(W),
                    _ => model.signal(W),
                }
                model.drain_inbox();
            }
            let thread = model.thread(W);
            assert_eq!(thread.ends, 1, "{how:?} {order:?}: the wait ended {} times", thread.ends);
            model.run(W);
            queue.finish(&model);
            assert_wait_over(&queue, &model, W);
        }
    }
}

/// Wake-one with two waiters: in each of the 12 orders, at least one of them
/// sees the condition or is woken, and with both registered the oldest is
/// woken.
#[test]
fn wake_one_of_two_waiters() {
    for how in [Wake::One, Wake::Irq, Wake::IrqUnderHeldLock] {
        for order in interleavings(&[1, 1, 2]) {
            let model = Model::with_threads(&[W, V, K]);
            let queue = EventQueue::new();
            let ready = Cell::new(false);
            let mut outcomes = [None, None];
            let mut waker_step = 0;
            for side in order.iter().copied() {
                match side {
                    0 | 1 => {
                        let tid = [W, V][side];
                        model.run_as(tid);
                        outcomes[side] = Some(queue.prepare_checked(&model, None, || !ready.get()));
                    }
                    _ => {
                        model.run_as(K);
                        if waker_step == 0 {
                            ready.set(true);
                        } else {
                            wake(&queue, &model, how);
                        }
                        waker_step += 1;
                    }
                }
            }
            model.drain_inbox();
            let proceeds = |i: usize, tid: u64| {
                outcomes[i] == Some(PrepareOutcome::Mismatch) || matches!(model.state(tid), State::Runnable(_))
            };
            assert!(proceeds(0, W) || proceeds(1, V), "{how:?} {order:?}: both waiters left blocked");
            let both_queued = outcomes.iter().all(|o| *o == Some(PrepareOutcome::Queued));
            if both_queued {
                let first = order.iter().position(|&s| s == 0).unwrap();
                let second = order.iter().position(|&s| s == 1).unwrap();
                let (older, younger) = if first < second { (W, V) } else { (V, W) };
                assert!(matches!(model.state(older), State::Runnable(_)), "{how:?} {order:?}: oldest not woken");
                assert!(matches!(model.state(younger), State::Blocked { .. }), "{how:?} {order:?}: woke two");
            }
        }
    }
}

/// B1 requirement 3: a failed reservation is out of memory, and the thread
/// is not published or registered.
#[test]
fn out_of_memory_at_reservation_changes_nothing() {
    let model = Model::with_threads(&[W]);
    let queue = EventQueue::new();
    let outcome = with_budget(0, || queue.prepare_checked(&model, None, || true));
    assert_eq!(outcome, PrepareOutcome::OutOfMemory);
    assert_eq!(with_budget(0, || queue.prepare(&model, None)), None);
    assert!(!with_budget(0, || queue.add(&model, W)));
    assert!(model.events().is_empty(), "something was published: {:?}", model.events());
    assert!(!queue.has_waiters(&model));
    assert_eq!(model.state(W), State::Running);
    assert!(!model.masked());
}

/// B1 requirement 6 after each way a wait ends: wake, timeout, signal, a
/// refused publication (a pending kill), a false condition and no memory.
#[test]
fn every_way_a_wait_ends_leaves_the_thread_running_and_unregistered() {
    #[derive(Clone, Copy, Debug)]
    enum How {
        Woken,
        Timeout,
        Signal,
        Killed,
        Mismatch,
        NoMemory,
    }
    for how in [How::Woken, How::Timeout, How::Signal, How::Killed, How::Mismatch, How::NoMemory] {
        let model = Model::with_threads(&[W]);
        let queue = EventQueue::new();
        if matches!(how, How::Killed) {
            model.set_must_not_sleep(W);
        }
        let prepare = || queue.prepare_checked(&model, Some(5), || !matches!(how, How::Mismatch));
        let outcome = if matches!(how, How::NoMemory) { with_budget(0, prepare) } else { prepare() };
        let expected = match how {
            How::Killed => PrepareOutcome::PublishFailed,
            How::Mismatch => PrepareOutcome::Mismatch,
            How::NoMemory => PrepareOutcome::OutOfMemory,
            _ => PrepareOutcome::Queued,
        };
        assert_eq!(outcome, expected, "{how:?}");
        // Only a queued wait leaves a registration before the finish.
        assert_eq!(queue.contains(&model, W), outcome == PrepareOutcome::Queued, "{how:?}");
        match how {
            How::Woken => {
                queue.wake_all(&model);
            }
            How::Timeout => model.expire(W),
            How::Signal => model.signal(W),
            _ => {}
        }
        model.run(W);
        queue.finish_for(&model, W);
        assert_wait_over(&queue, &model, W);
    }
}

// The queue's unit tests from before the rewrite, reworded to the new names.

#[test]
fn duplicate_registrations_are_one_entry() {
    let model = Model::with_threads(&[W]);
    let queue = EventQueue::new();
    assert!(queue.add(&model, W));
    assert!(queue.add(&model, W));
    assert_eq!(queue.wake_all(&model), 1);
    assert!(!queue.has_waiters(&model));
}

#[test]
fn wake_one_removes_only_the_oldest() {
    let model = Model::with_threads(&[W, V]);
    let queue = EventQueue::new();
    assert!(queue.add(&model, W));
    assert!(queue.add(&model, V));
    assert_eq!(queue.wake_n(&model, 1), 1);
    assert!(!queue.contains(&model, W));
    assert!(queue.contains(&model, V));
    assert_eq!(model.events().last(), Some(&Event::WakeTask { tid: W, masked: true }));
}

#[test]
fn wake_all_empties_the_queue() {
    let model = Model::with_threads(&[W, V]);
    let queue = EventQueue::new();
    assert!(queue.add(&model, W));
    assert!(queue.add(&model, V));
    assert_eq!(queue.wake_all(&model), 2);
    assert!(!queue.has_waiters(&model));
}

#[test]
fn take_removes_only_the_named_thread() {
    let model = Model::with_threads(&[W, V]);
    let queue = EventQueue::new();
    assert!(queue.add(&model, W));
    assert!(queue.add(&model, V));
    assert!(queue.take(&model, W));
    assert!(!queue.take(&model, W));
    assert!(queue.contains(&model, V));
    let mut out = [0; 4];
    assert_eq!(queue.take_oldest(&model, &mut out), 1);
    assert_eq!(out[0], V);
    assert!(!model.events().iter().any(|e| matches!(e, Event::WakeTask { .. })), "take wakes nobody");
}

#[test]
fn wake_n_zero_takes_nobody() {
    let model = Model::with_threads(&[W]);
    let queue = EventQueue::new();
    assert!(queue.add(&model, W));
    assert_eq!(queue.wake_n(&model, 0), 0);
    assert_eq!(queue.wake_n_from_irq(&model, 0), 0);
    assert!(queue.contains(&model, W));
    assert_eq!(queue.irq_wakes_owed(), 0);
}
