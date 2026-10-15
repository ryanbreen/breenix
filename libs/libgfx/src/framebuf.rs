//! Surfaces: pixel memory with a format, a clip stack and damage tracking
//! (docs/gui/breengel.md sections 2, 3, 4 and 6).

use alloc::vec::Vec;

use crate::color::Color;
use crate::work;

/// The largest width or height a surface may have.
pub const MAX_DIMENSION: usize = 16_384;
/// The largest `stride × height` a surface may span, in bytes.
pub const MAX_BYTES: usize = (1 << 31) - 1;

/// A half-open rectangle `x ≤ px < x + w`, `y ≤ py < y + h`; empty when `w ≤ 0` or `h ≤ 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// The damage `take_dirty` reports.
pub type DirtyRect = Rect;

/// How a surface stores a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// Bytes per pixel: 3 or 4.
    pub bpp: usize,
    /// Colour bytes in B, G, R order (else R, G, B).
    pub is_bgr: bool,
    /// The fourth byte is premultiplied alpha (needs `bpp = 4`); otherwise it is the X byte.
    pub premultiplied: bool,
}

impl Format {
    /// The canonical opaque format, BGRX8888.
    pub const BGRX: Format = Format { bpp: 4, is_bgr: true, premultiplied: false };
    pub const RGBX: Format = Format { bpp: 4, is_bgr: false, premultiplied: false };
    pub const BGR: Format = Format { bpp: 3, is_bgr: true, premultiplied: false };
    pub const RGB: Format = Format { bpp: 3, is_bgr: false, premultiplied: false };
    /// Premultiplied BGRA8888.
    pub const BGRA_PREMULTIPLIED: Format = Format { bpp: 4, is_bgr: true, premultiplied: true };
}

/// Why a surface could not be made (section 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceError {
    /// Width or height is 0.
    Empty,
    /// Width or height is above 16,384.
    TooLarge,
    /// Bytes per pixel other than 3 or 4, or premultiplied without 4.
    Format,
    /// The stride is below `width × bpp`.
    StrideTooShort,
    /// `stride × height` is above 2³¹ − 1 or overflows.
    SizeOverflow,
    /// The base pointer is null.
    Null,
    /// The base pointer is not 4-byte aligned.
    Misaligned,
    /// An owned surface's memory could not be allocated.
    OutOfMemory,
}

impl core::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let what = match self {
            SurfaceError::Empty => "width or height is 0",
            SurfaceError::TooLarge => "width or height is above 16384",
            SurfaceError::Format => "unsupported pixel format",
            SurfaceError::StrideTooShort => "stride is below width x bytes per pixel",
            SurfaceError::SizeOverflow => "stride x height is above 2^31 - 1",
            SurfaceError::Null => "base pointer is null",
            SurfaceError::Misaligned => "base pointer is not 4-byte aligned",
            SurfaceError::OutOfMemory => "surface memory could not be allocated",
        };
        f.write_str(what)
    }
}

/// A box `x0 ≤ x < x1`, `y0 ≤ y < y1` in i64; empty when either side is not positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Box64 {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl Box64 {
    pub(crate) const EMPTY: Box64 = Box64 { x0: 0, y0: 0, x1: 0, y1: 0 };

    /// The box of `(x, y, w, h)`, any i32 values.
    pub(crate) fn of(x: i32, y: i32, w: i32, h: i32) -> Box64 {
        let (x, y) = (x as i64, y as i64);
        Box64 { x0: x, y0: y, x1: x + w.max(0) as i64, y1: y + h.max(0) as i64 }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub(crate) fn intersect(&self, o: &Box64) -> Box64 {
        let b = Box64 { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) };
        if b.is_empty() { Box64::EMPTY } else { b }
    }

    fn union(&self, o: &Box64) -> Box64 {
        Box64 { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }

    pub(crate) fn width(&self) -> i64 {
        (self.x1 - self.x0).max(0)
    }

    pub(crate) fn height(&self) -> i64 {
        (self.y1 - self.y0).max(0)
    }

    /// As a `Rect`; the box lies inside a surface, so it fits i32.
    fn rect(&self) -> Rect {
        Rect { x: self.x0 as i32, y: self.y0 as i32, w: self.width() as i32, h: self.height() as i32 }
    }
}

/// A drawing target (section 3): pixel memory it owns or borrows, its format, the clip
/// stack and the damage accumulated since the last `take_dirty`.
///
/// The public geometry fields describe the surface for callers and are read-only by
/// convention; drawing uses a private copy fixed when the surface is made, so changing
/// them cannot move a write outside the memory.
pub struct FrameBuf {
    ptr: *mut u8,
    _owned: Option<Vec<u32>>,
    pub width: usize,
    pub height: usize,
    /// Bytes per row.
    pub stride: usize,
    /// Bytes per pixel (3 or 4).
    pub bpp: usize,
    pub is_bgr: bool,
    geom: Geom,
    clip: Box64,
    clips: Vec<Box64>,
    dirty: Option<Box64>,
}

#[derive(Clone, Copy)]
struct Geom {
    w: usize,
    h: usize,
    stride: usize,
    format: Format,
}

fn validate(width: usize, height: usize, stride: usize, format: Format) -> Result<(), SurfaceError> {
    if width == 0 || height == 0 {
        return Err(SurfaceError::Empty);
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(SurfaceError::TooLarge);
    }
    if !(format.bpp == 3 || format.bpp == 4) || (format.premultiplied && format.bpp != 4) {
        return Err(SurfaceError::Format);
    }
    if stride < width * format.bpp {
        return Err(SurfaceError::StrideTooShort);
    }
    match stride.checked_mul(height) {
        Some(bytes) if bytes <= MAX_BYTES => Ok(()),
        _ => Err(SurfaceError::SizeOverflow),
    }
}

impl FrameBuf {
    fn make(ptr: *mut u8, owned: Option<Vec<u32>>, width: usize, height: usize, stride: usize, format: Format) -> FrameBuf {
        FrameBuf {
            ptr,
            _owned: owned,
            width,
            height,
            stride,
            bpp: format.bpp,
            is_bgr: format.is_bgr,
            geom: Geom { w: width, h: height, stride, format },
            clip: Box64 { x0: 0, y0: 0, x1: width as i64, y1: height as i64 },
            clips: Vec::new(),
            dirty: None,
        }
    }

    /// A surface that owns its memory (stride `width × bpp`), cleared to zero.
    pub fn new(width: usize, height: usize, format: Format) -> Result<FrameBuf, SurfaceError> {
        let stride = width.checked_mul(format.bpp).ok_or(SurfaceError::SizeOverflow)?;
        validate(width, height, stride, format)?;
        let words = (stride * height).div_ceil(4);
        let mut mem: Vec<u32> = Vec::new();
        mem.try_reserve_exact(words).map_err(|_| SurfaceError::OutOfMemory)?;
        mem.resize(words, 0);
        let ptr = mem.as_mut_ptr() as *mut u8;
        Ok(FrameBuf::make(ptr, Some(mem), width, height, stride, format))
    }

    /// An owned surface cleared to the opaque colour `color`.
    pub fn new_filled(width: usize, height: usize, format: Format, color: Color) -> Result<FrameBuf, SurfaceError> {
        let mut fb = FrameBuf::new(width, height, format)?;
        fb.clear(color);
        fb.dirty = None;
        Ok(fb)
    }

    /// Wrap memory the caller owns as an opaque surface.
    ///
    /// # Safety
    /// `base` must stay valid for reads and writes of `stride × height` bytes, and must not
    /// be accessed through another mutable path, for the surface's lifetime. Every numeric
    /// rule of section 3 is checked here.
    pub unsafe fn from_raw(base: *mut u8, width: usize, height: usize, stride: usize, bpp: usize, is_bgr: bool)
        -> Result<FrameBuf, SurfaceError> {
        unsafe { FrameBuf::from_raw_format(base, width, height, stride, Format { bpp, is_bgr, premultiplied: false }) }
    }

    /// As `from_raw`, with a full `Format` (premultiplied surfaces).
    ///
    /// # Safety
    /// As for `from_raw`.
    pub unsafe fn from_raw_format(base: *mut u8, width: usize, height: usize, stride: usize, format: Format)
        -> Result<FrameBuf, SurfaceError> {
        validate(width, height, stride, format)?;
        if base.is_null() {
            return Err(SurfaceError::Null);
        }
        if (base as usize) % 4 != 0 {
            return Err(SurfaceError::Misaligned);
        }
        Ok(FrameBuf::make(base, None, width, height, stride, format))
    }

    pub fn format(&self) -> Format {
        self.geom.format
    }

    pub(crate) fn dims(&self) -> (usize, usize) {
        (self.geom.w, self.geom.h)
    }

    // ---- the clip stack (section 4) -------------------------------------------------

    /// Set the clip to the intersection of `rect` and the current clip.
    pub fn push_clip(&mut self, rect: Rect) {
        self.clips.push(self.clip);
        self.clip = self.clip.intersect(&Box64::of(rect.x, rect.y, rect.w, rect.h));
    }

    /// Restore the clip from before the matching `push_clip`; nothing without one.
    pub fn pop_clip(&mut self) {
        if let Some(c) = self.clips.pop() {
            self.clip = c;
        }
    }

    /// The current clip (the whole surface when none is pushed).
    pub fn clip(&self) -> Rect {
        self.clip.rect()
    }

    pub(crate) fn clip_box(&self) -> Box64 {
        self.clip
    }

    // ---- damage (section 6) ---------------------------------------------------------

    /// Mark `(x, y, w, h)`, intersected with the surface and clip, as written.
    pub fn mark_dirty(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.mark(Box64::of(x, y, w, h));
    }

    pub(crate) fn mark(&mut self, b: Box64) {
        let b = b.intersect(&self.clip);
        if b.is_empty() {
            return;
        }
        self.dirty = Some(match self.dirty {
            Some(d) => d.union(&b),
            None => b,
        });
    }

    /// The union of everything written since the last call, and reset it to nothing.
    pub fn take_dirty(&mut self) -> Option<DirtyRect> {
        self.dirty.take().map(|d| d.rect())
    }

    // ---- pixels ---------------------------------------------------------------------

    /// The bytes of an opaque pixel of `c` in memory order, with X (or alpha) 0xFF.
    #[inline]
    pub(crate) fn opaque(&self, c: Color) -> [u8; 4] {
        if self.geom.format.is_bgr { [c.b, c.g, c.r, 0xFF] } else { [c.r, c.g, c.b, 0xFF] }
    }

    #[inline]
    fn offset(&self, x: usize, y: usize) -> usize {
        y * self.geom.stride + x * self.geom.format.bpp
    }

    /// Write `px` (memory order) at pixels `x0..x1` of row `y`, all inside the surface.
    #[inline]
    pub(crate) fn span(&mut self, y: usize, x0: usize, x1: usize, px: [u8; 4]) {
        debug_assert!(y < self.geom.h && x1 <= self.geom.w);
        if x0 >= x1 {
            return;
        }
        work::add((x1 - x0) as u64);
        let bpp = self.geom.format.bpp;
        // SAFETY: (x0..x1, y) lies inside the surface, whose memory the constructor vouched for.
        unsafe {
            let row = self.ptr.add(self.offset(x0, y));
            if bpp == 4 && (row as usize) % 4 == 0 {
                let word = u32::from_ne_bytes(px);
                let p = row as *mut u32;
                for i in 0..x1 - x0 {
                    *p.add(i) = word;
                }
            } else {
                for i in 0..x1 - x0 {
                    let p = row.add(i * bpp);
                    *p = px[0];
                    *p.add(1) = px[1];
                    *p.add(2) = px[2];
                    if bpp == 4 {
                        *p.add(3) = px[3];
                    }
                }
            }
        }
    }

    /// Write `px` at (x, y), which the caller has clipped.
    #[inline]
    pub(crate) fn write(&mut self, x: usize, y: usize, px: [u8; 4]) {
        debug_assert!(x < self.geom.w && y < self.geom.h);
        let o = self.offset(x, y);
        // SAFETY: (x, y) lies inside the surface.
        unsafe {
            let p = self.ptr.add(o);
            *p = px[0];
            *p.add(1) = px[1];
            *p.add(2) = px[2];
            if self.geom.format.bpp == 4 {
                *p.add(3) = px[3];
            }
        }
    }

    /// The bytes at (x, y) in memory order; the fourth is 0xFF on a 3-byte surface.
    #[inline]
    pub(crate) fn read(&self, x: usize, y: usize) -> [u8; 4] {
        debug_assert!(x < self.geom.w && y < self.geom.h);
        let o = self.offset(x, y);
        // SAFETY: (x, y) lies inside the surface.
        unsafe {
            let p = self.ptr.add(o);
            [*p, *p.add(1), *p.add(2), if self.geom.format.bpp == 4 { *p.add(3) } else { 0xFF }]
        }
    }

    /// Whether (x, y) is inside the surface and the clip.
    #[inline]
    pub(crate) fn visible(&self, x: i64, y: i64) -> bool {
        x >= self.clip.x0 && x < self.clip.x1 && y >= self.clip.y0 && y < self.clip.y1
    }

    /// Write one opaque pixel inside the surface and clip.
    #[inline]
    pub fn put_pixel(&mut self, x: usize, y: usize, color: Color) {
        let (xi, yi) = (x as i64, y as i64);
        if x > i64::MAX as usize || y > i64::MAX as usize || !self.visible(xi, yi) {
            return;
        }
        let px = self.opaque(color);
        self.write(x, y, px);
        self.mark(Box64 { x0: xi, y0: yi, x1: xi + 1, y1: yi + 1 });
    }

    /// Read one pixel's colour; black outside the surface.
    #[inline]
    pub fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x >= self.geom.w || y >= self.geom.h {
            return Color::rgb(0, 0, 0);
        }
        let p = self.read(x, y);
        if self.geom.format.is_bgr { Color::rgb(p[2], p[1], p[0]) } else { Color::rgb(p[0], p[1], p[2]) }
    }

    /// The pixel at (x, y) as `[b, g, r, a]`: alpha from a premultiplied surface, 0xFF on an
    /// opaque one; all zero outside the surface.
    #[inline]
    pub fn get_bgra(&self, x: usize, y: usize) -> [u8; 4] {
        if x >= self.geom.w || y >= self.geom.h {
            return [0; 4];
        }
        let p = self.read(x, y);
        let a = if self.geom.format.premultiplied { p[3] } else { 0xFF };
        if self.geom.format.is_bgr { [p[0], p[1], p[2], a] } else { [p[2], p[1], p[0], a] }
    }

    /// Store `[b, g, r, a]` at (x, y) inside the surface and clip: the alpha byte on a
    /// premultiplied surface, X = 0xFF on an opaque one.
    #[inline]
    pub fn put_bgra(&mut self, x: usize, y: usize, bgra: [u8; 4]) {
        let (xi, yi) = (x as i64, y as i64);
        if x > i64::MAX as usize || y > i64::MAX as usize || !self.visible(xi, yi) {
            return;
        }
        let px = self.encode(bgra);
        self.write(x, y, px);
        self.mark(Box64 { x0: xi, y0: yi, x1: xi + 1, y1: yi + 1 });
    }

    /// `[b, g, r, a]` in this surface's memory order.
    #[inline]
    pub(crate) fn encode(&self, [b, g, r, a]: [u8; 4]) -> [u8; 4] {
        let a = if self.geom.format.premultiplied { a } else { 0xFF };
        if self.geom.format.is_bgr { [b, g, r, a] } else { [r, g, b, a] }
    }

    /// Fill the current clip (the whole surface when none is pushed) with `color`, never the
    /// row padding. Marks the clip.
    pub fn clear(&mut self, color: Color) {
        let c = self.clip;
        if c.is_empty() {
            return;
        }
        let px = self.opaque(color);
        work::add(c.height() as u64);
        for y in c.y0..c.y1 {
            self.span(y as usize, c.x0 as usize, c.x1 as usize, px);
        }
        self.mark(c);
    }

    /// Copy `min(byte_count, src.len(), (width − x) × bpp)` bytes from `src` to pixel
    /// (x, y), never past the end of row `y`, and only into the clip; nothing when
    /// `x ≥ width` or `y ≥ height`.
    pub fn copy_row_from(&mut self, x: usize, y: usize, src: &[u8], byte_count: usize) {
        let (w, h, bpp) = (self.geom.w, self.geom.h, self.geom.format.bpp);
        if x >= w || y >= h {
            return;
        }
        let k = byte_count.min(src.len()).min((w - x) * bpp);
        if k == 0 {
            return;
        }
        let (c, yi) = (self.clip, y as i64);
        if yi < c.y0 || yi >= c.y1 {
            return;
        }
        // The bytes [x·bpp, x·bpp + k) of the row, cut to the clip's columns.
        let start = (x * bpp).max(c.x0 as usize * bpp);
        let end = (x * bpp + k).min(c.x1 as usize * bpp);
        if start >= end {
            return;
        }
        work::add((end - start) as u64);
        // SAFETY: [start, end) lies inside row y's pixel bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr().add(start - x * bpp), self.ptr.add(y * self.geom.stride + start), end - start);
        }
        let xi = x as i64;
        self.mark(Box64 { x0: xi, y0: yi, x1: xi + k.div_ceil(bpp) as i64, y1: yi + 1 });
    }

    /// Raw pointer to the pixel memory (row `y` starts at `y × stride`).
    #[inline]
    pub fn raw_ptr(&self) -> *mut u8 {
        self.ptr
    }
}
