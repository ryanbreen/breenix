#[path = "../userspace/programs/src/bwm/timing.rs"]
mod timing;
use libbreenix::input::{key, kind, Event};

fn event(serial: u64, value: i32) -> Event {
    Event { serial, time_ns: 0, device: 1, kind: kind::KEY, code: key::A, value, flags: 0 }
}

#[test]
fn repeat_boundaries_and_cancellation() {
    let mut timing = timing::Timing::new();
    assert!(timing.key(event(10, 1), Some(3), false, 0));
    assert!(!timing.key(event(11, 2), Some(3), false, 100));
    assert!(timing.due(499_999_999, Some(3)).is_empty());
    let first = timing.due(500_000_000, Some(3));
    assert_eq!(first.iter().map(|r| r.time).collect::<Vec<_>>(), [500_000_000]);
    assert_eq!((first[0].source.serial, first[0].derived), (10, 2));
    assert_eq!(timing.due(600_000_000, Some(3)).iter().map(|r| r.time).collect::<Vec<_>>(), [550_000_000, 600_000_000]);
    timing.key(event(12, 0), Some(3), false, 600_000_001);
    assert!(timing.due(1_000_000_000, Some(3)).is_empty());
    timing.key(event(13, 1), Some(3), false, 0);
    assert!(timing.due(500_000_000, Some(4)).is_empty());
    assert!(timing.due(1_000_000_000, Some(3)).is_empty());
}

#[test]
fn press_timing_and_distance_are_inclusive() {
    let mut clicks = timing::Clicks::new();
    assert_eq!(clicks.press(1, 272, 8, (10, 10), 1), 1);
    assert_eq!(clicks.press(1, 272, 8, (14, 6), 400_000_001), 2);
    assert_eq!(clicks.press(1, 272, 8, (14, 6), 400_000_002), 1);
    assert_eq!(clicks.press(1, 272, 8, (19, 6), 400_000_003), 1);
    assert_eq!(clicks.press(1, 272, 8, (19, 6), 800_000_004), 1);
    assert!(timing::within(1, 400_000_001));
    assert!(!timing::within(1, 400_000_002));
    assert!(!timing::within(2, 1));
}
