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
    assert!(!b.observe(3, &Observation::new(vec![window(2, 42)], vec![])).unwrap());
    assert!(b.observe(4, &Observation::new(vec![window(2, 42)], vec![])).unwrap());
}

#[test]
fn drained_without_commit_times_out_with_window_and_serial() {
    let mut b = Barrier::new(0, vec![(7, 42)], vec![]);
    let o = Observation::new(vec![window(1, 41)], vec![]);
    assert!(!b.observe(1, &o).unwrap());
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
