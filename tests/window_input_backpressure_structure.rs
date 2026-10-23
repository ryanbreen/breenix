//! Input backpressure regression for #2163; run directly with rustc --test.

#[path = "../kernel/src/window/event_admission.rs"]
mod event_admission;

#[test]
fn a_full_v2_ring_refuses_the_next_key_without_changing_queued_keys() {
    use std::collections::VecDeque;
    let mut queue = VecDeque::new();
    for key in 1..=64 { assert!(event_admission::push(&mut queue, 64, key)); }
    let saved = queue.clone();
    for _ in 0..10 { assert!(!event_admission::push(&mut queue, 64, 65)); }
    assert_eq!(queue, saved);
    for key in 1..=64 { assert_eq!(queue.pop_front(), Some(key)); }
    assert!(event_admission::push(&mut queue, 64, 65));
    assert_eq!(queue.pop_front(), Some(65));
    let events = include_str!("../kernel/src/window/events.rs");
    let ops = include_str!("../kernel/src/window/ops.rs");
    assert!(events.contains("super::event_admission::push(&mut self.queue, limits::EVENT_RING, event)"));
    assert!(ops.contains("take_while(|e| w.ring.try_push(*e))"));
}
