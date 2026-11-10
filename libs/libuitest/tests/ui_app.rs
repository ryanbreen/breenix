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
