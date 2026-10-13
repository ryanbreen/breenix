//! Byte and canonical EOF records retain their order across wrap and flush.
const STDIN_BUF_SIZE: usize = 4096;
#[path = "../kernel/src/ipc/stdin_buffer.rs"]
mod buffer;
use buffer::StdinBuffer;

#[test]
fn eof_boundaries_preserve_data_on_both_sides() {
    let mut ring = StdinBuffer::new();
    assert!(ring.push_byte(b'a'));
    assert!(ring.push_eof());
    assert!(ring.push_byte(b'b'));
    assert!(ring.push_eof());
    let mut out = [0; 4];
    assert_eq!(ring.read_bytes(&mut out), 1);
    assert_eq!(out[0], b'a');
    assert_eq!(ring.read_bytes(&mut out), 0);
    assert!(!ring.is_empty());
    assert_eq!(ring.read_bytes(&mut out), 1);
    assert_eq!(out[0], b'b');
    assert_eq!(ring.read_bytes(&mut out), 0);
    assert!(ring.is_empty());
}

#[test]
fn consecutive_eof_records_are_bounded_and_flush_clears_them() {
    let mut ring = StdinBuffer::new();
    for _ in 0..STDIN_BUF_SIZE - 1 { assert!(ring.push_eof()); }
    assert!(!ring.push_eof());
    assert!(!ring.push_byte(b'a'));
    ring.flush();
    assert!(ring.is_empty());
    assert!(ring.push_byte(b'b'));
    let mut out = [0; 1];
    assert_eq!(ring.read_bytes(&mut out), 1);
    assert_eq!(out, [b'b']);
    assert!(ring.is_empty());
}

#[test]
fn eof_survives_a_wrapped_byte_ring_and_zero_length_read() {
    let mut ring = StdinBuffer::new();
    let mut out = [0; 4];
    for _ in 0..STDIN_BUF_SIZE + 3 {
        assert!(ring.push_byte(b'x'));
        assert_eq!(ring.read_bytes(&mut out), 1);
    }
    assert!(ring.push_eof());
    assert_eq!(ring.read_bytes(&mut []), 0);
    assert!(!ring.is_empty());
    assert_eq!(ring.read_bytes(&mut out), 0);
    assert!(ring.is_empty());
    assert_eq!(ring.available(), 0);
}
