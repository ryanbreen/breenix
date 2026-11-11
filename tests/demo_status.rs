extern crate alloc;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;
extern crate self as breengel;
#[path = "../userspace/programs/src/demos/status.rs"]
mod status;

#[test]
fn publishes_status_and_clickable_objects() {
    let objects = [status::Object { bounds: [10, 20, 30, 40], value: serde_json::json!({"vx": -5, "vy": 7}) }];
    let tree = status::build("bounce", 125, 42, true, 400, 300, "sphere", &objects);
    assert_eq!(tree.root.key, "status");
    assert_eq!(tree.root.role, tree::Role::Status);
    assert_eq!(tree.root.bounds, [0, 0, 400, 300]);
    assert_eq!(tree.root.value, serde_json::json!({"demo":"bounce", "objects":1, "speed":125, "epochs":42, "inBounds":true, "width":400, "height":300}));
    assert_eq!(tree.children.len(), 1);
    let child = &tree.children[0];
    assert_eq!(child.key, "sphere-0");
    assert_eq!(child.parent.as_deref(), Some("status"));
    assert_eq!(child.role, tree::Role::Button);
    assert_eq!(child.bounds, [10, 20, 30, 40]);
    assert_eq!(child.value, serde_json::json!({"vx":-5,"vy":7}));
}
