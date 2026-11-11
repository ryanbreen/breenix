#[path = "../userspace/programs/src/bterm/terminal_node.rs"]
mod terminal_node;

#[test]
fn visible_grid_preserves_rows_and_trims_only_trailing_spaces() {
    let mut vt = libvt::TermEmu::new(6, 3);
    vt.feed(b"abc\x1b[2;1Hhello\x1b[3;4Hx\x1b[2;5H\x1b]2;Terminal\x07");
    let value = terminal_node::terminal_value(&vt, 1, 2, "/fonts/mono.ttf", 18.0);
    assert_eq!(value, serde_json::json!({
        "cols": 6, "rows": 3, "cursor": [4, 1], "active_tab": 1, "tab_count": 2,
        "lines": ["abc", "hello", "   x"], "title": "Terminal",
        "alt": false, "scrollback": 0, "offset": 0, "runs": [[], [], []],
        "font": {"path": "/fonts/mono.ttf", "size": 18.0}
    }));
}

#[test]
fn history_view_publishes_the_cells_being_drawn() {
    let mut vt = libvt::TermEmu::new(6, 2);
    vt.feed(b"\x1b[31mone\r\n\x1b[0mtwo\r\nthree");
    vt.scroll_view(1);
    let value = terminal_node::terminal_value(&vt, 0, 1, "/fonts/mono.ttf", 18.0);
    assert_eq!(value["scrollback"], 1);
    assert_eq!(value["offset"], 1);
    assert_eq!(value["lines"], serde_json::json!(["one", "two"]));
    assert_eq!(value["runs"][0][0]["fg"], serde_json::json!([205, 49, 49]));
    vt.feed(b"!");
    let value = terminal_node::terminal_value(&vt, 0, 1, "/fonts/mono.ttf", 18.0);
    assert_eq!(value["offset"], 0);
    assert_eq!(value["lines"], serde_json::json!(["two", "three!"]));
}

#[path = "../userspace/programs/src/bterm/selection.rs"]
mod selection;

#[test]
fn tree_reports_selection_and_cell_geometry() {
    let mut vt = libvt::TermEmu::new(6, 2);
    vt.feed(b"hello");
    let g = selection::Geometry { x: 0, y: 24, w: 8, h: 16 };
    let mut s = selection::Selection::default();
    let value = terminal_node::selection_fields(&vt, &s, g);
    assert_eq!(value["selection"], serde_json::Value::Null);
    assert_eq!(value["cell"], serde_json::json!({"x": 0, "y": 24, "w": 8, "h": 16}));
    s.press(8, 24, g, vt.size()); s.release(24, 24, g, vt.size());
    assert!(s.contains(2, 0));
    let value = terminal_node::selection_fields(&vt, &s, g);
    assert_eq!(value["selection"], serde_json::json!({"start": [1, 0], "end": [3, 0], "text": "ell"}));
}
