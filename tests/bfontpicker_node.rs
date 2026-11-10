#[path = "../userspace/programs/src/bfontpicker/node.rs"]
mod node;

#[test]
fn publishes_font_rows_and_selection() {
    let rows = [node::Row { name: "Broken", path: "/fonts/b.ttf", section: "display", usable: false, active: false }];
    assert_eq!(node::fonts(&rows, Some(0)), serde_json::json!({"rows":[{"name":"Broken","path":"/fonts/b.ttf","section":"display","usable":false,"active":false}],"selected":0}));
}

#[test]
fn publishes_save_outcome_and_error() {
    assert_eq!(node::status("mono.ttf", 12.0, "display.ttf", 14.0, Some("unconfirmed"), Some("sync failed")), serde_json::json!({"mono":{"path":"mono.ttf","size":12.0},"display":{"path":"display.ttf","size":14.0},"save":"unconfirmed","error":"sync failed"}));
}
