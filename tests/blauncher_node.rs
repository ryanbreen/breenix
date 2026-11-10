#[path = "../userspace/programs/src/blauncher/catalog.rs"]
mod catalog;
#[path = "../userspace/programs/src/blauncher/node.rs"]
mod node;

#[test]
fn publishes_visible_rows_errors_and_empty_state() {
    let (entries, errors) = catalog::parse("Alpha\tFirst\tgui\t/bin/a\ninvalid\nBeta\tSecond\tterminal\t/bin/b\n");
    let value = node::value(&entries, &errors, "", 1, 1);
    assert_eq!(value, serde_json::json!({"query":"", "focused":"query", "selected":1,
        "rows":[{"name":"Alpha","description":"First","mode":"gui"}],
        "errors":[{"line":2,"reason":"too few fields"}], "empty":null}));
    let value = node::value(&entries, &errors, "missing", 0, 12);
    assert_eq!(value["rows"], serde_json::json!([]));
    assert_eq!(value["empty"], "No matches");
}
