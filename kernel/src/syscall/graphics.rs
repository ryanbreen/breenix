//! Graphics-related system calls.
//!
//! Provides syscalls for querying framebuffer information.
//!
//! This module is only compiled when the `interactive` feature is enabled,
//! as it depends on the framebuffer infrastructure.

#![cfg(feature = "interactive")]

use crate::logger::SHELL_FRAMEBUFFER;
use super::SyscallResult;

/// Framebuffer info structure returned by sys_fbinfo.
/// This matches the userspace FbInfo struct in libbreenix.
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

/// sys_fbinfo - Get framebuffer information
///
/// # Arguments
/// * `info_ptr` - Pointer to userspace FbInfo structure to fill
///
/// # Returns
/// * 0 on success
/// * -EFAULT if info_ptr is invalid
/// * -ENODEV if no framebuffer is available
pub fn sys_fbinfo(info_ptr: u64) -> SyscallResult {
    // Validate pointer
    if info_ptr == 0 {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Get framebuffer info from the shell framebuffer
    let fb = match SHELL_FRAMEBUFFER.get() {
        Some(fb) => fb,
        None => {
            log::warn!("sys_fbinfo: No framebuffer available");
            return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
        }
    };

    let fb_guard = fb.lock();

    // Get info through Canvas trait methods
    use crate::graphics::primitives::Canvas;
    let info = FbInfo {
        width: fb_guard.width() as u64,
        height: fb_guard.height() as u64,
        stride: fb_guard.stride() as u64,
        bytes_per_pixel: fb_guard.bytes_per_pixel() as u64,
        pixel_format: if fb_guard.is_bgr() { 1 } else { 0 },
    };

    // Copy to userspace
    // Note: In a real implementation, we'd validate the userspace pointer
    // and use proper copy_to_user semantics. For now, direct write.
    unsafe {
        let info_out = info_ptr as *mut FbInfo;
        core::ptr::write(info_out, info);
    }

    SyscallResult::Ok(0)
}
