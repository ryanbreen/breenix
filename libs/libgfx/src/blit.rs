//! Blits (docs/gui/breengel.md section 5.8): a GPU region copy (`copy`), source-over of a
//! premultiplied source (`over`), of a straight-alpha source premultiplied once (`over_straight`),
//! and integer scaling (`copy_scaled`). A source is a surface or a texture; each blit clips
//! the destination rectangle in i64 and anchors the pipeline's origins at the visible box, so
//! the parameters stay small whatever the coordinates.

use bgl::{pipeline, DeclaredFixture, Instance};
use libbreenix::gpu::{self, abi};

use crate::device;
use crate::surface::{Box64, Format, Surface, SurfaceError};
use crate::work;

/// A resource a blit can sample.
pub trait Source {
    /// The resource handle.
    fn resource(&self) -> u64;
    fn width(&self) -> usize;
    fn height(&self) -> usize;
}

impl Source for Surface {
    fn resource(&self) -> u64 {
        self.handle()
    }
    fn width(&self) -> usize {
        Surface::width(self)
    }
    fn height(&self) -> usize {
        Surface::height(self)
    }
}

/// A sampled image uploaded from a declared static table: the only CPU pixels a blit source
/// is made from (docs/gui/gpu.md section 2.4).
pub struct Texture {
    handle: u64,
    width: usize,
    height: usize,
    owner: u64,
}

impl Texture {
    /// A `width × height` texture holding `table`, whose rows are `width × layout.bpp` bytes
    /// in `layout` (straight or premultiplied as the blit that samples it expects).
    pub fn from_fixture(table: DeclaredFixture, width: usize, height: usize, layout: Format) -> Result<Texture, SurfaceError> {
        Surface::validate_layout(width, height, width * layout.bpp, layout)?;
        if table.len() != width * height * layout.bpp {
            return Err(SurfaceError::SizeOverflow);
        }
        let handle = gpu::texture_create(abi::format::B8G8R8A8, width as u32, height as u32).map_err(SurfaceError::Device)?;
        let texture = Texture { handle, width, height, owner: device::pid() };
        let rect = [0, 0, width as u32, height as u32];
        let stride = (width * layout.bpp) as u32;
        device::with(|d| bgl::fixture_upload(table, &d.context, handle, rect, stride, layout.upload_source()))
            .map_err(SurfaceError::Device)?;
        Ok(texture)
    }
}

impl Source for Texture {
    fn resource(&self) -> u64 {
        self.handle
    }
    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        if self.owner == device::pid() {
            gpu::resource_destroy(self.handle);
        }
    }
}

/// The visible destination box of `src` placed at `(x, y)`, scaled by `n`, and that box.
fn placed(dst: &Surface, src: &dyn Source, x: i32, y: i32, n: i64) -> (Box64, Box64) {
    let whole = Box64::wide(x as i64, y as i64, src.width() as i64 * n, src.height() as i64 * n);
    (whole, whole.intersect(&dst.clip_box()))
}

/// One instance of a sampling pipeline: `params` are its origins, already anchored.
fn sampled(dst: &mut Surface, pipe: u32, texture: u64, visible: Box64, params: &[i64]) {
    let mut p = [0u32; 16];
    for (slot, v) in p.iter_mut().zip(params) {
        *slot = *v as i32 as u32;
    }
    dst.submit(|c| c.draw(pipe, 0, texture, 0, 0, &[Instance { rect: visible.corners(), params: p }]));
}

/// Destination pixel `(x + i, y + j)` becomes source pixel `(i, j)`, converted by channel,
/// for each source pixel whose destination is visible: one `copy` instance.
pub fn blit(dst: &mut Surface, src: &dyn Source, x: i32, y: i32) {
    let (whole, v) = placed(dst, src, x, y, 1);
    if v.is_empty() {
        return;
    }
    work::add(1);
    let (sx, sy) = ((v.x0 - x as i64) as i32, (v.y0 - y as i64) as i32);
    let [x0, y0, x1, y1] = v.corners();
    let texture = src.resource();
    dst.submit(|c| c.copy(texture, &[[x0, y0, x1, y1, sx, sy]]));
    dst.mark(whole);
}

/// Source-over of a premultiplied BGRA source (section 2): one `over` instance.
pub fn blit_over(dst: &mut Surface, src: &dyn Source, x: i32, y: i32) {
    blend(dst, src, x, y, pipeline::OVER);
}

/// Source-over of a straight-alpha source, each pixel premultiplied once: one `over_straight`
/// instance.
pub fn blit_straight(dst: &mut Surface, src: &dyn Source, x: i32, y: i32) {
    blend(dst, src, x, y, pipeline::OVER_STRAIGHT);
}

fn blend(dst: &mut Surface, src: &dyn Source, x: i32, y: i32, pipe: u32) {
    let (whole, v) = placed(dst, src, x, y, 1);
    if v.is_empty() {
        return;
    }
    work::add(1);
    sampled(dst, pipe, src.resource(), v, &[v.x0, v.y0, v.x0 - x as i64, v.y0 - y as i64]);
    dst.mark(whole);
}

/// Source pixel `(i, j)` fills the `n × n` square at `(x + i·n, y + j·n)`, `n ≥ 1`: one
/// `copy_scaled` instance, its destination origin moved by whole source pixels to the
/// visible box's first covered pixel.
pub fn blit_scaled(dst: &mut Surface, src: &dyn Source, x: i32, y: i32, n: i32) {
    if n < 1 {
        return;
    }
    let n64 = n as i64;
    let (whole, v) = placed(dst, src, x, y, n64);
    if v.is_empty() {
        return;
    }
    work::add(1);
    let (kx, ky) = ((v.x0 - x as i64) / n64, (v.y0 - y as i64) / n64);
    let (ox, oy) = (x as i64 + kx * n64, y as i64 + ky * n64);
    sampled(dst, pipeline::COPY_SCALED, src.resource(), v, &[ox, oy, kx, ky, n64]);
    dst.mark(whole);
}
