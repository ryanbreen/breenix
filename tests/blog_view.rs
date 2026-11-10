#[path = "../userspace/programs/src/blog/view.rs"]
mod view;
use view::{Command, View};

fn lines() -> Vec<&'static [u8]> { vec![b"zero", b"hit one", b"two", b"hit three", b"four", b"hit five"] }
#[test]
fn line_moves_and_clamps_at_both_ends() {
    let mut v = View::new(); let l = lines();
    v.apply(Command::Top, &l, 2); v.apply(Command::Up, &l, 2); assert_eq!(v.offset, 0);
    v.apply(Command::Down, &l, 2); assert_eq!(v.offset, 1);
    v.apply(Command::Bottom, &l, 2); v.apply(Command::Down, &l, 2); assert_eq!(v.offset, 4);
    v.apply(Command::Up, &l, 2); assert_eq!(v.offset, 3);
}
#[test]
fn pages_and_wheel_move_and_clamp() {
    let mut v = View::new(); let l = lines(); v.apply(Command::Top, &l, 2);
    v.apply(Command::PageDown, &l, 2); assert_eq!(v.offset, 2);
    v.apply(Command::PageUp, &l, 2); assert_eq!(v.offset, 0);
    v.apply(Command::Wheel(-1), &l, 2); assert_eq!(v.offset, 3);
    v.apply(Command::Wheel(1), &l, 2); assert_eq!(v.offset, 0);
}
#[test]
fn top_bottom_and_follow_toggle() {
    let mut v = View::new(); let l = lines(); v.apply(Command::Top, &l, 2); assert!(!v.follow);
    v.apply(Command::Bottom, &l, 2); assert_eq!(v.offset, 4); assert!(v.follow);
    v.apply(Command::ToggleFollow, &l, 2); assert!(!v.follow);
    v.apply(Command::ToggleFollow, &l, 2); assert!(v.follow); assert_eq!(v.offset, 4);
}
#[test]
fn search_finds_from_top_and_wraps_both_directions() {
    let mut v = View::new(); let l = lines(); v.apply(Command::Top, &l, 2);
    v.apply(Command::Down, &l, 2); v.apply(Command::Down, &l, 2);
    v.apply(Command::Search, &l, 2); for c in b"hit" { v.apply(Command::Character(*c), &l, 2); }
    v.apply(Command::Enter, &l, 2); assert_eq!(v.matched, Some(3)); assert_eq!(v.matches, 3); assert_eq!(v.offset, 2);
    v.apply(Command::Next, &l, 2); assert_eq!(v.matched, Some(5)); assert_eq!(v.offset, 4);
    v.apply(Command::Next, &l, 2); assert_eq!(v.matched, Some(1)); assert_eq!(v.offset, 1);
    v.apply(Command::Previous, &l, 2); assert_eq!(v.matched, Some(5)); assert_eq!(v.offset, 4);
}
#[test]
fn escape_cancels_query_edit() {
    let mut v = View::new(); let l = lines(); v.apply(Command::Search, &l, 2);
    v.apply(Command::Character(b'x'), &l, 2); v.apply(Command::Escape, &l, 2);
    assert!(!v.editing); assert!(v.query.is_empty()); assert_eq!(v.matched, None);
}
