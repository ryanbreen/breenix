//! Graphics-related system calls.
//!
//! Provides syscalls for querying and drawing to the framebuffer.

// Architecture-specific framebuffer imports
#[cfg(all(target_arch = "x86_64", feature = "interactive"))]
use crate::logger::SHELL_FRAMEBUFFER;
#[cfg(target_arch = "aarch64")]
use crate::graphics::arm64_fb::SHELL_FRAMEBUFFER;

#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
use crate::graphics::primitives::{Color, Rect, fill_rect, draw_rect, fill_circle, draw_circle, draw_line};
use super::SyscallResult;

/// Framebuffer info structure returned by sys_fbinfo.
/// This matches the userspace FbInfo struct in libbreenix.
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
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
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
const USER_SPACE_MAX: u64 = crate::memory::layout::USER_STACK_REGION_END;

/// sys_fbinfo - Get framebuffer information
///
/// # Arguments
/// * `info_ptr` - Pointer to userspace FbInfo structure to fill
///
/// # Returns
/// * 0 on success
/// * -EFAULT if info_ptr is invalid or in kernel space
/// * -ENODEV if no framebuffer is available
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
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
#[cfg(not(any(target_arch = "aarch64", feature = "interactive")))]
pub fn sys_fbinfo(_info_ptr: u64) -> SyscallResult {
    // No framebuffer available in non-interactive mode
    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
}

/// Draw command operations for sys_fbdraw
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
#[repr(u32)]
#[allow(dead_code)]
pub enum FbDrawOp {
    /// Clear the left pane with a color
    Clear = 0,
    /// Fill a rectangle: x, y, width, height, color
    FillRect = 1,
    /// Draw rectangle outline: x, y, width, height, color
    DrawRect = 2,
    /// Fill a circle: cx, cy, radius, color
    FillCircle = 3,
    /// Draw circle outline: cx, cy, radius, color
    DrawCircle = 4,
    /// Draw a line: x1, y1, x2, y2, color
    DrawLine = 5,
    /// Flush the framebuffer (for double-buffering)
    Flush = 6,
}

/// Draw command structure passed from userspace.
/// Must match the FbDrawCmd struct in libbreenix.
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
#[repr(C)]
pub struct FbDrawCmd {
    /// Operation code (FbDrawOp)
    pub op: u32,
    /// First parameter (x, cx, x1, or unused)
    pub p1: i32,
    /// Second parameter (y, cy, y1, or unused)
    pub p2: i32,
    /// Third parameter (width, radius, x2, or unused)
    pub p3: i32,
    /// Fourth parameter (height, y2, or unused)
    pub p4: i32,
    /// Color as packed RGB (0x00RRGGBB)
    pub color: u32,
}

/// Get the width of the left (demo) pane
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
#[allow(dead_code)]
fn left_pane_width() -> usize {
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        let fb_guard = fb.lock();
        fb_guard.width() / 2
    } else {
        0
    }
}

/// Get the height of the framebuffer
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
#[allow(dead_code)]
fn fb_height() -> usize {
    if let Some(fb) = SHELL_FRAMEBUFFER.get() {
        let fb_guard = fb.lock();
        fb_guard.height()
    } else {
        0
    }
}

/// sys_fbdraw - Draw to the left pane of the framebuffer
///
/// # Arguments
/// * `cmd_ptr` - Pointer to userspace FbDrawCmd structure
///
/// # Returns
/// * 0 on success
/// * -EFAULT if cmd_ptr is invalid
/// * -ENODEV if no framebuffer is available
/// * -EINVAL if operation is invalid
#[cfg(any(target_arch = "aarch64", feature = "interactive"))]
pub fn sys_fbdraw(cmd_ptr: u64) -> SyscallResult {
    // Validate pointer
    if cmd_ptr == 0 || cmd_ptr >= USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Validate the entire FbDrawCmd struct fits in userspace
    let end_ptr = cmd_ptr.saturating_add(core::mem::size_of::<FbDrawCmd>() as u64);
    if end_ptr > USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // Read the command from userspace
    let cmd: FbDrawCmd = unsafe { core::ptr::read(cmd_ptr as *const FbDrawCmd) };

    // Get framebuffer
    let fb = match SHELL_FRAMEBUFFER.get() {
        Some(fb) => fb,
        None => return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64),
    };

    let mut fb_guard = fb.lock();

    // Get left pane dimensions (half the screen width)
    let pane_width = fb_guard.width() / 2;
    let pane_height = fb_guard.height();

    // Parse color
    let color = Color::rgb(
        ((cmd.color >> 16) & 0xFF) as u8,
        ((cmd.color >> 8) & 0xFF) as u8,
        (cmd.color & 0xFF) as u8,
    );

    match cmd.op {
        0 => {
            // Clear: fill entire left pane with color
            fill_rect(
                &mut *fb_guard,
                Rect {
                    x: 0,
                    y: 0,
                    width: pane_width as u32,
                    height: pane_height as u32,
                },
                color,
            );
        }
        1 => {
            // FillRect: x, y, width, height, color
            // Clip to left pane
            let x = cmd.p1.max(0) as i32;
            let y = cmd.p2.max(0) as i32;
            let w = cmd.p3.max(0) as u32;
            let h = cmd.p4.max(0) as u32;

            // Only draw if within left pane
            if (x as usize) < pane_width {
                let clipped_w = w.min((pane_width as i32 - x) as u32);
                fill_rect(
                    &mut *fb_guard,
                    Rect { x, y, width: clipped_w, height: h },
                    color,
                );
            }
        }
        2 => {
            // DrawRect: x, y, width, height, color
            let x = cmd.p1.max(0) as i32;
            let y = cmd.p2.max(0) as i32;
            let w = cmd.p3.max(0) as u32;
            let h = cmd.p4.max(0) as u32;

            if (x as usize) < pane_width {
                draw_rect(
                    &mut *fb_guard,
                    Rect { x, y, width: w, height: h },
                    color,
                );
            }
        }
        3 => {
            // FillCircle: cx, cy, radius, color
            let cx = cmd.p1;
            let cy = cmd.p2;
            let radius = cmd.p3.max(0) as u32;

            if (cx as usize) < pane_width {
                fill_circle(&mut *fb_guard, cx, cy, radius, color);
            }
        }
        4 => {
            // DrawCircle: cx, cy, radius, color
            let cx = cmd.p1;
            let cy = cmd.p2;
            let radius = cmd.p3.max(0) as u32;

            if (cx as usize) < pane_width {
                draw_circle(&mut *fb_guard, cx, cy, radius, color);
            }
        }
        5 => {
            // DrawLine: x1, y1, x2, y2, color
            let x1 = cmd.p1;
            let y1 = cmd.p2;
            let x2 = cmd.p3;
            let y2 = cmd.p4;

            // Allow lines that start or end in left pane
            if (x1 as usize) < pane_width || (x2 as usize) < pane_width {
                draw_line(&mut *fb_guard, x1, y1, x2, y2, color);
            }
        }
        6 => {
            // Flush: sync buffer to screen
            #[cfg(target_arch = "x86_64")]
            if let Some(db) = fb_guard.double_buffer_mut() {
                db.flush_full();
            }
            #[cfg(target_arch = "aarch64")]
            {
                // ARM64 uses VirtIO GPU flush
                let _ = fb_guard.flush();
            }
        }
        _ => {
            return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
        }
    }

    SyscallResult::Ok(0)
}

/// sys_fbdraw - Stub for non-interactive mode
#[cfg(not(any(target_arch = "aarch64", feature = "interactive")))]
pub fn sys_fbdraw(_cmd_ptr: u64) -> SyscallResult {
    SyscallResult::Err(super::ErrorCode::InvalidArgument as u64)
}
