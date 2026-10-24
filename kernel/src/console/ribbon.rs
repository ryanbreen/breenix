//! The boot stage ribbon on the console screen (docs/gui/breengel.md section 23.7, ruling
//! B35): one segment per ARM64 boot-path stage across the band (16, 752, 1248, 16) of the
//! 1280 by 800 stage layout, at the display's whole scale, below the text grid (B51).
//! Reached stages are pass green, unreached ones the pending outline colour, and each
//! segment's last two pixels the stage background. The console's read-back expects the
//! ribbon in that band (`colour`), computed apart from the painter (`paint`).

use super::raster::Layout;
use core::ops::Range;

const PASS: [u8; 3] = [26, 112, 77];
const PENDING: [u8; 3] = [48, 67, 96];
const BACKGROUND: [u8; 3] = [12, 19, 31];

/// The ribbon as the console draws it: the display's scale and each stage's state.
#[derive(Clone, PartialEq, Eq)]
pub struct Ribbon {
    scale: usize,
    lit: alloc::vec::Vec<bool>,
    drop_stage: bool,
}

impl Ribbon {
    /// Size the fixed boot-long grid above the band (B52).
    pub fn text_rows(layout: &Layout) -> usize {
        let scale = layout.width / 1280;
        if scale > 0 && layout.width == 1280 * scale && layout.height == 800 * scale {
            layout.rows().min(752 * scale / layout.font.line_height())
        } else {
            layout.rows()
        }
    }
    /// The ribbon for `layout` with the stages reached so far, when the display is the stage
    /// layout at a whole scale and has stages. `drop_stage` shows the first reached stage
    /// unreached (test point 15, `ribbon-drop-stage`).
    pub fn now(layout: &Layout, drop_stage: bool) -> Option<Ribbon> {
        if !crate::graphics::boot_ribbon::is_booting() { return None; }
        let scale = layout.width / 1280;
        if scale == 0 || layout.width != 1280 * scale || layout.height != 800 * scale {
            return None;
        }
        let lit = crate::graphics::boot_ribbon::reached();
        if lit.is_empty() || !crate::graphics::boot_ribbon::is_booting() {
            return None;
        }
        Some(Ribbon { scale, lit, drop_stage })
    }

    /// The pixel rows the band covers.
    pub fn rows(&self) -> Range<usize> {
        752 * self.scale..768 * self.scale
    }

    /// Draw the band's pixel rows within `rows` into `frame`: each segment, then its gap.
    pub fn paint(&self, frame: &mut [u8], layout: &Layout, rows: Range<usize>) {
        let band = self.rows();
        let rows = rows.start.max(band.start)..rows.end.min(band.end);
        if rows.is_empty() {
            return;
        }
        let (s, m) = (self.scale, self.lit.len());
        let first = self.lit.iter().position(|&lit| lit);
        for (i, &lit) in self.lit.iter().enumerate() {
            let lit = lit && !(self.drop_stage && first == Some(i));
            let (a, b) = (16 + 1248 * i / m, 16 + 1248 * (i + 1) / m);
            let colour = if lit { PASS } else { PENDING };
            fill(frame, layout, a * s..(b - 2) * s, rows.clone(), colour);
            fill(frame, layout, (b - 2) * s..b * s, rows.clone(), BACKGROUND);
        }
    }

    /// The read-back oracle: the ribbon's colour at physical pixel `(x, y)`, when the pixel is
    /// in the band.
    pub fn colour(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        let (s, m) = (self.scale, self.lit.len());
        let (lx, ly) = (x / s, y / s);
        if !(752..768).contains(&ly) || !(16..1264).contains(&lx) {
            return None;
        }
        // The stage whose segment and gap hold logical column lx: the last i with
        // 16 + floor(1248 i / m) <= lx.
        let start = |i: usize| 16 + 1248 * i / m;
        let mut i = ((lx - 16) * m / 1248).min(m - 1);
        while i + 1 < m && start(i + 1) <= lx {
            i += 1;
        }
        while start(i) > lx {
            i -= 1;
        }
        let end = start(i + 1);
        Some(if lx >= end - 2 {
            BACKGROUND
        } else if self.lit[i] {
            PASS
        } else {
            PENDING
        })
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
