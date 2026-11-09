#[path = "../libs/libuitest/src/settle.rs"]
mod settle;
use settle::{Barrier, Observation, Window};

fn window(commit: u64, processed: u64) -> Window {
    Window { id: 7, commit, processed_serial: processed }
}

#[test]
fn processed_requires_a_commit_and_a_matching_watermark() {
    let mut b = Barrier::new(0, vec![(7, 42)], vec![]);
    assert!(!b.observe(1, &Observation::new(vec![window(0, 42)], vec![])).unwrap());
    assert!(!b.observe(2, &Observation::new(vec![window(1, 41)], vec![])).unwrap());
    assert!(!b.observe(3, &Observation::new(vec![window(1, 41)], vec![])).unwrap());
    assert!(!b.observe(4, &Observation::new(vec![window(2, 42)], vec![])).unwrap());
    assert!(b.observe(5, &Observation::new(vec![window(2, 42)], vec![])).unwrap());
}

#[test]
fn drained_without_commit_times_out_with_window_and_serial() {
    let mut b = Barrier::new(0, vec![(7, 42)], vec![]);
    let o = Observation::new(vec![window(1, 41)], vec![]);
    assert!(!b.observe(1, &o).unwrap());
    assert!(!b.observe(1_999_999_999, &o).unwrap());
    let error = b.observe(2_000_000_000, &o).unwrap_err();
    assert!(error.contains("window=7") && error.contains("serial=42"), "{error}");
}

#[test]
fn no_change_ack_and_predicate_wait_for_stability() {
    let mut b = Barrier::new(0, vec![(7, 42)], vec!["saved".into()]);
    let waiting = Observation::new(vec![window(3, 42)], vec![("saved".into(), false)]);
    assert!(!b.observe(1, &waiting).unwrap());
    let ready = Observation::new(vec![window(3, 42)], vec![("saved".into(), true)]);
    assert!(!b.observe(2, &ready).unwrap());
    assert!(b.observe(3, &ready).unwrap());
}

#[test]
fn timeout_names_the_predicate() {
    let mut b = Barrier::new(0, vec![], vec!["saved".into()]);
    let error = b.observe(2_000_000_000, &Observation::new(vec![], vec![("saved".into(), false)])).unwrap_err();
    assert!(error.contains("saved"), "{error}");
}

#[test]
fn derived_serials_are_ordered_by_their_source() {
    let derived = libbreenix::input::abi::derived_serial(41, 2).unwrap();
    assert!(derived > 42);
    let mut b = Barrier::new(0, vec![(7, 42)], vec![]);
    let stale = Observation::new(vec![window(1, derived)], vec![]);
    assert!(!b.observe(1, &stale).unwrap());
    assert!(!b.observe(2, &stale).unwrap());
    let current = libbreenix::input::abi::derived_serial(42, 1).unwrap();
    let ready = Observation::new(vec![window(2, current)], vec![]);
    assert!(!b.observe(3, &ready).unwrap());
    assert!(b.observe(4, &ready).unwrap());
}

#[test]
fn idle_requires_a_committed_window_and_changes_restart_stability() {
    let mut b = Barrier::new(0, vec![(7, 0)], vec![]);
    assert!(!b.observe(1, &Observation::new(vec![], vec![])).unwrap());
    assert!(!b.observe(2, &Observation::new(vec![window(1, 0)], vec![])).unwrap());
    assert!(!b.observe(3, &Observation::new(vec![window(2, 0)], vec![])).unwrap());
    assert!(b.observe(4, &Observation::new(vec![window(2, 0)], vec![])).unwrap());
}

#[test]
fn a_late_ready_snapshot_cannot_rescue_the_real_deadline() {
    let mut b = Barrier::new(100, vec![(7, 42)], vec![]);
    let ready = Observation::new(vec![window(2, 42)], vec![]);
    assert!(!b.observe(2_000_000_099, &ready).unwrap());
    let error = b.observe(2_000_000_100, &ready).unwrap_err();
    assert!(error.contains("settle-timeout") && error.contains("window=7"), "{error}");
}

#[test]
fn every_target_and_predicate_must_be_present_together() {
    let mut b = Barrier::new(0, vec![(7, 42), (8, 43)], vec!["saved".into()]);
    let missing = Observation::new(vec![window(1, 42)], vec![("saved".into(), true)]);
    assert!(!b.observe(1, &missing).unwrap());
    let error = b.observe(2_000_000_000, &missing).unwrap_err();
    assert!(error.contains("window=8") && error.contains("serial=43"), "{error}");
    let mut b = Barrier::new(0, vec![(7, 42)], vec!["saved".into()]);
    let ready = Observation::new(vec![window(1, 42)], vec![("saved".into(), true)]);
    assert!(!b.observe(1, &ready).unwrap());
    assert!(!b.observe(2, &Observation::new(vec![window(1, 42)], vec![])).unwrap());
    assert!(!b.observe(3, &ready).unwrap());
    assert!(b.observe(4, &ready).unwrap());
}
