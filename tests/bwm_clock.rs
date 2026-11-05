#[path = "../userspace/programs/src/bwm/clock.rs"]
mod clock;
#[path = "../userspace/programs/src/bwm/timing.rs"]
mod timing;

use clock::Clock;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::FileExt;

#[test]
fn stepped_clock_acknowledges_without_clock_composites_or_fps() {
    let path = std::env::temp_dir().join(format!("bwm-clock-{}", std::process::id()));
    let page = OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
    let mut bytes = [0; 64];
    bytes[..8].copy_from_slice(b"BGCLK001");
    bytes[8..16].copy_from_slice(&1u64.to_le_bytes());
    bytes[16..24].copy_from_slice(&250_000_000u64.to_le_bytes());
    bytes[24..32].copy_from_slice(&1_767_225_600_000_000_000u64.to_le_bytes());
    page.write_all_at(&bytes, 0).unwrap();
    std::env::set_var("BREENIX_UI_CLOCK", &path);

    let reads = libbreenix::ui_clock::real_clock_reads();
    let mut clock = Clock::new(true);
    assert_eq!(clock.wall_sec(), 1_767_225_600);
    assert_eq!(clock.update_clock(), None);
    let ack = clock.acknowledgment(9);
    assert_eq!((ack.claim_epoch, ack.epoch, ack.presented, ack.flags), (9, 1, 0, 1));
    assert_eq!(ack.next_deadline_ns, 0, "a test clock schedules no timed repaint");

    clock.presented(42);
    clock.begin_epoch();
    assert_eq!(clock.acknowledgment(9).presented, 42);
    page.write_all_at(&2_250_000_000u64.to_le_bytes(), 16).unwrap();
    page.write_all_at(&2u64.to_le_bytes(), 8).unwrap();
    clock.begin_epoch();
    assert_eq!(clock.wall_sec(), 1_767_225_602);
    assert_eq!(clock.update_clock(), None, "stepping wall time must not composite chrome");
    let ack = clock.acknowledgment(9);
    assert_eq!((ack.epoch, ack.presented, ack.next_deadline_ns), (2, 0, 0));
    clock.presented(43);
    assert_eq!(clock.acknowledgment(9).presented, 43);
    assert_eq!(libbreenix::ui_clock::real_clock_reads(), reads);

    // The production loop has no FPS emitter to wake an otherwise idle desktop.
    assert!(!include_str!("../userspace/programs/src/bwm.rs").contains("[bwm-fps]"));
    let mut real = Clock::new(false);
    assert_eq!(real.update_clock(), None);
    let ack = real.acknowledgment(9);
    // Lane 2A schedules the next wall-second boundary in monotonic time.
    assert_eq!(ack.next_deadline_ns, 3_000_000_000);
    assert_eq!((ack.epoch, ack.flags), (2, 0));
    page.write_all_at(&3_000_000_000u64.to_le_bytes(), 16).unwrap();
    page.write_all_at(&3u64.to_le_bytes(), 8).unwrap();
    real.begin_epoch();
    assert_eq!(real.update_clock(), Some(1_767_225_603));
    assert_eq!(real.update_clock(), None);
    assert_eq!(real.acknowledgment(9).next_deadline_ns, 4_000_000_000);

    // The repeat scheduler consumes the mapped clock, including a frozen page.
    use libbreenix::{input::{Event, key, kind}, ui_clock};
    let source = Event { serial: 42, device: 7, kind: kind::KEY, code: key::A,
        value: 1, ..Event::default() };
    let mut repeats = timing::Timing::new();
    assert!(repeats.key(source, Some(9), false, ui_clock::now_ns()));
    assert_eq!(repeats.deadline(), Some(3_500_000_000));
    for _ in 0..100 {
        assert!(repeats.due(ui_clock::now_ns(), Some(9)).is_empty());
    }
    page.write_all_at(&3_499_999_999u64.to_le_bytes(), 16).unwrap();
    assert!(repeats.due(ui_clock::now_ns(), Some(9)).is_empty());
    page.write_all_at(&3_500_000_000u64.to_le_bytes(), 16).unwrap();
    let first = repeats.due(ui_clock::now_ns(), Some(9));
    assert_eq!(first.len(), 1);
    assert_eq!((first[0].source.serial, first[0].target, first[0].time, first[0].derived),
        (42, 9, 3_500_000_000, 2));
    assert!(!repeats.key(Event { value: 2, ..source }, Some(9), false, ui_clock::now_ns()));
    page.write_all_at(&3_600_000_000u64.to_le_bytes(), 16).unwrap();
    assert_eq!(repeats.due(ui_clock::now_ns(), Some(9)).iter().map(|r| r.time).collect::<Vec<_>>(),
        [3_550_000_000, 3_600_000_000]);
    repeats.key(Event { value: 0, ..source }, Some(9), false, ui_clock::now_ns());
    page.write_all_at(&4_000_000_000u64.to_le_bytes(), 16).unwrap();
    assert!(repeats.due(ui_clock::now_ns(), Some(9)).is_empty());
    let mut clicks = timing::Clicks::new();
    assert_eq!(clicks.press(7, 272, 9, (2, 3), ui_clock::now_ns()), 1);
    assert_eq!(clicks.release(7, 272), 1);
    page.write_all_at(&4_400_000_000u64.to_le_bytes(), 16).unwrap();
    assert_eq!(clicks.press(7, 272, 9, (2, 3), ui_clock::now_ns()), 2);
    assert!(timing::within(4_000_000_000, ui_clock::now_ns()));
    assert_eq!(ui_clock::real_clock_reads(), reads);
    std::env::remove_var("BREENIX_UI_CLOCK");
    fs::remove_file(path).unwrap();
}
