#[path = "../userspace/programs/src/bwm/wake.rs"]
mod wake;
use wake::{timeout, Deadlines};

#[test]
fn each_deadline_and_priority() {
    let base = Deadlines::default();
    assert_eq!(timeout(false, false, 100, 100, base), -1);
    assert_eq!(timeout(true, false, 100, 100, base), 0);
    assert_eq!(timeout(false, true, 100, 100, base), 100);
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
