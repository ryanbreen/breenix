fn body<'a>(source: &'a str, signature: &str) -> &'a str {
    let rest = &source[source.find(signature).expect(signature)..];
    let open = rest.find('{').unwrap();
    let mut depth = 1;
    for (i, c) in rest[open + 1..].char_indices() {
        match c { '{' => depth += 1, '}' => depth -= 1, _ => {} }
        if depth == 0 { return &rest[open + 1..open + 1 + i]; }
    }
    panic!("unterminated {signature}")
}

#[test]
fn dispatch_runs_clock_handlers_before_idle_acknowledgment() {
    let source = include_str!("../libs/breengel/src/window.rs");
    assert!(!body(source, "fn read(").contains("self.no_change()?"),
        "reading the epoch must not acknowledge unfinished timer handlers");
    for signature in ["pub fn dispatch(", "pub fn wait_dispatch("] {
        let dispatch = body(source, signature);
        assert!(dispatch.find("self.deliver(").unwrap() < dispatch.find("self.acknowledge_clock(").unwrap());
    }
    let deliver = body(source, "fn deliver(");
    assert!(deliver.contains("self.deliver_clock(handler)"));
    let ack = body(source, "fn acknowledge_clock(");
    assert!(ack.contains("!self.clock_redraw"));
}

#[test]
fn timer_client_publishes_no_redraw_input_watermarks() {
    let source = include_str!("../userspace/programs/src/uitest_timer_app.rs");
    let run = body(source, "pub fn run(");
    assert!(run.contains("let count = win.wait_dispatch("));
    assert!(run.contains("else if count != 0"));
    assert!(run.contains("win.commit_no_change()"), "a release must publish its processed serial with no new pixels");
}
