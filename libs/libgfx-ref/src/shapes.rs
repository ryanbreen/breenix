//! Drawing primitives (docs/gui/breengel.md section 5).
//!
//! Every primitive clips against the surface and the current clip in i64 before any
//! loop runs, writes opaque pixels (X byte 0xFF), marks the damage section 6 gives, and
//! takes at most `2 × (A + R + C) + 64` steps of `libgfx::work` for the area, rows and
//! columns of its clipped bounding rectangle (section 7).

use crate::color::Color;
use crate::framebuf::{Box64, FrameBuf};
use crate::math::isqrt_u64;
use crate::work;

/// Visible spans wider than this are found with an integer square root (32 steps); narrower
/// ones test each pixel, so a narrow clip never pays for the root.
const ROOT_SPAN: i64 = 48;

/// A circle outline whose clipped rectangle has at most this many pixels tests each of them
/// instead of walking: a walk may need two square roots (64 steps), more than a tiny clip's
/// bound allows.
const WALK_AREA: i64 = 64;

/// Fill the visible part of `b` (already clipped) with `px`.
fn fill_box(fb: &mut FrameBuf, b: Box64, px: [u8; 4]) {
    work::add(b.height() as u64);
    for y in b.y0..b.y1 {
        fb.span(y as usize, b.x0 as usize, b.x1 as usize, px);
    }
}

/// The pixels of the half-open rectangle `(x, y, w, h)`; nothing when `w ≤ 0` or `h ≤ 0`.
pub fn fill_rect(fb: &mut FrameBuf, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let r = Box64::of(x, y, w, h);
    let b = r.intersect(&fb.clip_box());
    if b.is_empty() {
        return;
    }
    let px = fb.opaque(color);
    fill_box(fb, b, px);
    fb.mark(r);
}

/// The one-pixel perimeter of `(x, y, w, h)`; nothing when `w ≤ 0` or `h ≤ 0`.
pub fn draw_rect(fb: &mut FrameBuf, x: i32, y: i32, w: i32, h: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let r = Box64::of(x, y, w, h);
    let clip = fb.clip_box();
    if r.intersect(&clip).is_empty() {
        return;
    }
    let px = fb.opaque(color);
    let edges = [
        Box64 { x0: r.x0, y0: r.y0, x1: r.x1, y1: r.y0 + 1 },
        Box64 { x0: r.x0, y0: r.y1 - 1, x1: r.x1, y1: r.y1 },
        Box64 { x0: r.x0, y0: r.y0 + 1, x1: r.x0 + 1, y1: r.y1 - 1 },
        Box64 { x0: r.x1 - 1, y0: r.y0 + 1, x1: r.x1, y1: r.y1 - 1 },
    ];
    work::add(edges.len() as u64);
    for e in edges {
        let b = e.intersect(&clip);
        if !b.is_empty() {
            fill_box(fb, b, px);
        }
    }
    fb.mark(r);
}

fn ceil_div(n: i128, d: i128) -> i128 {
    -((-n).div_euclid(d))
}

/// The line of section 5.3, both endpoints included, clipped before it is walked.
///
/// With `a = |x1 − x0|` and `b = |y1 − y0|`, the walk takes one step along the major axis
/// per point, and the minor coordinate of point `k` is `floor((a + 2kb) / 2a)` minor steps
/// from the start (`floor((b + 2ka) / 2b)` when `b > a`). That follows from the algorithm's
/// error term, which stays in a window of width `2a`; so the first and last visible points
/// are found in closed form and only they and the points between them are walked.
pub fn draw_line(fb: &mut FrameBuf, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
    let (x0, y0, x1, y1) = (x0 as i64, y0 as i64, x1 as i64, y1 as i64);
    let bbox = Box64 { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1) + 1, y1: y0.max(y1) + 1 };
    let c = bbox.intersect(&fb.clip_box());
    if c.is_empty() {
        return;
    }
    let px = fb.opaque(color);
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (a, b) = ((x1 - x0).abs() as i128, (y1 - y0).abs() as i128);
    // Major axis (p) and minor axis (q): start, direction, length and visible range.
    let (major_x, p0, sp, q0, sq, len, minor) = if a >= b {
        (true, x0, sx, y0, sy, a, b)
    } else {
        (false, y0, sy, x0, sx, b, a)
    };
    let (plo, phi, qlo, qhi) = if major_x { (c.x0, c.x1 - 1, c.y0, c.y1 - 1) } else { (c.y0, c.y1 - 1, c.x0, c.x1 - 1) };
    // Steps k along the major axis whose coordinate p0 + sp·k is visible.
    let (mut klo, mut khi) = if sp > 0 {
        ((plo - p0) as i128, (phi - p0) as i128)
    } else {
        ((p0 - phi) as i128, (p0 - plo) as i128)
    };
    // Minor steps n whose coordinate q0 + sq·n is visible.
    let (nlo, nhi) = if sq > 0 {
        ((qlo - q0) as i128, (qhi - q0) as i128)
    } else {
        ((q0 - qhi) as i128, (q0 - qlo) as i128)
    };
    klo = klo.max(0);
    khi = khi.min(len);
    if len == 0 {
        // A single point, inside the clip since its box is.
        klo = 0;
        khi = 0;
    } else if minor == 0 {
        if nlo > 0 || nhi < 0 {
            return;
        }
    } else {
        // n(k) = floor((len + 2k·minor) / 2len) is nondecreasing in k.
        klo = klo.max(ceil_div(2 * len * nlo - len, 2 * minor));
        khi = khi.min(ceil_div(2 * len * nhi + len, 2 * minor) - 1);
    }
    if klo > khi {
        fb.mark(bbox);
        return;
    }
    let two_len = 2 * len.max(1);
    let mut num = len + 2 * klo * minor;
    let mut n = num.div_euclid(two_len);
    num = num.rem_euclid(two_len);
    work::add((khi - klo + 1) as u64);
    for k in klo..=khi {
        let p = p0 as i128 + sp as i128 * k;
        let q = q0 as i128 + sq as i128 * n;
        let (x, y) = if major_x { (p as i64, q as i64) } else { (q as i64, p as i64) };
        if fb.visible(x, y) {
            fb.write(x as usize, y as usize, px);
        }
        num += 2 * minor;
        if num >= two_len {
            num -= two_len;
            n += 1;
        }
    }
    fb.mark(bbox);
}

/// The pixels with `(x − cx)² + (y − cy)² ≤ r²`; the centre pixel for `r = 0`, nothing for
/// `r < 0`.
pub fn fill_circle(fb: &mut FrameBuf, cx: i32, cy: i32, r: i32, color: Color) {
    if r < 0 {
        return;
    }
    let (cx, cy, r) = (cx as i64, cy as i64, r as i64);
    let bbox = Box64 { x0: cx - r, y0: cy - r, x1: cx + r + 1, y1: cy + r + 1 };
    let c = bbox.intersect(&fb.clip_box());
    if c.is_empty() {
        return;
    }
    let px = fb.opaque(color);
    let r2 = (r as i128) * (r as i128);
    work::add(c.height() as u64);
    for y in c.y0..c.y1 {
        let dy = (y - cy) as i128;
        let rem = r2 - dy * dy;
        if rem < 0 {
            continue;
        }
        if c.width() > ROOT_SPAN {
            let s = isqrt_u64(rem as u64) as i64;
            let (lo, hi) = ((cx - s).max(c.x0), (cx + s + 1).min(c.x1));
            if lo < hi {
                fb.span(y as usize, lo as usize, hi as usize, px);
            }
        } else {
            work::add(c.width() as u64);
            for x in c.x0..c.x1 {
                let dx = (x - cx) as i128;
                if dx * dx <= rem {
                    fb.write(x as usize, y as usize, px);
                }
            }
        }
    }
    fb.mark(bbox);
}

/// The midpoint circle's first-octant x for row offset `y`: the largest x with
/// `x² − x ≤ r² − y²` (the algorithm keeps `d = x² − x + (y + 1)² − r²` and moves x in by one
/// exactly when the current x stops satisfying that at the next y). `None` past the radius.
fn octant_x(r: i64, y: i64) -> Option<i64> {
    let n = (r as i128) * (r as i128) - (y as i128) * (y as i128);
    if n < 0 {
        return None;
    }
    let s = isqrt_u64(n as u64) as i128;
    Some(if (s + 1) * s <= n { s + 1 } else { s } as i64)
}

/// The eight-fold midpoint circle of section 5.5; the centre pixel for `r = 0`, nothing for
/// `r < 0`.
///
/// Four walks of the first octant cover it: two over the visible rows (the points
/// `(cx ± x, cy ± y)`) and two over the visible columns (`(cx ± y, cy ± x)`). A walk that
/// starts past `y = 0` starts from `octant_x`, so only visible rows and columns are walked.
/// A clipped rectangle of at most `WALK_AREA` pixels is tested pixel by pixel instead: the
/// octant's points are exactly the `(x, y)` with `y ≤ x` and `x = octant_x(r, y)`, that is
/// `x² − x ≤ r² − y² < x² + x`.
pub fn draw_circle(fb: &mut FrameBuf, cx: i32, cy: i32, r: i32, color: Color) {
    if r < 0 {
        return;
    }
    let (cx, cy, r) = (cx as i64, cy as i64, r as i64);
    let bbox = Box64 { x0: cx - r, y0: cy - r, x1: cx + r + 1, y1: cy + r + 1 };
    let c = bbox.intersect(&fb.clip_box());
    if c.is_empty() {
        return;
    }
    let px = fb.opaque(color);
    if r == 0 {
        fb.write(cx as usize, cy as usize, px);
        fb.mark(bbox);
        return;
    }
    if c.width() * c.height() <= WALK_AREA {
        let r2 = (r as i128) * (r as i128);
        let on = |x: i128, y: i128| y <= x && x * x - x <= r2 - y * y && r2 - y * y < x * x + x;
        work::add(c.height() as u64);
        for py in c.y0..c.y1 {
            work::add(c.width() as u64);
            for pxx in c.x0..c.x1 {
                let (a, b) = ((pxx - cx).unsigned_abs() as i128, (py - cy).unsigned_abs() as i128);
                if on(a, b) || on(b, a) {
                    fb.write(pxx as usize, py as usize, px);
                }
            }
        }
        fb.mark(bbox);
        return;
    }
    // (the octant y range, rows (true) or columns, the sign of the walked offset)
    let walks = [
        (c.y0 - cy, c.y1 - 1 - cy, true, 1),
        (cy - (c.y1 - 1), cy - c.y0, true, -1),
        (c.x0 - cx, c.x1 - 1 - cx, false, 1),
        (cx - (c.x1 - 1), cx - c.x0, false, -1),
    ];
    work::add(walks.len() as u64);
    for (lo, hi, rows, sign) in walks {
        let lo = lo.max(0);
        if lo > hi {
            continue;
        }
        let (mut x, mut y) = if lo == 0 {
            (r, 0)
        } else {
            match octant_x(r, lo) {
                Some(x) => (x, lo),
                None => continue,
            }
        };
        let mut d = (x as i128) * (x as i128) - x as i128 + ((y + 1) as i128) * ((y + 1) as i128) - (r as i128) * (r as i128);
        let mut steps = 0u64;
        while x >= y && y <= hi {
            steps += 1;
            let points = if rows {
                [(cx + x, cy + sign * y), (cx - x, cy + sign * y)]
            } else {
                [(cx + sign * y, cy + x), (cx + sign * y, cy - x)]
            };
            for (px_x, px_y) in points {
                if fb.visible(px_x, px_y) {
                    fb.write(px_x as usize, px_y as usize, px);
                }
            }
            y += 1;
            if d <= 0 {
                d += 2 * y as i128 + 1;
            } else {
                x -= 1;
                d += 2 * (y - x) as i128 + 1;
            }
        }
        // Each step of the walk runs the two-point loop above.
        work::add(3 * steps);
    }
    fb.mark(bbox);
}

/// `(x, y, w, h)` with its four corners rounded (section 5.6): in each `r × r` corner square,
/// `r = min(radius, w / 2, h / 2)`, a pixel is drawn when its centre lies in the quarter
/// circle of radius r about the square's inner corner. `r ≤ 0` is a plain `fill_rect`.
pub fn rounded_rect(fb: &mut FrameBuf, x: i32, y: i32, w: i32, h: i32, radius: i32, color: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let r = (radius as i64).min(w as i64 / 2).min(h as i64 / 2);
    if r <= 0 {
        fill_rect(fb, x, y, w, h, color);
        return;
    }
    let rect = Box64::of(x, y, w, h);
    let c = rect.intersect(&fb.clip_box());
    if c.is_empty() {
        return;
    }
    let px = fb.opaque(color);
    let four_r2 = 4 * (r as i128) * (r as i128);
    work::add(c.height() as u64);
    for py in c.y0..c.y1 {
        // Distance in rows into the nearer corner band, as the odd number 2(y + r − py) − 1.
        let v = if py < rect.y0 + r {
            Some(2 * (rect.y0 + r - py) - 1)
        } else if py >= rect.y1 - r {
            Some(2 * (py - (rect.y1 - r)) + 1)
        } else {
            None
        };
        let Some(v) = v else {
            fb.span(py as usize, c.x0 as usize, c.x1 as usize, px);
            continue;
        };
        let room = four_r2 - (v as i128) * (v as i128);
        if c.width() > ROOT_SPAN {
            // Drawn corner pixels have odd u = 2(x + r − px) − 1 with u² ≤ room.
            let drawn = if room < 0 { 0 } else { ((isqrt_u64(room as u64) as i64 + 1) / 2).min(r) };
            let inset = r - drawn;
            let (lo, hi) = ((rect.x0 + inset).max(c.x0), (rect.x1 - inset).min(c.x1));
            if lo < hi {
                fb.span(py as usize, lo as usize, hi as usize, px);
            }
        } else {
            work::add(c.width() as u64);
            for pxx in c.x0..c.x1 {
                let u = if pxx < rect.x0 + r {
                    Some(2 * (rect.x0 + r - pxx) - 1)
                } else if pxx >= rect.x1 - r {
                    Some(2 * (pxx - (rect.x1 - r)) + 1)
                } else {
                    None
                };
                if u.is_none_or(|u| (u as i128) * (u as i128) <= room) {
                    fb.write(pxx as usize, py as usize, px);
                }
            }
        }
    }
    fb.mark(rect);
}

/// A vertical gradient (section 5.7): row i of `(x, y, w, h)` has each channel
/// `(top × (h − 1 − i) + bottom × i + (h − 1) / 2) / (h − 1)`, and `top` when `h = 1`.
pub fn gradient_v(fb: &mut FrameBuf, x: i32, y: i32, w: i32, h: i32, top: Color, bottom: Color) {
    if w <= 0 || h <= 0 {
        return;
    }
    let rect = Box64::of(x, y, w, h);
    let c = rect.intersect(&fb.clip_box());
    if c.is_empty() {
        return;
    }
    let span = h as i64 - 1;
    let ch = |t: u8, b: u8, i: i64| -> u8 {
        if span == 0 {
            t
        } else {
            ((t as i64 * (span - i) + b as i64 * i + span / 2) / span) as u8
        }
    };
    work::add(c.height() as u64);
    for py in c.y0..c.y1 {
        let i = py - rect.y0;
        let color = Color::rgb(ch(top.r, bottom.r, i), ch(top.g, bottom.g, i), ch(top.b, bottom.b, i));
        let px = fb.opaque(color);
        fb.span(py as usize, c.x0 as usize, c.x1 as usize, px);
    }
    fb.mark(rect);
}
