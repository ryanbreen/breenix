//! Graphics syscall wrappers
//!
//! Provides userspace API for querying framebuffer information.

use crate::syscall::{nr, raw};

/// Framebuffer information structure.
/// Must match kernel's FbInfo in syscall/graphics.rs.
#[repr(C)]
pub struct FbInfo {
    /// Width in pixels
    pub width: u64,
    /// Height in pixels
    pub height: u64,
    /// Stride (pixels per scanline, may be > width for alignment)
    pub stride: u64,
    /// Bytes per pixel (typically 3 or 4)
    pub bytes_per_pixel: u64,
    /// Pixel format: 0 = RGB, 1 = BGR, 2 = U8 (grayscale)
    pub pixel_format: u64,
}

impl FbInfo {
    /// Create a zeroed FbInfo for syscall output
    pub const fn zeroed() -> Self {
        Self {
            width: 0,
            height: 0,
            stride: 0,
            bytes_per_pixel: 0,
            pixel_format: 0,
        }
    }

    /// Check if pixel format is BGR (common in UEFI framebuffers)
    pub fn is_bgr(&self) -> bool {
        self.pixel_format == 1
    }

    /// Check if pixel format is RGB
    pub fn is_rgb(&self) -> bool {
        self.pixel_format == 0
    }

    /// Check if pixel format is grayscale
    pub fn is_grayscale(&self) -> bool {
        self.pixel_format == 2
    }
}

/// Get framebuffer information
///
/// # Returns
/// * Ok(FbInfo) - Framebuffer information
/// * Err(errno) - Error code (ENODEV if no framebuffer)
pub fn fbinfo() -> Result<FbInfo, i32> {
    let mut info = FbInfo::zeroed();
    let result = unsafe { raw::syscall1(nr::FBINFO, &mut info as *mut FbInfo as u64) };

    if (result as i64) < 0 {
        Err(-(result as i64) as i32)
    } else {
        Ok(info)
    }
}
