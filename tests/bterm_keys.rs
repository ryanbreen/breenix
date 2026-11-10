#[path = "../userspace/programs/src/bterm/keys.rs"]
mod keys;

#[test]
fn terminal_keys_follow_xterm_sequences() {
    let rows: &[(u32, &[u8], &[u8])] = &[
        (103, b"\x1b[A", b"\x1bOA"), (108, b"\x1b[B", b"\x1bOB"),
        (106, b"\x1b[C", b"\x1bOC"), (105, b"\x1b[D", b"\x1bOD"),
        (102, b"\x1b[H", b"\x1bOH"), (107, b"\x1b[F", b"\x1bOF"),
        (111, b"\x1b[3~", b"\x1b[3~"), (104, b"\x1b[5~", b"\x1b[5~"),
        (109, b"\x1b[6~", b"\x1b[6~"), (59, b"\x1bOP", b"\x1bOP"),
        (60, b"\x1bOQ", b"\x1bOQ"), (61, b"\x1bOR", b"\x1bOR"),
        (62, b"\x1bOS", b"\x1bOS"), (14, b"\x7f", b"\x7f"), (28, b"\r", b"\r"),
    ];
    for &(key, normal, application) in rows {
        assert_eq!(keys::encode(key, false, "", false), normal, "key {key}");
        assert_eq!(keys::encode(key, false, "", true), application, "key {key} application");
    }
    for letter in b'a'..=b'z' {
        let text = (letter as char).to_string();
        assert_eq!(keys::encode(0, true, &text, false), [letter - b'a' + 1]);
    }
    assert_eq!(keys::encode(0, false, "é", false), "é".as_bytes());
}

#[test]
fn key_and_text_events_insert_printable_text_once() {
    for (key, text) in [(30, "a"), (30, "A"), (0, "é界")] {
        let mut bytes = keys::encode(key, false, text, false);
        bytes.extend(keys::encode_text(text, Some(17), 17));
        assert_eq!(bytes, text.as_bytes());
    }
    let mut bytes = keys::encode(30, true, "\x01", false);
    bytes.extend(keys::encode_text("\x01", Some(17), 17));
    assert_eq!(bytes, [1]);
    assert_eq!(keys::encode(103, false, "", false), b"\x1b[A");
    assert!(keys::encode_text("+", Some(17), 17).is_empty());
    assert_eq!(keys::encode_text("é界\n", None, 17), "é界\n".as_bytes());
    assert_eq!(keys::encode_text("long input continuation", Some(18), 17), b"long input continuation");
}
