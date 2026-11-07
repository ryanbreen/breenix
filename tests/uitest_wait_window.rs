#[path = "../libs/libuitest/src/window_wait.rs"]
pub mod window_wait;
#[path = "../libs/libuitest/src/supervisor.rs"]
pub mod supervisor;
use window_wait::{WaitWindow, WindowReady};

#[test]
fn recorded_sequences_need_both_the_exact_registry_title_and_completed_commit() {
    let wait = WaitWindow::new("editor", 300_000_000);
    let rows: &[(u64, &[u8])] = &[(41, b"editor"), (42, b"editor settings")];
    // A submitted composite, registration alone and a different window's
    // successful composite cannot satisfy the wait.
    assert_eq!(wait.observe(&[], "", 0).unwrap(), None);
    assert_eq!(wait.observe(rows, "", 10_000_000).unwrap(), None);
    assert_eq!(wait.observe(rows, "9 20000000 42 6 1\n", 20_000_000).unwrap(), None);
    assert_eq!(wait.observe(&[], "10 30000000 41 7 1\n", 30_000_000).unwrap(), None);
    assert_eq!(wait.observe(rows, "10 30000000 41 7 1\n11 40000000 41 8 2\n", 40_000_000).unwrap(),
        Some(WindowReady { id: 41, composite_serial: 10, presented_ns: 30_000_000, commit_id: 7, pixel_generation: 1 }));
    // Calling wait after the first composite still sees the completed receipt.
    assert!(wait.observe(rows, "10 30000000 41 7 1\n", 50_000_000).unwrap().is_some());
}

#[test]
fn deadline_is_exact_and_timeout_names_the_requested_title() {
    let wait = WaitWindow::new("never appears", 300_000_000);
    assert_eq!(wait.observe(&[], "", 299_999_999).unwrap(), None);
    let error = wait.observe(&[], "", 300_000_000).unwrap_err();
    assert!(error.contains("wait-window-timeout") && error.contains("never appears"), "{error}");
    let rows: &[(u64, &[u8])] = &[(41, b"never appears")];
    assert!(wait.observe(rows, "10 300000000 41 7 1\n", 300_000_000).unwrap().is_some());
    assert!(wait.observe(rows, "10 300000001 41 7 1\n", 300_000_001).is_err());
    assert!(wait.observe(rows, "10 299999999 41 7 1\n", 300_000_001).is_err());
}

#[test]
fn incomplete_corrupt_and_ambiguous_observations_do_not_report_success() {
    let wait = WaitWindow::new("editor", 300_000_000);
    let rows: &[(u64, &[u8])] = &[(41, b"editor")];
    for records in ["0 100 41 7 1\n", "10 0 41 7 1\n", "10 100 41 0 1\n", "10 100 41 7 0\n"] {
        assert_eq!(wait.observe(rows, records, 100).unwrap(), None);
    }
    for records in ["10 100 41 7\n", "10 100 41 7 1 extra\n", "10 nope 41 7 1\n"] {
        assert!(wait.observe(rows, records, 100).is_err(), "{records}");
    }
    assert!(wait.observe(&[(41, b"editor"), (42, b"editor")], "10 100 41 7 1\n", 100)
        .unwrap_err().contains("ambiguous"));
}

#[test]
fn compositor_start_request_round_trips_without_changing_app_launch() {
    use supervisor::{Message, encode, decode};
    for kiosk in [false, true] {
        let message = Message::StartCompositor { kiosk };
        let frame = encode(&message).unwrap();
        assert_eq!(decode(&frame[4..]).unwrap(), message);
    }
    assert!(decode(&[14, 2]).is_err());
    assert!(decode(&[14]).is_err());
}
