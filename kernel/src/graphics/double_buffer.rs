//! Double-buffered framebuffer implementation.
//!
//! Provides a shadow buffer for off-screen rendering with page flipping support.
//! All rendering happens to the shadow buffer, then `flush()` copies to hardware.

use alloc::vec::Vec;
use core::ptr;

const MAX_DIRTY_RECTS: usize = 4;
const MERGE_PROXIMITY: usize = 32;

/// Represents a rectangular region that has been modified.
///
/// Coordinates are byte offsets on each scanline.
#[derive(Debug, Clone, Copy)]
pub struct DirtyRect {
    /// X coordinate of top-left corner (in bytes, inclusive)
    pub x_start: usize,
    /// Y coordinate of top-left corner (in scanlines, inclusive)
    pub y_start: usize,
    /// X coordinate of bottom-right corner (in bytes, exclusive)
    pub x_end: usize,
    /// Y coordinate of bottom-right corner (in scanlines, exclusive)
    pub y_end: usize,
}

impl DirtyRect {
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
    #[allow(dead_code)] // Used for testing DirtyRect in isolation
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    fn union(&self, other: &Self) -> Self {
        Self {
            x_start: self.x_start.min(other.x_start),
            y_start: self.y_start.min(other.y_start),
            x_end: self.x_end.max(other.x_end),
            y_end: self.y_end.max(other.y_end),
        }
    }

    fn distance_to(&self, other: &Self) -> usize {
        let gap_x = if self.x_end < other.x_start {
            other.x_start - self.x_end
        } else if other.x_end < self.x_start {
            self.x_start - other.x_end
        } else {
            0
        };

        let gap_y = if self.y_end < other.y_start {
            other.y_start - self.y_end
        } else if other.y_end < self.y_start {
            self.y_start - other.y_end
        } else {
            0
        };

        gap_x.max(gap_y)
    }

    fn should_merge(&self, other: &Self) -> bool {
        if self.is_empty() || other.is_empty() {
            return false;
        }

        self.distance_to(other) <= MERGE_PROXIMITY
    }
}

/// Track multiple dirty rectangles for incremental flushes.
pub struct DirtyRegionTracker {
    rects: [Option<DirtyRect>; MAX_DIRTY_RECTS],
    count: usize,
}

impl DirtyRegionTracker {
    pub fn new() -> Self {
        Self {
            rects: [None; MAX_DIRTY_RECTS],
            count: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn clear(&mut self) {
        self.rects = [None; MAX_DIRTY_RECTS];
        self.count = 0;
    }

    pub fn rects(&self) -> impl Iterator<Item = DirtyRect> + '_ {
        self.rects.iter().filter_map(|rect| *rect)
    }

    pub fn mark_dirty(&mut self, y: usize, x_start: usize, x_end: usize) {
        if x_start >= x_end {
            return;
        }

        let mut new_rect = DirtyRect::new();
        new_rect.mark_dirty(y, x_start, x_end);

        self.absorb_merges(&mut new_rect);

        if self.count == MAX_DIRTY_RECTS {
            self.merge_closest_pair();
            self.absorb_merges(&mut new_rect);
        }

        self.insert(new_rect);
    }

    /// Mark an entire rectangular region as dirty in one operation.
    /// Much more efficient than calling mark_dirty() for each row.
    pub fn mark_rect_dirty(&mut self, y_start: usize, y_end: usize, x_start: usize, x_end: usize) {
        if x_start >= x_end || y_start >= y_end {
            return;
        }

        let mut new_rect = DirtyRect {
            x_start,
            x_end,
            y_start,
            y_end,
        };

        self.absorb_merges(&mut new_rect);

        if self.count == MAX_DIRTY_RECTS {
            self.merge_closest_pair();
            self.absorb_merges(&mut new_rect);
        }

        self.insert(new_rect);
    }

    fn absorb_merges(&mut self, rect: &mut DirtyRect) {
        loop {
            let mut merged_any = false;
            for slot in self.rects.iter_mut() {
                if let Some(existing) = *slot {
                    if existing.should_merge(rect) {
                        *rect = existing.union(rect);
                        *slot = None;
                        self.count -= 1;
                        merged_any = true;
                    }
                }
            }
            if !merged_any {
                break;
            }
        }
    }

    fn insert(&mut self, rect: DirtyRect) {
        if rect.is_empty() {
            return;
        }

        for slot in self.rects.iter_mut() {
            if slot.is_none() {
                *slot = Some(rect);
                self.count += 1;
                return;
            }
        }
    }

    fn merge_closest_pair(&mut self) {
        if self.count < 2 {
            return;
        }

        let mut best: Option<(usize, usize, usize)> = None;
        for i in 0..MAX_DIRTY_RECTS {
            let Some(rect_i) = self.rects[i] else {
                continue;
            };
            for j in (i + 1)..MAX_DIRTY_RECTS {
                let Some(rect_j) = self.rects[j] else {
                    continue;
                };
                let distance = rect_i.distance_to(&rect_j);
                let replace = best.map_or(true, |(best_distance, _, _)| distance < best_distance);
                if replace {
                    best = Some((distance, i, j));
                }
            }
        }

        if let Some((_, i, j)) = best {
            if let (Some(rect_i), Some(rect_j)) = (self.rects[i], self.rects[j]) {
                self.rects[i] = Some(rect_i.union(&rect_j));
                self.rects[j] = None;
                self.count -= 1;
            }
        }
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
    /// Track dirty rectangles for incremental flushes
    dirty_regions: DirtyRegionTracker,
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
            dirty_regions: DirtyRegionTracker::new(),
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
        if !self.dirty || self.dirty_regions.is_empty() {
            self.dirty = false;
            self.dirty_regions.clear();
            return;
        }

        let max_len = self.hardware_len.min(self.shadow_buffer.len());
        if max_len == 0 {
            self.dirty = false;
            self.dirty_regions.clear();
            return;
        }

        for rect in self.dirty_regions.rects() {
            let y_start = rect.y_start.min(self.height);
            let y_end = rect.y_end.min(self.height);
            let x_start = rect.x_start.min(self.stride);
            let x_end = rect.x_end.min(self.stride);

            if y_start >= y_end || x_start >= x_end {
                continue;
            }

            // Fast path: if dirty rect spans full width, copy entire block at once
            if x_start == 0 && x_end == self.stride {
                let start_offset = y_start * self.stride;
                let total_len = (y_end - y_start) * self.stride;
                if start_offset + total_len <= max_len {
                    // SAFETY: hardware_ptr is valid for hardware_len bytes (from bootloader),
                    // shadow_buffer is valid for its length, and we copy the minimum of both.
                    unsafe {
                        let src = self.shadow_buffer.as_ptr().add(start_offset);
                        let dst = self.hardware_ptr.add(start_offset);
                        ptr::copy_nonoverlapping(src, dst, total_len);
                    }
                    continue;
                }
            }

            // Slow path: partial width, need per-row copies
            let row_len = x_end - x_start;
            if row_len == 0 {
                continue;
            }

            for y in y_start..y_end {
                let row_offset = y * self.stride;
                let src_start = row_offset + x_start;
                if src_start + row_len > max_len {
                    continue;
                }

                // SAFETY: hardware_ptr is valid for hardware_len bytes (from bootloader),
                // shadow_buffer is valid for its length, and we copy the minimum of both.
                unsafe {
                    let src = self.shadow_buffer.as_ptr().add(src_start);
                    let dst = self.hardware_ptr.add(src_start);
                    ptr::copy_nonoverlapping(src, dst, row_len);
                }
            }
        }
        self.dirty = false;
        self.dirty_regions.clear();
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
        self.dirty_regions.clear();
    }

    /// Mark a rectangular region as dirty (in byte coordinates).
    pub fn mark_region_dirty(&mut self, y: usize, x_start: usize, x_end: usize) {
        self.dirty = true;
        self.dirty_regions.mark_dirty(y, x_start, x_end);
    }

    /// Mark an entire rectangular region as dirty in one operation.
    /// Much more efficient than calling mark_region_dirty() for each row.
    pub fn mark_region_dirty_rect(&mut self, y_start: usize, y_end: usize, x_start: usize, x_end: usize) {
        self.dirty = true;
        self.dirty_regions.mark_rect_dirty(y_start, y_end, x_start, x_end);
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
        let rect = DirtyRect::new();
        assert!(rect.is_empty());
    }

    #[test]
    fn dirty_region_mark_expands() {
        let mut rect = DirtyRect::new();
        rect.mark_dirty(2, 4, 8);
        assert!(!rect.is_empty());
        assert_eq!(rect.x_start, 4);
        assert_eq!(rect.x_end, 8);
        assert_eq!(rect.y_start, 2);
        assert_eq!(rect.y_end, 3);
    }

    #[test]
    fn dirty_region_mark_unions() {
        let mut rect = DirtyRect::new();
        rect.mark_dirty(2, 4, 8);
        rect.mark_dirty(1, 2, 6);
        assert_eq!(rect.x_start, 2);
        assert_eq!(rect.x_end, 8);
        assert_eq!(rect.y_start, 1);
        assert_eq!(rect.y_end, 3);
    }

    #[test]
    fn dirty_region_clear_resets() {
        let mut rect = DirtyRect::new();
        rect.mark_dirty(0, 1, 2);
        rect.clear();
        assert!(rect.is_empty());
    }

    #[test]
    fn dirty_region_tracker_keeps_separate_rects() {
        let mut tracker = DirtyRegionTracker::new();
        tracker.mark_dirty(0, 0, 4);
        tracker.mark_dirty(MERGE_PROXIMITY + 8, 0, 4);
        assert_eq!(tracker.rects().count(), 2);
    }

    #[test]
    fn dirty_region_tracker_merges_close_rects() {
        let mut tracker = DirtyRegionTracker::new();
        tracker.mark_dirty(0, 0, 4);
        tracker.mark_dirty(0, MERGE_PROXIMITY / 2, MERGE_PROXIMITY / 2 + 4);
        assert_eq!(tracker.rects().count(), 1);
        let rect = tracker.rects().next().unwrap();
        assert_eq!(rect.x_start, 0);
        assert_eq!(rect.x_end, MERGE_PROXIMITY / 2 + 4);
    }

    #[test]
    fn dirty_region_tracker_merges_overlapping_rects() {
        let mut tracker = DirtyRegionTracker::new();
        tracker.mark_dirty(0, 0, 6);
        tracker.mark_dirty(0, 4, 10);
        assert_eq!(tracker.rects().count(), 1);
        let rect = tracker.rects().next().unwrap();
        assert_eq!(rect.x_start, 0);
        assert_eq!(rect.x_end, 10);
    }

    #[test]
    fn dirty_region_tracker_merges_closest_when_full() {
        let mut tracker = DirtyRegionTracker::new();
        tracker.mark_dirty(0, 0, 2);
        tracker.mark_dirty(MERGE_PROXIMITY + 8, 0, 2);
        tracker.mark_dirty(3 * MERGE_PROXIMITY + 8, 0, 2);
        tracker.mark_dirty(5 * MERGE_PROXIMITY + 8, 0, 2);
        assert_eq!(tracker.rects().count(), MAX_DIRTY_RECTS);

        tracker.mark_dirty(7 * MERGE_PROXIMITY + 8, 0, 2);
        assert_eq!(tracker.rects().count(), MAX_DIRTY_RECTS);
        let merged = tracker
            .rects()
            .any(|rect| rect.y_start == 0 && rect.y_end == MERGE_PROXIMITY + 9);
        assert!(merged);
    }

    #[test]
    fn double_buffer_new_not_dirty() {
        let mut buf = [0u8; 100];
        let db = DoubleBufferedFrameBuffer::new(buf.as_mut_ptr(), buf.len(), 10, 10);
        assert!(!db.dirty);
        assert!(db.dirty_regions.is_empty());
    }

    #[test]
    fn double_buffer_mark_region_sets_dirty() {
        let mut buf = [0u8; 100];
        let mut db = DoubleBufferedFrameBuffer::new(buf.as_mut_ptr(), buf.len(), 10, 10);
        db.mark_region_dirty(1, 2, 4);
        assert!(db.dirty);
        assert!(!db.dirty_regions.is_empty());
    }

    #[test]
    fn double_buffer_flush_clears_dirty() {
        let mut buf = [0u8; 100];
        let mut db = DoubleBufferedFrameBuffer::new(buf.as_mut_ptr(), buf.len(), 10, 10);
        db.mark_region_dirty(1, 0, 2);
        db.flush();
        assert!(!db.dirty);
        assert!(db.dirty_regions.is_empty());
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
