extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;
#[path = "../userspace/programs/src/guskit/node.rs"]
mod node;
use libbui::tree::{Flag, Role};
use serde_json::json;

#[test]
fn builders_publish_canvas_status_tools_and_controls() {
    let bounds = [1, 2, 100, 100];
    let canvas = node::canvas(bounds, 2, 3, [0xab; 32]);
    assert_eq!(canvas.role, Role::Canvas);
    assert_eq!(canvas.value, json!({"width":2,"height":3,"sha256":"ab".repeat(32)}));
    let status = node::status(bounds, &node::Status { tool: "brush", size: "medium", color: [1,2,3], undo: 2, redo: 1, dirty: true, file: Some("a.bmp"), message: "saved", font_path: "/fonts/display.ttf", font_size: 16 });
    assert_eq!(status.role, Role::Status);
    assert_eq!(status.value, json!({"tool":"brush","size":"medium","color":[1,2,3],"undo":2,"redo":1,"dirty":true,"file":"a.bmp","message":"saved","font":{"path":"/fonts/display.ttf","size":16}}));
    let tools = node::tools([bounds; 7], "brush");
    assert_eq!(tools.len(), 7);
    assert_eq!(tools.iter().filter(|n| n.flags.contains(&Flag::Selected)).map(|n| n.key.as_str()).collect::<Vec<_>>(), ["tool-brush"]);
    assert!(tools.iter().all(|n| n.role == Role::Button));
    assert_eq!(node::hue(bounds).key, "hue");
    assert_eq!(node::sv(bounds).key, "sv");
}

#[test]
fn picker_mapping_matches_restored_controls() {
    assert_eq!(node::hue_at(50, 100), 180);
    assert_eq!(node::sv_at(99, 0, 100), (252, 252));
    assert_eq!(node::sv_at(0, 99, 100), (0, 0));
    assert_eq!(node::hsv_to_rgb(0, 255, 255), [255,0,0]);
    assert_eq!(node::hsv_to_rgb(120, 255, 255), [0,255,0]);
    assert_eq!(node::hsv_to_rgb(240, 255, 255), [0,0,255]);
    assert_eq!(node::hsv_to_rgb(30, 255, 255), [255,128,0]);
    assert_eq!(node::hsv_to_rgb(45, 0, 42), [42; 3]);
}
