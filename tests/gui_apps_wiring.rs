#[test]
fn gui_apps_cases_have_sessions_and_no_placeholder() {
    let source = include_str!("../userspace/programs/src/suite_gui_apps.rs");
    let cases: Vec<_> = source.lines().filter(|line| line.trim_start().starts_with("case(")).collect();
    assert_eq!(cases.len(), 22);
    for case in cases {
        assert!(case.contains(".session(libuitest::supervisor::serve)"), "missing session: {case}");
        assert!(!case.contains("pending::"), "placeholder: {case}");
    }
}

#[test]
fn bterm_handles_input_before_publishing_the_watermark() {
    let source = include_str!("../userspace/programs/src/bterm.rs");
    assert!(!source.contains("win.poll_events()"), "polling cannot acknowledge handled input");
    let dispatch = source.split("win.dispatch(|delivered| {").nth(1).expect("dispatch handler");
    let handler = dispatch.split("}).unwrap_or_else").next().unwrap();
    assert!(handler.contains("tab.send_input(&bytes)"), "PTY input must run inside the handler");
    assert!(handler.contains("Event::Text { text }"), "core text input must reach the PTY");
    assert!(handler.contains("Handled::Redraw"), "input must wait for the rendered commit");
}

#[test]
fn blauncher_dispatches_input_before_presenting() {
    let source = include_str!("../userspace/programs/src/blauncher.rs");
    assert!(!source.contains("win.poll_events()"));
    assert!(source.contains("win.dispatch(|delivered|"));
    assert!(source.contains("Handled::Redraw"));
    assert!(source.contains("Event::Text { text }"));
}
