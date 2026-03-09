//! Graphics syscall wrappers
//!
//! Provides userspace API for querying framebuffer information and drawing.
//!
//! Also provides the [`Framebuffer`] RAII wrapper for direct pixel writes
//! via memory-mapped access.

use crate::errno::Errno;
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
    /// Submit a VirGL GPU-rendered frame
    pub const VIRGL_SUBMIT_FRAME: u32 = 7;
    /// Batch flush multiple dirty rects with one DSB barrier
    pub const FLUSH_BATCH: u32 = 8;
    /// Submit VirGL GPU-rendered rectangles
    pub const VIRGL_SUBMIT_RECTS: u32 = 9;
    /// Composite a pixel buffer as a full-screen GPU texture
    pub const VIRGL_COMPOSITE: u32 = 10;
    /// Create a shared window pixel buffer
    pub const CREATE_WINDOW_BUFFER: u32 = 11;
    /// Register a window buffer with the compositor
    pub const REGISTER_WINDOW: u32 = 12;
    /// List all registered windows
    pub const LIST_WINDOWS: u32 = 13;
    /// Read a window's pixel data
    pub const READ_WINDOW_BUFFER: u32 = 14;
    /// Mark a window buffer as dirty (bump generation counter)
    pub const MARK_WINDOW_DIRTY: u32 = 15;
    /// Multi-window GPU compositing
    pub const COMPOSITE_WINDOWS: u32 = 16;
    /// Set window position for compositor
    pub const SET_WINDOW_POSITION: u32 = 17;
    /// Write input events to a window's ring buffer (BWM → client)
    pub const WRITE_WINDOW_INPUT: u32 = 18;
    /// Read input events from a window's ring buffer (client ← BWM)
    pub const READ_WINDOW_INPUT: u32 = 19;
    /// Map compositor texture into caller's address space
    pub const MAP_COMPOSITOR_TEXTURE: u32 = 20;
}

/// Ball descriptor for VirGL GPU rendering.
/// Must match kernel's VirglBall in drivers/virtio/gpu_pci.rs.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirglBall {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub color: [f32; 4],
}

/// Frame descriptor passed to the VirglSubmitFrame syscall.
#[repr(C)]
pub struct VirglFrameDesc {
    pub ball_count: u32,
    pub _pad: u32,
    pub balls: [VirglBall; 16],
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

/// Rectangle descriptor for VirGL GPU rendering.
/// Must match kernel's VirglRect in drivers/virtio/gpu_pci.rs.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirglRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// Frame descriptor passed to the VirglSubmitRects syscall.
#[repr(C)]
pub struct VirglRectDesc {
    pub rect_count: u32,
    pub _pad: u32,
    pub rects: [VirglRect; 60],
}

/// Submit GPU-rendered rectangles via VirGL DRAW_VBO.
///
/// Each rect is drawn as a colored quad on the GPU. Background is cleared
/// to bg_color first. Zero guest CPU pixel writes.
pub fn virgl_submit_rects(rects: &[VirglRect], bg_color: u32) -> Result<(), Error> {
    let mut desc = VirglRectDesc {
        rect_count: rects.len().min(60) as u32,
        _pad: 0,
        rects: [VirglRect::default(); 60],
    };
    for (i, rect) in rects.iter().take(60).enumerate() {
        desc.rects[i] = *rect;
    }
    let desc_ptr = &desc as *const VirglRectDesc as u64;
    let cmd = FbDrawCmd {
        op: draw_op::VIRGL_SUBMIT_RECTS,
        p1: desc_ptr as i32,
        p2: (desc_ptr >> 32) as i32,
        p3: 0,
        p4: 0,
        color: bg_color,
    };
    fbdraw(&cmd)
}

/// Submit a VirGL GPU-rendered frame.
///
/// Sends ball positions/colors to the kernel, which renders them via the host
/// GPU and DMA-copies the result to display memory. Zero guest CPU pixel writes.
pub fn virgl_submit_frame(balls: &[VirglBall], bg_color: u32) -> Result<(), Error> {
    let mut desc = VirglFrameDesc {
        ball_count: balls.len().min(16) as u32,
        _pad: 0,
        balls: [VirglBall::default(); 16],
    };
    for (i, ball) in balls.iter().take(16).enumerate() {
        desc.balls[i] = *ball;
    }
    let desc_ptr = &desc as *const VirglFrameDesc as u64;
    let cmd = FbDrawCmd {
        op: draw_op::VIRGL_SUBMIT_FRAME,
        p1: desc_ptr as i32,               // low 32 bits
        p2: (desc_ptr >> 32) as i32,        // high 32 bits
        p3: 0,
        p4: 0,
        color: bg_color,
    };
    fbdraw(&cmd)
}

/// Composite a CPU-rendered pixel buffer as a full-screen GPU texture.
///
/// Uploads the BGRA pixel buffer to the GPU and renders it as a full-screen
/// textured quad via VirGL. This is the primary compositing path for BWM —
/// render window contents to a pixel buffer, then display via the GPU.
///
/// # Arguments
/// * `pixels` - BGRA pixel data (one u32 per pixel, width × height elements)
/// * `width` - Width of the pixel buffer
/// * `height` - Height of the pixel buffer
pub fn virgl_composite(pixels: &[u32], width: u32, height: u32) -> Result<(), Error> {
    let buf_ptr = pixels.as_ptr() as u64;
    let cmd = FbDrawCmd {
        op: draw_op::VIRGL_COMPOSITE,
        p1: buf_ptr as i32,
        p2: (buf_ptr >> 32) as i32,
        p3: width as i32,
        p4: height as i32,
        color: 0,
    };
    fbdraw(&cmd)
}

// =============================================================================
// Window Buffer API — for GPU-composited window management
// =============================================================================

/// A window buffer backed by MAP_SHARED physical pages.
pub struct WindowBuffer {
    /// Unique buffer ID (used for registration and IPC)
    pub id: u32,
    /// Pointer to the mapped pixel buffer (BGRA u32)
    pub pixels: *mut u32,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
}

/// Info about a registered window (returned by list_windows).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WindowInfo {
    pub buffer_id: u32,
    pub owner_pid: u32,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub title_len: u32,
    pub title: [u8; 64],
}

impl Default for WindowInfo {
    fn default() -> Self {
        Self {
            buffer_id: 0,
            owner_pid: 0,
            width: 0,
            height: 0,
            x: 0,
            y: 0,
            title_len: 0,
            title: [0; 64],
        }
    }
}

/// Create a window buffer backed by shared physical pages.
///
/// Returns a WindowBuffer with a mapped pixel pointer and buffer ID.
/// The buffer can be registered with the compositor via `register_window()`.
pub fn create_window(width: u32, height: u32) -> Result<WindowBuffer, Error> {
    let mut mmap_addr: u64 = 0;
    let out_ptr = &mut mmap_addr as *mut u64 as u64;
    let cmd = FbDrawCmd {
        op: draw_op::CREATE_WINDOW_BUFFER,
        p1: width as i32,
        p2: height as i32,
        p3: out_ptr as i32,
        p4: (out_ptr >> 32) as i32,
        color: 0,
    };
    let ret = unsafe { raw::syscall1(nr::FBDRAW, &cmd as *const FbDrawCmd as u64) as i64 };
    if ret < 0 {
        return Err(Error::Os(Errno::from_raw(-ret)));
    }
    let buffer_id = ret as u32;
    Ok(WindowBuffer {
        id: buffer_id,
        pixels: mmap_addr as *mut u32,
        width,
        height,
    })
}

/// Register a window buffer with the compositor, making it visible.
pub fn register_window(buffer_id: u32, title: &[u8]) -> Result<(), Error> {
    let title_ptr = title.as_ptr() as u64;
    let cmd = FbDrawCmd {
        op: draw_op::REGISTER_WINDOW,
        p1: buffer_id as i32,
        p2: title_ptr as i32,
        p3: (title_ptr >> 32) as i32,
        p4: title.len() as i32,
        color: 0,
    };
    fbdraw(&cmd)
}

/// List all registered windows. Returns the number of windows found.
pub fn list_windows(out: &mut [WindowInfo]) -> Result<usize, Error> {
    let out_ptr = out.as_ptr() as u64;
    let cmd = FbDrawCmd {
        op: draw_op::LIST_WINDOWS,
        p1: out_ptr as i32,
        p2: (out_ptr >> 32) as i32,
        p3: out.len() as i32,
        p4: 0,
        color: 0,
    };
    let ret = unsafe { raw::syscall1(nr::FBDRAW, &cmd as *const FbDrawCmd as u64) as i64 };
    if ret < 0 {
        return Err(Error::Os(Errno::from_raw(-ret)));
    }
    Ok(ret as usize)
}

/// Read a window's pixel data into a local buffer.
///
/// Returns (width, height) of the window. The pixel data is copied to `dst`.
pub fn read_window_buffer(buffer_id: u32, dst: &mut [u8]) -> Result<(u32, u32), Error> {
    let dst_ptr = dst.as_ptr() as u64;
    let cmd = FbDrawCmd {
        op: draw_op::READ_WINDOW_BUFFER,
        p1: buffer_id as i32,
        p2: dst_ptr as i32,
        p3: (dst_ptr >> 32) as i32,
        p4: dst.len() as i32,
        color: 0,
    };
    let ret = unsafe { raw::syscall1(nr::FBDRAW, &cmd as *const FbDrawCmd as u64) as i64 };
    if ret < 0 {
        return Err(Error::Os(Errno::from_raw(-ret)));
    }
    let result = ret as u64;
    let width = (result >> 32) as u32;
    let height = (result & 0xFFFF_FFFF) as u32;
    Ok((width, height))
}

/// Mark a window buffer as dirty (bump generation counter).
///
/// Call this after writing pixels to the window buffer to signal the compositor
/// that the window's texture needs re-uploading.
pub fn mark_window_dirty(buffer_id: u32) -> Result<(), Error> {
    let cmd = FbDrawCmd {
        op: draw_op::MARK_WINDOW_DIRTY,
        p1: buffer_id as i32,
        p2: 0,
        p3: 0,
        p4: 0,
        color: 0,
    };
    fbdraw(&cmd)
}

/// Set window position in the compositor.
///
/// Tells the kernel where to place this window during compositing.
/// If position is never set, windows are auto-positioned.
pub fn set_window_position(buffer_id: u32, x: i32, y: i32) -> Result<(), Error> {
    let packed_xy = ((x as u16 as u32) | ((y as u16 as u32) << 16)) as i32;
    let cmd = FbDrawCmd {
        op: draw_op::SET_WINDOW_POSITION,
        p1: buffer_id as i32,
        p2: packed_xy,
        p3: 0,
        p4: 0,
        color: 0,
    };
    fbdraw(&cmd)
}

// =============================================================================
// Window input events (Breengel input pipeline)
// =============================================================================

/// Input event written by BWM into a window's kernel ring buffer.
/// Read by client apps via `read_window_input()`.
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
    pub _pad: u16,
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
}

/// Write an input event to a window's kernel ring buffer.
///
/// Called by BWM to route keyboard/mouse events to the focused window.
/// If the client is blocked on `read_window_input`, it will be woken.
pub fn write_window_input(buffer_id: u32, event: &WindowInputEvent) -> Result<(), Error> {
    let event_ptr = event as *const WindowInputEvent as u64;
    let cmd = FbDrawCmd {
        op: draw_op::WRITE_WINDOW_INPUT,
        p1: buffer_id as i32,
        p2: event_ptr as i32,
        p3: (event_ptr >> 32) as i32,
        p4: 0,
        color: 0,
    };
    fbdraw(&cmd)
}

/// Read input events from a window's kernel ring buffer.
///
/// Returns the number of events read. If `blocking` is true, blocks until
/// at least one event is available (with 100ms timeout). If false, returns
/// immediately with 0 if no events are pending.
pub fn read_window_input(buffer_id: u32, out: &mut [WindowInputEvent], blocking: bool) -> Result<usize, Error> {
    let out_ptr = out.as_ptr() as u64;
    let cmd = FbDrawCmd {
        op: draw_op::READ_WINDOW_INPUT,
        p1: buffer_id as i32,
        p2: out_ptr as i32,
        p3: (out_ptr >> 32) as i32,
        p4: out.len() as i32,
        color: if blocking { 0 } else { 1 }, // bit 0 = non-blocking
    };
    let ret = unsafe { raw::syscall1(nr::FBDRAW, &cmd as *const FbDrawCmd as u64) as i64 };
    if ret < 0 {
        return Err(Error::Os(Errno::from_raw(-ret)));
    }
    Ok(ret as usize)
}

// =============================================================================
// Multi-window compositing
// =============================================================================

/// Descriptor for multi-window GPU compositing.
///
/// bg_dirty values:
///   0 = no background change (cursor-only update, frame pacing)
///   1 = full background upload (entire buffer changed)
///   2 = partial background upload (only dirty_rect region changed)
#[repr(C)]
pub struct CompositeWindowsDesc {
    pub bg_pixels_ptr: u64,
    pub bg_width: u32,
    pub bg_height: u32,
    pub bg_dirty: u32,
    pub num_dirty_rects: u32,
    pub dirty_x: u32,
    pub dirty_y: u32,
    pub dirty_w: u32,
    pub dirty_h: u32,
}

/// Composite all registered windows via the GPU in a single batch.
///
/// bg_dirty=0: no background change (cursor/frame-pacing only)
/// bg_dirty=1: full background upload
/// bg_dirty=2: partial upload — only the (dirty_x, dirty_y, dirty_w, dirty_h) region
pub fn virgl_composite_windows(bg_pixels: &[u32], bg_w: u32, bg_h: u32, bg_dirty: bool) -> Result<(), Error> {
    virgl_composite_windows_rect(bg_pixels, bg_w, bg_h, if bg_dirty { 1 } else { 0 }, 0, 0, bg_w, bg_h)
}

/// Composite with a partial dirty rect. Only the specified sub-region of the
/// background buffer is copied and uploaded to the GPU texture.
pub fn virgl_composite_windows_rect(
    bg_pixels: &[u32], bg_w: u32, bg_h: u32,
    dirty_mode: u32, dirty_x: u32, dirty_y: u32, dirty_w: u32, dirty_h: u32,
) -> Result<(), Error> {
    let desc = CompositeWindowsDesc {
        bg_pixels_ptr: if bg_pixels.is_empty() { 0 } else { bg_pixels.as_ptr() as u64 },
        bg_width: bg_w,
        bg_height: bg_h,
        bg_dirty: dirty_mode,
        num_dirty_rects: if dirty_mode == 2 { 1 } else { 0 },
        dirty_x,
        dirty_y,
        dirty_w,
        dirty_h,
    };
    let desc_ptr = &desc as *const CompositeWindowsDesc as u64;
    let cmd = FbDrawCmd {
        op: draw_op::COMPOSITE_WINDOWS,
        p1: desc_ptr as i32,
        p2: (desc_ptr >> 32) as i32,
        p3: 0,
        p4: 0,
        color: 0,
    };
    fbdraw(&cmd)
}

/// Map the compositor texture backing into this process's address space.
///
/// Returns a pointer to the mapped memory along with the texture dimensions.
/// BWM can write pixels directly into this memory — they are the GPU texture
/// backing pages. This eliminates the kernel-side copy in the composite syscall.
///
/// # Returns
/// * Ok((ptr, width, height)) - Mapped pointer + dimensions
/// * Err(Error) - Error if compositor texture is not initialized
pub fn map_compositor_texture() -> Result<(*mut u32, u32, u32), Error> {
    let mut mapped_addr: u64 = 0;
    let out_ptr = &mut mapped_addr as *mut u64 as u64;
    let cmd = FbDrawCmd {
        op: draw_op::MAP_COMPOSITOR_TEXTURE,
        p1: out_ptr as i32,
        p2: (out_ptr >> 32) as i32,
        p3: 0,
        p4: 0,
        color: 0,
    };
    let ret = unsafe { raw::syscall1(nr::FBDRAW, &cmd as *const FbDrawCmd as u64) as i64 };
    if ret < 0 {
        return Err(Error::Os(Errno::from_raw(-ret)));
    }
    let result = ret as u64;
    let width = (result >> 32) as u32;
    let height = (result & 0xFFFF_FFFF) as u32;
    Ok((mapped_addr as *mut u32, width, height))
}

/// Get the current mouse cursor position.
///
/// # Returns
/// * Ok((x, y)) - Mouse position in screen coordinates
/// * Err(Error) - Error (ENODEV if no pointer device)
pub fn mouse_pos() -> Result<(u32, u32), Error> {
    let mut state: [u32; 3] = [0, 0, 0];
    let ret = unsafe { raw::syscall1(nr::GET_MOUSE_POS, &mut state as *mut [u32; 3] as u64) as i64 };
    Error::from_syscall(ret)?;
    Ok((state[0], state[1]))
}

/// Get the current mouse cursor position and button state.
///
/// # Returns
/// * Ok((x, y, buttons)) - Mouse position and button state (bit 0 = left button)
/// * Err(Error) - Error (ENODEV if no pointer device)
pub fn mouse_state() -> Result<(u32, u32, u32), Error> {
    let mut state: [u32; 3] = [0, 0, 0];
    let ret = unsafe { raw::syscall1(nr::GET_MOUSE_POS, &mut state as *mut [u32; 3] as u64) as i64 };
    Error::from_syscall(ret)?;
    Ok((state[0], state[1], state[2]))
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

/// Deactivate the kernel's terminal manager so userspace can take over the display.
///
/// After this call, the kernel will no longer render to the right-side terminal pane.
/// The calling process is responsible for all display rendering via fb_mmap.
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
