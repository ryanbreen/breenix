//! Blits between surfaces (docs/gui/breengel.md section 5.8).
//!
//! Each blit writes the destination pixels of its source rectangle that are inside the
//! destination surface and clip, and marks `(x, y, w·n, h·n)`.

use crate::alpha::{over, premultiply};
use crate::framebuf::{Box64, FrameBuf};
use crate::work;

/// The destination box of a `w × h` source at (x, y) with scale `n`, and its visible part.
fn target(dst: &FrameBuf, src: &FrameBuf, x: i32, y: i32, n: i64) -> (Box64, Box64) {
    let (w, h) = src.dims();
    let (x, y) = (x as i64, y as i64);
    let b = Box64 { x0: x, y0: y, x1: x + w as i64 * n, y1: y + h as i64 * n };
    (b, b.intersect(&dst.clip_box()))
}

/// Run `f(src_x, src_y, dst_x, dst_y)` over every visible destination pixel.
fn each(dst: &mut FrameBuf, src: &FrameBuf, x: i32, y: i32, n: i64, mut f: impl FnMut(&mut FrameBuf, usize, usize, usize, usize)) {
    let (b, v) = target(dst, src, x, y, n);
    if v.is_empty() {
        return;
    }
    work::add(v.height() as u64);
    for py in v.y0..v.y1 {
        let sy = ((py - b.y0) / n) as usize;
        work::add(v.width() as u64);
        for px in v.x0..v.x1 {
            let sx = ((px - b.x0) / n) as usize;
            f(dst, sx, sy, px as usize, py as usize);
        }
    }
    dst.mark(b);
}

/// Copy `src` to (x, y): each destination pixel becomes the source pixel, converted between
/// formats channel by channel (alpha copied between premultiplied surfaces, 0xFF otherwise).
pub fn blit(dst: &mut FrameBuf, src: &FrameBuf, x: i32, y: i32) {
    blit_scaled(dst, src, x, y, 1);
}

/// Source-over of a premultiplied BGRA `src` (an opaque source counts as alpha 255).
pub fn blit_over(dst: &mut FrameBuf, src: &FrameBuf, x: i32, y: i32) {
    each(dst, src, x, y, 1, |dst, sx, sy, dx, dy| {
        let out = over(src.get_bgra(sx, sy), dst.get_bgra(dx, dy));
        let px = dst.encode(out);
        dst.write(dx, dy, px);
    });
}

/// Source-over of a straight-alpha source (the fourth byte is straight alpha): each source
/// pixel is premultiplied once, `div255(c × a)`, then blended with `over`.
pub fn blit_straight(dst: &mut FrameBuf, src: &FrameBuf, x: i32, y: i32) {
    each(dst, src, x, y, 1, |dst, sx, sy, dx, dy| {
        let [b, g, r, a] = straight(src, sx, sy);
        let s = [premultiply(b, a), premultiply(g, a), premultiply(r, a), a];
        let out = over(s, dst.get_bgra(dx, dy));
        let px = dst.encode(out);
        dst.write(dx, dy, px);
    });
}

/// A source pixel as straight `[b, g, r, a]`: alpha is the fourth byte of a 4-byte surface.
fn straight(src: &FrameBuf, x: usize, y: usize) -> [u8; 4] {
    let [b, g, r, _] = src.get_bgra(x, y);
    let a = if src.format().bpp == 4 { src.read(x, y)[3] } else { 0xFF };
    [b, g, r, a]
}

/// `blit` with each source pixel `(i, j)` filling the `n × n` square at
/// `(x + i·n, y + j·n)`; nothing for `n < 1`.
pub fn blit_scaled(dst: &mut FrameBuf, src: &FrameBuf, x: i32, y: i32, n: i32) {
    if n < 1 {
        return;
    }
    each(dst, src, x, y, n as i64, |dst, sx, sy, dx, dy| {
        let px = dst.encode(src.get_bgra(sx, sy));
        dst.write(dx, dy, px);
    });
}
