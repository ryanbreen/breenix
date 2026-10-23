//! Shapes (docs/gui/breengel.md section 5): each primitive clips its bounding box against the
//! surface and the clip in i64, emits no instance when that is empty, and otherwise one instance
//! of its profile pipeline over the visible box (four `solid` rectangles for `draw_rect`).
//! The device evaluates each pixel's membership by the closed forms of gpu.md section 1.5.1,
//! so no path is walked on the CPU. CPU geometry setup is a constant number of steps per
//! primitive (section 7).

use bgl::{pipeline, Instance};

use crate::color::Color;
use crate::surface::{word, Box64, Surface};
use crate::work;

/// One instance of a shape pipeline over `visible`, with parameters `params`.
fn shape(fb: &mut Surface, pipe: u32, colour: u32, visible: Box64, params: &[i32]) {
    let mut p = [0u32; 16];
    for (slot, v) in p.iter_mut().zip(params) {
        *slot = *v as u32;
    }
    fb.submit_shape(|c| c.draw(pipe, colour, 0, 0, 0, &[Instance { rect: visible.corners(), params: p }]));
}

/// The pixels of the half-open rectangle `(x, y, w, h)` (section 5.1): one `solid` instance.
pub fn fill_rect(fb: &mut Surface, x: i32, y: i32, w: i32, h: i32, color: Color) {
    let r = Box64::of(x, y, w, h).intersect(&fb.clip_box());
    if r.is_empty() {
        return;
    }
    work::add(1);
    fb.submit_shape(|c| c.solid(word(color), &[r.corners()]));
    fb.mark(r);
}

/// The one-pixel perimeter of `(x, y, w, h)` (section 5.2): its visible edges as `solid`
/// rectangles in one draw. Marks the whole rectangle.
pub fn draw_rect(fb: &mut Surface, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let r = Box64::of(x, y, w, h);
    let clip = fb.clip_box();
    if r.intersect(&clip).is_empty() {
        return;
    }
    let edges = [
        Box64 { x0: r.x0, y0: r.y0, x1: r.x1, y1: r.y0 + 1 },
        Box64 { x0: r.x0, y0: r.y1 - 1, x1: r.x1, y1: r.y1 },
        Box64 { x0: r.x0, y0: r.y0 + 1, x1: r.x0 + 1, y1: r.y1 - 1 },
        Box64 { x0: r.x1 - 1, y0: r.y0 + 1, x1: r.x1, y1: r.y1 - 1 },
    ];
    work::add(edges.len() as u64);
    let mut visible = [[0i32; 4]; 4];
    let mut n = 0;
    for e in edges {
        let b = e.intersect(&clip);
        if !b.is_empty() {
            visible[n] = b.corners();
            n += 1;
        }
    }
    if n > 0 {
        fb.submit_shape(|c| c.solid(word(color), &visible[..n]));
    }
    fb.mark(r);
}

/// The pixels of section 5.3's integer walk from `(x0, y0)` to `(x1, y1)`, both endpoints
/// included, over the visible part of its bounding box: one `line` instance.
pub fn draw_line(fb: &mut Surface, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
    let (ax, ay, bx, by) = (x0 as i64, y0 as i64, x1 as i64, y1 as i64);
    let bbox = Box64 { x0: ax.min(bx), y0: ay.min(by), x1: ax.max(bx) + 1, y1: ay.max(by) + 1 };
    let visible = bbox.intersect(&fb.clip_box());
    if visible.is_empty() {
        return;
    }
    work::add(1);
    shape(fb, pipeline::LINE, word(color), visible, &[x0, y0, x1, y1]);
    fb.mark(bbox);
}

/// The circle's bounding box `(cx − r, cy − r, 2r + 1, 2r + 1)` in i64; empty when `r < 0`.
fn circle_box(cx: i32, cy: i32, r: i32) -> Box64 {
    if r < 0 {
        return Box64::EMPTY;
    }
    let (cx, cy, r) = (cx as i64, cy as i64, r as i64);
    Box64 { x0: cx - r, y0: cy - r, x1: cx + r + 1, y1: cy + r + 1 }
}

/// The pixels with `(x − cx)² + (y − cy)² ≤ r²` (section 5.4): one `circle_fill` instance.
pub fn fill_circle(fb: &mut Surface, cx: i32, cy: i32, r: i32, color: Color) {
    let bbox = circle_box(cx, cy, r);
    let visible = bbox.intersect(&fb.clip_box());
    if visible.is_empty() {
        return;
    }
    work::add(1);
    shape(fb, pipeline::CIRCLE_FILL, word(color), visible, &[cx, cy, r]);
    fb.mark(bbox);
}

/// The midpoint circle of section 5.5: one `circle_outline` instance.
pub fn draw_circle(fb: &mut Surface, cx: i32, cy: i32, r: i32, color: Color) {
    let bbox = circle_box(cx, cy, r);
    let visible = bbox.intersect(&fb.clip_box());
    if visible.is_empty() {
        return;
    }
    work::add(1);
    shape(fb, pipeline::CIRCLE_OUTLINE, word(color), visible, &[cx, cy, r]);
    fb.mark(bbox);
}

/// `(x, y, w, h)` with section 5.6's rounded corners: one `rounded_rect` instance; the
/// device applies `r = min(radius, w / 2, h / 2)` and treats `r ≤ 0` as a plain rectangle.
pub fn rounded_rect(fb: &mut Surface, x: i32, y: i32, w: i32, h: i32, radius: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let rect = Box64::of(x, y, w, h);
    let visible = rect.intersect(&fb.clip_box());
    if visible.is_empty() {
        return;
    }
    work::add(1);
    shape(fb, pipeline::ROUNDED_RECT, word(color), visible, &[x, y, w, h, radius]);
    fb.mark(rect);
}

/// Section 5.7's vertical gradient from `top` to `bottom` over `(x, y, w, h)`: one
/// `gradient_v` instance over the visible columns.
pub fn gradient_v(fb: &mut Surface, x: i32, y: i32, w: i32, h: i32, top: Color, bottom: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let rect = Box64::of(x, y, w, h);
    let visible = rect.intersect(&fb.clip_box());
    if visible.is_empty() {
        return;
    }
    work::add(1);
    shape(fb, pipeline::GRADIENT_V, word(top), visible, &[y, h, word(bottom) as i32]);
    fb.mark(rect);
}
