#[path = "../kernel/src/net/rx_claim.rs"]
mod rx_claim;

use rx_claim::{drain, PollOutcome};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

mod task {
    pub mod softirqd {
        use std::cell::Cell;
        thread_local! { pub static RAISED: Cell<u32> = const { Cell::new(0) }; }
        pub enum SoftirqType { NetRx }
        pub fn raise_softirq(_: SoftirqType) { RAISED.with(|count| count.set(count.get() + 1)); }
    }
}

#[test]
fn busy_holder_keeps_request_pending() {
    let requested = AtomicBool::new(false);
    let draining = AtomicBool::new(true);
    let outcome = drain(&requested, &draining, 4, |_| panic!("busy"), |_| panic!("busy"));
    assert_eq!(outcome, PollOutcome::InProgress);
    assert!(requested.load(SeqCst));
    assert!(draining.load(SeqCst));
}

#[test]
fn request_during_drain_causes_another_pass() {
    let requested = AtomicBool::new(false);
    let draining = AtomicBool::new(false);
    let calls = Cell::new(0);
    let delivered = Cell::new(0);
    let outcome = drain(&requested, &draining, 4, |buffer| {
        let call = calls.get();
        calls.set(call + 1);
        match call {
            0 => { requested.store(true, SeqCst); None }
            1 => { buffer[0] = 42; Some(1) }
            _ => None,
        }
    }, |frame| { assert_eq!(frame, &[42]); delivered.set(delivered.get() + 1); });
    assert_eq!(outcome, PollOutcome::Drained);
    assert_eq!(calls.get(), 3);
    assert_eq!(delivered.get(), 1);
    assert!(!draining.load(SeqCst));
}

#[test]
fn empty_ring_is_drained() {
    let requested = AtomicBool::new(false);
    let draining = AtomicBool::new(false);
    assert_eq!(drain(&requested, &draining, 4, |_| None, |_| panic!("empty")), PollOutcome::Drained);
    assert!(!draining.load(SeqCst));
}

#[test]
fn budget_limits_delivered_frames() {
    let requested = AtomicBool::new(false);
    let draining = AtomicBool::new(false);
    let delivered = Cell::new(0);
    let outcome = drain(&requested, &draining, 3, |buffer| { buffer[0] = 7; Some(1) }, |frame| {
        assert_eq!(frame, &[7]);
        delivered.set(delivered.get() + 1);
    });
    assert_eq!(outcome, PollOutcome::BudgetExhausted);
    assert_eq!(delivered.get(), 3);
    assert!(!draining.load(SeqCst));
}

#[test]
fn exhaustion_schedules_another_receive() {
    use task::softirqd::{raise_softirq, SoftirqType, RAISED};
    let _raise: fn(SoftirqType) = raise_softirq;
    let _ = SoftirqType::NetRx;
    let requested = AtomicBool::new(false);
    let draining = AtomicBool::new(false);
    let queued = Cell::new(65);
    let delivered = Cell::new(0);
    RAISED.with(|count| count.set(0));
    let outcome = drain(&requested, &draining, 64, |buffer| {
        if queued.get() == 0 { return None; }
        queued.set(queued.get() - 1);
        buffer[0] = 7;
        Some(1)
    }, |_| delivered.set(delivered.get() + 1));
    assert_eq!(outcome, PollOutcome::BudgetExhausted);
    assert_eq!(delivered.get(), 64);
    assert_eq!(queued.get(), 1);
    RAISED.with(|count| assert_eq!(count.get(), 1));
}
