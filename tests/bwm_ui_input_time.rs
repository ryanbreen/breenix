#[path = "../userspace/programs/src/bwm/timing.rs"]
mod timing;

use libbreenix::input::{key, kind, Event};

#[test]
fn stepped_input_uses_event_time_for_repeat_and_clicks() {
    let path = std::env::temp_dir().join(format!("bwm-input-time-{}", std::process::id()));
    libbreenix::ui_clock::ClockPage::create(&path, 0).unwrap();
    std::env::set_var("BREENIX_UI_CLOCK", &path);
    let page = libbreenix::ui_clock::ClockPage::open(&path).unwrap();
    let event = Event { serial: 1, device: 7, kind: kind::KEY, code: key::A, value: 1, time_ns: 0, ..Event::default() };
    let mut timing = timing::Timing::new();
    // The compositor drains the press 100 ms after its injected timestamp.
    page.advance(100).unwrap();
    timing.key(event, Some(9), false, libbreenix::ui_clock::now_ns());
    assert!(timing.deadline().is_some());
    page.advance(500).unwrap();
    let repeats = timing.due(libbreenix::ui_clock::now_ns(), Some(9));
    assert!(repeats.iter().all(|r| r.source.serial == 1 && r.target == 9 && r.derived >= 2));
    assert_eq!(repeats.iter().map(|r| r.time).collect::<Vec<_>>(), vec![500_000_000, 550_000_000, 600_000_000]);
    let mut clicks = timing::Clicks::new();
    assert_eq!(clicks.press(7, key::BUTTON_LEFT, 9, (3, 4), 0), 1);
    assert_eq!(clicks.release(7, key::BUTTON_LEFT), 1);
    assert_eq!(clicks.press(7, key::BUTTON_LEFT, 9, (3, 4), 300_000_000), 2);
    assert_eq!(clicks.release(7, key::BUTTON_LEFT), 2);
    let mut consumed = std::collections::BTreeSet::new();
    timing::retire_unregistered(&mut clicks, &mut consumed, |_| true);
    std::fs::remove_file(path).unwrap();
}
