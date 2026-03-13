//! Scanline coverage rasterizer: outlines -> GlyphBitmap.
//!
//! For each pixel row, subdivides into N sub-scanlines. Counts winding-rule
//! coverage per sub-scanline. Coverage = filled_sub_scanlines / N * 255.

use alloc::vec;
use alloc::vec::Vec;
use crate::outline::LineSegment;

/// Rasterized glyph output — per-pixel coverage values ready for alpha blending.
#[derive(Debug, Clone)]
pub struct GlyphBitmap {
    pub width: usize,
    pub height: usize,
    pub x_offset: i32,
    pub y_offset: i32,
    pub coverage: Vec<u8>,
}

/// Number of sub-scanlines per pixel row for anti-aliasing.
const SUB_SCANLINES: usize = 5;

/// Rasterize a set of line segments (flattened outline) into a coverage bitmap.
///
/// The segments should already be in pixel coordinates.
/// `width` and `height` are the bitmap dimensions.
/// `x_offset` and `y_offset` are the bearing offsets.
pub fn rasterize(
    segments: &[LineSegment],
    width: usize,
    height: usize,
    x_offset: i32,
    y_offset: i32,
) -> GlyphBitmap {
    if width == 0 || height == 0 || segments.is_empty() {
        return GlyphBitmap {
            width,
            height,
            x_offset,
            y_offset,
            coverage: vec![0; width * height],
        };
    }

    let mut coverage = vec![0u8; width * height];
    let mut x_crossings = Vec::with_capacity(16);

    for row in 0..height {
        // Accumulate sub-scanline coverage per pixel column
        let mut sub_coverage = vec![0u16; width];

        for sub in 0..SUB_SCANLINES {
            let y = row as f32 + (sub as f32 + 0.5) / SUB_SCANLINES as f32;

            // Find all x-crossings at this y
            x_crossings.clear();
            for seg in segments {
                if let Some(x) = intersect_scanline(seg, y) {
                    x_crossings.push(x);
                }
            }

            if x_crossings.is_empty() {
                continue;
            }

            x_crossings.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));

            // Non-zero winding fill: toggle fill at each crossing
            let mut inside = false;
            let mut crossing_idx = 0;
            for px in 0..width {
                let px_left = px as f32;
                let px_right = (px + 1) as f32;

                // Process crossings that fall within or before this pixel
                while crossing_idx < x_crossings.len() && x_crossings[crossing_idx] < px_right {
                    if x_crossings[crossing_idx] >= px_left {
                        // Crossing inside this pixel — partial coverage
                        // Use simple toggle for now
                        inside = !inside;
                    } else if x_crossings[crossing_idx] < px_left {
                        inside = !inside;
                    }
                    crossing_idx += 1;
                }

                if inside {
                    sub_coverage[px] += 1;
                }
            }
        }

        // Convert sub-scanline counts to 0-255 coverage
        let row_offset = row * width;
        for px in 0..width {
            let c = sub_coverage[px] as u32 * 255 / SUB_SCANLINES as u32;
            coverage[row_offset + px] = c.min(255) as u8;
        }
    }

    GlyphBitmap {
        width,
        height,
        x_offset,
        y_offset,
        coverage,
    }
}

/// Find x-intersection of a line segment with a horizontal scanline at y.
fn intersect_scanline(seg: &LineSegment, y: f32) -> Option<f32> {
    let y0 = seg.y0;
    let y1 = seg.y1;

    // Segment must cross the scanline (not just touch)
    if (y0 < y && y1 < y) || (y0 >= y && y1 >= y) {
        return None;
    }

    // Avoid division by zero for horizontal segments
    let dy = y1 - y0;
    if (if dy < 0.0 { -dy } else { dy }) < 1e-10 {
        return None;
    }

    let t = (y - y0) / dy;
    let x = seg.x0 + t * (seg.x1 - seg.x0);
    Some(x)
}
