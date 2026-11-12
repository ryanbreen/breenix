#[path = "../userspace/programs/src/bterm/selection.rs"]
mod selection;
use selection::{Geometry, Selection};

fn fixture() -> (libvt::TermEmu, Geometry) {
    let mut vt = libvt::TermEmu::new(6, 3);
    vt.feed(b"abc de\x1b[2;1Hxy    \x1b[3;1Hlast");
    (vt, Geometry { x: 10, y: 20, w: 8, h: 10 })
}

#[test]
fn drag_selects_rows_in_either_direction() {
    let (vt, g) = fixture();
    let mut s = Selection::default();
    s.press(18, 20, g, vt.size()); s.release(42, 20, g, vt.size());
    assert_eq!(s.text(&vt).as_deref(), Some("bc d"));
    assert!(s.contains(1, 0));
    assert!(!s.contains(0, 0));
    s.press(18, 20, g, vt.size()); s.release(26, 40, g, vt.size());
    assert_eq!(s.text(&vt).as_deref(), Some("bc de\nxy\nlas"));
    s.press(26, 40, g, vt.size()); s.release(18, 20, g, vt.size());
    assert_eq!(s.text(&vt).as_deref(), Some("bc de\nxy\nlas"));
}

#[test]
fn drag_clamps_and_click_clears() {
    let (vt, g) = fixture();
    let mut s = Selection::default();
    s.press(-100, -100, g, vt.size()); s.release(1000, 1000, g, vt.size());
    assert_eq!(s.text(&vt).as_deref(), Some("abc de\nxy\nlast"));
    s.press(10, 20, g, vt.size()); s.release(10, 20, g, vt.size());
    assert_eq!(s.text(&vt), None);
}

#[test]
fn tab_bar_release_preserves_selection_and_grid_drag_can_end_there() {
    let (vt, g) = fixture();
    let mut s = Selection::default();
    s.press(18, 20, g, vt.size());
    s.release(42, 20, g, vt.size());
    let selected = s.text(&vt);
    assert_eq!(selected.as_deref(), Some("bc d"));
    s.release(42, g.y - 1, g, vt.size());
    assert_eq!(s.text(&vt), selected);

    s.press(26, 30, g, vt.size());
    s.release(18, g.y - 1, g, vt.size());
    assert_eq!(s.text(&vt).as_deref(), Some("bc de\nxy"));

    s.press(10, 20, g, vt.size());
    s.release(10, 20, g, vt.size());
    assert_eq!(s.text(&vt), None);
}

#[test]
fn vt_reports_scrolling_even_when_history_is_full() {
    let mut vt = libvt::TermEmu::new(2, 2);
    for _ in 0..1001 { vt.feed(b"x\r\n"); }
    let before = vt.scroll_generation();
    vt.feed(b"x\r\n");
    assert_ne!(vt.scroll_generation(), before);
    let before = vt.scroll_generation();
    vt.feed(b"a");
    assert_eq!(vt.scroll_generation(), before);
}
