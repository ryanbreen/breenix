//! Input backpressure regression for #2163; run directly with rustc --test.

#[path = "../kernel/src/syscall/graphics/input_ring.rs"]
mod input_ring;

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
