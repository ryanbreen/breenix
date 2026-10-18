//! Drawing the console's text rows as pixels, and the pixels they should show.
//!
//! The font size follows the display (`Font::for_display`, the size rule of
//! `docs/console/contract.md` section 2): the largest of 16, 20, 24 and 32
//! whose grid is at least 128 columns by 36 rows. A cell is the glyph's width
//! by its height plus two pixel rows where the cursor is drawn; the grid starts
//! at the framebuffer's top-left pixel and the margin is right and below.
//!
//! Text is grey on black, the same byte in each colour channel, so RGB and BGR
//! framebuffers show the same thing; a fourth byte per pixel is written as the
//! display wants it (`Layout::reserved`).
//!
//! `draw_row` is the drawing path. `expected_intensity` is the read-back
//! check's oracle: it works out one pixel from the font and the row's text
//! alone, sharing only the font with `draw_row`.

use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight, RasterizedChar};

const WEIGHT: FontWeight = FontWeight::Regular;

pub const CURSOR_HEIGHT: usize = 2;

/// The intensity of the cursor's pixels.
pub const CURSOR: u8 = 0xff;

/// The grid the size rule asks for, and the cap on each dimension.
const MIN_COLS: usize = 128;
const MIN_ROWS: usize = 36;
pub const MAX_CELLS: usize = 255;

/// The console font at one size.
#[derive(Clone, Copy, Debug)]
pub struct Font {
    size: RasterHeight,
}

impl PartialEq for Font {
    fn eq(&self, other: &Self) -> bool {
        self.size() == other.size()
    }
}

impl Eq for Font {}

impl Font {
    pub const SIZES: [RasterHeight; 4] =
        [RasterHeight::Size16, RasterHeight::Size20, RasterHeight::Size24, RasterHeight::Size32];

    pub const fn new(size: RasterHeight) -> Self {
        Self { size }
    }

    /// The largest size whose grid on a `width` x `height` display is at least
    /// 128 columns by 36 rows; size 16 when no size reaches that.
    pub fn for_display(width: usize, height: usize) -> Self {
        Self::SIZES
            .iter()
            .rev()
            .map(|&size| Self::new(size))
            .find(|font| width / font.glyph_width() >= MIN_COLS && height / font.line_height() >= MIN_ROWS)
            .unwrap_or(Self::new(RasterHeight::Size16))
    }

    /// The font size `S`, in pixels.
    pub fn size(&self) -> usize {
        self.size.val()
    }

    pub fn glyph_width(&self) -> usize {
        get_raster_width(WEIGHT, self.size)
    }

    pub fn glyph_height(&self) -> usize {
        self.size.val()
    }

    /// A cell's height: the glyph and the cursor band under it.
    pub fn line_height(&self) -> usize {
        self.glyph_height() + CURSOR_HEIGHT
    }

    /// The glyph for a cell's byte; `?` for a byte the font has no glyph for.
    fn glyph(&self, byte: u8) -> RasterizedChar {
        get_raster(char::from(byte), WEIGHT, self.size)
            .or_else(|| get_raster('?', WEIGHT, self.size))
            .expect("the console font has '?'")
    }
}

/// A framebuffer's shape and the font drawn on it. Pixels are 3 or 4 bytes;
/// `stride_bytes` is the distance between pixel rows, and `reserved` the value
/// of a pixel's fourth byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: usize,
    pub height: usize,
    pub bytes_per_pixel: usize,
    pub stride_bytes: usize,
    pub reserved: u8,
    pub font: Font,
}

impl Layout {
    pub fn cols(&self) -> usize {
        (self.width / self.font.glyph_width()).min(MAX_CELLS)
    }

    pub fn rows(&self) -> usize {
        (self.height / self.font.line_height()).min(MAX_CELLS)
    }

    /// The framebuffer's size in bytes.
    pub fn bytes(&self) -> usize {
        self.stride_bytes * self.height
    }

    /// The pixel rows text row `row` covers.
    pub fn band(&self, row: usize) -> core::ops::Range<usize> {
        let line_height = self.font.line_height();
        let start = (row * line_height).min(self.height);
        start..(start + line_height).min(self.height)
    }

    /// The bytes of pixel row `y` that hold its `width` pixels.
    pub fn line(&self, y: usize) -> core::ops::Range<usize> {
        let start = y * self.stride_bytes;
        start..start + self.width * self.bytes_per_pixel
    }

    /// Write `intensity` into each pixel of `pixels`.
    fn fill(&self, pixels: &mut [u8], intensity: u8) {
        for pixel in pixels.chunks_exact_mut(self.bytes_per_pixel) {
            pixel[..3].fill(intensity);
            pixel[3..].fill(self.reserved);
        }
    }

    /// Whether `pixel`, `bytes_per_pixel` bytes, shows `intensity` as
    /// `draw_row` writes it.
    pub fn pixel_is(&self, pixel: &[u8], intensity: u8) -> bool {
        pixel[..3].iter().all(|&byte| byte == intensity) && pixel[3..].iter().all(|&byte| byte == self.reserved)
    }
}

/// Fill each pixel row of `frame`, a whole framebuffer laid out as `layout`,
/// with background.
pub fn clear(frame: &mut [u8], layout: &Layout) {
    for y in 0..layout.height {
        layout.fill(&mut frame[layout.line(y)], 0);
    }
}

/// Draw text row `row` into `frame`, a whole framebuffer laid out as `layout`:
/// each cell's glyph, the cursor when `cursor` is a column of this row, and
/// background in the rest of the band, the right margin included.
pub fn draw_row(frame: &mut [u8], layout: &Layout, row: usize, text: &[u8], cursor: Option<usize>) {
    let font = layout.font;
    let band = layout.band(row);
    let bpp = layout.bytes_per_pixel;
    let cell = font.glyph_width() * bpp;
    let cells = text.len().min(layout.cols());
    for y in band.clone() {
        layout.fill(&mut frame[layout.line(y)], 0);
    }
    for (col, &byte) in text[..cells].iter().enumerate() {
        let raster = font.glyph(byte);
        for (dy, glyph_row) in raster.raster().iter().enumerate() {
            let y = band.start + dy;
            if y >= band.end {
                break;
            }
            let start = layout.line(y).start + col * cell;
            for (pixel, &intensity) in frame[start..start + cell].chunks_exact_mut(bpp).zip(glyph_row.iter()) {
                pixel[..3].fill(intensity);
            }
        }
    }
    if let Some(col) = cursor.filter(|&col| col < cells) {
        for y in band.start + font.glyph_height()..band.end {
            let start = layout.line(y).start + col * cell;
            layout.fill(&mut frame[start..start + cell], CURSOR);
        }
    }
}

/// The oracle: the intensity pixel `x` of pixel row `y` within a text row's
/// band should show, in `font`, for a row holding `text` with the cursor at
/// `cursor`.
pub fn expected_intensity(font: Font, text: &[u8], cursor: Option<usize>, x: usize, y: usize) -> u8 {
    let col = x / font.glyph_width();
    let Some(&byte) = text.get(col) else {
        return 0;
    };
    if y < font.glyph_height() {
        font.glyph(byte).raster()[y][x % font.glyph_width()]
    } else if cursor == Some(col) {
        CURSOR
    } else {
        0
    }
}
