//! Pure RGB canvas. Coordinates outside the canvas are clipped.
extern crate alloc;
use self::alloc::{vec, vec::Vec};

pub type Color = [u8; 3];
pub type Point = (i32, i32);

/// Brush and eraser disc radii in pixels.
#[derive(Clone, Copy)]
pub enum Radius {
    Small,
    Medium,
    Large,
}
impl Radius {
    fn pixels(self) -> i64 {
        match self {
            Self::Small => 2,
            Self::Medium => 5,
            Self::Large => 10,
        }
    }
}

#[derive(Clone)]
pub struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}
impl Canvas {
    /// A white, row-major RGB canvas.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![
                255;
                width
                    .checked_mul(height)
                    .and_then(|n| n.checked_mul(3))
                    .expect("canvas size overflow")
            ],
        }
    }
    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] { &mut self.pixels }
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
    pub fn digest(&self) -> [u8; 32] {
        libpixcheck::sha256(&self.pixels)
    }
    fn put(&mut self, x: i64, y: i64, color: Color) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let i = (y as usize * self.width + x as usize) * 3;
            self.pixels[i..i + 3].copy_from_slice(&color);
        }
    }
    fn trace(&mut self, a: Point, b: Point, mut stamp: impl FnMut(&mut Self, i64, i64)) {
        let (mut x, mut y) = (a.0 as i64, a.1 as i64);
        let (x1, y1) = (b.0 as i64, b.1 as i64);
        let dx = (x1 - x).abs();
        let dy = -(y1 - y).abs();
        let sx = if x < x1 { 1 } else { -1 };
        let sy = if y < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            stamp(self, x, y);
            if x == x1 && y == y1 {
                break;
            }
            let e = 2 * err;
            if e >= dy {
                err += dy;
                x += sx;
            }
            if e <= dx {
                err += dx;
                y += sy;
            }
        }
    }
    fn stroke(&mut self, points: &[Point], radius: i64, color: Color) {
        let stamp = |c: &mut Self, x: i64, y: i64| {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx * dx + dy * dy <= radius * radius {
                        c.put(x + dx, y + dy, color);
                    }
                }
            }
        };
        if let Some(&first) = points.first() {
            self.trace(first, first, stamp);
            for pair in points.windows(2) {
                self.trace(pair[0], pair[1], stamp);
            }
        }
    }
    /// Pencil: one-pixel Bresenham polyline through the delivered points.
    pub fn pencil(&mut self, points: &[Point], color: Color) {
        self.stroke(points, 0, color);
    }
    /// Brush: stamp the selected disc along the Bresenham polyline.
    pub fn brush(&mut self, points: &[Point], radius: Radius, color: Color) {
        self.stroke(points, radius.pixels(), color);
    }
    /// Eraser: stamp the selected disc in the white background colour.
    pub fn erase(&mut self, points: &[Point], radius: Radius) {
        self.stroke(points, radius.pixels(), [255; 3]);
    }
    /// Line: one-pixel Bresenham segment from press to release.
    pub fn line(&mut self, press: Point, release: Point, color: Color) {
        self.trace(press, release, |c, x, y| c.put(x, y, color));
    }
    /// Rectangle: one-pixel outline with press and release as opposite corners.
    pub fn rectangle(&mut self, a: Point, b: Point, color: Color) {
        self.line(a, (b.0, a.1), color);
        self.line((b.0, a.1), b, color);
        self.line(b, (a.0, b.1), color);
        self.line((a.0, b.1), a, color);
    }
    /// Circle: midpoint outline centred at the press, with floor Euclidean radius.
    pub fn circle(&mut self, press: Point, release: Point, color: Color) {
        let dx = release.0 as i64 - press.0 as i64;
        let dy = release.1 as i64 - press.1 as i64;
        let n = (dx as i128 * dx as i128 + dy as i128 * dy as i128) as u128;
        let (mut lo, mut hi) = (0u128, 1u128 << 33);
        while lo + 1 < hi {
            let m = (lo + hi) / 2;
            if m * m <= n {
                lo = m;
            } else {
                hi = m;
            }
        }
        let (mut x, mut y, mut err) = (lo as i64, 0i64, 1 - lo as i64);
        while x >= y {
            for (ox, oy) in [
                (x, y),
                (y, x),
                (-y, x),
                (-x, y),
                (-x, -y),
                (-y, -x),
                (y, -x),
                (x, -y),
            ] {
                self.put(press.0 as i64 + ox, press.1 as i64 + oy, color);
            }
            y += 1;
            if err < 0 {
                err += 2 * y + 1;
            } else {
                x -= 1;
                err += 2 * (y - x) + 1;
            }
        }
    }
    /// Fill: replace the pressed pixel's four-connected colour region.
    pub fn fill(&mut self, point: Point, color: Color) {
        let (x, y) = point;
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let index = (y as usize * self.width + x as usize) * 3;
        let target = [
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
        ];
        if target == color {
            return;
        }
        let mut stack = vec![(x as usize, y as usize)];
        self.put(x as i64, y as i64, color);
        while let Some((x, y)) = stack.pop() {
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx < self.width && ny < self.height {
                    let i = (ny * self.width + nx) * 3;
                    if self.pixels[i..i + 3] == target {
                        self.pixels[i..i + 3].copy_from_slice(&color);
                        stack.push((nx, ny));
                    }
                }
            }
        }
    }
}

/// Completed canvas states. Push once when an operation is complete, including
/// a stroke or shape on release, a fill, New, or a successfully opened image.
pub struct History {
    base: Canvas,
    past: Vec<Canvas>,
    future: Vec<Canvas>,
}
impl History {
    pub fn new(base: Canvas) -> Self {
        Self { base, past: Vec::new(), future: Vec::new() }
    }
    pub fn canvas(&self) -> &Canvas { self.past.last().unwrap_or(&self.base) }
    pub fn undo_depth(&self) -> usize { self.past.len() }
    pub fn redo_depth(&self) -> usize { self.future.len() }
    pub fn push(&mut self, canvas: Canvas) {
        self.future.clear();
        self.past.push(canvas);
        if self.past.len() > 32 { self.base = self.past.remove(0); }
    }
    pub fn undo(&mut self) -> bool {
        match self.past.pop() {
            Some(canvas) => { self.future.push(canvas); true }
            None => false,
        }
    }
    pub fn redo(&mut self) -> bool {
        match self.future.pop() {
            Some(canvas) => { self.past.push(canvas); true }
            None => false,
        }
    }
}
