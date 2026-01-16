//! Double-buffered framebuffer implementation.
//!
//! Provides a shadow buffer for off-screen rendering with page flipping support.
//! All rendering happens to the shadow buffer, then `flush()` copies to hardware.

use alloc::vec::Vec;
use core::ptr;

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
}

impl DoubleBufferedFrameBuffer {
    /// Create a new double-buffered framebuffer.
    ///
    /// Allocates a shadow buffer on the heap that mirrors the hardware framebuffer.
    ///
    /// # Arguments
    /// * `hardware_ptr` - Pointer to the hardware framebuffer memory
    /// * `hardware_len` - Length of the hardware buffer in bytes
    pub fn new(hardware_ptr: *mut u8, hardware_len: usize) -> Self {
        let mut shadow_buffer = Vec::with_capacity(hardware_len);
        shadow_buffer.resize(hardware_len, 0);

        Self {
            hardware_ptr,
            hardware_len,
            shadow_buffer,
            dirty: false,
        }
    }

    /// Get mutable access to the shadow buffer for rendering.
    #[inline]
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        &mut self.shadow_buffer
    }

    /// Copy the shadow buffer to the hardware framebuffer.
    ///
    /// This is the "page flip" operation that makes rendered content visible.
    pub fn flush(&mut self) {
        let len = self.hardware_len.min(self.shadow_buffer.len());
        if len == 0 {
            self.dirty = false;
            return;
        }

        // SAFETY: hardware_ptr is valid for hardware_len bytes (from bootloader),
        // shadow_buffer is valid for its length, and we copy the minimum of both.
        unsafe {
            ptr::copy_nonoverlapping(self.shadow_buffer.as_ptr(), self.hardware_ptr, len);
        }
        self.dirty = false;
    }

    /// Mark the shadow buffer as modified.
    ///
    /// Call this after writing to the buffer to track that a flush is needed.
    #[inline]
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Flush only if the buffer has been modified since the last flush.
    #[inline]
    pub fn flush_if_dirty(&mut self) {
        if self.dirty {
            self.flush();
        }
    }
}

// SAFETY: The hardware_ptr is only accessed during flush(), which requires &mut self.
// The shadow_buffer is a standard Vec which is Send.
unsafe impl Send for DoubleBufferedFrameBuffer {}

// SAFETY: All access to internal state requires &mut self, so there's no data race risk.
// The Mutex wrapper in SHELL_FRAMEBUFFER provides the actual synchronization.
unsafe impl Sync for DoubleBufferedFrameBuffer {}
