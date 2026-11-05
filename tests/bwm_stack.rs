#[path = "../userspace/programs/src/bwm/stack.rs"]
mod stack;
use stack::Stack;

#[test]
fn raise_uses_ids_and_keeps_chromeless_above_frames() {
    let mut s = Stack::new();
    s.add(1, false); s.add(2, false); s.add(3, true);
    assert_eq!(s.raise(1), (Some(3), Some(1)));
    assert_eq!(s.order(), &[2, 1, 3]);
    s.raise(2);
    assert_eq!(s.order(), &[1, 2, 3]);
    assert_eq!(s.focus(), Some(2));
}

#[test]
fn minimize_restore_and_all_minimized() {
    let mut s = Stack::new();
    s.add(1, false); s.add(2, false);
    assert_eq!(s.minimize(2), (Some(2), Some(1)));
    assert!(s.minimized(2));
    assert_eq!(s.minimize(1), (Some(1), None));
    assert_eq!(s.focus(), None);
    assert_eq!(s.restore(2), (None, Some(2)));
    assert!(!s.minimized(2));
}

#[test]
fn remove_keeps_survivor_order_and_focus_is_unique() {
    let mut s = Stack::new();
    s.add(1, false); s.add(2, false); s.add(3, true);
    s.raise(1);
    assert_eq!(s.remove(1), (Some(1), Some(3)));
    assert_eq!(s.order(), &[2, 3]);
    assert_eq!(s.order().iter().filter(|&&id| s.focus() == Some(id) && !s.minimized(id)).count(), 1);
    s.minimize(3); s.minimize(2);
    assert_eq!(s.order().iter().filter(|&&id| s.focus() == Some(id) && !s.minimized(id)).count(), 0);
}

#[test]
fn published_state_follows_order_and_focus_through_minimize_and_exit() {
    let mut s = Stack::new();
    s.add(10, false);
    s.add(20, true);
    s.add(30, false);
    assert_eq!(s.order(), &[10, 30, 20]);
    assert_eq!(s.state(30), Some((1, false, true)));
    assert_eq!(s.state(20), Some((2, false, false)));
    assert_eq!(s.minimize(10), (Some(30), Some(30)));
    assert_eq!(s.state(10), Some((0, true, false)));
    assert_eq!(s.minimize(30), (Some(30), Some(20)));
    assert_eq!(s.state(20), Some((2, false, true)));
    assert_eq!(s.minimize(20), (Some(20), None));
    for &id in s.order() {
        let (_, minimized, focused) = s.state(id).unwrap();
        assert!(minimized);
        assert!(!focused);
    }
    assert_eq!(s.restore(10), (None, Some(10)));
    assert_eq!(s.order(), &[30, 10, 20]);
    assert_eq!(s.remove(30), (Some(10), Some(10)));
    assert_eq!(s.order(), &[10, 20]);
    assert_eq!(s.state(10), Some((0, false, true)));
    assert_eq!(s.state(20), Some((1, true, false)));
    assert_eq!(s.state(30), None);
    assert_eq!(s.raise(99), (Some(10), Some(10)));
    assert_eq!(s.order().iter().filter(|&&id| s.state(id).unwrap().2).count(), 1);
}
