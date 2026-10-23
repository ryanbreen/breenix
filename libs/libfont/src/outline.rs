//! Glyph outlines: compound expansion and flattening into line segments
//! (docs/gui/breengel.md sections 7 and 10).

use alloc::vec::Vec;

use crate::error::FontError;
use crate::work;

/// A point of an expanded outline, in font units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutlinePoint {
    pub x: f32,
    pub y: f32,
    pub on_curve: bool,
}

/// A glyph's outline after compound expansion, with the glyph's header bounding box. The
/// points of every contour are stored in one array; `ends[i]` is the index one past contour
/// i's last point.
#[derive(Debug, Clone)]
pub struct Outline {
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
    pub points: Vec<OutlinePoint>,
    pub ends: Vec<u32>,
}

impl Outline {
    /// The number of points.
    pub fn point_count(&self) -> usize {
        self.points.len()
    }

    /// Each contour's points, in order.
    pub fn contours(&self) -> impl Iterator<Item = &[OutlinePoint]> + '_ {
        let mut start = 0usize;
        self.ends.iter().map(move |&end| {
            let c = &self.points[start..end as usize];
            start = end as usize;
            c
        })
    }
}

/// A line segment in bitmap pixel coordinates (y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineSegment {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Compound nesting allowed, in compound levels (the outermost compound is level 1).
pub const MAX_DEPTH: usize = 8;
/// Quadratic subdivision depth: a curve gives at most 2⁹ = 512 segments.
pub const MAX_SUBDIVISION: u32 = 9;
/// Segments one glyph may flatten to.
pub const MAX_SEGMENTS: usize = 65_536;

/// Flatten `outline` scaled by `scale`, translated so a font-unit point (x, y) lands at
/// (x·scale + x_off, y_off − y·scale): TrueType y points up, bitmap y down. More than
/// `MAX_SEGMENTS` segments is `GlyphTooComplex`.
pub fn flatten(outline: &Outline, scale: f32, x_off: f32, y_off: f32) -> Result<Vec<LineSegment>, FontError> {
    let mut out = Vec::new();
    let map = |p: &OutlinePoint| (p.x * scale + x_off, y_off - p.y * scale);
    for contour in outline.contours() {
        work::add(1);
        if contour.len() < 2 {
            continue;
        }
        // The contour with TrueType's implied on-curve midpoints between consecutive
        // off-curve points made explicit.
        let n = contour.len();
        let mut pts: Vec<((f32, f32), bool)> = Vec::with_capacity(2 * n);
        work::add(n as u64);
        for i in 0..n {
            let (cur, next) = (&contour[i], &contour[(i + 1) % n]);
            pts.push((map(cur), cur.on_curve));
            if !cur.on_curve && !next.on_curve {
                let mid = OutlinePoint { x: (cur.x + next.x) * 0.5, y: (cur.y + next.y) * 0.5, on_curve: true };
                pts.push((map(&mid), true));
            }
        }
        let m = pts.len();
        let Some(start) = pts.iter().position(|p| p.1) else { continue };
        let first = pts[start].0;
        let mut cursor = first;
        let mut i = 1;
        while i <= m {
            work::add(1);
            let (p, on) = pts[(start + i) % m];
            if on {
                push(&mut out, cursor, p)?;
                cursor = p;
                i += 1;
            } else {
                let end = pts[(start + i + 1) % m].0;
                quad(&mut out, cursor, p, end, 0)?;
                cursor = end;
                i += 2;
            }
        }
    }
    Ok(out)
}

fn push(out: &mut Vec<LineSegment>, a: (f32, f32), b: (f32, f32)) -> Result<(), FontError> {
    if a == b {
        return Ok(());
    }
    if out.len() == MAX_SEGMENTS {
        return Err(FontError::GlyphTooComplex);
    }
    out.push(LineSegment { x0: a.0, y0: a.1, x1: b.0, y1: b.1 });
    Ok(())
}

/// Subdivide the quadratic (p0, p1, p2) at t = ½ until its control point is within 0.35 px
/// of the chord's midpoint, or `MAX_SUBDIVISION` levels.
fn quad(out: &mut Vec<LineSegment>, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), depth: u32) -> Result<(), FontError> {
    work::add(1);
    let (mx, my) = ((p0.0 + p2.0) * 0.5, (p0.1 + p2.1) * 0.5);
    let (dx, dy) = (p1.0 - mx, p1.1 - my);
    if depth >= MAX_SUBDIVISION || dx * dx + dy * dy <= 0.35 * 0.35 {
        return push(out, p0, p2);
    }
    let p01 = ((p0.0 + p1.0) * 0.5, (p0.1 + p1.1) * 0.5);
    let p12 = ((p1.0 + p2.0) * 0.5, (p1.1 + p2.1) * 0.5);
    let mid = ((p01.0 + p12.0) * 0.5, (p01.1 + p12.1) * 0.5);
    quad(out, p0, p01, mid, depth + 1)?;
    quad(out, mid, p12, p2, depth + 1)
}
