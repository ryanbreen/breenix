#[path = "../userspace/programs/src/bwm/timing.rs"]
mod timing;
use libbreenix::input::{key, kind, syn, Event};

fn event(serial: u64, value: i32) -> Event {
    Event { serial, time_ns: 0, device: 1, kind: kind::KEY, code: key::A, value, flags: 0 }
}

#[test]
fn repeat_boundaries_and_cancellation() {
    let mut timing = timing::Timing::new();
    assert!(timing.key(event(10, 1), Some(3), false, 0));
    assert!(!timing.key(event(11, 2), Some(3), false, 100));
    assert_eq!(timing.deadline(), Some(500_000_000));
    assert!(timing.due(499_999_999, Some(3)).is_empty());
    let first = timing.due(500_000_000, Some(3));
    assert_eq!(first.iter().map(|r| r.time).collect::<Vec<_>>(), [500_000_000]);
    assert_eq!((first[0].source.serial, first[0].derived, first[0].target), (10, 2, 3));
    assert_eq!(timing.due(600_000_000, Some(3)).iter().map(|r| r.time).collect::<Vec<_>>(), [550_000_000, 600_000_000]);
    timing.key(event(12, 0), Some(3), false, 600_000_001);
    assert!(timing.due(1_000_000_000, Some(3)).is_empty());
    timing.key(event(13, 1), Some(3), false, 0);
    assert!(timing.due(500_000_000, Some(4)).is_empty());
    assert!(timing.due(1_000_000_000, Some(3)).is_empty());
    assert_eq!(timing.deadline(), None);
}

#[test]
fn stream_loss_cancels_repeats_even_when_the_marker_names_another_device() {
    let mut timing = timing::Timing::new();
    timing.key(event(10, 1), Some(3), false, 0);
    timing.key(Event { device: 2, ..event(11, 1) }, Some(3), false, 0);
    assert_eq!(timing.due(500_000_000, Some(3)).len(), 2);
    // Reader overflow can evict batches from several devices but publishes
    // one marker with the incoming device's id.
    let dropped = Event { kind: kind::SYN, code: syn::DROPPED, device: 99,
        ..event(12, 0) };
    timing.key(dropped, Some(3), false, 500_000_001);
    assert_eq!(timing.deadline(), None);
    assert!(timing.due(1_000_000_000, Some(3)).is_empty());
    timing.key(event(13, 1), Some(3), false, 1_000_000_000);
    assert_eq!(timing.due(1_500_000_000, Some(3)).len(), 1);
}

#[test]
fn press_timing_and_distance_are_inclusive() {
    let mut clicks = timing::Clicks::new();
    assert_eq!(clicks.press(1, 272, 8, (10, 10), 1), 1);
    assert_eq!(clicks.press(1, 272, 8, (14, 6), 400_000_001), 2);
    assert_eq!(clicks.release(1, 272), 2);
    assert_eq!(clicks.press(1, 272, 8, (14, 6), 400_000_002), 1);
    assert_eq!(clicks.press(1, 272, 8, (19, 6), 400_000_003), 1);
    assert_eq!(clicks.press(1, 272, 8, (19, 6), 800_000_004), 1);
    assert!(timing::within(1, 400_000_001));
    assert!(!timing::within(1, 400_000_002));
    assert!(!timing::within(2, 1));
}

#[test]
fn another_devices_earlier_timestamp_preserves_a_pending_double_click() {
    let mut clicks = timing::Clicks::new();
    assert_eq!(clicks.press(1, 272, 8, (10, 10), 500_000_000), 1);
    assert_eq!(clicks.release(1, 272), 1);
    // Injector timestamps are per device; this event does not age device 1.
    assert_eq!(clicks.press(2, 272, 9, (10, 10), 1), 1);
    assert_eq!(clicks.release(2, 272), 1);
    assert_eq!(clicks.press(1, 272, 8, (10, 10), 600_000_000), 2);
    assert_eq!(clicks.release(1, 272), 2);
}

#[test]
fn another_devices_later_timestamp_preserves_a_pending_double_click() {
    let mut clicks = timing::Clicks::new();
    assert_eq!(clicks.press(1, 272, 8, (10, 10), 1), 1);
    assert_eq!(clicks.release(1, 272), 1);
    assert_eq!(clicks.press(2, 272, 9, (10, 10), 5_000_000_000), 1);
    assert_eq!(clicks.release(2, 272), 1);
    assert_eq!(clicks.press(1, 272, 8, (10, 10), 100_000_000), 2);
    assert_eq!(clicks.release(1, 272), 2);
}
