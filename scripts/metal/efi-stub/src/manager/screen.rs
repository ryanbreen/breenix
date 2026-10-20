//! The menu on the firmware's graphics output (UEFI 2.10 section 12.9), drawn with the kernel console's font and read
//! back: each glyph cell on the screen is compared with the font's pixels for the character drawn there, as the
//! kernel's `/proc/fbcon/check` does for its console. Pixels are one foreground and one background colour (white and
//! black), which each pixel format represents exactly, so the read-back compares them exactly.
use core::ffi::c_void;
use core::ptr::{null, null_mut};

use metal_efi::say;
use metal_efi::uefi::*;
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

pub const COLS: usize = 76;
pub const ROWS: usize = 14;
const HEIGHT: RasterHeight = RasterHeight::Size20;
const WEIGHT: FontWeight = FontWeight::Regular;
const GAP: usize = 6;
const FG: BltPixel = BltPixel { blue: 255, green: 255, red: 255, reserved: 0 };
const BG: BltPixel = BltPixel { blue: 0, green: 0, red: 0, reserved: 0 };

#[derive(Clone, Copy)]
pub struct Row {
    pub text: [u8; COLS],
    pub len: usize,
    pub inverted: bool,
}

impl Row {
    const BLANK: Row = Row { text: [b' '; COLS], len: 0, inverted: false };

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.text[..self.len]).unwrap_or("?")
    }
}

/// A row's text, built with `write!`.
pub struct Line {
    pub row: Row,
}

impl Line {
    pub fn new() -> Self {
        Self { row: Row::BLANK }
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        for byte in text.bytes() {
            if self.row.len < COLS {
                self.row.text[self.row.len] = if (0x20..0x7f).contains(&byte) { byte } else { b'?' };
                self.row.len += 1;
            }
        }
        Ok(())
    }
}

pub struct Screen<'a> {
    boot: &'a BootServices,
    gop: *mut GraphicsOutput,
    cell_w: usize,
    cell_h: usize,
    cols: usize,
    x0: usize,
    y0: usize,
    pixels: *mut BltPixel,
    check: *mut BltPixel,
    pub rows: [Row; ROWS],
    pub width: usize,
    pub height: usize,
}

/// The read-back of the rows drawn: rows checked, glyph cells checked, cells that differ.
pub struct Check {
    pub rows: usize,
    pub cells: usize,
    pub differing: usize,
}

impl<'a> Screen<'a> {
    /// The console's graphics output, or the first one the firmware has; absent when the machine has no display.
    ///
    /// # Safety
    /// `system` must be the firmware's system table.
    pub unsafe fn open(boot: &'a BootServices, system: *const SystemTable) -> Option<Self> {
        let mut gop: *mut c_void = null_mut();
        if (boot.handle_protocol)((*system).console_out_handle, &GRAPHICS_OUTPUT, &mut gop) != SUCCESS || gop.is_null() {
            gop = null_mut();
            if (boot.locate_protocol)(&GRAPHICS_OUTPUT, null(), &mut gop) != SUCCESS || gop.is_null() {
                return None;
            }
        }
        let gop = gop.cast::<GraphicsOutput>();
        let info = &*(*(*gop).mode).info;
        let (width, height) = (info.horizontal_resolution as usize, info.vertical_resolution as usize);
        let cell_w = get_raster_width(WEIGHT, HEIGHT);
        let cell_h = HEIGHT.val();
        let cols = COLS.min(width / cell_w);
        if cols < 40 || height < ROWS * (cell_h + GAP) {
            say!("the screen ({}x{}) is too small for the menu; menu on COM1 only", width, height);
            return None;
        }
        let bytes = cols * cell_w * cell_h * core::mem::size_of::<BltPixel>();
        let mut pixels = null_mut();
        let mut check = null_mut();
        if (boot.allocate_pool)(LOADER_DATA, bytes, &mut pixels) != SUCCESS
            || (boot.allocate_pool)(LOADER_DATA, bytes, &mut check) != SUCCESS
        {
            return None;
        }
        let x0 = (width - cols * cell_w) / 2;
        let y0 = (height - ROWS * (cell_h + GAP)) / 3;
        say!("screen: {}x{}, pixel format {}, {} columns of {}x{} cells", width, height, info.pixel_format, cols, cell_w, cell_h);
        Some(Self {
            boot,
            gop,
            cell_w,
            cell_h,
            cols,
            x0,
            y0,
            pixels: pixels.cast(),
            check: check.cast(),
            rows: [Row::BLANK; ROWS],
            width,
            height,
        })
    }

    pub fn clear(&mut self) {
        let mut fill = BG;
        // SAFETY: a video fill of the whole screen from one pixel.
        unsafe {
            ((*self.gop).blt)(self.gop, &mut fill, BLT_VIDEO_FILL, 0, 0, 0, 0, self.width, self.height, 0);
        }
    }

    /// The font's raster for a character, `?` for one the font lacks.
    fn glyph(character: u8) -> Option<&'static [&'static [u8]]> {
        get_raster(character as char, WEIGHT, HEIGHT).or_else(|| get_raster('?', WEIGHT, HEIGHT)).map(|glyph| glyph.raster())
    }

    fn pixel(glyph: Option<&'static [&'static [u8]]>, x: usize, y: usize, inverted: bool) -> BltPixel {
        let on = glyph.map_or(false, |raster| raster[y][x] >= 128);
        if on != inverted { FG } else { BG }
    }

    fn render(&self, row: &Row) {
        let line = self.cols * self.cell_w;
        for col in 0..self.cols {
            let glyph = Self::glyph(if col < row.len { row.text[col] } else { b' ' });
            for y in 0..self.cell_h {
                for x in 0..self.cell_w {
                    // SAFETY: inside the row buffer of cols * cell_w * cell_h pixels.
                    unsafe { *self.pixels.add(y * line + col * self.cell_w + x) = Self::pixel(glyph, x, y, row.inverted) };
                }
            }
        }
    }

    /// Draws the rows.
    pub fn draw(&mut self) {
        for index in 0..ROWS {
            let row = self.rows[index];
            self.render(&row);
            // SAFETY: the row buffer, drawn at its place on the screen.
            unsafe {
                ((*self.gop).blt)(
                    self.gop,
                    self.pixels,
                    BLT_BUFFER_TO_VIDEO,
                    0,
                    0,
                    self.x0,
                    self.y0 + index * (self.cell_h + GAP),
                    self.cols * self.cell_w,
                    self.cell_h,
                    0,
                );
            }
        }
    }

    /// Reads the drawn rows back from the screen and compares each glyph cell with the font.
    pub fn read_back(&self) -> Check {
        let line = self.cols * self.cell_w;
        let mut check = Check { rows: 0, cells: 0, differing: 0 };
        for (index, row) in self.rows.iter().enumerate() {
            // SAFETY: reading one row's rectangle of the screen into the check buffer.
            let status = unsafe {
                ((*self.gop).blt)(
                    self.gop,
                    self.check,
                    BLT_VIDEO_TO_BUFFER,
                    self.x0,
                    self.y0 + index * (self.cell_h + GAP),
                    0,
                    0,
                    self.cols * self.cell_w,
                    self.cell_h,
                    0,
                )
            };
            check.rows += 1;
            for col in 0..self.cols {
                let glyph = Self::glyph(if col < row.len { row.text[col] } else { b' ' });
                let mut same = status == SUCCESS;
                for y in 0..self.cell_h {
                    for x in 0..self.cell_w {
                        // SAFETY: inside the check buffer.
                        let found = unsafe { *self.check.add(y * line + col * self.cell_w + x) };
                        let wanted = Self::pixel(glyph, x, y, row.inverted);
                        if (found.red, found.green, found.blue) != (wanted.red, wanted.green, wanted.blue) {
                            same = false;
                        }
                    }
                }
                check.cells += 1;
                if !same {
                    check.differing += 1;
                }
            }
        }
        check
    }

    /// Reports the read-back and each non-blank row on COM1.
    pub fn report(&self) {
        let check = self.read_back();
        if check.differing == 0 {
            say!("screen read back identical: {} rows, {} glyph cells match the font", check.rows, check.cells);
        } else {
            say!("screen read back differs: {} of {} glyph cells in {} rows", check.differing, check.cells, check.rows);
        }
        for row in self.rows.iter().filter(|row| row.len > 0) {
            say!("row: {}", row.as_str().trim_end());
        }
    }
}

impl Drop for Screen<'_> {
    fn drop(&mut self) {
        // SAFETY: the two row buffers this screen allocated.
        unsafe {
            (self.boot.free_pool)(self.pixels.cast());
            (self.boot.free_pool)(self.check.cast());
        }
    }
}
