#[path = "../libs/libuitest/src/messages.rs"] pub mod messages;
extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;
#[path = "../libs/libuitest/src/tree.rs"]
mod queries;
use tree::{Flag, Node, Role, Tree};
use serde_json::json;

fn sample() -> Tree {
    Tree { root: Node { key: "root".into(), parent: None, role: Role::Window,
        name: "capture \"window\"".into(), value: json!({"commit": 7, "producer": 8}),
        bounds: [0, 0, 640, 480], clip: [0, 0, 640, 480],
        flags: vec![Flag::Enabled, Flag::Visible] },
        children: vec![Node { key: "button".into(), parent: Some("root".into()),
            role: Role::Button, name: "Go\nnow".into(), value: json!("ready"),
            bounds: [10, 20, 100, 30], clip: [10, 20, 90, 30],
            flags: vec![Flag::Enabled, Flag::Visible, Flag::Focused] }] }
}

#[test]
fn dump_roundtrip() {
    let tree = sample();
    let mut page = vec![0xa5; 4096];
    let len = tree.write(&mut page).unwrap();
    assert_eq!(page[len - 1], 0);
    assert_eq!(Tree::parse(&page).unwrap(), tree);
    let legacy = br#"{"key":"root","parent":null,"role":"window","name":"capture","value":{"commit":7,"producer":8},"bounds":[0,0,16,4],"clip":[0,0,16,4],"flags":["enabled","visible"]}\0"#;
    let mut legacy = legacy.to_vec(); legacy.truncate(legacy.len()-2); legacy.push(0);
    let parsed = Tree::parse(&legacy).unwrap();
    assert!(parsed.children.is_empty());
    assert_eq!(parsed.root.value, json!({"commit":7,"producer":8}));
}

#[test]
fn truncation_flagged() {
    let mut tree = sample();
    tree.children.push(Node { key: "large".into(), name: "x".repeat(4000), ..tree.children[0].clone() });
    let mut page = vec![0; 1024];
    tree.write(&mut page).unwrap();
    let parsed = Tree::parse(&page).unwrap();
    assert!(parsed.root.flags.contains(&Flag::Truncated));
    assert_eq!(parsed.children, tree.children[..1]);
    let end = page.iter().position(|&b| b == 0).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&page[..end]).unwrap();
    assert_eq!(value["children"].as_array().unwrap().len(), 1);
    assert!(tree.write(&mut [0; 16]).is_err());
}

#[test]
fn find_unique() {
    use queries::{find, Query};
    let tree = sample();
    for query in [
        Query { key: Some("button"), ..Query::default() },
        Query { role: Some(Role::Button), name: Some("Go\nnow"), ..Query::default() },
        Query { key: Some("root"), ..Query::default() },
    ] {
        let node = find(&tree, &query).unwrap();
        assert_eq!(node.key, query.key.unwrap_or("button"));
    }
}

#[test]
fn find_ambiguous_fails_naming_both() {
    let mut tree = sample();
    tree.children.push(Node { key: "other-button".into(), ..tree.children[0].clone() });
    let err = queries::find(&tree, &queries::Query {
        role: Some(Role::Button), ..Default::default()
    }).unwrap_err();
    assert!(err.contains("ambiguous") && err.contains("button") && err.contains("other-button"), "{err}");
    tree.children[1].key = "button".into();
    assert!(queries::find(&tree, &queries::Query { key: Some("button"), ..Default::default() }).is_err());
}

#[test]
fn find_none_fails_naming_query() {
    let tree = sample();
    let err = queries::find(&tree, &queries::Query {
        key: Some("absent-submit"), role: Some(Role::Checkbox), name: Some("Accept"),
    }).unwrap_err();
    for expected in ["absent-submit", "Checkbox", "Accept"] { assert!(err.contains(expected), "{err}"); }
    // Matching the key alone must not bypass the remaining filters.
    assert!(queries::find(&tree, &queries::Query {
        key: Some("button"), role: Some(Role::Checkbox), name: None,
    }).is_err());
}

fn roles() -> Tree {
    let mut tree = sample();
    tree.children.clear();
    for (key, role, child, child_role) in [
        ("menu", Role::Menu, "item", Role::Menuitem),
        ("tabs", Role::Tablist, "tab", Role::Tab),
        ("track", Role::Scrollbar, "thumb", Role::ScrollThumb),
    ] {
        tree.children.push(Node { key: key.into(), role, bounds: [10, 20, 100, 40],
            clip: [10, 20, 100, 40], ..tree.root.clone() });
        tree.children.push(Node { key: child.into(), parent: Some(key.into()), role: child_role,
            bounds: [20, 25, 30, 20], clip: [20, 25, 30, 20], ..tree.root.clone() });
    }
    tree
}

#[test]
fn role_invariants() {
    let tree = roles();
    queries::role_invariants(&tree).unwrap();
    for index in [1, 3, 5] {
        for bounds in [[9, 25, 30, 20], [20, 19, 30, 20], [100, 25, 30, 20],
            [20, 55, 30, 20], [20, 25, -1, 20]] {
            let mut bad = tree.clone();
            bad.children[index].bounds = bounds;
            let err = queries::role_invariants(&bad).unwrap_err();
            assert!(err.contains(&bad.children[index].key) && err.contains("bounds"), "{err}");
        }
        for parent in [None, Some("missing"), Some("root")] {
            let mut bad = tree.clone();
            bad.children[index].parent = parent.map(Into::into);
            assert!(queries::role_invariants(&bad).unwrap_err().contains(&bad.children[index].key));
        }
    }
    let mut missing = tree.clone();
    missing.children.pop();
    assert!(queries::role_invariants(&missing).unwrap_err().contains("track"));
}

#[test]
fn fault_lie_bounds_uses_literal_geometry() {
    let mut tree = sample();
    queries::role_invariants(&tree).unwrap();
    queries::expect_bounds(&tree.children[0], [10, 20, 100, 30]).unwrap();
    tree.children[0].bounds[0] += 1;
    queries::role_invariants(&tree).unwrap();
    let err = queries::expect_bounds(&tree.children[0], [10, 20, 100, 30]).unwrap_err();
    for expected in ["tree-mismatch", "expect_bounds(button)", "bounds", "[10, 20, 100, 30]", "[11, 20, 100, 30]"] {
        assert!(err.contains(expected), "{err}");
    }
}
