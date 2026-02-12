//! Drawing primitives: filled circles, filled rectangles, outlined rectangles.

use crate::color::Color;
use crate::framebuf::FrameBuf;
use crate::math::isqrt_i64;

/// Fill a circle at (cx, cy) with the given radius and color.
pub fn fill_circle(fb: &mut FrameBuf, cx: i32, cy: i32, radius: i32, color: Color) {
    let r2 = (radius as i64) * (radius as i64);
    let (c0, c1, c2) = if fb.is_bgr {
        (color.b, color.g, color.r)
    } else {
        (color.r, color.g, color.b)
    };

    for dy in -radius..=radius {
        let dx_max_sq = r2 - (dy as i64) * (dy as i64);
        if dx_max_sq < 0 {
            continue;
        }
        let dx_max = isqrt_i64(dx_max_sq) as i32;
        let y = cy + dy;
        if y < 0 || y >= fb.height as i32 {
            continue;
        }
        let x_start = (cx - dx_max).max(0) as usize;
        let x_end = (cx + dx_max).min(fb.width as i32 - 1) as usize;
        if x_start > x_end {
            continue;
        }
        let row = (y as usize) * fb.stride;
        let ptr = fb.raw_ptr();
        if fb.bpp == 4 {
            for x in x_start..=x_end {
                let o = row + x * 4;
                unsafe {
                    *ptr.add(o) = c0;
                    *ptr.add(o + 1) = c1;
                    *ptr.add(o + 2) = c2;
                    *ptr.add(o + 3) = 0;
                }
            }
        } else {
            for x in x_start..=x_end {
                let o = row + x * 3;
                unsafe {
                    *ptr.add(o) = c0;
                    *ptr.add(o + 1) = c1;
                    *ptr.add(o + 2) = c2;
                }
            }
        }
    }

    // Mark bounding box dirty
    let bx = (cx - radius).max(0);
    let by = (cy - radius).max(0);
    let bw = ((cx + radius + 1).min(fb.width as i32) - bx).max(0);
    let bh = ((cy + radius + 1).min(fb.height as i32) - by).max(0);
    fb.mark_dirty(bx, by, bw, bh);
}

/// Fill a rectangle with the given color.
pub fn fill_rect(fb: &mut FrameBuf, x: i32, y: i32, w: i32, h: i32, color: Color) {
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = ((x + w) as usize).min(fb.width);
    let y1 = ((y + h) as usize).min(fb.height);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let (c0, c1, c2) = if fb.is_bgr {
        (color.b, color.g, color.r)
    } else {
        (color.r, color.g, color.b)
    };
    let ptr = fb.raw_ptr();

    for row_y in y0..y1 {
        let row = row_y * fb.stride;
        if fb.bpp == 4 {
            for px in x0..x1 {
                let o = row + px * 4;
                unsafe {
                    *ptr.add(o) = c0;
                    *ptr.add(o + 1) = c1;
                    *ptr.add(o + 2) = c2;
                    *ptr.add(o + 3) = 0;
                }
            }
        } else {
            for px in x0..x1 {
                let o = row + px * 3;
                unsafe {
                    *ptr.add(o) = c0;
                    *ptr.add(o + 1) = c1;
                    *ptr.add(o + 2) = c2;
                }
            }
        }
    }

    fb.mark_dirty(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32);
}

/// Draw a rectangle outline with the given color.
pub fn draw_rect(fb: &mut FrameBuf, x: i32, y: i32, w: i32, h: i32, color: Color) {
    // Top edge
    fill_rect(fb, x, y, w, 1, color);
    // Bottom edge
    fill_rect(fb, x, y + h - 1, w, 1, color);
    // Left edge
    fill_rect(fb, x, y, 1, h, color);
    // Right edge
    fill_rect(fb, x + w - 1, y, 1, h, color);
}
