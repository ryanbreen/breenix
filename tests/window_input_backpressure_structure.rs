//! Input backpressure regression for #2163; run directly with rustc --test.

#[cfg(target_arch = "x86_64")]
#[path = "../kernel/src/syscall/graphics/input_ring.rs"]
mod input_ring;

#[cfg(target_arch = "x86_64")]
#[test]
fn a_full_ring_refuses_the_next_key_without_changing_queued_keys() {
    let mut ring = [0u16; 64];
    let (mut head, mut tail) = (0, 0);
    for key in 1..64 {
        assert!(input_ring::push(&mut ring, &mut head, tail, key));
    }
    let saved = ring;
    let previous_head = head;
    assert!(!input_ring::push(&mut ring, &mut head, tail, 64));
    assert_eq!(head, previous_head);
    assert_eq!(ring, saved);
    for key in 1..64 {
        assert_eq!(ring[tail], key);
        tail = (tail + 1) % ring.len();
    }
    assert_eq!(tail, head);
    assert!(input_ring::push(&mut ring, &mut head, tail, 64));
    assert_eq!(ring[tail], 64);
}

#[cfg(target_arch = "aarch64")]
#[path = "../kernel/src/window/event_admission.rs"]
mod event_admission;

#[cfg(target_arch = "aarch64")]
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
