#[path = "../kernel/src/test_control/point.rs"]
mod point;
use point::Point;

#[test]
fn failed_preparation_preserves_the_one_shot_for_the_next_submission() {
    let p = Point::new();
    p.arm(17, 29);
    drop(p.reserve(17, |_| true).unwrap());
    assert_eq!(p.fired(), 0);
    assert_eq!(p.reserve(17, |_| true).unwrap().fire(), 29);
    assert_eq!(p.fired(), 1);
    assert!(p.reserve(17, |_| true).is_none());
}

#[test]
fn target_parameter_and_disarm_remain_effective() {
    let p = Point::new();
    p.arm(17, 29);
    assert!(p.reserve(18, |_| true).is_none());
    assert!(p.reserve(17, |n| n == 30).is_none());
    assert_eq!(p.fired(), 0);
    assert_eq!(p.reserve(17, |n| n == 29).unwrap().fire(), 29);
    p.arm(0, 31);
    assert_eq!(p.reserve(99, |_| true).unwrap().fire(), 31);
    p.arm(0, 0);
    p.disarm();
    assert!(p.reserve(0, |_| true).is_none());
    assert_eq!(p.fired(), 2);
}

#[test]
fn rollback_does_not_overwrite_a_new_arm_or_disarm() {
    let p = Point::new();
    p.arm(17, 29);
    let old = p.reserve(17, |_| true).unwrap();
    p.arm(18, 30);
    drop(old);
    assert!(p.reserve(17, |_| true).is_none());
    assert_eq!(p.reserve(18, |_| true).unwrap().fire(), 30);
    p.arm(17, 29);
    let old = p.reserve(17, |_| true).unwrap();
    p.disarm();
    drop(old);
    assert!(p.reserve(17, |_| true).is_none());
    assert_eq!(p.fired(), 1);
}

#[test]
fn queued_effect_counts_once_without_consuming_a_newer_arm() {
    let p = Point::new();
    p.arm(17, 29);
    let old = p.reserve(17, |_| true).unwrap();
    assert_eq!(p.fired(), 0);
    p.arm(18, 30);
    assert_eq!(old.fire(), 29);
    assert_eq!(p.fired(), 1);
    assert_eq!(p.reserve(18, |_| true).unwrap().fire(), 30);
    assert_eq!(p.fired(), 2);
}
