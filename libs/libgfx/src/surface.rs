//! Surfaces: GPU render targets with a format, a clip stack and damage tracking
//! (docs/gui/breengel.md sections 2, 3, 4 and 6).
//!
//! A surface owns a target made by `RESOURCE_CREATE`; no pixel pointer is accepted and a
//! handle is not an address. Drawing submits BGL commands to the process's context, each
//! primitive at once; `read` waits for them through `READBACK`.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

use libbreenix::gpu::{self, abi};

use crate::color::Color;
use crate::device;

/// The largest width or height a surface may have (the profile's cap on `CAPS`).
pub const MAX_DIMENSION: usize = 16_384;
/// The largest `stride × height` a readback or conversion layout may span, in bytes.
pub const MAX_BYTES: usize = (1 << 31) - 1;

/// A half-open rectangle `x ≤ px < x + w`, `y ≤ py < y + h`; empty when `w ≤ 0` or `h ≤ 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// The damage `take_dirty` reports.
pub type DirtyRect = Rect;

/// A pixel layout: a render target's (4 bytes) or an upload or conversion layout (3 or 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// Bytes per pixel: 3 or 4.
    pub bpp: usize,
    /// Colour bytes in B, G, R order (else R, G, B).
    pub is_bgr: bool,
    /// The fourth byte is premultiplied alpha (needs `bpp = 4`); otherwise it is the X byte.
    pub premultiplied: bool,
}

impl Format {
    /// The canonical opaque format, BGRX8888.
    pub const BGRX: Format = Format { bpp: 4, is_bgr: true, premultiplied: false };
    pub const RGBX: Format = Format { bpp: 4, is_bgr: false, premultiplied: false };
    /// Three-byte layouts: upload and conversion only, not a render target (section 2).
    pub const BGR: Format = Format { bpp: 3, is_bgr: true, premultiplied: false };
    pub const RGB: Format = Format { bpp: 3, is_bgr: false, premultiplied: false };
    /// Premultiplied BGRA8888.
    pub const BGRA_PREMULTIPLIED: Format = Format { bpp: 4, is_bgr: true, premultiplied: true };

    /// The `UPLOAD` source layout of bytes in this format.
    pub(crate) fn upload_source(self) -> u32 {
        match (self.bpp, self.is_bgr) {
            (4, true) => abi::upload_source::B8G8R8A8,
            (4, false) => abi::upload_source::R8G8B8A8,
            (_, true) => abi::upload_source::B8G8R8,
            (_, false) => abi::upload_source::R8G8B8,
        }
    }
}

/// Why a surface, texture or layout could not be made (section 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceError {
    /// Width or height is 0.
    Empty,
    /// Width or height is above the device maximum (at most 16,384).
    TooLarge,
    /// Not a renderable format (a render target needs 4 bytes per pixel).
    Format,
    /// The stride is below `width × bpp`.
    StrideTooShort,
    /// `stride × height` is above 2³¹ − 1 or overflows.
    SizeOverflow,
    /// The GPU core refused the request with this negated errno.
    Device(i64),
}

impl core::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SurfaceError::Empty => f.write_str("width or height is 0"),
            SurfaceError::TooLarge => f.write_str("width or height is above 16384"),
            SurfaceError::Format => f.write_str("unsupported pixel format"),
            SurfaceError::StrideTooShort => f.write_str("stride is below width x bytes per pixel"),
            SurfaceError::SizeOverflow => f.write_str("stride x height is above 2^31 - 1"),
            SurfaceError::Device(e) => write!(f, "the GPU core returned {e}"),
        }
    }
}

/// A box `x0 ≤ x < x1`, `y0 ≤ y < y1` in i64; empty when either side is not positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Box64 {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl Box64 {
    pub(crate) const EMPTY: Box64 = Box64 { x0: 0, y0: 0, x1: 0, y1: 0 };

    /// The box of `(x, y, w, h)`, any i32 values.
    pub(crate) fn of(x: i32, y: i32, w: i32, h: i32) -> Box64 {
        Box64::wide(x as i64, y as i64, w as i64, h as i64)
    }

    /// The box of `(x, y, w, h)` from wider values; a non-positive size is empty.
    pub(crate) fn wide(x: i64, y: i64, w: i64, h: i64) -> Box64 {
        Box64 { x0: x, y0: y, x1: x.saturating_add(w.max(0)), y1: y.saturating_add(h.max(0)) }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub(crate) fn intersect(&self, o: &Box64) -> Box64 {
        let b = Box64 { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) };
        if b.is_empty() { Box64::EMPTY } else { b }
    }

    fn union(&self, o: &Box64) -> Box64 {
        Box64 { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }

    /// As BGL's `[x0, y0, x1, y1]`; the box lies inside a surface, so it fits i32.
    pub(crate) fn corners(&self) -> [i32; 4] {
        [self.x0 as i32, self.y0 as i32, self.x1 as i32, self.y1 as i32]
    }

    fn rect(&self) -> Rect {
        Rect { x: self.x0 as i32, y: self.y0 as i32, w: (self.x1 - self.x0).max(0) as i32, h: (self.y1 - self.y0).max(0) as i32 }
    }
}

/// Counts the submissions this process has made from any surface: a reader that cached a
/// readback knows it is stale when this has moved.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The number of drawing submissions this process has made so far.
pub fn generation() -> u64 {
    GENERATION.load(Relaxed)
}

/// The canonical BGRA colour word of an opaque colour (gpu-abi.md: B in bits 0 to 7).
pub(crate) fn word(c: Color) -> u32 {
    c.b as u32 | (c.g as u32) << 8 | (c.r as u32) << 16 | 0xFF << 24
}

/// A GPU render target (section 3) with its clip stack (section 4) and damage (section 6).
pub struct Surface {
    target: u64,
    width: usize,
    height: usize,
    format: Format,
    clip: Box64,
    clips: Vec<Box64>,
    dirty: Option<Box64>,
    owner: u64,
    owns_handle: bool,
    error: Option<i64>,
    ui_scale: u32,
    render: Option<bgl::Fence>,
}

impl Surface {
    /// Check a readback or conversion layout of `width × height` pixels at `stride` (section 3).
    pub fn validate_layout(width: usize, height: usize, stride: usize, format: Format) -> Result<(), SurfaceError> {
        if width == 0 || height == 0 {
            return Err(SurfaceError::Empty);
        }
        if width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(SurfaceError::TooLarge);
        }
        if !matches!(format.bpp, 3 | 4) || (format.premultiplied && format.bpp != 4) {
            return Err(SurfaceError::Format);
        }
        if stride < width * format.bpp {
            return Err(SurfaceError::StrideTooShort);
        }
        match stride.checked_mul(height) {
            Some(n) if n <= MAX_BYTES => Ok(()),
            _ => Err(SurfaceError::SizeOverflow),
        }
    }

    /// A new `width × height` target in a four-byte `format`, zeroed by the core.
    pub fn new(width: usize, height: usize, format: Format) -> Result<Surface, SurfaceError> {
        if width == 0 || height == 0 {
            return Err(SurfaceError::Empty);
        }
        if width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(SurfaceError::TooLarge);
        }
        if format.bpp != 4 {
            return Err(SurfaceError::Format);
        }
        let device_format = if format.is_bgr { abi::format::B8G8R8A8 } else { abi::format::R8G8B8A8 };
        let target = gpu::target_create(device_format, width as u32, height as u32).map_err(SurfaceError::Device)?;
        let whole = Box64 { x0: 0, y0: 0, x1: width as i64, y1: height as i64 };
        Ok(Surface { target, width, height, format, clip: whole, clips: Vec::new(), dirty: None, owner: device::pid(), owns_handle: true, error: None, render: None, ui_scale: 1 })
    }

    /// A non-owning view of a target returned by window CREATE/RESIZE. The window
    /// revokes the handle; the view does not own a pool buffer.
    /// Core admission still enforces ownership, target dimensions and sealing.
    pub fn from_window_target(target: u64, width: usize, height: usize) -> Result<Self, SurfaceError> {
        Self::validate_layout(width,height,width.checked_mul(4).ok_or(SurfaceError::SizeOverflow)?,Format::BGRX)?;
        if target==0 { return Err(SurfaceError::Device(-abi::errno::EINVAL)); }
        Ok(Self { target,width,height,format:Format::BGRX,
            clip:Box64 {x0:0,y0:0,x1:width as i64,y1:height as i64},clips:Vec::new(),dirty:None,
            owner:device::pid(),owns_handle:false,error:None,render:None,ui_scale:1 })

    }

    /// Physical surfaces default to scale 1. A logical UI view explicitly selects
    /// its display scale; raster APIs continue to take physical pixel coordinates.
    pub fn set_ui_scale(&mut self, scale: u32) { self.ui_scale = scale.clamp(1, 2); }
    pub fn ui_scale(&self) -> u32 { self.ui_scale }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// The target's resource handle.
    pub fn handle(&self) -> u64 {
        self.target
    }

    /// The first error the core returned for a submission on this surface, if any.
    pub fn error(&self) -> Option<i64> {
        self.error
    }

    // ---- the clip stack (section 4): emitted as GPU scissors ------------------------

    /// Set the clip to the intersection of `rect` and the current clip.
    pub fn push_clip(&mut self, rect: Rect) {
        self.clips.push(self.clip);
        self.clip = self.clip.intersect(&Box64::of(rect.x, rect.y, rect.w, rect.h));
    }

    /// Restore the clip from before the matching `push_clip`; no change without one.
    pub fn pop_clip(&mut self) {
        if let Some(c) = self.clips.pop() {
            self.clip = c;
        }
    }

    /// The current clip (the whole surface while the clip stack is empty).
    pub fn clip(&self) -> Rect {
        self.clip.rect()
    }

    pub(crate) fn clip_box(&self) -> Box64 {
        self.clip
    }

    // ---- damage (section 6) ---------------------------------------------------------

    /// Mark `(x, y, w, h)`, intersected with the surface and clip, as written.
    pub fn mark_dirty(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.mark(Box64::of(x, y, w, h));
    }

    pub(crate) fn mark(&mut self, b: Box64) {
        let b = b.intersect(&self.clip);
        if b.is_empty() {
            return;
        }
        self.dirty = Some(match self.dirty {
            Some(d) => d.union(&b),
            None => b,
        });
    }

    /// The union of the marks since the last call, and reset it to empty.
    pub fn take_dirty(&mut self) -> Option<DirtyRect> {
        self.dirty.take().map(|d| d.rect())
    }

    // ---- submission -----------------------------------------------------------------

    /// Encode `draw` for this target under the current clip and submit it. Callers pass only
    /// geometry already clipped to something visible: a primitive clipped to an empty box
    /// submits no command (section 7).
    pub(crate) fn submit(&mut self, draw: impl FnOnce(&mut bgl::Context)) {
        let (target, w, h, clip) = (self.target, self.width as u32, self.height as u32, self.clip);
        let whole = clip == Box64 { x0: 0, y0: 0, x1: w as i64, y1: h as i64 };
        let result = device::with(|d| {
            let context = &mut d.context;
            context.begin(target, w, h);
            if !whole {
                context.set_clip(clip.corners());
            }
            draw(context);
            context.submit().inspect_err(|_| context.discard())
        });
        GENERATION.fetch_add(1, Relaxed);
        match result {
            Ok(fence) => self.render = Some(fence),
            Err(e) => { self.error.get_or_insert(e); }
        }
    }

    /// Record a failure to prepare a draw (a table upload) as this surface's error.
    pub(crate) fn fail(&mut self, e: i64) {
        self.error.get_or_insert(e);
    }

    /// Wait for the most recent draw on this surface. The process context orders the
    /// surface's draws; window COMMIT separately seals writers from other contexts.
    pub fn wait(&self) -> Result<(), i64> {
        if let Some(error) = self.error { return Err(error); }
        self.render.map_or(Ok(()), |fence| fence.wait(u64::MAX))
    }

    // ---- pixels ---------------------------------------------------------------------

    /// Write one opaque pixel inside the surface and clip: one `solid` instance.
    pub fn put_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x > i64::MAX as usize || y > i64::MAX as usize {
            return;
        }
        let b = Box64::wide(x as i64, y as i64, 1, 1).intersect(&self.clip);
        if b.is_empty() {
            return;
        }
        crate::work::add(1);
        self.submit(|c| c.solid(word(color), &[b.corners()]));
        self.mark(b);
    }

    /// Fill the current clip (the whole surface while the clip stack is empty) with `color` on the
    /// GPU: a clear of the whole target, or one `solid` instance over a smaller clip.
    pub fn clear(&mut self, color: Color) {
        let c = self.clip;
        if c.is_empty() {
            return;
        }
        crate::work::add(1);
        let whole = c == Box64 { x0: 0, y0: 0, x1: self.width as i64, y1: self.height as i64 };
        self.submit(|ctx| if whole { ctx.clear(word(color)) } else { ctx.solid(word(color), &[c.corners()]) });
        self.mark(c);
    }

    /// Read the whole surface back as canonical BGRA rows `stride` bytes apart, after the
    /// submissions that write it; the row padding is left as it was (section 3).
    pub fn read(&self, pixels: &mut [u8], stride: usize) -> Result<(), i64> {
        let stride = u32::try_from(stride).map_err(|_| -abi::errno::EINVAL)?;
        let mut record = [0u8; abi::record::SIZE];
        match gpu::readback(self.target, [0, 0, self.width as u32, self.height as u32], pixels, stride, &mut record) {
            0 => Ok(()),
            e => Err(e),
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // A forked child holds no GPU handles of its parent's (section 17.2).
        if self.owns_handle && self.owner == device::pid() {
            gpu::resource_destroy(self.target);
        }
    }
}

/// Convert canonical BGRA rows (a completed readback) into `format`'s layout: the channel
/// order of `format`, the fourth byte passed through for four-byte layouts and dropped for
/// three-byte ones. Capture and encoding output only; not drawing (section 2).
pub fn convert(
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
    dst: &mut [u8],
    dst_stride: usize,
    format: Format,
) -> Result<(), SurfaceError> {
    Surface::validate_layout(width, height, dst_stride, format)?;
    Surface::validate_layout(width, height, src_stride, Format::BGRX)?;
    if src.len() < (height - 1) * src_stride + width * 4 || dst.len() < (height - 1) * dst_stride + width * format.bpp {
        return Err(SurfaceError::SizeOverflow);
    }
    for y in 0..height {
        let (s, d) = (&src[y * src_stride..][..width * 4], &mut dst[y * dst_stride..][..width * format.bpp]);
        for (p, q) in s.chunks_exact(4).zip(d.chunks_exact_mut(format.bpp)) {
            let (b0, b2) = if format.is_bgr { (p[0], p[2]) } else { (p[2], p[0]) };
            q[0] = b0;
            q[1] = p[1];
            q[2] = b2;
            if format.bpp == 4 {
                q[3] = p[3];
            }
        }
    }
    Ok(())
}
