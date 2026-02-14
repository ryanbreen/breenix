//! ARM64 Framebuffer implementation using VirtIO GPU
//!
//! Provides a Canvas implementation for the VirtIO GPU framebuffer,
//! enabling the graphics primitives to work on ARM64.
//!
//! This module also provides a SHELL_FRAMEBUFFER interface compatible with
//! the x86_64 version in logger.rs, allowing split_screen.rs
//! to work on both architectures.

#![cfg(target_arch = "aarch64")]

use super::primitives::{Canvas, Color};
use crate::drivers::virtio::gpu_mmio;
use conquer_once::spin::OnceCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

// =============================================================================
// Dirty Rect Tracking (lock-free, used to decouple pixel writes from GPU flush)
// =============================================================================

/// Whether any region has been modified since the last flush.
static FB_DIRTY: AtomicBool = AtomicBool::new(false);
/// Dirty rect left edge (minimum x).
static DIRTY_X_MIN: AtomicU32 = AtomicU32::new(u32::MAX);
/// Dirty rect top edge (minimum y).
static DIRTY_Y_MIN: AtomicU32 = AtomicU32::new(u32::MAX);
/// Dirty rect right edge (maximum x + width, exclusive).
static DIRTY_X_MAX: AtomicU32 = AtomicU32::new(0);
/// Dirty rect bottom edge (maximum y + height, exclusive).
static DIRTY_Y_MAX: AtomicU32 = AtomicU32::new(0);

/// Mark a rectangular region as dirty (union with existing dirty rect).
///
/// This is lock-free and safe to call from any context (syscall, kthread, etc.).
/// Uses atomic min/max to expand the dirty rect to include the new region.
pub fn mark_dirty(x: u32, y: u32, w: u32, h: u32) {
    if w == 0 || h == 0 {
        return;
    }
    let x2 = x.saturating_add(w);
    let y2 = y.saturating_add(h);

    // Expand dirty rect using atomic min/max
    fetch_min_u32(&DIRTY_X_MIN, x);
    fetch_min_u32(&DIRTY_Y_MIN, y);
    fetch_max_u32(&DIRTY_X_MAX, x2);
    fetch_max_u32(&DIRTY_Y_MAX, y2);

    // Set dirty flag last — readers check this first
    FB_DIRTY.store(true, Ordering::Release);
}

/// Mark the entire framebuffer as dirty.
pub fn mark_full_dirty() {
    if let Some((w, h)) = gpu_mmio::dimensions() {
        mark_dirty(0, 0, w, h);
    }
}

/// Take the dirty rect, resetting to clean.
///
/// Returns `Some((x, y, w, h))` if any region was dirty, `None` if clean.
/// The dirty state is atomically cleared so the next call returns None
/// unless new dirty regions are marked in between.
///
/// The returned rect is clamped to the display dimensions. This prevents
/// out-of-bounds coordinates (e.g., from cursor mark_dirty near screen edges)
/// from being sent to the VirtIO GPU, which rejects invalid rects.
pub fn take_dirty_rect() -> Option<(u32, u32, u32, u32)> {
    if !FB_DIRTY.swap(false, Ordering::Acquire) {
        return None;
    }

    // Read and reset the rect bounds
    let x_min = DIRTY_X_MIN.swap(u32::MAX, Ordering::Relaxed);
    let y_min = DIRTY_Y_MIN.swap(u32::MAX, Ordering::Relaxed);
    let x_max = DIRTY_X_MAX.swap(0, Ordering::Relaxed);
    let y_max = DIRTY_Y_MAX.swap(0, Ordering::Relaxed);

    if x_min >= x_max || y_min >= y_max {
        return None;
    }

    // Clamp to display dimensions — cursor mark_dirty near screen edges can
    // produce rects that extend beyond the display (e.g., cursor at x=1270
    // marks dirty (1254, y, 32, 32) → x_max = 1286 > 1280). VirtIO GPU
    // rejects transfer_to_host with out-of-bounds coordinates.
    let (x_min, y_min, x_max, y_max) = if let Some((dw, dh)) = gpu_mmio::dimensions() {
        (x_min.min(dw), y_min.min(dh), x_max.min(dw), y_max.min(dh))
    } else {
        (x_min, y_min, x_max, y_max)
    };

    if x_min >= x_max || y_min >= y_max {
        return None;
    }

    Some((x_min, y_min, x_max - x_min, y_max - y_min))
}

/// Atomic fetch_min for u32 (CAS loop).
#[inline]
fn fetch_min_u32(atom: &AtomicU32, val: u32) {
    let mut current = atom.load(Ordering::Relaxed);
    while val < current {
        match atom.compare_exchange_weak(current, val, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

/// Atomic fetch_max for u32 (CAS loop).
#[inline]
fn fetch_max_u32(atom: &AtomicU32, val: u32) {
    let mut current = atom.load(Ordering::Relaxed);
    while val > current {
        match atom.compare_exchange_weak(current, val, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

/// ARM64 framebuffer wrapper that implements Canvas trait
pub struct Arm64FrameBuffer {
    /// Display width in pixels
    width: usize,
    /// Display height in pixels
    height: usize,
    /// Bytes per pixel (always 4 for BGRA)
    bytes_per_pixel: usize,
    /// Stride in pixels (same as width for VirtIO GPU)
    stride: usize,
}

impl Arm64FrameBuffer {
    /// Create a new ARM64 framebuffer wrapper
    ///
    /// Returns None if the VirtIO GPU is not initialized
    pub fn new() -> Option<Self> {
        let (width, height) = gpu_mmio::dimensions()?;

        Some(Self {
            width: width as usize,
            height: height as usize,
            bytes_per_pixel: 4, // BGRA format
            stride: width as usize,
        })
    }

    /// Flush the framebuffer to the display
    pub fn flush(&self) -> Result<(), &'static str> {
        gpu_mmio::flush()
    }

    /// Flush a rectangular region of the framebuffer to the display
    pub fn flush_rect(&self, x: u32, y: u32, w: u32, h: u32) -> Result<(), &'static str> {
        gpu_mmio::flush_rect(x, y, w, h)
    }
}

impl Canvas for Arm64FrameBuffer {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn bytes_per_pixel(&self) -> usize {
        self.bytes_per_pixel
    }

    fn stride(&self) -> usize {
        self.stride
    }

    fn is_bgr(&self) -> bool {
        true // VirtIO GPU uses B8G8R8A8_UNORM format
    }

    fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 {
            return;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.width || y >= self.height {
            return;
        }

        if let Some(buffer) = gpu_mmio::framebuffer() {
            let pixel_bytes = color.to_pixel_bytes(self.bytes_per_pixel, true);
            let offset = (y * self.stride + x) * self.bytes_per_pixel;

            if offset + self.bytes_per_pixel <= buffer.len() {
                buffer[offset..offset + self.bytes_per_pixel]
                    .copy_from_slice(&pixel_bytes[..self.bytes_per_pixel]);
            }
        }
    }

    fn get_pixel(&self, x: i32, y: i32) -> Option<Color> {
        if x < 0 || y < 0 {
            return None;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.width || y >= self.height {
            return None;
        }

        let buffer = gpu_mmio::framebuffer()?;
        let offset = (y * self.stride + x) * self.bytes_per_pixel;

        if offset + self.bytes_per_pixel > buffer.len() {
            return None;
        }

        Some(Color::from_pixel_bytes(
            &buffer[offset..offset + self.bytes_per_pixel],
            self.bytes_per_pixel,
            true,
        ))
    }

    fn buffer_mut(&mut self) -> &mut [u8] {
        gpu_mmio::framebuffer().unwrap_or(&mut [])
    }

    fn buffer(&self) -> &[u8] {
        // Safe because we're only reading
        gpu_mmio::framebuffer().map(|b| &*b).unwrap_or(&[])
    }
}

/// Global framebuffer instance for ARM64
pub static ARM64_FRAMEBUFFER: Mutex<Option<Arm64FrameBuffer>> = Mutex::new(None);

/// Initialize the ARM64 framebuffer
///
/// Must be called after VirtIO GPU initialization
pub fn init() -> Result<(), &'static str> {
    let fb = Arm64FrameBuffer::new().ok_or("Failed to create ARM64 framebuffer")?;

    crate::serial_println!(
        "[arm64-fb] Framebuffer initialized: {}x{} @ {}bpp",
        fb.width(),
        fb.height(),
        fb.bytes_per_pixel() * 8
    );

    *ARM64_FRAMEBUFFER.lock() = Some(fb);
    Ok(())
}

/// Draw a test rectangle to verify the framebuffer is working
pub fn draw_test_pattern() -> Result<(), &'static str> {
    use super::primitives::{fill_rect, Rect};

    let mut guard = ARM64_FRAMEBUFFER.lock();
    let fb = guard.as_mut().ok_or("Framebuffer not initialized")?;

    let (width, height) = (fb.width() as u32, fb.height() as u32);

    // Clear screen with dark blue
    fill_rect(
        fb,
        Rect { x: 0, y: 0, width, height },
        Color::rgb(20, 30, 50),
    );

    // Draw a red rectangle in the top-left
    fill_rect(
        fb,
        Rect { x: 50, y: 50, width: 200, height: 150 },
        Color::RED,
    );

    // Draw a green rectangle
    fill_rect(
        fb,
        Rect { x: 300, y: 100, width: 200, height: 150 },
        Color::GREEN,
    );

    // Draw a blue rectangle
    fill_rect(
        fb,
        Rect { x: 550, y: 150, width: 200, height: 150 },
        Color::BLUE,
    );

    // Draw a white rectangle
    fill_rect(
        fb,
        Rect { x: 200, y: 350, width: 300, height: 100 },
        Color::WHITE,
    );

    // Flush to display
    fb.flush()?;

    crate::serial_println!("[arm64-fb] Test pattern drawn successfully");
    Ok(())
}

/// Draw text to the framebuffer
#[allow(dead_code)]
pub fn draw_text(x: i32, y: i32, text: &str, color: Color) -> Result<(), &'static str> {
    use super::primitives::{draw_text, TextStyle};

    let mut guard = ARM64_FRAMEBUFFER.lock();
    let fb = guard.as_mut().ok_or("Framebuffer not initialized")?;

    let style = TextStyle::new().with_color(color);
    draw_text(fb, x, y, text, &style);

    fb.flush()
}

/// Clear the screen with a color
#[allow(dead_code)]
pub fn clear_screen(color: Color) -> Result<(), &'static str> {
    use super::primitives::{fill_rect, Rect};

    let mut guard = ARM64_FRAMEBUFFER.lock();
    let fb = guard.as_mut().ok_or("Framebuffer not initialized")?;

    let (width, height) = (fb.width() as u32, fb.height() as u32);
    fill_rect(fb, Rect { x: 0, y: 0, width, height }, color);

    fb.flush()
}

// =============================================================================
// SHELL_FRAMEBUFFER Interface (compatible with x86_64 logger.rs)
// =============================================================================

/// Shell framebuffer wrapper for ARM64
///
/// This provides an interface compatible with x86_64's ShellFrameBuffer in logger.rs,
/// allowing the split_screen module to work on both architectures.
pub struct ShellFrameBuffer {
    /// The underlying framebuffer
    fb: Arm64FrameBuffer,
}

impl ShellFrameBuffer {
    /// Create a new shell framebuffer
    pub fn new() -> Option<Self> {
        Some(Self {
            fb: Arm64FrameBuffer::new()?,
        })
    }

    /// Get framebuffer width
    pub fn width(&self) -> usize {
        self.fb.width
    }

    /// Get framebuffer height
    pub fn height(&self) -> usize {
        self.fb.height
    }

    /// Flush the framebuffer to the display
    ///
    /// On ARM64, this calls VirtIO GPU flush.
    /// Unlike x86_64 which has double buffering, we flush directly to the GPU.
    pub fn flush(&self) {
        let _ = self.fb.flush();
    }

    /// Flush a rectangular region of the framebuffer to the display
    pub fn flush_rect(&self, x: u32, y: u32, w: u32, h: u32) {
        let _ = self.fb.flush_rect(x, y, w, h);
    }

    /// Flush the framebuffer, returning any GPU errors.
    ///
    /// Unlike `flush()` which silently discards errors, this propagates
    /// the Result so callers can detect and report GPU command failures.
    pub fn flush_result(&self) -> Result<(), &'static str> {
        self.fb.flush()
    }

    /// Get double buffer (returns None on ARM64)
    ///
    /// On ARM64, the VirtIO GPU handles buffering, so we don't need
    /// a software double buffer. This method exists for API compatibility.
    #[allow(dead_code)]
    pub fn double_buffer_mut(&mut self) -> Option<&mut super::double_buffer::DoubleBufferedFrameBuffer> {
        // ARM64 VirtIO GPU handles buffering internally
        None
    }
}

impl Canvas for ShellFrameBuffer {
    fn width(&self) -> usize {
        self.fb.width()
    }

    fn height(&self) -> usize {
        self.fb.height()
    }

    fn bytes_per_pixel(&self) -> usize {
        self.fb.bytes_per_pixel()
    }

    fn stride(&self) -> usize {
        self.fb.stride()
    }

    fn is_bgr(&self) -> bool {
        self.fb.is_bgr()
    }

    fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        self.fb.set_pixel(x, y, color);
    }

    fn get_pixel(&self, x: i32, y: i32) -> Option<Color> {
        self.fb.get_pixel(x, y)
    }

    fn buffer_mut(&mut self) -> &mut [u8] {
        self.fb.buffer_mut()
    }

    fn buffer(&self) -> &[u8] {
        self.fb.buffer()
    }
}

/// Global shell framebuffer instance (compatible with x86_64 logger.rs interface)
pub static SHELL_FRAMEBUFFER: OnceCell<Mutex<ShellFrameBuffer>> = OnceCell::uninit();

/// Initialize the shell framebuffer
///
/// Must be called after VirtIO GPU initialization.
/// This initializes both ARM64_FRAMEBUFFER and SHELL_FRAMEBUFFER.
pub fn init_shell_framebuffer() -> Result<(), &'static str> {
    let fb = ShellFrameBuffer::new().ok_or("Failed to create shell framebuffer")?;

    crate::serial_println!(
        "[arm64-fb] Shell framebuffer initialized: {}x{}",
        fb.width(),
        fb.height()
    );

    let _ = SHELL_FRAMEBUFFER.try_init_once(|| Mutex::new(fb));
    Ok(())
}

/// Get the framebuffer dimensions
pub fn dimensions() -> Option<(usize, usize)> {
    SHELL_FRAMEBUFFER.get().map(|fb| {
        let guard = fb.lock();
        (guard.width(), guard.height())
    })
}

/// Flush the shell framebuffer to the display
pub fn flush_shell_framebuffer() {
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        fb.lock().flush();
    }
}
