//! Graphics syscall wrappers
//!
//! Provides userspace API for querying framebuffer information and drawing.

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

    /// Get the width of the left (demo) pane
    pub fn left_pane_width(&self) -> u64 {
        self.width / 2
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

/// Draw command structure for sys_fbdraw.
/// Must match kernel's FbDrawCmd in syscall/graphics.rs.
#[repr(C)]
pub struct FbDrawCmd {
    /// Operation code
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

/// Draw operation codes
pub mod draw_op {
    /// Clear the left pane with a color
    pub const CLEAR: u32 = 0;
    /// Fill a rectangle: x, y, width, height, color
    pub const FILL_RECT: u32 = 1;
    /// Draw rectangle outline: x, y, width, height, color
    pub const DRAW_RECT: u32 = 2;
    /// Fill a circle: cx, cy, radius, color
    pub const FILL_CIRCLE: u32 = 3;
    /// Draw circle outline: cx, cy, radius, color
    pub const DRAW_CIRCLE: u32 = 4;
    /// Draw a line: x1, y1, x2, y2, color
    pub const DRAW_LINE: u32 = 5;
    /// Flush the framebuffer (for double-buffering)
    pub const FLUSH: u32 = 6;
}

/// Pack RGB color into u32
#[inline]
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Execute a draw command
fn fbdraw(cmd: &FbDrawCmd) -> Result<(), i32> {
    let result = unsafe { raw::syscall1(nr::FBDRAW, cmd as *const FbDrawCmd as u64) };

    if (result as i64) < 0 {
        Err(-(result as i64) as i32)
    } else {
        Ok(())
    }
}

/// Clear the left pane with a color
pub fn fb_clear(color: u32) -> Result<(), i32> {
    let cmd = FbDrawCmd {
        op: draw_op::CLEAR,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: 0,
        color,
    };
    fbdraw(&cmd)
}

/// Fill a rectangle on the left pane
pub fn fb_fill_rect(x: i32, y: i32, width: i32, height: i32, color: u32) -> Result<(), i32> {
    let cmd = FbDrawCmd {
        op: draw_op::FILL_RECT,
        p1: x,
        p2: y,
        p3: width,
        p4: height,
        color,
    };
    fbdraw(&cmd)
}

/// Draw a rectangle outline on the left pane
pub fn fb_draw_rect(x: i32, y: i32, width: i32, height: i32, color: u32) -> Result<(), i32> {
    let cmd = FbDrawCmd {
        op: draw_op::DRAW_RECT,
        p1: x,
        p2: y,
        p3: width,
        p4: height,
        color,
    };
    fbdraw(&cmd)
}

/// Fill a circle on the left pane
pub fn fb_fill_circle(cx: i32, cy: i32, radius: i32, color: u32) -> Result<(), i32> {
    let cmd = FbDrawCmd {
        op: draw_op::FILL_CIRCLE,
        p1: cx,
        p2: cy,
        p3: radius,
        p4: 0,
        color,
    };
    fbdraw(&cmd)
}

/// Draw a circle outline on the left pane
pub fn fb_draw_circle(cx: i32, cy: i32, radius: i32, color: u32) -> Result<(), i32> {
    let cmd = FbDrawCmd {
        op: draw_op::DRAW_CIRCLE,
        p1: cx,
        p2: cy,
        p3: radius,
        p4: 0,
        color,
    };
    fbdraw(&cmd)
}

/// Draw a line on the left pane
pub fn fb_draw_line(x1: i32, y1: i32, x2: i32, y2: i32, color: u32) -> Result<(), i32> {
    let cmd = FbDrawCmd {
        op: draw_op::DRAW_LINE,
        p1: x1,
        p2: y1,
        p3: x2,
        p4: y2,
        color,
    };
    fbdraw(&cmd)
}

/// Map a framebuffer buffer into this process's address space.
///
/// Returns a pointer to a compact left-pane buffer that can be drawn to
/// directly with zero syscalls. Call `fb_flush()` after drawing to sync
/// the buffer to the screen.
///
/// The buffer layout is: `stride = left_pane_width * bytes_per_pixel` (compact).
/// Use `fbinfo()` to get dimensions and pixel format.
pub fn fb_mmap() -> Result<*mut u8, i32> {
    let result = unsafe { raw::syscall0(nr::FBMMAP) };
    if (result as i64) < 0 {
        Err(-(result as i64) as i32)
    } else {
        Ok(result as *mut u8)
    }
}

/// Flush the framebuffer (sync double buffer to screen)
pub fn fb_flush() -> Result<(), i32> {
    let cmd = FbDrawCmd {
        op: draw_op::FLUSH,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: 0,
        color: 0,
    };
    fbdraw(&cmd)
}

/// Flush a rectangular region of the framebuffer.
///
/// Only the specified rect (x, y, w, h) is transferred to the display.
/// Falls back to full flush in the kernel if the rect covers the entire screen.
pub fn fb_flush_rect(x: i32, y: i32, w: i32, h: i32) -> Result<(), i32> {
    let cmd = FbDrawCmd {
        op: draw_op::FLUSH,
        p1: x,
        p2: y,
        p3: w,
        p4: h,
        color: 0,
    };
    fbdraw(&cmd)
}
