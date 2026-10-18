//! Drawing the console's text rows as pixels, and the pixels they should show.
//!
//! Each text row is a band of `LINE_HEIGHT` pixel rows: the glyphs' 16 rows
//! and, under them, two rows where the cursor is drawn. Text is grey on black,
//! the same byte in each colour channel, so RGB and BGR framebuffers show the
//! same thing; a fourth byte per pixel is written 0.
//!
//! `draw_row` is the drawing path. `expected_intensity` is the read-back
//! check's oracle: it works out one pixel from the font and the row's text
//! alone, sharing only the font with `draw_row`.

use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight, RasterizedChar};

const WEIGHT: FontWeight = FontWeight::Regular;
const SIZE: RasterHeight = RasterHeight::Size16;

pub const GLYPH_WIDTH: usize = get_raster_width(WEIGHT, SIZE);
pub const GLYPH_HEIGHT: usize = SIZE.val();
pub const CURSOR_HEIGHT: usize = 2;
pub const LINE_HEIGHT: usize = GLYPH_HEIGHT + CURSOR_HEIGHT;

/// The intensity of the cursor's pixels.
pub const CURSOR: u8 = 0xff;

/// A framebuffer's shape. Pixels are 3 or 4 bytes; `stride_bytes` is the
/// distance between pixel rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub bytes_per_pixel: usize,
    pub stride_bytes: usize,
}

impl Layout {
    pub fn cols(&self) -> usize {
        self.width / GLYPH_WIDTH
    }

    pub fn rows(&self) -> usize {
        self.height / LINE_HEIGHT
    }

    /// The framebuffer's size in bytes.
    pub fn bytes(&self) -> usize {
        self.stride_bytes * self.height
    }

    /// The pixel rows text row `row` covers.
    pub fn band(&self, row: usize) -> core::ops::Range<usize> {
        let start = (row * LINE_HEIGHT).min(self.height);
        start..(start + LINE_HEIGHT).min(self.height)
    }

    /// The bytes of pixel row `y` that hold its `width` pixels.
    pub fn line(&self, y: usize) -> core::ops::Range<usize> {
        let start = y * self.stride_bytes;
        start..start + self.width * self.bytes_per_pixel
    }
}

/// The glyph for a cell's byte; `?` for a byte the font has no glyph for.
fn glyph(byte: u8) -> RasterizedChar {
    get_raster(char::from(byte), WEIGHT, SIZE)
        .or_else(|| get_raster('?', WEIGHT, SIZE))
        .expect("the console font has '?'")
}

/// Draw text row `row` into `frame`, a whole framebuffer laid out as `layout`:
/// each cell's glyph, the cursor when `cursor` is a column of this row, and
/// background in the rest of the band, the right margin included.
pub fn draw_row(frame: &mut [u8], layout: &Layout, row: usize, text: &[u8], cursor: Option<usize>) {
    let band = layout.band(row);
    let bpp = layout.bytes_per_pixel;
    let cells = text.len().min(layout.cols());
    for y in band.clone() {
        frame[layout.line(y)].fill(0);
    }
    for (col, &byte) in text[..cells].iter().enumerate() {
        let raster = glyph(byte);
        for (dy, glyph_row) in raster.raster().iter().enumerate() {
            let y = band.start + dy;
            if y >= band.end {
                break;
            }
            let start = layout.line(y).start + col * GLYPH_WIDTH * bpp;
            let pixels = &mut frame[start..start + GLYPH_WIDTH * bpp];
            for (pixel, &intensity) in pixels.chunks_exact_mut(bpp).zip(glyph_row.iter()) {
                pixel[..3].fill(intensity);
            }
        }
    }
    if let Some(col) = cursor.filter(|&col| col < cells) {
        for y in band.start + GLYPH_HEIGHT..band.end {
            let start = layout.line(y).start + col * GLYPH_WIDTH * bpp;
            for pixel in frame[start..start + GLYPH_WIDTH * bpp].chunks_exact_mut(bpp) {
                pixel[..3].fill(CURSOR);
            }
        }
    }
}

/// The oracle: the intensity pixel `x` of pixel row `y` within a text row's
/// band should show, for a row holding `text` with the cursor at `cursor`.
pub fn expected_intensity(text: &[u8], cursor: Option<usize>, x: usize, y: usize) -> u8 {
    let col = x / GLYPH_WIDTH;
    let Some(&byte) = text.get(col) else {
        return 0;
    };
    if y < GLYPH_HEIGHT {
        glyph(byte).raster()[y][x % GLYPH_WIDTH]
    } else if cursor == Some(col) {
        CURSOR
    } else {
        0
    }
}

/// Whether `pixel`, `bytes_per_pixel` bytes, shows `intensity` as `draw_row`
/// writes it.
pub fn pixel_is(pixel: &[u8], intensity: u8) -> bool {
    pixel[..3].iter().all(|&byte| byte == intensity) && pixel[3..].iter().all(|&byte| byte == 0)
}
