#![allow(dead_code)]
type Fd = u32;
mod io { pub fn write(_: super::Fd, _: &[u8]) -> Result<usize, ()> { Err(()) } }
mod paint {
    #[derive(Default)] pub struct Paint;
    impl Paint { pub fn invalidate(&mut self) {} }
}
#[path = "SELECTION_PATH"] mod selection;
// TERMINAL_CODE

fn select(term: &mut TermEmu) {
    let geometry = selection::Geometry { x: 0, y: 0, w: 1, h: 1 };
    term.selection.press(0, 0, geometry, term.vt.size());
    term.selection.release(2, 0, geometry, term.vt.size());
    assert!(term.selection.bounds().is_some());
}

#[test]
fn replacement_clears_selection() {
    for sequence in [&b"\x1b[2J"[..], &b"\x1b[3J"[..], &b"\x1b[H\x1b[J"[..], &b"\x1b[?1049h"[..]] {
        let mut term = TermEmu::new(10, 4);
        select(&mut term);
        term.feed(sequence, 0);
        assert!(term.selection.bounds().is_none(), "selection survived {sequence:?}");
    }
}

#[test]
fn leaving_alternate_screen_clears_selection() {
    let mut term = TermEmu::new(10, 4);
    term.feed(b"\x1b[?1049h", 0);
    select(&mut term);
    term.feed(b"\x1b[?1049l", 0);
    assert!(term.selection.bounds().is_none());
}

#[test]
fn ordinary_updates_keep_selection() {
    for sequence in [&b"text"[..], &b"\x1b[K"[..], &b"\x1b[2;1H\x1b[J"[..]] {
        let mut term = TermEmu::new(10, 4);
        select(&mut term);
        term.feed(sequence, 0);
        assert!(term.selection.bounds().is_some(), "selection lost for {sequence:?}");
    }
}
