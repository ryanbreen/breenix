#[path = "../userspace/programs/src/blauncher/query.rs"]
mod query;

#[test]
fn enter_text_keeps_failed_selection_for_retry_and_edits_reset_it() {
    let (mut bytes, mut len, mut selected) = ([0; 64], 0, 1);
    for text in ["\r", "\n", "\t", ""] {
        query::append(&mut bytes, &mut len, &mut selected, text);
        assert_eq!((len, selected), (0, 1), "unchanged query: {text:?}");
    }
    query::append(&mut bytes, &mut len, &mut selected, "Log");
    assert_eq!((&bytes[..len], selected), (&b"Log"[..], 0));
    bytes.fill(b'a'); len = 63; selected = 1;
    query::append(&mut bytes, &mut len, &mut selected, "x");
    assert_eq!((len, selected), (63, 1));
}

#[test]
fn paste_stops_at_line_end_and_drops_controls() {
    let (mut bytes, mut len, mut selected) = ([0; 64], 0, 3);
    query::paste(&mut bytes, &mut len, &mut selected, b"Pick\x01er\t!\rignored\n");
    assert_eq!((&bytes[..len], selected), (&b"Picker!"[..], 0));
    query::paste(&mut bytes, &mut len, &mut selected, b" next\nignored");
    assert_eq!(&bytes[..len], b"Picker! next");
}

#[test]
fn paste_respects_utf8_and_all_64_bytes() {
    let (mut bytes, mut len, mut selected) = ([b'a'; 64], 62, 2);
    query::paste(&mut bytes, &mut len, &mut selected, "éx".as_bytes());
    assert_eq!(len, 64);
    assert_eq!(&bytes[62..len], "é".as_bytes());
    assert_eq!(selected, 0);
    len = 63;
    query::paste(&mut bytes, &mut len, &mut selected, "é".as_bytes());
    assert_eq!(len, 63);
}
