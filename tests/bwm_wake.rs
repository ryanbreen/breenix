#[path = "../userspace/programs/src/bwm/wake.rs"]
mod wake;
use wake::{timeout, Deadlines};

#[test]
fn each_deadline_and_priority() {
    let base = Deadlines::default();
    assert_eq!(timeout(false, false, 100, 100, base), -1);
    assert_eq!(timeout(true, false, 100, 100, base), 0);
    assert_eq!(timeout(false, true, 100, 100, base), 100);
    assert_eq!(timeout(true, true, 100, 100, base), 100, "lost display backs off even with a dirty frame");
    for deadlines in [
        Deadlines { closing: Some(2_000_100), ..base },
        Deadlines { clipboard: Some(2_000_100), ..base },
        Deadlines { repeat: Some(2_000_100), ..base },
        Deadlines { launch: Some(2_000_100), ..base },
        Deadlines { check_after_ms: Some(3), ..base },
        Deadlines { wall: Some(2_000_100), ..base },
    ] {
        let expected = if deadlines.check_after_ms.is_some() { 3 } else { 2 };
        assert_eq!(timeout(false, false, 100, 100, deadlines), expected);
    }
    assert_eq!(timeout(false, false, 100, 100, Deadlines { closing: Some(100), launch: Some(9_000_000), ..base }), 0);
    assert_eq!(timeout(false, false, 100, 100, Deadlines { repeat: Some(100_101), ..base }), 1);
}

#[test]
fn idle_five_seconds_wakes_only_for_wall_seconds() {
    let mut now = 0u64;
    let mut wakes = 0;
    let mut composites = 0;
    while now < 5_000_000_000 {
        let wall = (now / 1_000_000_000 + 1) * 1_000_000_000;
        let ms = timeout(false, false, now, now, Deadlines { wall: Some(wall), ..Deadlines::default() });
        assert_eq!(ms, 1000);
        now += ms as u64 * 1_000_000;
        wakes += 1;
        composites += 1;
    }
    assert_eq!((wakes, composites), (5, 5));
    assert_eq!(5000 / 10, 500);
}

#[test]
fn independent_clock_domains_choose_the_earliest_work() {
    let deadlines = Deadlines {
        presentation: None,
        closing: Some(9_000_000_000), clipboard: Some(8_000_000_000),
        repeat: Some(7_000_000_000), launch: Some(6_000_000_000),
        check_after_ms: Some(100_125), wall: Some(5_000_000_000),
    };
    assert_eq!(timeout(false, false, 4_000_000_000, 100_000_000_000, deadlines), 125);
    assert_eq!(timeout(false, false, 4_000_000_000, 100_000_000_000,
        Deadlines { check_after_ms: None, ..deadlines }), 1000);
    assert_eq!(timeout(false, false, 0, 0,
        Deadlines { wall: Some(u64::MAX), ..Deadlines::default() }), i32::MAX);
}

#[test]
fn response_hold_uses_the_monotonic_clock_and_does_not_hide_other_deadlines() {
    let pending = Deadlines { presentation: Some(1_001_200_000), ..Deadlines::default() };
    assert_eq!(timeout(false, false, 80_000_000_000, 1_000_000_000, pending), 2);
    assert_eq!(timeout(false, false, 80_000_000_000, 1_002_000_000, pending), 0);
    assert_eq!(timeout(false, false, 80_000_000_000, 1_000_000_000,
        Deadlines { closing: Some(80_000_100_000), ..pending }), 1);
}
