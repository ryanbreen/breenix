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
