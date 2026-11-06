#[path = "../userspace/programs/src/bwm/close.rs"]
mod close;

use close::{Action, Close};

#[test]
fn close_escalates_on_ui_time_and_forgets_removed_windows() {
    let mut close = Close::default();
    let mut signals = Vec::new();
    let mut now = 10_000_000_000;
    assert_eq!(close.request(7, 42, now), Action::Request);
    close.due(now, |pid, signal| signals.push((pid, signal)));
    assert!(signals.is_empty());
    assert_eq!(close.request(7, 42, now), Action::Term(42));
    assert_eq!(close.timeout_ms(now, 3_000), 2_000);
    now += 1_999_000_000;
    assert_eq!(close.timeout_ms(now, 10), 1);
    assert!(!close.has_due(now));
    close.due(now, |pid, signal| signals.push((pid, signal)));
    assert!(signals.is_empty());
    now += 1_000_000;
    assert!(close.has_due(now));
    close.due(now, |pid, signal| signals.push((pid, signal)));
    assert_eq!(signals, vec![(42, Action::Kill(42))]);

    assert_eq!(close.request(8, 43, now), Action::Request);
    assert_eq!(close.request(8, 43, now), Action::Term(43));
    close.retain(|id, _owner| id != 8);
    close.due(now + 2_000_000_000, |pid, signal| signals.push((pid, signal)));
    assert_eq!(signals, vec![(42, Action::Kill(42))]);
}

#[test]
fn close_never_signals_the_window_manager_itself() {
    let own = 77;
    let mut close = Close::for_process(own);
    let mut signals = Vec::new();
    let now = 10_000_000_000;
    assert_eq!(close.request(9, own, now), Action::Request);
    assert_eq!(close.request(9, own, now), Action::None);
    assert_eq!(close.timeout_ms(now, 3_000), 3_000);
    close.due(now + 10_000_000_000, |pid, signal| signals.push((pid, signal)));
    assert!(signals.is_empty());
    assert_eq!(close.request(10, 42, now), Action::Request);
    assert_eq!(close.request(10, 42, now), Action::Term(42));
}
