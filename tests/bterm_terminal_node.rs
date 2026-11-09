#[path = "../userspace/programs/src/bterm/terminal_node.rs"]
mod terminal_node;

#[test]
fn visible_grid_preserves_rows_and_trims_only_trailing_spaces() {
    let mut vt = libvt::TermEmu::new(6, 3);
    vt.feed(b"abc\x1b[2;1Hhello\x1b[3;4Hx\x1b[2;5H\x1b]2;Terminal\x07");
    let value = terminal_node::terminal_value(&vt);
    assert_eq!(value, serde_json::json!({
        "cols": 6, "rows": 3, "cursor": [4, 1],
        "lines": ["abc", "hello", "   x"], "title": "Terminal",
        "alt": false, "scrollback": 0, "offset": 0, "runs": [[], [], []]
    }));
}
