//! Graphics syscall wrappers
//!
//! Provides userspace API for querying framebuffer information and drawing.
//!
//! Also provides the [`Framebuffer`] RAII wrapper for direct pixel writes
//! via memory-mapped access.

use crate::error::Error;
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
/// * Err(Error) - Error (ENODEV if no framebuffer)
pub fn fbinfo() -> Result<FbInfo, Error> {
    let mut info = FbInfo::zeroed();
    let ret = unsafe { raw::syscall1(nr::FBINFO, &mut info as *mut FbInfo as u64) as i64 };
    Error::from_syscall(ret)?;
    Ok(info)
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
fn fbdraw(cmd: &FbDrawCmd) -> Result<(), Error> {
    let ret = unsafe { raw::syscall1(nr::FBDRAW, cmd as *const FbDrawCmd as u64) as i64 };
    Error::from_syscall(ret)?;
    Ok(())
}

/// Clear the left pane with a color
pub fn fb_clear(color: u32) -> Result<(), Error> {
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
pub fn fb_fill_rect(x: i32, y: i32, width: i32, height: i32, color: u32) -> Result<(), Error> {
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
pub fn fb_draw_rect(x: i32, y: i32, width: i32, height: i32, color: u32) -> Result<(), Error> {
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
pub fn fb_fill_circle(cx: i32, cy: i32, radius: i32, color: u32) -> Result<(), Error> {
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
pub fn fb_draw_circle(cx: i32, cy: i32, radius: i32, color: u32) -> Result<(), Error> {
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
pub fn fb_draw_line(x1: i32, y1: i32, x2: i32, y2: i32, color: u32) -> Result<(), Error> {
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
pub fn fb_mmap() -> Result<*mut u8, Error> {
    let ret = unsafe { raw::syscall0(nr::FBMMAP) as i64 };
    Error::from_syscall(ret).map(|v| v as *mut u8)
}

/// Flush the framebuffer (sync double buffer to screen)
pub fn fb_flush() -> Result<(), Error> {
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
pub fn fb_flush_rect(x: i32, y: i32, w: i32, h: i32) -> Result<(), Error> {
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

/// Get the current mouse cursor position.
///
/// # Returns
/// * Ok((x, y)) - Mouse position in screen coordinates
/// * Err(Error) - Error (ENODEV if no pointer device)
pub fn mouse_pos() -> Result<(u32, u32), Error> {
    let mut pos: [u32; 2] = [0, 0];
    let ret = unsafe { raw::syscall1(nr::GET_MOUSE_POS, &mut pos as *mut [u32; 2] as u64) as i64 };
    Error::from_syscall(ret)?;
    Ok((pos[0], pos[1]))
}

// ============================================================================
// RAII Framebuffer Wrapper
// ============================================================================

/// Safe, RAII framebuffer handle for direct pixel writes.
///
/// Maps the framebuffer into userspace memory. All drawing operations write
/// directly to the buffer with no syscall overhead. Call `flush()` to sync
/// to the display (1 syscall per frame).
pub struct Framebuffer {
    ptr: *mut u8,
    width: u32,
    height: u32,
    stride: u32,  // row stride in bytes
    bpp: u32,     // bytes per pixel
    bgr: bool,
}

impl Framebuffer {
    /// Map the framebuffer and return a safe handle.
    pub fn new() -> Result<Framebuffer, Error> {
        let info = fbinfo()?;
        let ptr = fb_mmap()?;
        Ok(Framebuffer {
            ptr,
            width: info.left_pane_width() as u32,
            height: info.height as u32,
            stride: info.left_pane_width() as u32 * info.bytes_per_pixel as u32,
            bpp: info.bytes_per_pixel as u32,
            bgr: info.is_bgr(),
        })
    }

    /// Get the width in pixels.
    pub fn width(&self) -> u32 { self.width }

    /// Get the height in pixels.
    pub fn height(&self) -> u32 { self.height }

    /// Get the row stride in bytes.
    pub fn stride(&self) -> u32 { self.stride }

    /// Get bytes per pixel.
    pub fn bpp(&self) -> u32 { self.bpp }

    /// Check if pixel format is BGR.
    pub fn is_bgr(&self) -> bool { self.bgr }

    /// Set a single pixel. No-op if coordinates are out of bounds.
    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x < self.width && y < self.height {
            let offset = (y * self.stride + x * self.bpp) as usize;
            unsafe {
                let p = self.ptr.add(offset) as *mut u32;
                *p = color;
            }
        }
    }

    /// Get a mutable pointer to the start of a row (for bulk operations).
    /// Returns None if y is out of bounds.
    pub fn row_ptr(&mut self, y: u32) -> Option<*mut u8> {
        if y < self.height {
            Some(unsafe { self.ptr.add((y * self.stride) as usize) })
        } else {
            None
        }
    }

    /// Raw mutable access to the entire buffer.
    ///
    /// # Safety
    /// Caller must not write beyond pixel boundaries.
    pub unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
        core::slice::from_raw_parts_mut(self.ptr, (self.height * self.stride) as usize)
    }

    /// Raw const access to the entire buffer.
    ///
    /// # Safety
    /// Caller must not hold this reference while mutating the buffer.
    pub unsafe fn as_slice(&self) -> &[u8] {
        core::slice::from_raw_parts(self.ptr, (self.height * self.stride) as usize)
    }

    /// Flush entire buffer to display (1 syscall).
    pub fn flush(&self) -> Result<(), Error> {
        fb_flush()
    }

    /// Flush a rectangular region to display.
    pub fn flush_rect(&self, x: u32, y: u32, w: u32, h: u32) -> Result<(), Error> {
        fb_flush_rect(x as i32, y as i32, w as i32, h as i32)
    }

    /// Convert RGB to this framebuffer's native pixel format.
    pub fn color(&self, r: u8, g: u8, b: u8) -> u32 {
        if self.bgr {
            (b as u32) | ((g as u32) << 8) | ((r as u32) << 16)
        } else {
            (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
        }
    }

    /// Fill a rectangular region with a solid color.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for row in y..y_end {
            let row_offset = (row * self.stride) as usize;
            for col in x..x_end {
                let offset = row_offset + (col * self.bpp) as usize;
                unsafe {
                    let p = self.ptr.add(offset) as *mut u32;
                    *p = color;
                }
            }
        }
    }
}

// Note: No Drop impl needed. The mmap'd buffer is cleaned up by the kernel
// when the process exits. Explicitly munmapping is optional.
