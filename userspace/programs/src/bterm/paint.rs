//! Visual cell history for the window's retained terminal pixels.
use super::selection::Selection;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Visual {
    ch: char,
    fg: libvt::Color,
    bg: libvt::Color,
    bold: bool,
    underline: bool,
    inverse: bool,
    selected: bool,
    cursor: bool,
}

#[derive(Default)]
pub struct Paint {
    cells: Vec<Visual>,
    geometry: Option<(usize, usize, usize, usize, usize, usize, u32)>,
}
impl Paint {
    pub fn invalidate(&mut self) { self.geometry = None; }

    pub fn changes(&mut self, vt: &libvt::TermEmu, selection: &Selection,
        geometry: (usize, usize, usize, usize, usize, usize, u32)) -> Vec<bool> {
        let (_, _, cols, rows, _, _, _) = geometry;
        let reset = self.geometry != Some(geometry);
        if reset { self.cells.clear(); }
        self.geometry = Some(geometry);
        let cursor = (vt.view_offset() == 0 && vt.cursor_visible()).then(|| vt.cursor());
        let mut changed = Vec::with_capacity(cols * rows);
        for y in 0..rows {
            for x in 0..cols {
                let cell = vt.view_cell(x, y);
                let visual = Visual { ch: cell.ch, fg: cell.fg, bg: cell.bg,
                    bold: cell.bold, underline: cell.underline, inverse: cell.inverse,
                    selected: selection.contains(x, y), cursor: cursor == Some((x, y)) };
                let i = y * cols + x;
                changed.push(reset || self.cells[i] != visual);
                if reset { self.cells.push(visual); } else { self.cells[i] = visual; }
            }
        }
        changed
    }
}
