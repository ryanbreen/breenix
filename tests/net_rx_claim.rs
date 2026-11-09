#[path = "../kernel/src/net/rx_claim.rs"]
mod rx_claim;

use rx_claim::{drain, PollOutcome};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

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
