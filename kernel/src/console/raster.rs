//! Attributed console cells as RGB pixels, sized by the console contract.

use noto_sans_mono_bitmap::{
    get_raster, get_raster_width, FontWeight, RasterHeight, RasterizedChar,
};

use super::boxdraw;
use super::grid::{Cell, BOLD, UNDERLINE};

const WEIGHT: FontWeight = FontWeight::Regular;

/// The contract's palette in SGR order.
pub const PALETTE: [[u8; 3]; 16] = [
    [15, 20, 35],
    [205, 70, 70],
    [70, 200, 120],
    [215, 180, 70],
    [70, 120, 215],
    [175, 95, 200],
    [60, 175, 185],
    [205, 215, 235],
    [90, 100, 125],
    [245, 110, 110],
    [120, 230, 160],
    [245, 215, 110],
    [120, 165, 245],
    [215, 140, 240],
    [110, 215, 225],
    [245, 248, 255],
];

pub fn nearest_colour(rgb: [u8; 3]) -> u8 {
    let mut nearest = 0;
    let mut distance = u32::MAX;
    for (index, colour) in PALETTE.iter().enumerate() {
        let squared: u32 = rgb
            .iter()
            .zip(colour)
            .map(|(&a, &b)| {
                let delta = i32::from(a) - i32::from(b);
                (delta * delta) as u32
            })
            .sum();
        if squared < distance {
            nearest = index as u8;
            distance = squared;
        }
    }
    nearest
}

pub const CURSOR_HEIGHT: usize = 2;

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
    pub const SIZES: [RasterHeight; 4] = [
        RasterHeight::Size16,
        RasterHeight::Size20,
        RasterHeight::Size24,
        RasterHeight::Size32,
    ];

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
            .find(|font| {
                width / font.glyph_width() >= MIN_COLS && height / font.line_height() >= MIN_ROWS
            })
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

    /// Font data is shared with the read-back check, including its bold weight.
    fn glyph(&self, cell: Cell) -> RasterizedChar {
        let weight = if cell.flags & BOLD != 0 {
            FontWeight::Bold
        } else {
            WEIGHT
        };
        let character = match cell.glyph {
            0x00a0 => ' ', // Non-breaking space occupies a blank cell.
            0x00ad => '-', // A soft hyphen has the hyphen shape in this fixed grid.
            _ => cell.character(),
        };
        get_raster(character, weight, self.size)
            .or_else(|| get_raster('?', weight, self.size))
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
    pub bgr: bool,
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

    fn put_pixel(&self, pixel: &mut [u8], rgb: [u8; 3]) {
        let [r, g, b] = rgb;
        pixel[..3].copy_from_slice(&if self.bgr { [b, g, r] } else { rgb });
        pixel[3..].fill(self.reserved);
    }

    fn fill(&self, pixels: &mut [u8], colour: [u8; 3]) {
        for pixel in pixels.chunks_exact_mut(self.bytes_per_pixel) {
            self.put_pixel(pixel, colour);
        }
    }

    pub fn pixel_is(&self, pixel: &[u8], rgb: [u8; 3]) -> bool {
        let [r, g, b] = rgb;
        pixel[..3] == if self.bgr { [b, g, r] } else { rgb }
            && pixel[3..].iter().all(|&byte| byte == self.reserved)
    }
}

/// Fill columns `x` of pixel rows `y` with `colour`.
#[cfg(target_arch = "aarch64")]
pub fn fill_rect(frame: &mut [u8], layout: &Layout, x: core::ops::Range<usize>, y: core::ops::Range<usize>, colour: [u8; 3]) {
    let bpp = layout.bytes_per_pixel;
    let x = x.start.min(layout.width)..x.end.min(layout.width);
    for row in y.start.min(layout.height)..y.end.min(layout.height) {
        let line = layout.line(row);
        layout.fill(&mut frame[line.start + x.start * bpp..line.start + x.end * bpp], colour);
    }
}

pub fn clear(frame: &mut [u8], layout: &Layout) {
    for y in 0..layout.height {
        layout.fill(&mut frame[layout.line(y)], PALETTE[0]);
    }
}

fn blend(fg: [u8; 3], bg: [u8; 3], coverage: u8) -> [u8; 3] {
    let a = u32::from(coverage);
    core::array::from_fn(|i| {
        ((u32::from(fg[i]) * a + u32::from(bg[i]) * (255 - a) + 127) / 255) as u8
    })
}

/// Rasterize cells into the shadow. Box glyphs span the cell's whole height,
/// so a vertical stroke joins the next text row through the cursor band.
pub fn draw_row(
    frame: &mut [u8],
    layout: &Layout,
    row: usize,
    text: &[Cell],
    cursor: Option<usize>,
) {
    let font = layout.font;
    let band = layout.band(row);
    let bpp = layout.bytes_per_pixel;
    let width = font.glyph_width();
    for y in band.clone() {
        layout.fill(&mut frame[layout.line(y)], PALETTE[0]);
    }
    for (col, &cell) in text.iter().take(layout.cols()).enumerate() {
        let (fg, bg) = cell.displayed_colours();
        let raster = font.glyph(cell);
        for y in band.clone() {
            let dy = y - band.start;
            let start = layout.line(y).start + col * width * bpp;
            for (dx, pixel) in frame[start..start + width * bpp]
                .chunks_exact_mut(bpp)
                .enumerate()
            {
                let coverage = if (cursor == Some(col) && dy >= font.glyph_height())
                    || (cell.flags & UNDERLINE != 0 && dy == font.glyph_height())
                {
                    255
                } else if (0x2500..=0x257f).contains(&cell.glyph) {
                    boxdraw::coverage(cell.glyph, width, font.line_height(), dx, dy)
                } else if dy < font.glyph_height() {
                    raster.raster()[dy][dx]
                } else {
                    0
                };
                layout.put_pixel(pixel, blend(PALETTE[fg], PALETTE[bg], coverage));
            }
        }
    }
}

/// Read-back oracle: derive an RGB pixel from the retained cell and the font
/// or box geometry, independently of the scanline writer and pixel encoding.
pub fn expected_colour(
    font: Font,
    text: &[Cell],
    cursor: Option<usize>,
    x: usize,
    y: usize,
) -> [u8; 3] {
    let col = x / font.glyph_width();
    let Some(&cell) = text.get(col) else {
        return PALETTE[0];
    };
    let (fg, bg) = cell.displayed_colours();
    let coverage = if y >= font.glyph_height()
        && (cursor == Some(col) || (cell.flags & UNDERLINE != 0 && y == font.glyph_height()))
    {
        255
    } else if (0x2500..=0x257f).contains(&cell.glyph) {
        boxdraw::coverage(
            cell.glyph,
            font.glyph_width(),
            font.line_height(),
            x % font.glyph_width(),
            y,
        )
    } else if y < font.glyph_height() {
        font.glyph(cell).raster()[y][x % font.glyph_width()]
    } else {
        0
    };
    let mut colour = [0; 3];
    for i in 0..3 {
        colour[i] = ((u32::from(PALETTE[fg][i]) * u32::from(coverage)
            + u32::from(PALETTE[bg][i]) * (255 - u32::from(coverage))
            + 127)
            / 255) as u8;
    }
    colour
}
