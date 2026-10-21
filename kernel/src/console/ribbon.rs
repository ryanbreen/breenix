//! The boot stage ribbon on the console screen (docs/gui/breengel.md section 23.7, ruling
//! B35): one segment per ARM64 boot-path stage across the band (16, 752, 1248, 16) of the
//! 1280 by 800 stage layout, at the display's whole scale, drawn over the text grid.
//! Reached stages are pass green, unreached ones the pending outline colour, and each
//! segment's last two pixels the stage background. The console's read-back expects the
//! ribbon in that band (`colour`).

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
}

impl Ribbon {
    /// The ribbon for `layout` with the stages reached so far, when the display is the stage
    /// layout at a whole scale and has stages. `drop_stage` shows the first reached stage
    /// unreached (test point 15, `ribbon-drop-stage`).
    pub fn now(layout: &Layout, drop_stage: bool) -> Option<Ribbon> {
        let scale = layout.width / 1280;
        if scale == 0 || layout.width != 1280 * scale || layout.height != 800 * scale {
            return None;
        }
        let mut lit = crate::graphics::boot_ribbon::reached();
        if lit.is_empty() {
            return None;
        }
        if drop_stage {
            if let Some(first) = lit.iter_mut().find(|s| **s) {
                *first = false;
            }
        }
        Some(Ribbon { scale, lit })
    }

    /// The pixel rows the band covers.
    pub fn rows(&self) -> Range<usize> {
        752 * self.scale..768 * self.scale
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
