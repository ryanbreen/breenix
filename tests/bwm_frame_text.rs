extern crate alloc;
extern crate self as libbui;

#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;
#[path = "../userspace/programs/src/bterm/selection.rs"]
mod selection;
#[path = "../userspace/programs/src/bterm/terminal_node.rs"]
mod terminal_node;
#[path = "../userspace/programs/src/bwm/chrome/frame_text.rs"]
mod frame_text;

use tree::{Node, Role, Tree};

fn page(role: Role, value: serde_json::Value) -> Vec<u8> {
    let tree = Tree { root: Node { key: "grid".into(), parent: None, role,
        name: "Terminal".into(), value, bounds: [0, 0, 640, 480],
        clip: [0, 0, 640, 480], flags: vec![] }, children: vec![] };
    let mut page = vec![0; 16384];
    tree.write(&mut page).unwrap();
    page
}

#[test]
fn extracts_terminal_cursor_row_from_captured_page() {
    let mut vt = libvt::TermEmu::new(80, 24);
    vt.feed(b"bsh /> ");
    let value = terminal_node::terminal_value(&vt, 0, 1, "", 16.0);
    let tree = Tree::parse(&page(Role::Terminal, value)).unwrap();
    assert_eq!(frame_text::cursor_row(&tree), Some((0, "bsh />")));

    let empty = libvt::TermEmu::new(80, 24);
    let value = terminal_node::terminal_value(&empty, 0, 1, "", 16.0);
    let tree = Tree::parse(&page(Role::Terminal, value.clone())).unwrap();
    assert_eq!(frame_text::cursor_row(&tree), Some((0, "")));
    let tree = Tree::parse(&page(Role::Window, value)).unwrap();
    assert_eq!(frame_text::cursor_row(&tree), None);
}

#[test]
fn rejects_malformed_terminal_values() {
    for value in [serde_json::json!({}), serde_json::json!({"cursor": [0, -1], "lines": [""]}),
        serde_json::json!({"cursor": [0, 1], "lines": [""]}),
        serde_json::json!({"cursor": [0, 0], "lines": [7]})] {
        let tree = Tree::parse(&page(Role::Terminal, value)).unwrap();
        assert_eq!(frame_text::cursor_row(&tree), None);
    }
}
