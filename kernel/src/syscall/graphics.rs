//! Graphics-related system calls.
//!
//! Provides syscalls for querying framebuffer information.

#[cfg(feature = "interactive")]
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

/// Maximum valid userspace address (canonical lower half)
/// Addresses above this are kernel space and must be rejected.
const USER_SPACE_MAX: u64 = 0x0000_8000_0000_0000;

/// sys_fbinfo - Get framebuffer information
///
/// # Arguments
/// * `info_ptr` - Pointer to userspace FbInfo structure to fill
///
/// # Returns
/// * 0 on success
/// * -EFAULT if info_ptr is invalid or in kernel space
/// * -ENODEV if no framebuffer is available
#[cfg(feature = "interactive")]
pub fn sys_fbinfo(info_ptr: u64) -> SyscallResult {
    // Validate pointer: must be non-null and in userspace address range
    if info_ptr == 0 {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Reject kernel-space pointers to prevent kernel memory corruption
    if info_ptr >= USER_SPACE_MAX {
        log::warn!("sys_fbinfo: rejected kernel-space pointer {:#x}", info_ptr);
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Validate the entire FbInfo struct fits in userspace
    let end_ptr = info_ptr.saturating_add(core::mem::size_of::<FbInfo>() as u64);
    if end_ptr > USER_SPACE_MAX {
        log::warn!("sys_fbinfo: buffer extends into kernel space");
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

    // Copy to userspace (pointer already validated above)
    unsafe {
        let info_out = info_ptr as *mut FbInfo;
        core::ptr::write(info_out, info);
    }

    SyscallResult::Ok(0)
}

/// sys_fbinfo - Stub for non-interactive mode (returns ENODEV)
#[cfg(not(feature = "interactive"))]
pub fn sys_fbinfo(_info_ptr: u64) -> SyscallResult {
    // No framebuffer available in non-interactive mode
    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
}
