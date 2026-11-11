#[path = "../src/app.rs"]
mod app;
use app::{deadline_expired, matches_row, timeout_message, WATCHDOG_NS};

#[test]
fn expect_text_matches_a_whole_trimmed_row_only() {
    assert!(matches_row(&["  hello  ", "$ echo hello"], "hello"));
    assert!(!matches_row(&["$ echo hello", "hello world"], "hello"));
}

#[test]
fn watchdog_deadline_and_failure_text() {
    assert_eq!(WATCHDOG_NS, 10_000_000_000);
    assert!(!deadline_expired(12, 12 + WATCHDOG_NS - 1));
    assert!(deadline_expired(12, 12 + WATCHDOG_NS));
    let message = timeout_message("hello", &["$ echo hello", "last row"]);
    assert!(message.contains("hello"));
    assert!(message.contains("$ echo hello"));
    assert!(message.contains("last row"));
}

#[test]
fn finish_requires_expected_exit_or_teardown_kill_for_each_app() {
    use app::finish_verdict;
    assert!(finish_verdict(&[(10, Some(0)), (20, None)], &[(10, 0), (20, 9)]).is_ok());
    assert!(finish_verdict(&[(10, Some(0)), (20, None)], &[(10, 0), (20, 0)]).is_err());
    assert!(finish_verdict(&[(10, None)], &[]).is_err());
    assert!(finish_verdict(&[(10, Some(0))], &[(10, 9)]).is_err());
}

#[test]
fn wait_node_failure_reports_key_and_last_value() {
    let message = app::node_timeout_message("query", Some(&serde_json::json!({"text":"last"})));
    assert!(message.contains("query"));
    assert!(message.contains("last"));
    assert!(app::node_timeout_message("missing", None).contains("missing"));
}

#[test]
fn exit_input_requires_targeted_delivery_and_clean_expected_exit() {
    use app::exit_input_verdict;
    // The client destroys its window in the callback: the durable delivery
    // record plus its supervised exit must both survive that ordering.
    assert!(exit_input_verdict("delivered", 12, 12, 0, 0).is_ok());
    assert!(exit_input_verdict("delivered", 13, 12, 0, 0).is_err());
    assert!(exit_input_verdict("wm-handled", 12, 12, 0, 0).is_err());
    assert!(exit_input_verdict("missing", 0, 12, 0, 0).is_err());
    assert!(exit_input_verdict("delivered", 12, 12, 9, 0).is_err());
    assert!(exit_input_verdict("delivered", 12, 12, 256, 0).is_err());
}

#[test]
fn exit_release_requires_a_disposition_and_allows_only_no_focus_rejection() {
    use app::exit_release_verdict;
    assert!(exit_release_verdict("delivered", 13, "none").is_ok());
    assert!(exit_release_verdict("wm-handled", 0, "none").is_ok());
    assert!(exit_release_verdict("rejected", 0, "no-focus").is_ok());
    assert!(exit_release_verdict("rejected", 13, "no-focus").is_err());
    assert!(exit_release_verdict("rejected", 0, "overflow").is_err());
    assert!(exit_release_verdict("missing", 0, "none").is_err());
    assert!(exit_release_verdict("client-rejected", 0, "no-focus").is_err());
}

#[test]
fn desktop_exit_requires_wm_handling_and_clean_exit() {
    use app::desktop_exit_input_verdict as verdict;
    assert!(verdict("wm-handled", 0, 0).is_ok());
    assert!(verdict("delivered", 12, 0).is_err());
    assert!(verdict("rejected", 0, 0).is_err());
    assert!(verdict("wm-handled", 0, 9).is_err());
}

#[test]
fn drag_start_must_be_inside_visible_node_bounds() {
    use libbui::tree::{Flag, Node, Role};
    let mut node = Node { key: "canvas".into(), parent: None, role: Role::Canvas,
        name: String::new(), value: serde_json::Value::Null, bounds: [10, 20, 8, 6],
        clip: [12, 21, 5, 4], flags: vec![Flag::Visible] };
    for point in [(2, 1), (6, 1), (2, 4), (6, 4), (4, 2)] {
        assert!(app::drag_start(&node, point).is_ok(), "{point:?}");
    }
    for point in [(1, 2), (7, 2), (3, 0), (3, 5), (-1, 0), (8, 6)] {
        assert!(app::drag_start(&node, point).is_err(), "{point:?}");
    }
    node.clip = [0, 0, 0, 0];
    assert!(app::drag_start(&node, (0, 0)).is_err());
    node.clip = node.bounds;
    node.flags.clear();
    assert!(app::drag_start(&node, (0, 0)).is_err());
}
