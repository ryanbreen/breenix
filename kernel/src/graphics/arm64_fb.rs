//! ARM64 Framebuffer implementation (VirtIO GPU + UEFI GOP backends)
//!
//! Provides a Canvas implementation for the ARM64 framebuffer with two backends:
//! - **VirtIO GPU**: Used on QEMU with virtio-gpu-device (original backend)
//! - **UEFI GOP**: Linear framebuffer in physical memory (used on Parallels)
//!
//! This module also provides a SHELL_FRAMEBUFFER interface compatible with
//! the x86_64 version in logger.rs, allowing split_screen.rs
//! to work on both architectures.

#![cfg(target_arch = "aarch64")]

use super::primitives::{Canvas, Color};
use crate::drivers::virtio::gpu_mmio;
use conquer_once::spin::OnceCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
    // Try FB_INFO_CACHE first (works for both VirtIO and GOP)
    if let Some(cache) = FB_INFO_CACHE.get() {
        mark_dirty(0, 0, cache.width as u32, cache.height as u32);
        return;
    }
    // Fall back to VirtIO GPU PCI, then MMIO
    if let Some((w, h)) = crate::drivers::virtio::gpu_pci::dimensions() {
        mark_dirty(0, 0, w, h);
    } else if let Some((w, h)) = gpu_mmio::dimensions() {
        mark_dirty(0, 0, w, h);
    }
}

/// Check if any region is dirty without consuming the state.
///
/// Used by the render thread to decide whether to yield or loop back
/// for another flush. Does not reset the dirty flag.
pub fn has_dirty_rect() -> bool {
    FB_DIRTY.load(Ordering::Acquire)
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
    // produce rects that extend beyond the display. Both VirtIO GPU and GOP
    // need clamped coordinates.
    let (x_min, y_min, x_max, y_max) = if let Some(cache) = FB_INFO_CACHE.get() {
        let dw = cache.width as u32;
        let dh = cache.height as u32;
        (x_min.min(dw), y_min.min(dh), x_max.min(dw), y_max.min(dh))
    } else if let Some((dw, dh)) = crate::drivers::virtio::gpu_pci::dimensions() {
        (x_min.min(dw), y_min.min(dh), x_max.min(dw), y_max.min(dh))
    } else if let Some((dw, dh)) = gpu_mmio::dimensions() {
        (x_min.min(dw), y_min.min(dh), x_max.min(dw), y_max.min(dh))
    } else {
        (x_min, y_min, x_max, y_max)
    };

    if x_min >= x_max || y_min >= y_max {
        return None;
    }

    Some((x_min, y_min, x_max - x_min, y_max - y_min))
}

/// Flush a dirty rectangle to the display.
///
/// This is called by the render thread without holding the SHELL_FRAMEBUFFER lock.
/// For GOP, this is a no-op data barrier (writes are already in display memory).
/// For VirtIO GPU, this issues transfer_to_host + resource_flush commands.
pub fn flush_dirty_rect(x: u32, y: u32, w: u32, h: u32) -> Result<(), &'static str> {
    if is_gop_active() {
        // GOP: writes go directly to display memory. DSB ensures visibility.
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)); }
        Ok(())
    } else if crate::drivers::virtio::gpu_pci::is_initialized() {
        crate::drivers::virtio::gpu_pci::flush_rect(x, y, w, h)
    } else {
        gpu_mmio::flush_rect(x, y, w, h)
    }
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

// =============================================================================
// GOP Framebuffer Backend (UEFI linear buffer, used on Parallels)
// =============================================================================

/// GOP framebuffer virtual address (HHDM-mapped)
static GOP_FB_PTR: AtomicU64 = AtomicU64::new(0);
/// GOP framebuffer size in bytes
static GOP_FB_LEN: AtomicU64 = AtomicU64::new(0);
/// Whether GOP backend is active
static GOP_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Check if GOP framebuffer is active (vs VirtIO GPU).
pub fn is_gop_active() -> bool {
    GOP_ACTIVE.load(Ordering::Relaxed)
}

/// Get the GOP framebuffer as a mutable byte slice.
/// Returns None if GOP is not initialized.
fn gop_framebuffer() -> Option<&'static mut [u8]> {
    let ptr = GOP_FB_PTR.load(Ordering::Relaxed);
    let len = GOP_FB_LEN.load(Ordering::Relaxed);
    if ptr == 0 || len == 0 {
        return None;
    }
    unsafe { Some(core::slice::from_raw_parts_mut(ptr as *mut u8, len as usize)) }
}

/// Get the GOP framebuffer as an immutable byte slice.
fn gop_framebuffer_ref() -> Option<&'static [u8]> {
    let ptr = GOP_FB_PTR.load(Ordering::Relaxed);
    let len = GOP_FB_LEN.load(Ordering::Relaxed);
    if ptr == 0 || len == 0 {
        return None;
    }
    unsafe { Some(core::slice::from_raw_parts(ptr as *const u8, len as usize)) }
}

/// Initialize the GOP framebuffer from platform_config data.
///
/// Maps the framebuffer physical address via HHDM and sets up the
/// SHELL_FRAMEBUFFER with GOP dimensions. Call this instead of
/// init_shell_framebuffer() when a GOP framebuffer is available.
pub fn init_gop_framebuffer() -> Result<(), &'static str> {
    let base = crate::platform_config::fb_base_phys();
    let size = crate::platform_config::fb_size();
    let width = crate::platform_config::fb_width();
    let height = crate::platform_config::fb_height();
    let stride = crate::platform_config::fb_stride();
    let is_bgr = crate::platform_config::fb_is_bgr();

    if base == 0 || size == 0 {
        return Err("No GOP framebuffer info in platform config");
    }

    // Map via HHDM (higher-half direct map)
    let hhdm_base = crate::arch_impl::aarch64::constants::HHDM_BASE;
    let virt_ptr = hhdm_base + base;

    GOP_FB_PTR.store(virt_ptr, Ordering::Relaxed);
    GOP_FB_LEN.store(size, Ordering::Relaxed);
    GOP_ACTIVE.store(true, Ordering::Relaxed);

    crate::serial_println!(
        "[arm64-fb] GOP framebuffer: {}x{} stride={} {} base_phys={:#x} virt={:#x} size={:#x}",
        width, height, stride,
        if is_bgr { "BGR" } else { "RGB" },
        base, virt_ptr, size
    );

    // Create Arm64FrameBuffer with GOP parameters
    let fb = Arm64FrameBuffer {
        width: width as usize,
        height: height as usize,
        bytes_per_pixel: 4,
        stride: stride as usize,
        is_gop: true,
        is_bgr_flag: is_bgr,
    };

    // Initialize SHELL_FRAMEBUFFER
    let shell_fb = ShellFrameBuffer { fb };

    // Cache immutable dimensions for lock-free access by sys_fbinfo
    let _ = FB_INFO_CACHE.try_init_once(|| FbInfoCache {
        width: width as usize,
        height: height as usize,
        stride: stride as usize,
        bytes_per_pixel: 4,
        is_bgr,
    });

    let _ = SHELL_FRAMEBUFFER.try_init_once(|| Mutex::new(shell_fb));

    crate::serial_println!("[arm64-fb] GOP shell framebuffer initialized: {}x{}", width, height);
    Ok(())
}

/// Initialize framebuffer using VirtIO GPU PCI resolution but GOP/BAR0 memory.
///
/// On Parallels, VirtIO GPU `set_scanout` controls the display mode (resolution,
/// stride) but actual pixels are read from BAR0 (the GOP framebuffer at 0x10000000).
/// This function sets up a GOP-style framebuffer at the VirtIO GPU's configured
/// resolution, giving us higher resolution than the GOP-reported 1024x768.
///
/// Must be called AFTER `drivers::init()` (which initializes GPU PCI).
pub fn init_gpu_pci_gop_framebuffer() -> Result<(), &'static str> {
    if !crate::drivers::virtio::gpu_pci::is_initialized() {
        return Err("GPU PCI not initialized");
    }

    let (width, height) = crate::drivers::virtio::gpu_pci::dimensions()
        .ok_or("GPU PCI has no dimensions")?;
    let width = width as usize;
    let height = height as usize;
    let stride = width; // VirtIO GPU stride = width (no padding)
    let bytes_per_pixel = 4usize;

    // GOP framebuffer base: the BAR0/GOP address that the display reads from
    let base = crate::platform_config::fb_base_phys();
    if base == 0 {
        return Err("No GOP framebuffer base address");
    }

    // Ensure the GOP memory is large enough for the new resolution.
    // BAR0 is typically 64MB; we need width * height * 4 bytes.
    let needed = width * height * bytes_per_pixel;
    let gop_size = crate::platform_config::fb_size() as usize;
    crate::serial_println!(
        "[arm64-fb] GPU PCI+GOP hybrid: {}x{} stride={} need={} bytes, GOP region={} bytes",
        width, height, stride, needed, gop_size
    );
    // GOP size from UEFI may report only 1024x768 worth; the actual BAR is larger.
    // Proceed even if needed > gop_size — the BAR0 region extends well beyond.

    // Map via HHDM
    let hhdm_base = crate::arch_impl::aarch64::constants::HHDM_BASE;
    let virt_ptr = hhdm_base + base;

    // Update GOP globals with the new (larger) dimensions
    GOP_FB_PTR.store(virt_ptr, Ordering::Relaxed);
    GOP_FB_LEN.store(needed as u64, Ordering::Relaxed);
    GOP_ACTIVE.store(true, Ordering::Relaxed);

    // Create framebuffer with GPU PCI dimensions but GOP backend
    let fb = Arm64FrameBuffer {
        width,
        height,
        bytes_per_pixel,
        stride,
        is_gop: true, // Writes go to GOP memory, flush uses DSB
        is_bgr_flag: true, // B8G8R8A8_UNORM
    };

    let shell_fb = ShellFrameBuffer { fb };

    let _ = FB_INFO_CACHE.try_init_once(|| FbInfoCache {
        width,
        height,
        stride,
        bytes_per_pixel,
        is_bgr: true,
    });

    let _ = SHELL_FRAMEBUFFER.try_init_once(|| Mutex::new(shell_fb));

    crate::serial_println!(
        "[arm64-fb] GPU PCI+GOP hybrid framebuffer: {}x{} base_phys={:#x}",
        width, height, base
    );
    Ok(())
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
    /// Whether this framebuffer uses the GOP backend (vs VirtIO GPU)
    is_gop: bool,
    /// Whether pixel format is BGR (true) or RGB (false)
    is_bgr_flag: bool,
}

impl Arm64FrameBuffer {
    /// Create a new ARM64 framebuffer wrapper
    ///
    /// Tries GPU PCI first, then falls back to GPU MMIO.
    /// Returns None if no VirtIO GPU is initialized.
    pub fn new() -> Option<Self> {
        // Try GPU PCI first (Parallels), then GPU MMIO (QEMU)
        let (width, height) = crate::drivers::virtio::gpu_pci::dimensions()
            .or_else(|| gpu_mmio::dimensions())?;

        Some(Self {
            width: width as usize,
            height: height as usize,
            bytes_per_pixel: 4, // BGRA format
            stride: width as usize,
            is_gop: false,
            is_bgr_flag: true, // VirtIO GPU uses B8G8R8A8_UNORM
        })
    }

    /// Flush the framebuffer to the display
    pub fn flush(&self) -> Result<(), &'static str> {
        if self.is_gop {
            // GOP: writes go directly to display memory. DSB ensures visibility.
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)); }
            Ok(())
        } else if crate::drivers::virtio::gpu_pci::is_initialized() {
            crate::drivers::virtio::gpu_pci::flush()
        } else {
            gpu_mmio::flush()
        }
    }

    /// Flush a rectangular region of the framebuffer to the display
    pub fn flush_rect(&self, x: u32, y: u32, w: u32, h: u32) -> Result<(), &'static str> {
        if self.is_gop {
            unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)); }
            Ok(())
        } else if crate::drivers::virtio::gpu_pci::is_initialized() {
            crate::drivers::virtio::gpu_pci::flush_rect(x, y, w, h)
        } else {
            gpu_mmio::flush_rect(x, y, w, h)
        }
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
        self.is_bgr_flag
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

        let buffer = if self.is_gop {
            match gop_framebuffer() { Some(b) => b, None => return }
        } else if crate::drivers::virtio::gpu_pci::is_initialized() {
            match crate::drivers::virtio::gpu_pci::framebuffer() { Some(b) => b, None => return }
        } else {
            match gpu_mmio::framebuffer() { Some(b) => b, None => return }
        };

        let pixel_bytes = color.to_pixel_bytes(self.bytes_per_pixel, self.is_bgr_flag);
        let offset = (y * self.stride + x) * self.bytes_per_pixel;
        if offset + self.bytes_per_pixel <= buffer.len() {
            buffer[offset..offset + self.bytes_per_pixel]
                .copy_from_slice(&pixel_bytes[..self.bytes_per_pixel]);
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

        let buffer: &[u8] = if self.is_gop {
            gop_framebuffer_ref()?
        } else if crate::drivers::virtio::gpu_pci::is_initialized() {
            crate::drivers::virtio::gpu_pci::framebuffer().map(|b| &*b)?
        } else {
            gpu_mmio::framebuffer().map(|b| &*b)?
        };

        let offset = (y * self.stride + x) * self.bytes_per_pixel;
        if offset + self.bytes_per_pixel > buffer.len() {
            return None;
        }
        Some(Color::from_pixel_bytes(
            &buffer[offset..offset + self.bytes_per_pixel],
            self.bytes_per_pixel,
            self.is_bgr_flag,
        ))
    }

    fn buffer_mut(&mut self) -> &mut [u8] {
        if self.is_gop {
            gop_framebuffer().unwrap_or(&mut [])
        } else if crate::drivers::virtio::gpu_pci::is_initialized() {
            crate::drivers::virtio::gpu_pci::framebuffer().unwrap_or(&mut [])
        } else {
            gpu_mmio::framebuffer().unwrap_or(&mut [])
        }
    }

    fn buffer(&self) -> &[u8] {
        if self.is_gop {
            gop_framebuffer_ref().unwrap_or(&[])
        } else if crate::drivers::virtio::gpu_pci::is_initialized() {
            crate::drivers::virtio::gpu_pci::framebuffer().map(|b| &*b).unwrap_or(&[])
        } else {
            gpu_mmio::framebuffer().map(|b| &*b).unwrap_or(&[])
        }
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

/// Cached framebuffer dimensions, set once during init and never modified.
/// This allows sys_fbinfo to read dimensions without acquiring the framebuffer lock,
/// avoiding contention with BWM's flush operations which hold the lock for ~400μs
/// during full-screen pixel copies.
pub static FB_INFO_CACHE: OnceCell<FbInfoCache> = OnceCell::uninit();

/// Immutable framebuffer info cached at initialization time.
pub struct FbInfoCache {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub is_bgr: bool,
}

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

    // Cache immutable dimensions for lock-free access by sys_fbinfo
    let _ = FB_INFO_CACHE.try_init_once(|| FbInfoCache {
        width: fb.width(),
        height: fb.height(),
        stride: fb.stride(),
        bytes_per_pixel: fb.bytes_per_pixel(),
        is_bgr: fb.is_bgr(),
    });

    let _ = SHELL_FRAMEBUFFER.try_init_once(|| Mutex::new(fb));
    Ok(())
}

/// Get the framebuffer dimensions
pub fn dimensions() -> Option<(usize, usize)> {
    SHELL_FRAMEBUFFER.get().and_then(|fb| {
        fb.try_lock().map(|guard| (guard.width(), guard.height()))
    })
}
