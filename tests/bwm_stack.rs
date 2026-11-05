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
