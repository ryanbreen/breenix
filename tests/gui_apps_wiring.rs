#[test]
fn gui_apps_cases_have_sessions_and_no_placeholder() {
    let source = include_str!("../userspace/programs/src/suite_gui_apps.rs");
    let cases: Vec<_> = source.lines().filter(|line| line.trim_start().starts_with("case(")).collect();
    // #2880 corrected the base to 55; collaboration adds six cases.
    assert_eq!(cases.len(), 70);
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
    let source = include_str!("../userspace/programs/src/guskit.rs");
    assert!(source.contains("win.dispatch("));
    assert!(source.contains("session.poll(10)"));
    assert!(include_str!("../userspace/programs/Cargo.toml").contains("libcollab"));
    assert!(source.contains("win.set_tree("));
    for forbidden in ["poll_events", "framebuf", "fb_mmap"] {
        assert!(!source.contains(forbidden), "unexpected {forbidden}");
    }
    assert!(include_str!("../userspace/programs/build.sh").contains("guskit"));
}

#[test]
fn bounce_uses_animation_dispatch_and_publishes_status() {
    let source = include_str!("../userspace/programs/src/bounce.rs");
    for forbidden in ["poll_events", "clock_monotonic", "libbreenix::graphics", "fb_mmap"] {
        assert!(!source.contains(forbidden), "unexpected {forbidden}");
    }
    for required in ["Event::CloseRequested", "win.set_animating(true)", "win.wait_dispatch(", "win.set_tree("] {
        assert!(source.contains(required), "missing {required}");
    }
}

#[test]
fn demos_use_window_animation_and_status() {
    for name in ["particles", "confetti", "demo", "rectangles", "biconkit"] {
        let source = std::fs::read_to_string(format!("userspace/programs/src/{name}.rs")).expect("demo source");
        for forbidden in ["poll_events", "libbreenix::graphics", "libgfx_ref", "fb_mmap", "mouse_pos", "clock_monotonic", "now_monotonic"] {
            assert!(!source.contains(forbidden), "{name}: unexpected {forbidden}");
        }
        for required in ["Window::new_without_fonts", "Event::CloseRequested", "Event::AnimationTick", "win.set_animating(true)", "win.wait_dispatch(", "win.surface()", "win.set_tree(", "Event::ResizeRequested"] {
            assert!(source.contains(required), "{name}: missing {required}");
        }
    }
}

#[test]
fn app_hover_uses_visible_point_without_button() {
    let source = include_str!("../libs/libuitest/src/app.rs");
    let hover = source.split("pub fn hover(").nth(1).expect("hover helper")
        .split("pub fn ").next().unwrap();
    assert!(hover.contains("drag_start(node, at)"));
    assert!(hover.contains("live.move_to("));
    assert!(!hover.contains(".button("));
}

#[test]
fn app_close_uses_alt_and_desktop_exit() {
    let source = include_str!("../libs/libuitest/src/app.rs");
    let close = source.split("pub fn close_exit(").nth(1).expect("close helper")
        .split("pub fn ").next().unwrap();
    let down = close.find("wm_key(key::LEFT_ALT, true)").expect("Alt down");
    let exit = close.find("input_exit(16, false, 0, true)").expect("desktop Q exit");
    let up = close.find("wm_key(key::LEFT_ALT, false)").expect("Alt up");
    assert!(down < exit && exit < up);
}

#[test]
fn blog_font_change_republishes_the_drawing_font() {
    let source = include_str!("../userspace/programs/src/blog.rs");
    let publish = source.split("fn publish_tree(").nth(1).unwrap().split("fn format_line_number(").next().unwrap();
    assert!(publish.contains("win.mono_path()"));
    assert!(publish.contains("fonts.size"));
    let events = source.split("Event::FontChanged =>").nth(1).unwrap();
    let reload = events.split("if reload_font {").nth(1).unwrap();
    assert!(reload.contains("fonts.size = new_size"));
    assert!(reload.contains("needs_redraw = true"));
    assert!(reload.contains("publish_tree(&mut win, &tabs, tab_bar.selected(), visible_lines, &fonts)"));
}

#[test]
fn guskit_reloads_and_draws_with_the_display_font() {
    let source = include_str!("../userspace/programs/src/guskit.rs");
    assert!(source.contains("Event::FontChanged"));
    assert!(source.matches("take_display_font()").count() >= 2);
    assert!(source.contains("ttf_font::draw_text"));
    assert!(source.contains("display_path()"));
}
