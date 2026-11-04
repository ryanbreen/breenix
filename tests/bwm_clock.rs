#[path = "../userspace/programs/src/bwm/clock.rs"]
mod clock;

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
    std::env::remove_var("BREENIX_UI_CLOCK");
    fs::remove_file(path).unwrap();
}
