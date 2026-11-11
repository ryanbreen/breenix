use breengel::tree::{Flag, Node, Role, Tree};
use serde_json::{json, Value};

pub struct Object {
    pub bounds: [i64; 4],
    pub value: Value,
}

pub fn build(demo: &str, speed: i32, epochs: u64, in_bounds: bool, width: u32, height: u32, name: &str, objects: &[Object]) -> Tree {
    let bounds = [0, 0, width as i64, height as i64];
    let children = objects.iter().enumerate().map(|(index, object)| Node {
        key: format!("{name}-{index}"), parent: Some("status".into()), role: Role::Button,
        name: format!("{name}-{index}"), value: object.value.clone(), bounds: object.bounds,
        clip: bounds, flags: vec![Flag::Enabled, Flag::Visible],
    }).collect();
    Tree { root: Node {
        key: "status".into(), parent: None, role: Role::Status, name: demo.into(),
        value: json!({"demo": demo, "objects": objects.len(), "speed": speed, "epochs": epochs,
            "inBounds": in_bounds, "width": width, "height": height}),
        bounds, clip: bounds, flags: vec![Flag::Enabled, Flag::Visible],
    }, children }
}
