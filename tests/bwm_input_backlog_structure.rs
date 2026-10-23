//! Slow-client delivery under #2163 and B48, through the live admission paths.
#[path = "../userspace/programs/src/bwm/input_backlog.rs"]
mod input_backlog;
#[cfg(target_arch = "x86_64")]
#[path = "../kernel/src/syscall/graphics/input_ring.rs"]
mod input_ring;
#[cfg(target_arch = "aarch64")]
#[path = "../kernel/src/window/event_admission.rs"]
mod event_admission;
use input_backlog::{Admission, Backlog, Delivery, MAX_PENDING};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Event { kind: u16, value: u16, scroll: i16 }
fn motion(old: &Event, new: &Event) -> bool {
    old.kind == 3 && new.kind == 3 && old.scroll == 0 && new.scroll == 0
}
fn overflow(lost: u32) -> Event { Event { kind: 14, value: lost as u16, scroll: 0 } }
fn event(kind: u16, value: u16) -> Event { Event { kind, value, scroll: 0 } }

// Exercise v2's admission on ARM64 and main's retained input_ring on x86.
struct Ring {
    #[cfg(target_arch = "x86_64")] values: [Event; 64],
    #[cfg(target_arch = "x86_64")] head: usize,
    #[cfg(target_arch = "x86_64")] tail: usize,
    #[cfg(target_arch = "aarch64")] values: std::collections::VecDeque<Event>,
}
impl Ring {
    fn new() -> Self {
        Self {
            #[cfg(target_arch = "x86_64")] values: [Event::default(); 64],
            #[cfg(target_arch = "x86_64")] head: 0,
            #[cfg(target_arch = "x86_64")] tail: 0,
            #[cfg(target_arch = "aarch64")] values: std::collections::VecDeque::new(),
        }
    }
    fn push(&mut self, event: Event) -> bool {
        #[cfg(target_arch = "x86_64")]
        { input_ring::push(&mut self.values, &mut self.head, self.tail, event) }
        #[cfg(target_arch = "aarch64")]
        { event_admission::push(&mut self.values, 64, event) }
    }
    fn pop(&mut self) -> Option<Event> {
        #[cfg(target_arch = "x86_64")]
        {
            if self.head == self.tail { return None; }
            let value = self.values[self.tail];
            self.tail = (self.tail + 1) % self.values.len();
            Some(value)
        }
        #[cfg(target_arch = "aarch64")]
        { self.values.pop_front() }
    }
}

#[test]
fn a_slow_client_receives_discrete_events_in_order_and_motion_keeps_barriers() {
    let mut backlog = Backlog::new();
    let mut expected = Vec::new();
    for value in 0..201 {
        let e = event(1, value); expected.push(e); backlog.push(1, e, motion);
        if value == 100 {
            for kind in [2, 4, 5, 6, 8] {
                let e = event(kind, value); expected.push(e); backlog.push(1, e, motion);
            }
        }
    }
    let mut ring = Ring::new();
    let mut received = Vec::new();
    loop {
        backlog.flush(|id, e| {
            assert_eq!(id, 1);
            if ring.push(*e) { Delivery::Accepted } else { Delivery::Full }
        }, overflow);
        let mut drained = 0;
        for _ in 0..16 {
            if let Some(e) = ring.pop() { received.push(e); drained += 1; } else { break; }
        }
        if backlog.is_empty() && drained == 0 { break; }
    }
    assert_eq!(received, expected);
    for value in 0..200 { backlog.push(1, event(3, value), motion); }
    for e in [event(1, 30), event(3, 201), Event { kind: 3, value: 202, scroll: 1 }, event(3, 203)] {
        backlog.push(1, e, motion);
    }
    backlog.push(2, event(1, 99), motion);
    let mut other = Vec::new();
    backlog.flush(|id, e| {
        if id == 1 { Delivery::Full } else { other.push(*e); Delivery::Accepted }
    }, overflow);
    assert_eq!(other, [event(1, 99)]);
    let mut moves = Vec::new();
    backlog.flush(|_, e| { moves.push(*e); Delivery::Accepted }, overflow);
    assert_eq!(moves, [event(3, 199), event(1, 30), event(3, 201), Event { kind: 3, value: 202, scroll: 1 }, event(3, 203)]);
    backlog.push(3, event(1, 1), motion);
    backlog.flush(|_, _| Delivery::Gone, overflow);
    assert!(backlog.is_empty());
}

#[test]
fn backlog_limit_drops_oldest_and_reports_loss_without_disabling_other_windows() {
    let mut backlog = Backlog::new();
    for value in 0..MAX_PENDING { assert_eq!(backlog.push(1, event(1, value as u16), motion), Admission::Queued); }
    let mut attempts = 0;
    backlog.flush(|_, _| { attempts += 1; Delivery::Full }, overflow);
    assert_eq!(attempts, 1);
    for value in MAX_PENDING..MAX_PENDING + 2 {
        assert_eq!(backlog.push(1, event(1, value as u16), motion), Admission::Overflow);
    }
    assert_eq!(backlog.push(2, event(1, 42), motion), Admission::Queued);
    let mut other = Vec::new();
    backlog.flush(|id, e| {
        if id == 1 { assert_eq!(*e, overflow(2)); Delivery::Full }
        else { other.push(*e); Delivery::Accepted }
    }, overflow);
    assert_eq!(other, [event(1, 42)]);
    let mut delivered = Vec::new();
    backlog.flush(|id, e| { assert_eq!(id, 1); delivered.push(*e); Delivery::Accepted }, overflow);
    assert_eq!(delivered[0], overflow(2));
    assert_eq!(delivered[1..], (2..MAX_PENDING + 2).map(|v| event(1, v as u16)).collect::<Vec<_>>());
    backlog.push(1, event(1, 99), motion);
    backlog.retain_windows(|id| id != 1);
    assert!(backlog.is_empty());
    assert_eq!(backlog.push(1, event(1, 100), motion), Admission::Queued);
}

#[test]
fn permanent_delivery_error_is_not_retried_or_overtaken() {
    let mut backlog = Backlog::new();
    backlog.push(1, event(1, 1), motion); backlog.push(1, event(1, 2), motion);
    let mut attempts = 0;
    backlog.flush(|_, e| { attempts += 1; assert_eq!(*e, event(1, 1)); Delivery::Failed }, overflow);
    assert_eq!(attempts, 1);
    assert_eq!(backlog.push(1, event(1, 3), motion), Admission::Disabled);
    backlog.flush(|_, _| panic!("permanent delivery failure was retried"), overflow);
    assert!(backlog.is_empty());
}

#[test]
fn adjacent_motion_can_coalesce_at_the_limit_without_losing_discrete_events() {
    let mut backlog = Backlog::new();
    for value in 0..MAX_PENDING - 1 {
        assert_eq!(backlog.push(1, event(1, value as u16), motion), Admission::Queued);
    }
    for value in [50, 51] { assert_eq!(backlog.push(1, event(3, value), motion), Admission::Queued); }
    let mut received = Vec::new();
    backlog.flush(|_, e| { received.push(*e); Delivery::Accepted }, overflow);
    assert_eq!(received[..MAX_PENDING-1], (0..MAX_PENDING-1).map(|v| event(1, v as u16)).collect::<Vec<_>>());
    assert_eq!(received[MAX_PENDING-1], event(3, 51));
    assert_eq!(received.len(), MAX_PENDING);
}

#[test]
fn close_survives_overflow_and_retries_until_the_client_drains() {
    let mut backlog = Backlog::new();
    for value in 0..MAX_PENDING { backlog.push(1, event(1, value as u16), motion); }
    assert_eq!(backlog.push(1, event(7, 0), motion), Admission::Overflow);
    let mut ring = Ring::new();
    while ring.push(event(1, 99)) {}
    backlog.flush(|_, e| if ring.push(*e) { Delivery::Accepted } else { Delivery::Full }, overflow);
    assert!(!backlog.is_empty());
    while ring.pop().is_some() {}
    let mut received = Vec::new();
    while !backlog.is_empty() {
        backlog.flush(|_, e| if ring.push(*e) { Delivery::Accepted } else { Delivery::Full }, overflow);
        while let Some(e) = ring.pop() { received.push(e); }
    }
    assert_eq!(received[0], overflow(1));
    assert_eq!(*received.last().unwrap(), event(7, 0));
    assert_eq!(received.iter().filter(|e| e.kind == 7).count(), 1);
    backlog.flush(|_, _| panic!("close was duplicated"), overflow);
    backlog.request_close(1, event(7, 0));
    backlog.retain_windows(|_| false);
    assert!(backlog.is_empty());
}
