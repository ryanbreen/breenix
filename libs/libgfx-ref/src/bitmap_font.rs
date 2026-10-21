//! Anti-aliased bitmap font rendering using noto-sans-mono-bitmap.
//!
//! Provides the same professional Noto Sans Mono font used in the kernel,
//! with alpha-blended glyph rendering for smooth text output.

use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight};
/// The raster sizes this crate carries: 16 px and 32 px.
pub use noto_sans_mono_bitmap::RasterHeight;

use crate::color::Color;
use crate::framebuf::{Box64, FrameBuf};
use crate::work;

/// Font metrics for layout calculations.
#[derive(Debug, Clone, Copy)]
pub struct FontMetrics {
    /// Width of each character in pixels.
    pub char_width: usize,
    /// Height of each character in pixels.
    pub char_height: usize,
    /// Additional vertical space between lines.
    pub line_spacing: usize,
}

impl FontMetrics {
    /// Total line height (char height + line spacing).
    pub fn line_height(&self) -> usize {
        self.char_height + self.line_spacing
    }
}

/// Get the font metrics for the 16px regular Noto Sans Mono font.
pub fn metrics() -> FontMetrics {
    metrics_at(RasterHeight::Size16)
}

/// The font metrics at raster size `height`; line spacing is an eighth of the size.
pub fn metrics_at(height: RasterHeight) -> FontMetrics {
    FontMetrics {
        char_width: get_raster_width(FontWeight::Regular, height),
        char_height: height.val(),
        line_spacing: height.val() / 8,
    }
}

/// Draw a single character with its raster's top-left at (x0, y0) (docs/gui/breengel.md
/// section 8.2): each pixel is the coverage blend of `fg` over the pixel already there. A
/// character the crate lacks draws the raster of '?'. Marks the raster's rectangle.
pub fn draw_char(fb: &mut FrameBuf, ch: char, x0: usize, y0: usize, fg: Color) {
    draw_char_at(fb, ch, x0, y0, fg, RasterHeight::Size16);
}

/// Draw a single character from the raster of size `height`, as `draw_char` does at 16 px.
pub fn draw_char_at(fb: &mut FrameBuf, ch: char, x0: usize, y0: usize, fg: Color, height: RasterHeight) {
    if x0 > i64::MAX as usize / 2 || y0 > i64::MAX as usize / 2 {return;}
    let (x0, y0) = (x0 as i64, y0 as i64);
    let rc = match get_raster(ch, FontWeight::Regular, height) {
        Some(rc) => rc,
        None => match get_raster('?', FontWeight::Regular, height) {Some(rc)=>rc,None=>return},
    };
    let raster = rc.raster();
    let cell = Box64 { x0, y0, x1: x0 + rc.width() as i64, y1: y0 + raster.len() as i64 };
    let v = cell.intersect(&fb.clip_box());
    if !v.is_empty() {
        work::add(v.height() as u64 * (1 + v.width() as u64));
        for py in v.y0..v.y1 {
            let row = &raster[(py - y0) as usize];
            for px in v.x0..v.x1 {
                let a = row[(px - x0) as usize];
                if a == 0 {
                    continue;
                }
                let c = if a == 255 { fg } else { fg.blend(fb.get_pixel(px as usize, py as usize), a) };
                let p = fb.opaque(c);
                fb.write(px as usize, py as usize, p);
            }
        }
    }
    fb.mark(cell);
}

/// Draw a string of ASCII bytes at (x, y) with the given foreground color.
///
/// Characters are spaced at the font's monospace advance width (no letter spacing).
pub fn draw_text(fb: &mut FrameBuf, text: &[u8], x: usize, y: usize, fg: Color) {
    let m = metrics();
    work::add(text.len() as u64);
    for (i, &ch) in text.iter().enumerate() {
        let Some(cx) = i.checked_mul(m.char_width).and_then(|dx| x.checked_add(dx)) else { return };
        draw_char(fb, ch as char, cx, y, fg);
    }
}

/// Measure the pixel width of a byte string.
pub fn text_width(text: &[u8]) -> usize {
    let m = metrics();
    text.len() * m.char_width
}
