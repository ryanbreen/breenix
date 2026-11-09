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


use libbreenix::window::abi::{self, CompositeEntry, CompositeRecordHeader, CompositorStatus, WinInfo};
use settle::Presentation;
fn scene() -> Presentation {
    let flags = abi::win_flags::REGISTERED | abi::win_flags::VISIBLE;
    Presentation {
        windows: vec![WinInfo { id: 7, flags, commit_id: 9, pixel_gen: 3, ..Default::default() },
            WinInfo { id: 8, flags, commit_id: 4, pixel_gen: 2, ..Default::default() }],
        header: CompositeRecordHeader { serial: 11, count: 2, ..Default::default() },
        entries: vec![CompositeEntry { id: 7, commit_id: 8, pixel_gen: 3, pixels: 400 },
            CompositeEntry { id: 8, commit_id: 4, pixel_gen: 2, pixels: 0 }],
        status: CompositorStatus { claim_epoch: 1, epoch: 5, presented: 11, ..Default::default() },
    }
}
#[test]
fn composite_requires_every_newest_generation_and_accepts_zero_pixel_occlusion() {
    let mut s = scene();
    assert_eq!(s.waiting(5, 0, &[]), None);
    s.entries.pop(); s.header.count = 1;
    assert!(s.waiting(5, 0, &[]).unwrap().contains("window=8"));
    s.entries.push(CompositeEntry { id: 8, pixel_gen: 1, ..Default::default() }); s.header.count = 2;
    assert!(s.waiting(5, 0, &[]).unwrap().contains("window=8"));
    s.entries[1].pixel_gen = 2;
    assert_eq!(s.waiting(5, 0, &[]), None);
    s.header.flags = abi::record_flags::FAILED;
    assert!(s.waiting(5, 0, &[]).is_some());
}
#[test]
fn metadata_only_commit_reuses_completed_pixels_and_minimized_still_needs_app_ack() {
    let mut s = scene();
    s.windows[0].commit_id += 1;
    assert_eq!(s.waiting(5, 0, &[]), None);
    s.windows[0].flags |= abi::win_flags::MINIMIZED;
    s.windows[0].pixel_gen = 99;
    assert_eq!(s.waiting(5, 0, &[]), None);
    let mut b = Barrier::new(0, vec![(7, 42)], vec![]);
    let stale = Observation::new(vec![window(9, 41)], vec![]);
    assert!(!b.observe_composite(1, &stale, &s, 5, 0, &[]).unwrap());
    assert!(!b.observe_composite(2, &stale, &s, 5, 0, &[]).unwrap());
    let ready = Observation::new(vec![window(10, 42)], vec![]);
    assert!(!b.observe_composite(3, &ready, &s, 5, 0, &[]).unwrap());
    assert!(b.observe_composite(4, &ready, &s, 5, 0, &[]).unwrap());
    s.windows[0].flags &= !abi::win_flags::MINIMIZED;
    assert!(s.waiting(5, 0, &[]).unwrap().contains("window=7"));
    assert_eq!(s.waiting(5, 0, &[7]), None);
}
#[test]
fn none_needed_and_wm_handled_require_honest_epoch_and_receipt() {
    let mut s = scene();
    s.status.presented = 0; s.status.flags = abi::compositor_ack_flags::NONE_NEEDED;
    s.status.next_deadline_ns = 6_000_000;
    assert_eq!(s.waiting(5, 0, &[]), None);
    assert!(s.waiting(6, 0, &[]).unwrap().contains("epoch"));
    assert!(s.waiting(5, 42, &[]).unwrap().contains("serial=42"));
    s.status.handled_serial = 42;
    assert_eq!(s.waiting(5, 42, &[]), None);
    s.status.presented = 11;
    assert!(s.waiting(5, 42, &[]).is_some());
    s.status.flags = 0; s.status.presented = 12;
    assert!(s.waiting(5, 42, &[]).is_some());
    s.header.serial = 12;
    assert_eq!(s.waiting(5, 42, &[]), None);
}
#[test]
fn a_pending_composite_resets_stability_and_names_its_window_at_deadline() {
    let mut b = Barrier::new(0, vec![(7, 42)], vec![]);
    let o = Observation::new(vec![window(9, 42)], vec![]);
    let mut s = scene();
    assert!(!b.observe_composite(1, &o, &s, 5, 0, &[]).unwrap());
    s.windows[1].pixel_gen = 3;
    assert!(!b.observe_composite(2, &o, &s, 5, 0, &[]).unwrap());
    assert!(!b.observe_composite(3, &o, &s, 5, 0, &[]).unwrap());
    let e = b.observe_composite(2_000_000_000, &o, &s, 5, 0, &[]).unwrap_err();
    assert!(e.contains("waiting=presentation") && e.contains("window=8"), "{e}");
    let mut b = Barrier::new(0, vec![(7, 42)], vec![]);
    s.windows[1].pixel_gen = 2;
    assert!(!b.observe_composite(1, &o, &s, 5, 0, &[]).unwrap());
    assert!(b.observe_composite(2, &o, &s, 5, 0, &[]).unwrap());
}
