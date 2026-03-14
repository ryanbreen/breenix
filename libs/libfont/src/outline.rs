//! Scaled glyph outline — contours of points in pixel coordinates.
//!
//! Takes raw glyph contours from the `glyf` table and scales them to pixel
//! coordinates, resolving TrueType's implicit on-curve midpoints between
//! consecutive off-curve points.

use alloc::vec::Vec;
use crate::tables::glyf::{SimpleGlyph, GlyphPoint};

#[inline]
fn fabs(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

/// A point in scaled pixel coordinates.
#[derive(Debug, Clone, Copy)]
pub struct ScaledPoint {
    pub x: f32,
    pub y: f32,
}

/// A line segment in the flattened outline.
#[derive(Debug, Clone, Copy)]
pub struct LineSegment {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Flatten a glyph's contours into line segments, scaled to pixel coordinates.
///
/// `scale` = pixel_size / units_per_em
/// `y_offset` = ascender * scale (to position the glyph baseline)
///
/// The y-axis is flipped: TrueType y increases upward, but pixel y increases downward.
pub fn flatten_glyph(
    glyph: &SimpleGlyph,
    scale: f32,
    x_offset: f32,
    y_offset: f32,
) -> Vec<LineSegment> {
    let mut segments = Vec::new();

    for contour in &glyph.contours {
        if contour.len() < 2 {
            continue;
        }
        flatten_contour(contour, scale, x_offset, y_offset, &mut segments);
    }

    segments
}

fn flatten_contour(
    points: &[GlyphPoint],
    scale: f32,
    x_off: f32,
    y_off: f32,
    segments: &mut Vec<LineSegment>,
) {
    // Resolve TrueType implicit on-curve points:
    // Between two consecutive off-curve points, insert an on-curve midpoint.
    let mut resolved = Vec::with_capacity(points.len() * 2);

    for i in 0..points.len() {
        let curr = &points[i];
        let next = &points[(i + 1) % points.len()];

        resolved.push(ScaledPoint {
            x: curr.x as f32 * scale + x_off,
            y: y_off - curr.y as f32 * scale,
        });

        // If both current and next are off-curve, insert implicit on-curve midpoint
        if !curr.on_curve && !next.on_curve {
            resolved.push(ScaledPoint {
                x: (curr.x as f32 + next.x as f32) * 0.5 * scale + x_off,
                y: y_off - (curr.y as f32 + next.y as f32) * 0.5 * scale,
            });
        }
    }

    if resolved.len() < 2 {
        return;
    }

    // Re-derive on_curve flags for the resolved list
    // Original on-curve points keep their status; inserted midpoints are on-curve.
    let mut on_curve_flags = Vec::with_capacity(resolved.len());
    let mut ri = 0;
    for i in 0..points.len() {
        on_curve_flags.push(points[i].on_curve);
        ri += 1;
        let next = &points[(i + 1) % points.len()];
        if !points[i].on_curve && !next.on_curve {
            on_curve_flags.push(true); // inserted midpoint is on-curve
            ri += 1;
        }
    }
    let _ = ri;

    // Find first on-curve point to start from
    let start = on_curve_flags.iter().position(|&oc| oc).unwrap_or(0);
    let n = resolved.len();

    let mut cursor = resolved[start];
    let mut i = 1;
    while i < n {
        let idx = (start + i) % n;
        if on_curve_flags[idx] {
            // Line segment
            let target = resolved[idx];
            segments.push(LineSegment {
                x0: cursor.x, y0: cursor.y,
                x1: target.x, y1: target.y,
            });
            cursor = target;
            i += 1;
        } else {
            // Quadratic bezier: cursor -> off-curve -> next on-curve
            let control = resolved[idx];
            let end_idx = (start + i + 1) % n;
            let end_pt = resolved[end_idx];
            flatten_quad_bezier(cursor, control, end_pt, segments);
            cursor = end_pt;
            i += 2;
        }
    }

    // Close the contour
    let first = resolved[start];
    if fabs(cursor.x - first.x) > 0.01 || fabs(cursor.y - first.y) > 0.01 {
        segments.push(LineSegment {
            x0: cursor.x, y0: cursor.y,
            x1: first.x, y1: first.y,
        });
    }
}

/// Adaptively flatten a quadratic bezier into line segments.
///
/// Subdivision threshold: if the control point is within 0.35px of the
/// chord midpoint, emit a straight line; otherwise split at t=0.5.
fn flatten_quad_bezier(
    p0: ScaledPoint,
    p1: ScaledPoint,
    p2: ScaledPoint,
    segments: &mut Vec<LineSegment>,
) {
    flatten_quad_recursive(p0, p1, p2, segments, 0);
}

fn flatten_quad_recursive(
    p0: ScaledPoint,
    p1: ScaledPoint,
    p2: ScaledPoint,
    segments: &mut Vec<LineSegment>,
    depth: u32,
) {
    // Max recursion depth to prevent stack overflow
    if depth > 8 {
        segments.push(LineSegment {
            x0: p0.x, y0: p0.y,
            x1: p2.x, y1: p2.y,
        });
        return;
    }

    // Check if control point is close enough to chord midpoint
    let mid_x = (p0.x + p2.x) * 0.5;
    let mid_y = (p0.y + p2.y) * 0.5;
    let dx = p1.x - mid_x;
    let dy = p1.y - mid_y;
    let dist_sq = dx * dx + dy * dy;

    if dist_sq <= 0.35 * 0.35 {
        segments.push(LineSegment {
            x0: p0.x, y0: p0.y,
            x1: p2.x, y1: p2.y,
        });
        return;
    }

    // Split at t=0.5 using de Casteljau
    let p01 = ScaledPoint {
        x: (p0.x + p1.x) * 0.5,
        y: (p0.y + p1.y) * 0.5,
    };
    let p12 = ScaledPoint {
        x: (p1.x + p2.x) * 0.5,
        y: (p1.y + p2.y) * 0.5,
    };
    let p012 = ScaledPoint {
        x: (p01.x + p12.x) * 0.5,
        y: (p01.y + p12.y) * 0.5,
    };

    flatten_quad_recursive(p0, p01, p012, segments, depth + 1);
    flatten_quad_recursive(p012, p12, p2, segments, depth + 1);
}
