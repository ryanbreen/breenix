#[test]
fn gui_apps_cases_have_sessions_and_no_placeholder() {
    let source = include_str!("../userspace/programs/src/suite_gui_apps.rs");
    let cases: Vec<_> = source.lines().filter(|line| line.trim_start().starts_with("case(")).collect();
    assert_eq!(cases.len(), 30);
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

#[test]
fn launcher_cases_wait_for_the_registered_chromeless_title() {
    let launcher = include_str!("../userspace/programs/src/blauncher.rs");
    let cases = include_str!("../userspace/programs/src/guiapps_suite/blauncher.rs");
    assert!(launcher.contains(r#"Window::new(b"\x01Launcher""#));
    assert!(!cases.contains(r#""Launcher")?"#), "launcher waits must include the chromeless title prefix");
}

#[test]
fn acknowledgment_fixture_publishes_handled_input() {
    let source = include_str!("../userspace/programs/src/ack_app_test.rs");
    assert!(!source.contains("win.wait_event()"), "poll-only fixture never acknowledges attach-probe input");
    assert!(source.contains("win.wait_dispatch("));
    assert!(source.contains("Handled::NoChange"));
    assert!(source.contains("win.commit_no_change()"));
}

#[test]
fn hotkey_open_observes_focus_without_activating_the_window() {
    let source = include_str!("../libs/libuitest/src/app.rs");
    let hotkey = source.split("pub fn hotkey(").nth(1).unwrap().split("pub fn launch_next(").next().unwrap();
    assert!(!hotkey.contains(".activate("), "activation conceals missing WM focus");
}

#[test]
fn both_close_actions_use_the_one_declared_session() {
    let source = include_str!("../userspace/programs/src/guiapps_suite/blauncher.rs");
    let close = source.split("pub fn close()").nth(1).unwrap();
    assert_eq!(close.matches("App::launch(").count(), 1,
        "take_session_control is single-use; relaunch within the existing session");
    assert_eq!(close.matches("t.finish()").count(), 1);
    assert!(close.contains("t.launch_next("));
}

#[test]
fn blog_dispatches_input_before_presenting() {
    let source = include_str!("../userspace/programs/src/blog.rs");
    assert!(!source.contains("win.poll_events()"));
    assert!(source.contains("win.dispatch(|delivered|"));
    assert!(source.contains("Handled::Redraw"));
}
