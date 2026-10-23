//! The production x86 probe's mapped framebuffer, retained under ruling B42.
//! ARM64 uses GPU surfaces; neither path links the suites' CPU reference renderer.

use libgfx::color::Color;

pub struct FrameBuf {
    ptr: *mut u8,
    pub width: usize,
    pub height: usize,
    stride: usize,
    bpp: usize,
    bgr: bool,
}

impl FrameBuf {
    /// The caller supplies a writable pane of at least stride * height bytes.
    pub unsafe fn from_raw(ptr: *mut u8, width: usize, height: usize, stride: usize,
        bpp: usize, bgr: bool) -> Result<Self, &'static str> {
        if ptr.is_null() || width == 0 || height == 0 || !(3..=4).contains(&bpp)
            || width.checked_mul(bpp).is_none_or(|row| row > stride)
            || stride.checked_mul(height).is_none_or(|len| len > isize::MAX as usize) {
            return Err("invalid mapped framebuffer");
        }
        Ok(Self { ptr, width, height, stride, bpp, bgr })
    }
    pub fn width(&self) -> usize { self.width }
    pub fn height(&self) -> usize { self.height }
    pub fn put_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height { return; }
        let channels = if self.bgr { [color.b, color.g, color.r, 255] }
            else { [color.r, color.g, color.b, 255] };
        // SAFETY: construction validates the mapped extent; coordinates are clipped above.
        unsafe { core::ptr::copy_nonoverlapping(channels.as_ptr(),
            self.ptr.add(y * self.stride + x * self.bpp), self.bpp); }
    }
    pub fn clear(&mut self, color: Color) {
        shapes::fill_rect(self, 0, 0, self.width as i32, self.height as i32, color);
    }
}

pub mod shapes {
    use super::*;
    pub fn fill_rect(fb: &mut FrameBuf, x: i32, y: i32, w: i32, h: i32, color: Color) {
        if w <= 0 || h <= 0 { return; }
        let x0 = (x as i64).clamp(0, fb.width as i64) as usize;
        let x1 = (x as i64 + w as i64).clamp(0, fb.width as i64) as usize;
        let y0 = (y as i64).clamp(0, fb.height as i64) as usize;
        let y1 = (y as i64 + h as i64).clamp(0, fb.height as i64) as usize;
        for row in y0..y1 { for col in x0..x1 { fb.put_pixel(col, row, color); } }
    }
}

pub mod font {
    use super::*;
    pub fn draw_text(fb: &mut FrameBuf, text: &[u8], x: usize, y: usize, color: Color, scale: usize) {
        for (i, &ch) in text.iter().enumerate() {
            for row in 0..7 { for col in 0..5 {
                if libgfx::font::FONT_TABLE[ch as usize * 7 + row] & (0x10 >> col) != 0 {
                    shapes::fill_rect(fb, (x + (6 * i + col) * scale) as i32,
                        (y + row * scale) as i32, scale as i32, scale as i32, color);
                }
            } }
        }
    }
}
