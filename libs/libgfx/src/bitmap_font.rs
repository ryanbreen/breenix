//! The anti-aliased bitmap font (docs/gui/breengel.md section 8.2): `noto-sans-mono-bitmap`
//! rasters drawn by `bitmap_text` from a coverage table per size (format 1), blended by
//! section 2's coverage blend on the device.
//!
//! Each size's table is laid out as the contract fixes it (ruling B10): the glyphs U+0020
//! to U+007E in order, then U+FFFD, each glyph's coverage rows stacked vertically, uploaded
//! whole once per context from the crate's static rows.

use alloc::vec::Vec;

use bgl::{pipeline, DeclaredFixture, Instance};
use libbreenix::gpu::{self, abi};
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight};
/// The raster sizes this crate carries: 16 px and 32 px.
pub use noto_sans_mono_bitmap::RasterHeight;

use crate::color::Color;
use crate::device::{self, Table};
use crate::surface::{word, Box64, Surface};
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

/// B10 fixes the complete glyph order at compile time, including row descriptors.
const fn rows<const N: usize>(height: RasterHeight) -> [&'static [u8]; N] {
    let mut out: [&'static [u8]; N] = [&[]; N];
    let mut glyph = 0;
    while glyph < 96 {
        let ch = if glyph == 95 { '\u{fffd}' } else { (glyph as u8 + 32) as char };
        let raster = match get_raster(ch, FontWeight::Regular, height) {
            Some(r) => r,
            None => panic!("missing B10 bitmap glyph"),
        };
        let mut row = 0;
        while row < height.val() {
            out[glyph * height.val() + row] = raster.raster()[row];
            row += 1;
        }
        glyph += 1;
    }
    out
}
pub static TABLE16: [&[u8]; 96 * 16] = rows(RasterHeight::Size16);
pub static TABLE32: [&[u8]; 96 * 32] = rows(RasterHeight::Size32);

/// The whole static table for this size, including its fixed row order.
pub fn table_rows(height: RasterHeight) -> &'static [&'static [u8]] {
    match height { RasterHeight::Size16 => &TABLE16, _ => &TABLE32 }
}

/// The table row of `ch`: its own glyph, or `?`'s for a character the table lacks.
fn table_index(ch: char) -> u32 {
    match ch {
        ' '..='~' => ch as u32 - 0x20,
        '\u{fffd}' => 95,
        _ => '?' as u32 - 0x20,
    }
}

/// The size's table texture in the process's context, uploaded whole the first time.
fn texture(height: RasterHeight) -> Result<Table, i64> {
    let index = if matches!(height, RasterHeight::Size16) { 1 } else { 2 };
    device::with(|d| {
        d.table(index, |context| {
            let (w, h) = (get_raster_width(FontWeight::Regular, height) as u32, height.val() as u32);
            let rows = table_rows(height);
            let rect = [0, 0, w, 96 * h];
            let texture = gpu::texture_create(abi::format::R8, w, 96 * h)?;
            match bgl::fixture_upload(DeclaredFixture::rows(rows), context, texture, rect, w, abi::upload_source::R8) {
                Ok(()) => Ok(Table { texture, cell_w: w, cell_h: h }),
                Err(e) => {
                    gpu::resource_destroy(texture);
                    Err(e)
                }
            }
        })
    })
    .and_then(|t| if t.cell_h as usize == height.val() { Ok(t) } else { Err(-abi::errno::EINVAL) })
}

/// The cell of `ch` at `(x, y)` as one `bitmap_text` instance over `visible`.
fn glyph(table: Table, ch: char, x: i64, y: i64, visible: Box64) -> Instance {
    let mut params = [0u32; 16];
    let values = [x, y, 0, (table_index(ch) * table.cell_h) as i64, table.cell_w as i64, table.cell_h as i64, 1, 1];
    for (slot, v) in params.iter_mut().zip(values) {
        *slot = v as i32 as u32;
    }
    Instance { rect: visible.corners(), params }
}

/// Draw a single character with its raster's top-left at (x0, y0) (docs/gui/breengel.md
/// section 8.2): each pixel is the coverage blend of `fg` over the pixel already there. A
/// character the table lacks draws the raster of '?'. Marks the raster's rectangle.
pub fn draw_char(fb: &mut Surface, ch: char, x0: usize, y0: usize, fg: Color) {
    draw_char_at(fb, ch, x0, y0, fg, RasterHeight::Size16);
}

/// Draw a single character from the raster of size `height`, as `draw_char` does at 16 px.
pub fn draw_char_at(fb: &mut Surface, ch: char, x0: usize, y0: usize, fg: Color, height: RasterHeight) {
    run(fb, 1, |_| ch, x0, y0, fg, height);
}

/// Draw a string of ASCII bytes at (x, y) with the given foreground color.
///
/// Characters are spaced at the font's monospace advance width (no letter spacing). Only
/// the characters whose cells meet the clip are visited (section 7).
pub fn draw_text(fb: &mut Surface, text: &[u8], x: usize, y: usize, fg: Color) {
    run(fb, text.len(), |i| text[i] as char, x, y, fg, RasterHeight::Size16);
}

/// Measure the pixel width of a byte string.
pub fn text_width(text: &[u8]) -> usize {
    let m = metrics();
    text.len() * m.char_width
}

/// The visible cells of the `len` characters `char_at(i)` advancing from `(x, y)`, in one
/// draw; each visible cell is marked.
fn run(fb: &mut Surface, len: usize, char_at: impl Fn(usize) -> char, x: usize, y: usize, fg: Color, height: RasterHeight) {
    if len == 0 || x > i64::MAX as usize / 2 || y > i64::MAX as usize / 2 {
        return;
    }
    let (w, h) = (get_raster_width(FontWeight::Regular, height) as i64, height.val() as i64);
    let (x, y, c) = (x as i64, y as i64, fb.clip_box());
    if y >= c.y1 || y + h <= c.y0 || x >= c.x1 {
        return;
    }
    // Character i covers columns [x + i·w, x + (i + 1)·w).
    let first = if x + w > c.x0 { 0 } else { (c.x0 - x - w) / w + 1 };
    let last = ((c.x1 - 1 - x) / w).min(len as i64 - 1);
    if first > last {
        return;
    }
    work::add((last - first + 1) as u64);
    let table = match texture(height) {
        Ok(t) => t,
        Err(e) => return fb.fail(e),
    };
    let cells: Vec<(Box64, Instance)> = (first..=last)
        .map(|i| {
            let cell = Box64::wide(x + i * w, y, w, h);
            (cell, glyph(table, char_at(i as usize), cell.x0, cell.y0, cell.intersect(&c)))
        })
        .collect();
    let glyphs: Vec<Instance> = cells.iter().map(|&(_, g)| g).collect();
    fb.submit(|ctx| ctx.draw(pipeline::BITMAP_TEXT, word(fg), table.texture, 0, 0, &glyphs));
    for &(cell, _) in &cells {
        fb.mark(cell);
    }
}
