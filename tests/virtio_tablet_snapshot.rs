#[path = "../kernel/src/drivers/virtio/tablet_snapshot.rs"]
mod tablet_snapshot;
use tablet_snapshot::TabletSnapshot;

#[test]
fn release_position_precedes_post_release_motion() {
    let mut pointer = TabletSnapshot::new();
    pointer.report(10, 20, 1);
    assert_eq!(pointer.take(), (10, 20, 1));
    pointer.report(50, 60, 1);
    pointer.report(50, 60, 0);
    pointer.report(900, 800, 0);
    assert_eq!(pointer.current(), (900, 800, 0));
    assert_eq!(pointer.take(), (50, 60, 0));
    assert_eq!(pointer.take(), (900, 800, 0));
}

#[test]
fn held_and_unheld_motion_without_release_remain_current() {
    let mut pointer = TabletSnapshot::new();
    pointer.report(10, 20, 0);
    pointer.report(30, 40, 0);
    assert_eq!(pointer.take(), (30, 40, 0));
    pointer.report(30, 40, 1);
    pointer.report(50, 60, 1);
    assert_eq!(pointer.take(), (50, 60, 1));
}

#[test]
fn release_at_zero_is_retained_until_consumed() {
    let mut pointer = TabletSnapshot::new();
    pointer.report(10, 20, 1);
    pointer.take();
    pointer.report(0, 0, 0);
    pointer.report(100, 200, 0);
    assert_eq!(pointer.take(), (0, 0, 0));
    assert_eq!(pointer.take(), (100, 200, 0));
}

#[test]
fn an_unobserved_press_does_not_start_a_gesture_at_release() {
    let mut pointer = TabletSnapshot::new();
    pointer.report(10, 20, 1);
    pointer.report(50, 60, 0);
    pointer.report(900, 800, 0);
    assert_eq!(pointer.take(), (50, 60, 0));
    assert_eq!(pointer.take(), (900, 800, 0));
}

#[test]
fn active_reader_latches_a_fast_click_before_its_release() {
    let mut pointer = TabletSnapshot::new();
    assert_eq!(pointer.take_for(1), (0, 0, 0));
    pointer.report(10, 20, 1);
    pointer.report(50, 60, 0);
    pointer.report(900, 800, 0);
    assert_eq!(pointer.take_for(1), (10, 20, 1));
    assert_eq!(pointer.take_for(1), (50, 60, 0));
    assert_eq!(pointer.take_for(1), (900, 800, 0));
}

#[test]
fn another_reader_and_move_control_start_at_current_position() {
    let mut pointer = TabletSnapshot::new();
    pointer.report(10, 20, 1);
    pointer.take_for(1);
    pointer.report(50, 60, 0);
    pointer.report(900, 800, 0);
    assert_eq!(pointer.take_for(2), (900, 800, 0));
    pointer.report(10, 20, 1);
    pointer.take_for(2);
    pointer.report(50, 60, 0);
    pointer.move_to(100, 200);
    assert_eq!(pointer.take_for(2), (100, 200, 0));
    pointer.report(300, 400, 1);
    pointer.move_to(500, 600);
    assert_eq!(pointer.take_for(2), (500, 600, 1));
}

#[test]
fn unrelated_reader_does_not_consume_the_active_readers_release() {
    let mut pointer = TabletSnapshot::new();
    pointer.report(10, 20, 1);
    assert_eq!(pointer.take_for(1), (10, 20, 1));
    pointer.report(50, 60, 0);
    pointer.report(900, 800, 0);
    assert_eq!(pointer.take_for(2), (900, 800, 0));
    assert_eq!(pointer.take_for(1), (50, 60, 0));
    assert_eq!(pointer.take_for(2), (900, 800, 0));
}
