#[path = "../userspace/programs/src/bterm/terminal_node.rs"]
mod terminal_node;

#[test]
fn visible_grid_preserves_rows_and_trims_only_trailing_spaces() {
    let value = terminal_node::terminal_value(6, 3, [4, 1], b"abc   hello     x ", "Terminal");
    assert_eq!(value, serde_json::json!({
        "cols": 6, "rows": 3, "cursor": [4, 1],
        "lines": ["abc", "hello", "   x"], "title": "Terminal"
    }));
}
