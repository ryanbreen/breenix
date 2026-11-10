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
