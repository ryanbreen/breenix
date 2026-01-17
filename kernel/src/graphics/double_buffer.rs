//! Double-buffered framebuffer implementation.
//!
//! Provides a shadow buffer for off-screen rendering with page flipping support.
//! All rendering happens to the shadow buffer, then `flush()` copies to hardware.

use alloc::vec::Vec;
use core::ptr;

/// Represents a rectangular region that has been modified.
///
/// Coordinates are byte offsets on each scanline.
#[derive(Debug, Clone, Copy)]
pub struct DirtyRegion {
    /// X coordinate of top-left corner (in bytes, inclusive)
    pub x_start: usize,
    /// Y coordinate of top-left corner (in scanlines, inclusive)
    pub y_start: usize,
    /// X coordinate of bottom-right corner (in bytes, exclusive)
    pub x_end: usize,
    /// Y coordinate of bottom-right corner (in scanlines, exclusive)
    pub y_end: usize,
}

impl DirtyRegion {
    pub fn new() -> Self {
        Self {
            x_start: usize::MAX,
            y_start: usize::MAX,
            x_end: 0,
            y_end: 0,
        }
    }

    /// Check if region is empty (nothing dirty).
    pub fn is_empty(&self) -> bool {
        self.x_start >= self.x_end || self.y_start >= self.y_end
    }

    /// Expand region to include a byte range on a scanline.
    pub fn mark_dirty(&mut self, y: usize, x_start: usize, x_end: usize) {
        self.x_start = self.x_start.min(x_start);
        self.x_end = self.x_end.max(x_end);
        self.y_start = self.y_start.min(y);
        self.y_end = self.y_end.max(y.saturating_add(1));
    }

    /// Reset to empty.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

/// Double-buffered framebuffer for tear-free rendering.
///
/// Maintains a shadow buffer in heap memory that mirrors the hardware framebuffer.
/// All writes go to the shadow buffer, and `flush()` copies to the hardware buffer.
pub struct DoubleBufferedFrameBuffer {
    /// Pointer to hardware framebuffer memory (from bootloader)
    hardware_ptr: *mut u8,
    /// Length of hardware buffer in bytes
    hardware_len: usize,
    /// Shadow buffer for off-screen rendering (heap allocated)
    shadow_buffer: Vec<u8>,
    /// Track if shadow buffer has been modified since last flush
    dirty: bool,
    /// Track the bounding box of modified regions
    dirty_region: DirtyRegion,
    /// Bytes per scanline
    stride: usize,
    /// Number of scanlines
    height: usize,
}

impl DoubleBufferedFrameBuffer {
    /// Create a new double-buffered framebuffer.
    ///
    /// Allocates a shadow buffer on the heap that mirrors the hardware framebuffer.
    ///
    /// # Arguments
    /// * `hardware_ptr` - Pointer to the hardware framebuffer memory
    /// * `hardware_len` - Length of the hardware buffer in bytes
    /// * `stride` - Bytes per scanline
    /// * `height` - Number of scanlines
    pub fn new(hardware_ptr: *mut u8, hardware_len: usize, stride: usize, height: usize) -> Self {
        let mut shadow_buffer = Vec::with_capacity(hardware_len);
        shadow_buffer.resize(hardware_len, 0);

        Self {
            hardware_ptr,
            hardware_len,
            shadow_buffer,
            dirty: false,
            dirty_region: DirtyRegion::new(),
            stride,
            height,
        }
    }

    /// Get mutable access to the shadow buffer for rendering.
    #[inline]
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        &mut self.shadow_buffer
    }

    /// Get read-only access to the shadow buffer.
    #[inline]
    pub fn buffer(&self) -> &[u8] {
        &self.shadow_buffer
    }

    /// Copy the shadow buffer to the hardware framebuffer.
    ///
    /// This is the "page flip" operation that makes rendered content visible.
    pub fn flush(&mut self) {
        if !self.dirty || self.dirty_region.is_empty() {
            self.dirty = false;
            self.dirty_region.clear();
            return;
        }

        let y_start = self.dirty_region.y_start.min(self.height);
        let y_end = self.dirty_region.y_end.min(self.height);
        let x_start = self.dirty_region.x_start.min(self.stride);
        let x_end = self.dirty_region.x_end.min(self.stride);
        let max_len = self.hardware_len.min(self.shadow_buffer.len());

        if y_start >= y_end || x_start >= x_end || max_len == 0 {
            self.dirty = false;
            self.dirty_region.clear();
            return;
        }

        for y in y_start..y_end {
            let row_offset = y * self.stride;
            let src_start = row_offset + x_start;
            let src_end = row_offset + x_end;
            if src_end > max_len {
                continue;
            }

            let len = x_end - x_start;
            if len == 0 {
                continue;
            }

            // SAFETY: hardware_ptr is valid for hardware_len bytes (from bootloader),
            // shadow_buffer is valid for its length, and we copy the minimum of both.
            unsafe {
                let src = self.shadow_buffer.as_ptr().add(src_start);
                let dst = self.hardware_ptr.add(src_start);
                ptr::copy_nonoverlapping(src, dst, len);
            }
        }
        self.dirty = false;
        self.dirty_region.clear();
    }

    /// Force a full buffer flush (used for clear operations).
    pub fn flush_full(&mut self) {
        let len = self.hardware_len.min(self.shadow_buffer.len());
        if len > 0 {
            // SAFETY: hardware_ptr is valid for hardware_len bytes (from bootloader),
            // shadow_buffer is valid for its length, and we copy the minimum of both.
            unsafe {
                ptr::copy_nonoverlapping(self.shadow_buffer.as_ptr(), self.hardware_ptr, len);
            }
        }
        self.dirty = false;
        self.dirty_region.clear();
    }

    /// Mark a rectangular region as dirty (in byte coordinates).
    pub fn mark_region_dirty(&mut self, y: usize, x_start: usize, x_end: usize) {
        self.dirty = true;
        self.dirty_region.mark_dirty(y, x_start, x_end);
    }

    /// Flush only if the buffer has been modified since the last flush.
    #[inline]
    pub fn flush_if_dirty(&mut self) {
        if self.dirty {
            self.flush();
        }
    }

    /// Shift hardware buffer up by the given byte count.
    ///
    /// Assumes the shadow buffer has already been scrolled the same way.
    pub fn scroll_hardware_up(&mut self, scroll_bytes: usize) {
        let len = self.hardware_len.min(self.shadow_buffer.len());
        if scroll_bytes >= len {
            return;
        }

        // SAFETY: hardware_ptr is valid for hardware_len bytes. ptr::copy handles overlap.
        unsafe {
            let src = self.hardware_ptr.add(scroll_bytes);
            ptr::copy(src, self.hardware_ptr, len - scroll_bytes);
        }
    }
}

// SAFETY: The hardware_ptr is only accessed during flush(), which requires &mut self.
// The shadow_buffer is a standard Vec which is Send.
unsafe impl Send for DoubleBufferedFrameBuffer {}

// SAFETY: All access to internal state requires &mut self, so there's no data race risk.
// The Mutex wrapper in SHELL_FRAMEBUFFER provides the actual synchronization.
unsafe impl Sync for DoubleBufferedFrameBuffer {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_region_new_is_empty() {
        let region = DirtyRegion::new();
        assert!(region.is_empty());
    }

    #[test]
    fn dirty_region_mark_expands() {
        let mut region = DirtyRegion::new();
        region.mark_dirty(2, 4, 8);
        assert!(!region.is_empty());
        assert_eq!(region.x_start, 4);
        assert_eq!(region.x_end, 8);
        assert_eq!(region.y_start, 2);
        assert_eq!(region.y_end, 3);
    }

    #[test]
    fn dirty_region_mark_unions() {
        let mut region = DirtyRegion::new();
        region.mark_dirty(2, 4, 8);
        region.mark_dirty(1, 2, 6);
        assert_eq!(region.x_start, 2);
        assert_eq!(region.x_end, 8);
        assert_eq!(region.y_start, 1);
        assert_eq!(region.y_end, 3);
    }

    #[test]
    fn dirty_region_clear_resets() {
        let mut region = DirtyRegion::new();
        region.mark_dirty(0, 1, 2);
        region.clear();
        assert!(region.is_empty());
    }

    #[test]
    fn double_buffer_new_not_dirty() {
        let mut buf = [0u8; 100];
        let db = DoubleBufferedFrameBuffer::new(buf.as_mut_ptr(), buf.len(), 10, 10);
        assert!(!db.dirty);
        assert!(db.dirty_region.is_empty());
    }

    #[test]
    fn double_buffer_mark_region_sets_dirty() {
        let mut buf = [0u8; 100];
        let mut db = DoubleBufferedFrameBuffer::new(buf.as_mut_ptr(), buf.len(), 10, 10);
        db.mark_region_dirty(1, 2, 4);
        assert!(db.dirty);
        assert!(!db.dirty_region.is_empty());
    }

    #[test]
    fn double_buffer_flush_clears_dirty() {
        let mut buf = [0u8; 100];
        let mut db = DoubleBufferedFrameBuffer::new(buf.as_mut_ptr(), buf.len(), 10, 10);
        db.mark_region_dirty(1, 0, 2);
        db.flush();
        assert!(!db.dirty);
        assert!(db.dirty_region.is_empty());
    }

    #[test]
    fn double_buffer_flush_copies_dirty_bytes() {
        let mut hw_buf = [0u8; 100];
        let mut db = DoubleBufferedFrameBuffer::new(hw_buf.as_mut_ptr(), hw_buf.len(), 10, 10);

        let shadow = db.buffer_mut();
        shadow[23] = 0xAA;
        shadow[24] = 0xBB;
        shadow[25] = 0xCC;

        db.mark_region_dirty(2, 3, 6);
        db.flush();

        assert_eq!(hw_buf[23], 0xAA);
        assert_eq!(hw_buf[24], 0xBB);
        assert_eq!(hw_buf[25], 0xCC);
    }

    #[test]
    fn double_buffer_flush_only_copies_dirty_region() {
        let mut hw_buf = [0u8; 100];
        let mut db = DoubleBufferedFrameBuffer::new(hw_buf.as_mut_ptr(), hw_buf.len(), 10, 10);

        let shadow = db.buffer_mut();
        shadow[5] = 0x11;
        shadow[23] = 0xAA;
        shadow[45] = 0x22;

        db.mark_region_dirty(2, 3, 4);
        db.flush();

        assert_eq!(hw_buf[23], 0xAA);
        assert_eq!(hw_buf[5], 0x00, "Row 0 should not be touched");
        assert_eq!(hw_buf[45], 0x00, "Row 4 should not be touched");
    }

    #[test]
    fn double_buffer_flush_full_copies_everything() {
        let mut hw_buf = [0u8; 100];
        let mut db = DoubleBufferedFrameBuffer::new(hw_buf.as_mut_ptr(), hw_buf.len(), 10, 10);

        let shadow = db.buffer_mut();
        shadow[5] = 0x11;
        shadow[50] = 0x22;
        shadow[95] = 0x33;

        db.flush_full();

        assert_eq!(hw_buf[5], 0x11);
        assert_eq!(hw_buf[50], 0x22);
        assert_eq!(hw_buf[95], 0x33);
    }

    #[test]
    fn double_buffer_coordinate_interpretation() {
        let mut hw_buf = [0u8; 100];
        let mut db = DoubleBufferedFrameBuffer::new(hw_buf.as_mut_ptr(), hw_buf.len(), 10, 10);

        let shadow = db.buffer_mut();
        shadow[52] = 0xDE;
        shadow[53] = 0xAD;
        shadow[54] = 0xBE;

        db.mark_region_dirty(5, 2, 5);
        db.flush();

        assert_eq!(hw_buf[52], 0xDE);
        assert_eq!(hw_buf[53], 0xAD);
        assert_eq!(hw_buf[54], 0xBE);
        assert_eq!(hw_buf[2], 0x00, "Row 0 col 2 should not be touched");
    }

    #[test]
    fn double_buffer_scroll_hardware_up() {
        let mut hw_buf = [0u8; 100];
        for (idx, byte) in hw_buf.iter_mut().enumerate() {
            *byte = idx as u8;
        }

        let mut db = DoubleBufferedFrameBuffer::new(hw_buf.as_mut_ptr(), hw_buf.len(), 10, 10);
        db.scroll_hardware_up(10);

        assert_eq!(hw_buf[0], 10);
        assert_eq!(hw_buf[9], 19);
        assert_eq!(hw_buf[80], 90);
    }
}
