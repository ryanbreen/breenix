//! The B36 thin boot-stage row, retained across console display give-back.

use super::raster::Layout;
use core::ops::Range;

const PASS: [u8; 3] = [26, 112, 77];
const PENDING: [u8; 3] = [48, 67, 96];
const BACKGROUND: [u8; 3] = [12, 19, 31];

/// The ribbon as the console draws it: the display's scale and each stage's state.
#[derive(Clone, PartialEq, Eq)]
pub struct Ribbon {
    geometry: super::status::PanelGeometry,
    lit: alloc::vec::Vec<bool>,
}

impl Ribbon {
    /// The ribbon for `layout` with the stages reached so far, when the display is the stage
    /// layout at a whole scale and has stages. `drop_stage` shows the first reached stage
    /// unreached (test point 15, `ribbon-drop-stage`).
    pub fn new(layout: &Layout, mut lit: alloc::vec::Vec<bool>, drop_stage: bool) -> Option<Ribbon> {
        let geometry = layout.panel()?;
        if lit.is_empty() { return None; }
        if drop_stage {
            if let Some(first) = lit.iter_mut().find(|s| **s) { *first = false; }
        }
        Some(Ribbon { geometry, lit })
    }

    /// The pixel rows the band covers.
    pub fn rows(&self) -> Range<usize> {
        self.geometry.ribbon.y..self.geometry.ribbon.y + self.geometry.ribbon.height
    }

    /// Draw the band's pixel rows within `rows` into `frame`: each segment, then its gap.
    pub fn paint(&self, frame: &mut [u8], layout: &Layout, rows: Range<usize>) {
        let band = self.rows();
        let rows = rows.start.max(band.start)..rows.end.min(band.end);
        if rows.is_empty() {
            return;
        }
        let (s, m) = (self.geometry.stage_scale.max(1), self.lit.len());
        let r = self.geometry.ribbon;
        for (i, &lit) in self.lit.iter().enumerate() {
            let (a, b) = (r.x + (r.width / s * i / m) * s, r.x + (r.width / s * (i + 1) / m) * s);
            let colour = if lit { PASS } else { PENDING };
            fill(frame, layout, a..b.saturating_sub(2 * s).max(a), rows.clone(), colour);
            fill(frame, layout, b.saturating_sub(2 * s).max(a)..b, rows.clone(), BACKGROUND);
        }
    }

    /// The read-back oracle: the ribbon's colour at physical pixel `(x, y)`, when the pixel is
    /// in the band.
    pub fn colour(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        let (s, m) = (self.geometry.stage_scale.max(1), self.lit.len());
        let r = self.geometry.ribbon;
        if !r.contains(x, y) { return None; }
        let lx = (x - r.x) / s;
        let width = r.width / s;
        let start = |i: usize| width * i / m;
        let mut i = (lx * m / width).min(m - 1);
        while i + 1 < m && start(i + 1) <= lx { i += 1; }
        while start(i) > lx { i -= 1; }
        let end = start(i + 1);
        Some(if lx >= end.saturating_sub(2).max(start(i)) { BACKGROUND }
            else if self.lit[i] { PASS } else { PENDING })
    }
}

/// Fill columns `x` of pixel rows `y` with `colour`.
fn fill(frame: &mut [u8], layout: &Layout, x: Range<usize>, y: Range<usize>, colour: [u8; 3]) {
    let bpp = layout.bytes_per_pixel;
    let x = x.start.min(layout.width)..x.end.min(layout.width);
    for row in y.start.min(layout.height)..y.end.min(layout.height) {
        let line = layout.line(row);
        layout.fill(&mut frame[line.start + x.start * bpp..line.start + x.end * bpp], colour);
    }
}
