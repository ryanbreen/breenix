//! Anti-aliased bitmap font rendering using noto-sans-mono-bitmap.
//!
//! Provides the same professional Noto Sans Mono font used in the kernel,
//! with alpha-blended glyph rendering for smooth text output.

use noto_sans_mono_bitmap::{
    get_raster, get_raster_width, FontWeight, RasterHeight,
};

use crate::color::Color;
use crate::framebuf::FrameBuf;

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
    FontMetrics {
        char_width: get_raster_width(FontWeight::Regular, RasterHeight::Size16),
        char_height: 16,
        line_spacing: 2,
    }
}

/// Draw a single character with anti-aliased alpha blending.
///
/// The character is rendered at (x0, y0) with the given foreground color.
/// Background pixels are read from the framebuffer and blended with the
/// glyph intensity for smooth anti-aliased edges.
pub fn draw_char(fb: &mut FrameBuf, ch: char, x0: usize, y0: usize, fg: Color) {
    let rc = match get_raster(ch, FontWeight::Regular, RasterHeight::Size16) {
        Some(rc) => rc,
        None => match get_raster('?', FontWeight::Regular, RasterHeight::Size16) {
            Some(rc) => rc,
            None => return,
        },
    };

    let width = rc.width();
    for (y, row) in rc.raster().iter().enumerate() {
        for (x, &intensity) in row.iter().take(width).enumerate() {
            if intensity == 0 {
                continue;
            }
            let px = x0 + x;
            let py = y0 + y;
            if intensity == 255 {
                fb.put_pixel(px, py, fg);
            } else {
                let bg = fb.get_pixel(px, py);
                fb.put_pixel(px, py, fg.blend(bg, intensity));
            }
        }
    }
}

/// Draw a string of ASCII bytes at (x, y) with the given foreground color.
///
/// Characters are spaced at the font's monospace advance width (no letter spacing).
pub fn draw_text(fb: &mut FrameBuf, text: &[u8], x: usize, y: usize, fg: Color) {
    let m = metrics();
    for (i, &ch) in text.iter().enumerate() {
        draw_char(fb, ch as char, x + i * m.char_width, y, fg);
    }
}

/// Measure the pixel width of a byte string.
pub fn text_width(text: &[u8]) -> usize {
    let m = metrics();
    text.len() * m.char_width
}
