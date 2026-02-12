//! Framebuffer wrapper with dirty-rectangle tracking.

use crate::color::Color;

/// A dirty rectangle (pixel coordinates).
#[derive(Clone, Copy, Debug)]
pub struct DirtyRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A raw pixel buffer with dimensions, pixel format, and dirty tracking.
///
/// Does not own the buffer memory — callers provide a pointer (e.g. from mmap).
/// Tracks which region has been modified so that only the dirty rect needs flushing.
pub struct FrameBuf {
    ptr: *mut u8,
    pub width: usize,
    pub height: usize,
    pub stride: usize, // bytes per row
    pub bpp: usize,    // bytes per pixel (3 or 4)
    pub is_bgr: bool,
    dirty: Option<DirtyRect>,
}

impl FrameBuf {
    /// Create a FrameBuf from a raw pointer and dimensions.
    ///
    /// # Safety
    /// The caller must ensure `ptr` points to a valid buffer of at least
    /// `stride * height` bytes that remains valid for the lifetime of this FrameBuf.
    pub unsafe fn from_raw(
        ptr: *mut u8,
        width: usize,
        height: usize,
        stride: usize,
        bpp: usize,
        is_bgr: bool,
    ) -> Self {
        Self {
            ptr,
            width,
            height,
            stride,
            bpp,
            is_bgr,
            dirty: None,
        }
    }

    /// Write a single pixel. Expands the dirty rect.
    #[inline]
    pub fn put_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let off = y * self.stride + x * self.bpp;
        let (c0, c1, c2) = if self.is_bgr {
            (color.b, color.g, color.r)
        } else {
            (color.r, color.g, color.b)
        };
        unsafe {
            *self.ptr.add(off) = c0;
            *self.ptr.add(off + 1) = c1;
            *self.ptr.add(off + 2) = c2;
            if self.bpp == 4 {
                *self.ptr.add(off + 3) = 0;
            }
        }
        self.expand_dirty(x as i32, y as i32, 1, 1);
    }

    /// Fill the entire buffer with a solid color. Marks the full buffer dirty.
    pub fn clear(&mut self, color: Color) {
        let buf = unsafe { core::slice::from_raw_parts_mut(self.ptr, self.stride * self.height) };
        let (c0, c1, c2) = if self.is_bgr {
            (color.b, color.g, color.r)
        } else {
            (color.r, color.g, color.b)
        };
        // Fill first row
        if self.bpp == 4 {
            for x in 0..self.width {
                let o = x * 4;
                buf[o] = c0;
                buf[o + 1] = c1;
                buf[o + 2] = c2;
                buf[o + 3] = 0;
            }
        } else {
            for x in 0..self.width {
                let o = x * 3;
                buf[o] = c0;
                buf[o + 1] = c1;
                buf[o + 2] = c2;
            }
        }
        // Copy first row to all remaining rows
        for y in 1..self.height {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buf.as_ptr(),
                    buf.as_mut_ptr().add(y * self.stride),
                    self.stride,
                );
            }
        }
        self.dirty = Some(DirtyRect {
            x: 0,
            y: 0,
            w: self.width as i32,
            h: self.height as i32,
        });
    }

    /// Expand the dirty rect to include a new region.
    pub fn mark_dirty(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.expand_dirty(x, y, w, h);
    }

    /// Take and reset the current dirty rect. Returns `None` if nothing was drawn.
    pub fn take_dirty(&mut self) -> Option<DirtyRect> {
        self.dirty.take()
    }

    fn expand_dirty(&mut self, x: i32, y: i32, w: i32, h: i32) {
        match self.dirty {
            Some(ref mut d) => {
                let x2 = (d.x + d.w).max(x + w);
                let y2 = (d.y + d.h).max(y + h);
                d.x = d.x.min(x);
                d.y = d.y.min(y);
                d.w = x2 - d.x;
                d.h = y2 - d.y;
            }
            None => {
                self.dirty = Some(DirtyRect { x, y, w, h });
            }
        }
    }

    /// Raw pointer to the buffer (for direct scanline writes in shapes).
    #[inline]
    pub fn raw_ptr(&self) -> *mut u8 {
        self.ptr
    }
}
