//! Run the current per-boot intent receiver without a guest.
#[path = "../kernel/src/boot/intent.rs"]
mod intent;

#[test]
fn current_console_target_is_retained_and_obsolete_payload_is_refused() {
    assert!(intent::get().is_none());
    for bytes in [&[0; 2048][..], b"BREENIX-CONSOLE 1\nsession\n",
                  b"BREENIX-CONSOLE 1\nmeasure\n", b"BREENIX-BOOT 1\n"] {
        assert!(!intent::capture(bytes));
    }
    let oversized = [b'c'; 257];
    let mut payload = b"BREENIX-BOOT 1\n".to_vec();
    payload.extend_from_slice(&oversized);
    assert!(!intent::capture(&payload));
    assert!(intent::get().is_none());
    assert!(intent::capture(b"BREENIX-BOOT 1\nconsole replug\n"));
    assert_eq!(intent::get(), Some(&b"console replug\n"[..]));
}
