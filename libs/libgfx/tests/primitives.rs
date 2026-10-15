//! Host checks of libgfx's primitives against direct transcriptions of
//! docs/gui/breengel.md sections 2 to 7. The guest suite (/sbin/suite-breengel) gives the
//! verdict; these are the development loop.

use std::collections::HashSet;

use libgfx::alpha;
use libgfx::blit;
use libgfx::color::Color;
use libgfx::framebuf::{Format, FrameBuf, Rect, SurfaceError};
use libgfx::shapes;
use libgfx::work;

const INK: Color = Color::rgb(200, 30, 90);

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }
}

fn surface(w: usize, h: usize) -> FrameBuf {
    FrameBuf::new(w, h, Format::BGRX).unwrap()
}

fn inked(fb: &FrameBuf) -> HashSet<(i64, i64)> {
    let mut set = HashSet::new();
    for y in 0..fb.height {
        for x in 0..fb.width {
            if fb.get_pixel(x, y) == INK {
                set.insert((x as i64, y as i64));
            }
        }
    }
    set
}

fn visible(set: impl IntoIterator<Item = (i64, i64)>, clip: (i64, i64, i64, i64)) -> HashSet<(i64, i64)> {
    let (x0, y0, x1, y1) = clip;
    set.into_iter().filter(|&(x, y)| x >= x0 && x < x1 && y >= y0 && y < y1).collect()
}

/// Section 5.3, transcribed.
fn line_path(x0: i64, y0: i64, x1: i64, y1: i64) -> Vec<(i64, i64)> {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut err, mut x, mut y) = (dx + dy, x0, y0);
    let mut pts = Vec::new();
    loop {
        pts.push((x, y));
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    pts
}

/// Section 5.5, transcribed.
fn circle_path(cx: i64, cy: i64, r: i64) -> HashSet<(i64, i64)> {
    let mut set = HashSet::new();
    if r < 0 {
        return set;
    }
    if r == 0 {
        set.insert((cx, cy));
        return set;
    }
    let (mut x, mut y, mut d) = (r, 0i64, 1 - r);
    while x >= y {
        for (a, b) in [(x, y), (y, x)] {
            for (sa, sb) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
                set.insert((cx + sa * a, cy + sb * b));
            }
        }
        y += 1;
        if d <= 0 {
            d += 2 * y + 1;
        } else {
            x -= 1;
            d += 2 * (y - x) + 1;
        }
    }
    set
}

fn bound(area: i64, rows: i64, cols: i64) -> u64 {
    (2 * (area + rows + cols) + 64) as u64
}

#[test]
fn lines_match_the_walk_clipped() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for _ in 0..20_000 {
        let (w, h) = (rng.range(1, 40) as usize, rng.range(1, 30) as usize);
        let mut fb = surface(w, h);
        let clip = if rng.range(0, 1) == 0 {
            (0, 0, w as i64, h as i64)
        } else {
            let (cx, cy) = (rng.range(-5, 40), rng.range(-5, 30));
            let (cw, ch) = (rng.range(-2, 30), rng.range(-2, 30));
            fb.push_clip(Rect::new(cx as i32, cy as i32, cw as i32, ch as i32));
            let c = fb.clip();
            (c.x as i64, c.y as i64, (c.x + c.w.max(0)) as i64, (c.y + c.h.max(0)) as i64)
        };
        let (x0, y0, x1, y1) = (rng.range(-60, 100), rng.range(-60, 100), rng.range(-60, 100), rng.range(-60, 100));
        shapes::draw_line(&mut fb, x0 as i32, y0 as i32, x1 as i32, y1 as i32, INK);
        let want = visible(line_path(x0, y0, x1, y1), clip);
        assert_eq!(inked(&fb), want, "line {x0},{y0} -> {x1},{y1} on {w}x{h} clip {clip:?}");
    }
}

#[test]
fn far_lines_are_clipped_before_the_walk() {
    for &(x0, y0, x1, y1) in &[
        (i32::MIN / 2, 3, i32::MAX / 2, 4),
        (5, i32::MIN / 2, 6, i32::MAX / 2),
        (-100_000, -3, 100_000, 40),
        (i32::MIN, i32::MIN, i32::MAX, i32::MAX),
        (i32::MAX, 0, i32::MIN, 17),
        (-1_000_003, -999_999, 1_000_001, 1_000_007),
    ] {
        let mut fb = surface(32, 24);
        work::reset();
        shapes::draw_line(&mut fb, x0, y0, x1, y1, INK);
        let steps = work::steps();
        assert!(steps <= bound(32 * 24, 24, 32), "line {x0},{y0} -> {x1},{y1}: {steps} steps");
        // The visible part of the walk, by a walk along the major axis from the formula's
        // own definition, checked on the cases small enough to walk.
        if (x1 as i64 - x0 as i64).abs() + (y1 as i64 - y0 as i64).abs() < 5_000_000 {
            let want = visible(line_path(x0 as i64, y0 as i64, x1 as i64, y1 as i64), (0, 0, 32, 24));
            assert_eq!(inked(&fb), want, "line {x0},{y0} -> {x1},{y1}");
        }
    }
}

#[test]
fn circles_match_the_midpoint_walk() {
    for r in 0..=60 {
        for &(cx, cy) in &[(30, 30), (-3, 10), (10, -7), (55, 40), (0, 0), (59, 59)] {
            let mut fb = surface(60, 60);
            shapes::draw_circle(&mut fb, cx, cy, r, INK);
            let want = visible(circle_path(cx as i64, cy as i64, r as i64), (0, 0, 60, 60));
            assert_eq!(inked(&fb), want, "draw_circle({cx}, {cy}, {r})");
            let mut fb = surface(60, 60);
            shapes::fill_circle(&mut fb, cx, cy, r, INK);
            let mut want = HashSet::new();
            for y in 0..60i64 {
                for x in 0..60i64 {
                    let (dx, dy) = (x - cx as i64, y - cy as i64);
                    if dx * dx + dy * dy <= (r as i64) * (r as i64) {
                        want.insert((x, y));
                    }
                }
            }
            assert_eq!(inked(&fb), want, "fill_circle({cx}, {cy}, {r})");
        }
    }
}

#[test]
fn big_circles_match_and_stay_bounded() {
    let mut rng = Rng(77);
    for _ in 0..300 {
        let r = rng.range(1, 3_000);
        let (cx, cy) = (rng.range(-3_000, 3_000), rng.range(-3_000, 3_000));
        let mut fb = surface(48, 40);
        work::reset();
        shapes::draw_circle(&mut fb, cx as i32, cy as i32, r as i32, INK);
        let steps = work::steps();
        let want = visible(circle_path(cx, cy, r), (0, 0, 48, 40));
        assert_eq!(inked(&fb), want, "draw_circle({cx}, {cy}, {r})");
        let (x0, y0, x1, y1) = ((cx - r).max(0), (cy - r).max(0), (cx + r + 1).min(48), (cy + r + 1).min(40));
        if x0 < x1 && y0 < y1 {
            let (c, rr) = (x1 - x0, y1 - y0);
            assert!(steps <= bound(c * rr, rr, c), "draw_circle({cx}, {cy}, {r}): {steps} steps");
        } else {
            assert!(steps <= 64, "draw_circle({cx}, {cy}, {r}) outside: {steps} steps");
        }
    }
}

#[test]
fn extreme_calls_stay_within_the_work_bound() {
    type Call = (&'static str, Box<dyn Fn(&mut FrameBuf)>);
    let calls: Vec<Call> = vec![
        ("fill_rect(MIN, MIN, MAX, MAX)", Box::new(|fb| shapes::fill_rect(fb, i32::MIN, i32::MIN, i32::MAX, i32::MAX, INK))),
        ("fill_rect(MAX, MAX, MAX, MAX)", Box::new(|fb| shapes::fill_rect(fb, i32::MAX, i32::MAX, i32::MAX, i32::MAX, INK))),
        ("draw_rect(MIN, MIN, MAX, MAX)", Box::new(|fb| shapes::draw_rect(fb, i32::MIN, i32::MIN, i32::MAX, i32::MAX, INK))),
        ("fill_circle(5, 5, 46340)", Box::new(|fb| shapes::fill_circle(fb, 5, 5, 46_340, INK))),
        ("fill_circle(5, 5, MAX)", Box::new(|fb| shapes::fill_circle(fb, 5, 5, i32::MAX, INK))),
        ("draw_circle(5, 5, 1000000)", Box::new(|fb| shapes::draw_circle(fb, 5, 5, 1_000_000, INK))),
        ("draw_circle(5, 5, MAX)", Box::new(|fb| shapes::draw_circle(fb, 5, 5, i32::MAX, INK))),
        ("draw_circle(MIN, MIN, MAX)", Box::new(|fb| shapes::draw_circle(fb, i32::MIN, i32::MIN, i32::MAX, INK))),
        ("rounded_rect(MIN, MIN, MAX, MAX, MAX)", Box::new(|fb| shapes::rounded_rect(fb, i32::MIN, i32::MIN, i32::MAX, i32::MAX, i32::MAX, INK))),
        ("rounded_rect(-1000, -1000, 2000, 2000, 900)", Box::new(|fb| shapes::rounded_rect(fb, -1000, -1000, 2000, 2000, 900, INK))),
        ("gradient_v(MIN, MIN, MAX, MAX)", Box::new(|fb| shapes::gradient_v(fb, i32::MIN, i32::MIN, i32::MAX, i32::MAX, INK, Color::WHITE))),
    ];
    for (name, call) in calls {
        let mut fb = surface(32, 24);
        work::reset();
        call(&mut fb);
        let steps = work::steps();
        assert!(steps <= bound(32 * 24, 24, 32), "{name}: {steps} steps");
    }
}

#[test]
fn rounded_rect_matches_the_corner_predicate() {
    for &(x, y, w, h, radius) in &[(2, 3, 30, 20, 6), (0, 0, 9, 9, 100), (5, 5, 1, 7, 3), (-4, -6, 25, 30, 9), (3, 3, 12, 5, 0), (1, 2, 40, 33, 16)] {
        for clip in [None, Some(Rect::new(4, 1, 7, 50))] {
            let mut fb = surface(48, 40);
            if let Some(c) = clip {
                fb.push_clip(c);
            }
            shapes::rounded_rect(&mut fb, x, y, w, h, radius, INK);
            let r = (radius as i64).min(w as i64 / 2).min(h as i64 / 2);
            let (x, y, w, h) = (x as i64, y as i64, w as i64, h as i64);
            let mut want = HashSet::new();
            for py in y..y + h {
                for px in x..x + w {
                    let u = if px < x + r { Some(2 * (x + r - px) - 1) } else if px >= x + w - r { Some(2 * (px - (x + w - r)) + 1) } else { None };
                    let v = if py < y + r { Some(2 * (y + r - py) - 1) } else if py >= y + h - r { Some(2 * (py - (y + h - r)) + 1) } else { None };
                    let drawn = match (u, v) {
                        (Some(u), Some(v)) if r > 0 => u * u + v * v <= 4 * r * r,
                        _ => true,
                    };
                    if drawn {
                        want.insert((px, py));
                    }
                }
            }
            let c = fb.clip();
            let want = visible(want, (c.x as i64, c.y as i64, (c.x + c.w) as i64, (c.y + c.h) as i64));
            assert_eq!(inked(&fb), want, "rounded_rect({x}, {y}, {w}, {h}, {radius}) clip {clip:?}");
        }
    }
}

#[test]
fn gradient_rows_follow_the_formula() {
    let (top, bottom) = (Color::rgb(250, 0, 17), Color::rgb(3, 255, 200));
    for h in 1..40i64 {
        let mut fb = surface(4, 40);
        shapes::gradient_v(&mut fb, 0, 0, 4, h as i32, top, bottom);
        for i in 0..h {
            let ch = |t: u8, b: u8| if h == 1 { t } else { ((t as i64 * (h - 1 - i) + b as i64 * i + (h - 1) / 2) / (h - 1)) as u8 };
            assert_eq!(fb.get_pixel(2, i as usize), Color::rgb(ch(top.r, bottom.r), ch(top.g, bottom.g), ch(top.b, bottom.b)), "h {h} row {i}");
        }
    }
}

#[test]
fn alpha_examples_and_div255() {
    for x in 0..=65_025u32 {
        let want = ((x as f64) / 255.0).round() as u32;
        assert_eq!(alpha::div255(x) as u32, want, "div255({x})");
    }
    for &((sc, sa), (dc, da), (oc, oa)) in &[
        ((128, 128), (0, 255), (128, 255)),
        ((128, 128), (255, 255), (255, 255)),
        ((64, 128), (200, 255), (164, 255)),
        ((64, 128), (100, 100), (114, 178)),
        ((0, 0), (77, 77), (77, 77)),
        ((200, 255), (13, 50), (200, 255)),
    ] {
        let out = alpha::over([sc, sc, sc, sa], [dc, dc, dc, da]);
        assert_eq!(out, [oc, oc, oc, oa]);
    }
}

#[test]
fn surfaces_validate() {
    assert_eq!(FrameBuf::new(0, 4, Format::BGRX).err(), Some(SurfaceError::Empty));
    assert_eq!(FrameBuf::new(16_385, 4, Format::BGRX).err(), Some(SurfaceError::TooLarge));
    assert_eq!(FrameBuf::new(4, 4, Format { bpp: 3, is_bgr: true, premultiplied: true }).err(), Some(SurfaceError::Format));
    let mut mem = vec![0u32; 64];
    let base = mem.as_mut_ptr() as *mut u8;
    unsafe {
        assert_eq!(FrameBuf::from_raw(base, 8, 4, 31, 4, true).err(), Some(SurfaceError::StrideTooShort));
        assert_eq!(FrameBuf::from_raw(base, 8, 4, 32, 2, true).err(), Some(SurfaceError::Format));
        assert_eq!(FrameBuf::from_raw(base, 16_384, 16_384, 1 << 20, 4, true).err(), Some(SurfaceError::SizeOverflow));
        assert_eq!(FrameBuf::from_raw(core::ptr::null_mut(), 8, 4, 32, 4, true).err(), Some(SurfaceError::Null));
        assert_eq!(FrameBuf::from_raw(base.add(1), 8, 4, 32, 4, true).err(), Some(SurfaceError::Misaligned));
        assert!(FrameBuf::from_raw(base, 8, 4, 32, 4, true).is_ok());
    }
}

#[test]
fn blits_convert_blend_and_scale() {
    let mut src = FrameBuf::new(3, 2, Format::BGRA_PREMULTIPLIED).unwrap();
    src.put_bgra(0, 0, [10, 20, 30, 128]);
    src.put_bgra(2, 1, [40, 50, 60, 255]);
    let mut dst = FrameBuf::new_filled(8, 8, Format::BGRX, Color::rgb(100, 100, 100)).unwrap();
    blit::blit_over(&mut dst, &src, -1, 6);
    assert_eq!(dst.get_bgra(1, 7), [40, 50, 60, 255]);
    let mut dst = FrameBuf::new_filled(8, 8, Format::BGRX, Color::rgb(100, 100, 100)).unwrap();
    blit::blit_over(&mut dst, &src, 2, 2);
    assert_eq!(dst.get_bgra(2, 2), alpha::over([10, 20, 30, 128], [100, 100, 100, 255]));
    assert_eq!(dst.get_bgra(3, 2), [100, 100, 100, 255]);
    let mut dst = FrameBuf::new(12, 9, Format::RGB).unwrap();
    blit::blit_scaled(&mut dst, &src, 1, 1, 3);
    for y in 0..9 {
        for x in 0..12 {
            let inside = (1..10).contains(&x) && (1..7).contains(&y);
            let want = if inside { let p = src.get_bgra((x - 1) / 3, (y - 1) / 3); [p[0], p[1], p[2], 255] } else { [0, 0, 0, 255] };
            assert_eq!(dst.get_bgra(x, y), want, "({x},{y})");
        }
    }
}
