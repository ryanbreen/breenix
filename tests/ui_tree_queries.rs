extern crate alloc;
#[path = "../libs/libbui/src/tree.rs"]
mod tree;
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
