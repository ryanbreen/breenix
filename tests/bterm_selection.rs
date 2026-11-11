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
