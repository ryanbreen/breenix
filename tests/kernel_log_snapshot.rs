//! The logger's snapshot must advance even while a full ring keeps its size.
extern crate alloc;
#[path = "../kernel/src/log_buffer.rs"]
mod log_buffer;

#[test]
fn full_ring_has_absolute_offsets_and_byte_exact_payload() {
    log_buffer::init();
    log_buffer::capture_fmt(format_args!("first\n"));
    let first = log_buffer::sequenced_snapshot();
    assert!(first.starts_with("BREENIX-KMSG 1 start=0 end=6\n"));
    assert_eq!(log_buffer::read_all(), "first\n");
    let input: Vec<u8> = (0..70_000).map(|n| (n % 256) as u8).collect();
    log_buffer::capture_bytes(&input);
    let snapshot = log_buffer::sequenced_snapshot();
    let (header, hex) = snapshot.split_once('\n').unwrap();
    assert_eq!(header, "BREENIX-KMSG 1 start=37239 end=70006");
    let bytes: Vec<u8> = hex.trim().as_bytes().chunks_exact(2).map(|pair|
        u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap()).collect();
    assert_eq!(bytes, input[input.len() - 32767..]);
    log_buffer::capture_bytes(b"new\n");
    let next = log_buffer::sequenced_snapshot();
    assert!(next.starts_with("BREENIX-KMSG 1 start=37243 end=70010\n"));
    assert!(next.ends_with("6e65770a\n"));
}
