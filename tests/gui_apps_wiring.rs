#[test]
fn gui_apps_cases_have_sessions_and_no_placeholder() {
    let source = include_str!("../userspace/programs/src/suite_gui_apps.rs");
    let cases: Vec<_> = source.lines().filter(|line| line.trim_start().starts_with("case(")).collect();
    assert_eq!(cases.len(), 52);
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

#[test]
fn bfontpicker_dispatches_input_before_presenting() {
    let source = include_str!("../userspace/programs/src/bfontpicker.rs");
    assert!(!source.contains("win.poll_events()"));
    assert!(source.contains("win.dispatch(|delivered|"));
    assert!(source.contains("Handled::Redraw"));
}

#[test]
fn app_launch_as_passes_the_requested_uid() {
    let source = include_str!("../libs/libuitest/src/app.rs");
    let launch = source.split("pub fn launch_as(").nth(1).expect("launch_as").split("pub fn hotkey(").next().unwrap();
    assert!(launch.contains("launch_app(argv, uid)"));
    assert!(source.contains("Self::launch_as(argv, title, 0)"));
}

#[test]
fn live_font_change_advances_the_session_clock_before_waiting_for_reload() {
    let source = include_str!("../userspace/programs/src/guiapps_suite/bfontpicker.rs");
    let live = source.split("pub fn live_bterm()").nth(1).unwrap()
        .split("pub fn save_fails_cleanly()").next().unwrap();
    let after_save = live.split("select(&mut t)?;").nth(1).unwrap();
    let before_wait = after_save.split("t.wait_node(\"grid\"").next().unwrap();
    assert!(before_wait.contains("t.step(250)?;"),
        "font reload is paced at 250 ms of the frozen session clock; wall-clock polling cannot advance it");
}

#[test]
fn bcheck_dispatches_input_before_presenting() {
    let source = include_str!("../userspace/programs/src/bcheck.rs");
    assert!(!source.contains("win.poll_events()"));
    assert!(source.contains("win.dispatch(|delivered|"));
    assert!(source.contains("Handled::Redraw"));
}

#[test]
fn guskit_uses_window_dispatch_and_publishes_tree() {
    let source = std::fs::read_to_string("userspace/programs/src/guskit.rs").expect("Gus Kit binary");
    assert!(source.contains("win.dispatch("));
    assert!(source.contains("win.set_tree("));
    for forbidden in ["poll_events", "framebuf", "fb_mmap", "libcollab"] {
        assert!(!source.contains(forbidden), "unexpected {forbidden}");
    }
    assert!(include_str!("../userspace/programs/build.sh").contains("guskit"));
}
