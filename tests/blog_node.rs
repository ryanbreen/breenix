#[path = "../userspace/programs/src/blog/tail.rs"]
mod tail;
#[path = "../userspace/programs/src/blog/node.rs"]
mod node;

#[test]
fn visible_lines_have_absolute_numbers_and_offset() {
    let mut tail = tail::Tail::new();
    tail.feed(b"one\ntwo\nthree\n");
    assert_eq!(node::lines(&tail, 1, 1), serde_json::json!({"rows":[{"n":2,"text":"two"}],"offset":1,"total":3}));
}

#[test]
fn status_reports_open_and_waiting_files() {
    assert_eq!(node::status(1, "/tmp/log", true, false), serde_json::json!({"tab":1,"path":"/tmp/log","follow":true,"state":"waiting","search":null}));
    assert_eq!(node::status(0, "/var/log/kernel.log", false, true)["state"], "open");
}
