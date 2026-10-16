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

/// `fb_draw` operation codes. Ops 0 to 5, 7 and 9 and the version 1 window ops 10 to 26
/// and 31 were retired (docs/gui/breengel.md section 17.6); 27 to 30 exist only in
/// `boot_tests` kernels.
pub mod draw_op {
    /// Flush the framebuffer (for double-buffering)
    pub const FLUSH: u32 = 6;
    /// Batch flush multiple dirty rects with one DSB barrier
    pub const FLUSH_BATCH: u32 = 8;
    /// F32c waitqueue stress reset.
    pub const WAIT_STRESS_RESET: u32 = 27;
    /// F32c waitqueue stress wait.
    pub const WAIT_STRESS_WAIT: u32 = 28;
    /// F32c waitqueue stress wake.
    pub const WAIT_STRESS_WAKE: u32 = 29;
    /// F32c waitqueue stress stats.
    pub const WAIT_STRESS_STATS: u32 = 30;
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

/// Reset the F32c waitqueue stress counters and wake any stale waiter.
pub fn wait_stress_reset() -> Result<(), Error> {
    let cmd = FbDrawCmd {
        op: draw_op::WAIT_STRESS_RESET,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: 0,
        color: 0,
    };
    fbdraw(&cmd)
}

/// Block once on the F32c waitqueue stress queue.
pub fn wait_stress_wait() -> Result<u64, Error> {
    let cmd = FbDrawCmd {
        op: draw_op::WAIT_STRESS_WAIT,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: 0,
        color: 0,
    };
    let ret = unsafe { raw::syscall1(nr::FBDRAW, &cmd as *const FbDrawCmd as u64) as i64 };
    Error::from_syscall(ret).map(|value| value as u64)
}

/// Wake the F32c waitqueue stress queue once.
pub fn wait_stress_wake() -> Result<u64, Error> {
    let cmd = FbDrawCmd {
        op: draw_op::WAIT_STRESS_WAKE,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: 0,
        color: 0,
    };
    let ret = unsafe { raw::syscall1(nr::FBDRAW, &cmd as *const FbDrawCmd as u64) as i64 };
    Error::from_syscall(ret).map(|value| value as u64)
}

/// Read F32c waitqueue stress counters: entered, returned, wakes, has_waiters.
pub fn wait_stress_stats(out: &mut [u64; 4]) -> Result<(), Error> {
    let ptr = out.as_mut_ptr() as u64;
    let cmd = FbDrawCmd {
        op: draw_op::WAIT_STRESS_STATS,
        p1: ptr as u32 as i32,
        p2: (ptr >> 32) as u32 as i32,
        p3: out.len() as i32,
        p4: 0,
        color: 0,
    };
    fbdraw(&cmd)
}

/// Map a framebuffer buffer into this process's address space.
///
/// Returns a pointer to a compact buffer placed at x 0 that can be drawn to
/// directly, without syscalls. Call `fb_flush()` after drawing to sync
/// the buffer to the screen.
///
/// On aarch64 a process that called `take_over_display()` first gets the whole
/// screen (`width` pixels wide); other processes, and on x86_64 the owner too,
/// get the left half (`left_pane_width()` pixels). The layout is
/// compact: `stride = pane width * bytes_per_pixel`. Use `fb_mmap_pane()` to
/// learn which width was mapped, and `fbinfo()` for the pixel format.
pub fn fb_mmap() -> Result<*mut u8, Error> {
    fb_mmap_pane().map(|(ptr, _)| ptr)
}

/// Like `fb_mmap()`, and also returns the width in pixels of the mapped pane.
pub fn fb_mmap_pane() -> Result<(*mut u8, u64), Error> {
    let mut width: u64 = 0;
    let ret = unsafe { raw::syscall1(nr::FBMMAP, &mut width as *mut u64 as u64) as i64 };
    let ptr = Error::from_syscall(ret)? as *mut u8;
    // A kernel that reports no width (x86_64) maps the left half.
    if width == 0 {
        width = fbinfo()?.left_pane_width();
    }
    Ok((ptr, width))
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

/// A dirty rectangle for batch flushing.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlushRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Batch flush multiple dirty rectangles with a single syscall and DSB barrier.
///
/// Instead of calling `fb_flush_rect()` N times (N syscalls, N DSB barriers),
/// this sends all dirty rects at once: 1 syscall, 1 DSB. Each rect is copied
/// from the mmap buffer to BAR0 in sequence.
pub fn fb_flush_rects(rects: &[FlushRect]) -> Result<(), Error> {
    if rects.is_empty() {
        return Ok(());
    }
    let rects_ptr = rects.as_ptr() as u64;
    let cmd = FbDrawCmd {
        op: draw_op::FLUSH_BATCH,
        p1: rects_ptr as i32,
        p2: (rects_ptr >> 32) as i32,
        p3: rects.len() as i32,
        p4: 0,
        color: 0,
    };
    fbdraw(&cmd)
}

// =============================================================================
// bwm's input events
// =============================================================================

/// bwm's input event: what its stdin parser and pointer loop produce, before its display
/// adapter turns it into a window event of the window interface (`docs/gui/breengel.md`
/// section 19).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct WindowInputEvent {
    /// Event type (see `input_event_type` constants)
    pub event_type: u16,
    /// USB HID keycode or ASCII character
    pub keycode: u16,
    /// Window-local mouse X coordinate
    pub mouse_x: i16,
    /// Window-local mouse Y coordinate
    pub mouse_y: i16,
    /// Modifier bitmask (bit 0=shift, bit 1=ctrl, bit 2=alt)
    pub modifiers: u16,
    /// Scroll wheel delta. Positive = scroll up, negative = scroll down.
    /// Set on MOUSE_MOVE and MOUSE_BUTTON events when the wheel moved this frame.
    /// Also the sole payload for MOUSE_SCROLL events.
    pub scroll_y: i16,
}

/// Input event type constants
pub mod input_event_type {
    pub const KEY_PRESS: u16 = 1;
    pub const KEY_RELEASE: u16 = 2;
    pub const MOUSE_MOVE: u16 = 3;
    pub const MOUSE_BUTTON: u16 = 4;
    pub const FOCUS_GAINED: u16 = 5;
    pub const FOCUS_LOST: u16 = 6;
    pub const CLOSE_REQUESTED: u16 = 7;
    pub const WINDOW_RESIZED: u16 = 8;
    /// Scroll wheel event. `scroll_y` > 0 = scroll up, < 0 = scroll down.
    pub const MOUSE_SCROLL: u16 = 9;
}

/// Get the current mouse cursor position.
///
/// # Returns
/// * Ok((x, y)) - Mouse position in screen coordinates
/// * Err(Error) - Error (ENODEV if no pointer device)
pub fn mouse_pos() -> Result<(u32, u32), Error> {
    let mut state: [u32; 4] = [0, 0, 0, 0];
    let ret = unsafe { raw::syscall2(nr::GET_MOUSE_POS, &mut state as *mut [u32; 4] as u64, 16) as i64 };
    Error::from_syscall(ret)?;
    Ok((state[0], state[1]))
}

/// Get the current mouse cursor position and button state.
///
/// # Returns
/// * Ok((x, y, buttons)) - Mouse position and button state (bit 0 = left button)
/// * Err(Error) - Error (ENODEV if no pointer device)
pub fn mouse_state() -> Result<(u32, u32, u32), Error> {
    let mut state: [u32; 4] = [0, 0, 0, 0];
    let ret = unsafe { raw::syscall2(nr::GET_MOUSE_POS, &mut state as *mut [u32; 4] as u64, 16) as i64 };
    Error::from_syscall(ret)?;
    Ok((state[0], state[1], state[2]))
}

/// Get the current mouse cursor position, button state, and scroll wheel delta.
///
/// The scroll delta is an accumulated signed value: positive = scroll up,
/// negative = scroll down. The kernel resets the accumulator on each read.
///
/// # Returns
/// * Ok((x, y, buttons, scroll_y)) - Mouse position, buttons, and wheel delta
/// * Err(Error) - Error (ENODEV if no pointer device)
pub fn mouse_state_with_scroll() -> Result<(u32, u32, u32, i32), Error> {
    let mut state: [u32; 4] = [0, 0, 0, 0];
    let ret = unsafe { raw::syscall2(nr::GET_MOUSE_POS, &mut state as *mut [u32; 4] as u64, 16) as i64 };
    Error::from_syscall(ret)?;
    Ok((state[0], state[1], state[2], state[3] as i32))
}

/// The keyboard state bwm reads with the pointer: the modifier keys held now (bit 0 Shift,
/// 1 Ctrl, 2 Alt, 3 Super) and how many Super presses the driver latched since the last read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyState {
    pub modifiers: u32,
    pub super_taps: u32,
}

/// Read the modifier keys held and take the latched Super presses: syscall 413 asked for its
/// 8-byte key words instead of its 16-byte pointer words, so the pointer state is untouched.
pub fn key_state() -> Result<KeyState, Error> {
    let mut state: [u32; 2] = [0; 2];
    let ret = unsafe { raw::syscall2(nr::GET_MOUSE_POS, &mut state as *mut [u32; 2] as u64, 8) as i64 };
    Error::from_syscall(ret)?;
    Ok(KeyState { modifiers: state[0], super_taps: state[1] })
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
    /// Map the framebuffer and return a safe handle: the whole screen for the
    /// display owner, else the left half (see `fb_mmap`).
    pub fn new() -> Result<Framebuffer, Error> {
        let info = fbinfo()?;
        let (ptr, width) = fb_mmap_pane()?;
        Ok(Framebuffer {
            ptr,
            width: width as u32,
            height: info.height as u32,
            stride: width as u32 * info.bytes_per_pixel as u32,
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

/// Deactivate the kernel's terminal manager so userspace can take over the display.
///
/// After this call the kernel stops drawing its own screen, and a later `fb_mmap()`
/// by this process maps the whole screen (aarch64). The calling process is
/// responsible for drawing the display. A later call by another process moves the
/// display to it; this process's whole-screen draws then fail with `EPERM`.
pub fn take_over_display() -> Result<(), Error> {
    let result = unsafe { raw::syscall0(nr::TAKE_OVER_DISPLAY) };
    Error::from_syscall(result as i64).map(|_| ())
}

/// Reactivate the kernel's terminal manager after userspace releases the display.
///
/// Called by init when BWM crashes to restore kernel terminal rendering.
pub fn give_back_display() -> Result<(), Error> {
    let result = unsafe { raw::syscall0(nr::GIVE_BACK_DISPLAY) };
    Error::from_syscall(result as i64).map(|_| ())
}
