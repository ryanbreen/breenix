//! Scanline coverage rasterizer: outlines -> GlyphBitmap.
//!
//! Uses non-zero winding rule with fractional horizontal coverage.
//! For each pixel row, N sub-scanlines sample the vertical axis. On each
//! sub-scanline, filled spans from the winding rule contribute fractional
//! horizontal coverage (how much of the pixel's width is inside the glyph).
//! This gives smooth anti-aliased edges comparable to FreeType/stb_truetype.

use alloc::vec;
use alloc::vec::Vec;
use crate::outline::LineSegment;
use crate::float::ceil;

/// Rasterized glyph output — per-pixel coverage values ready for alpha blending.
#[derive(Debug, Clone)]
pub struct GlyphBitmap {
    pub width: usize,
    pub height: usize,
    pub x_offset: i32,
    pub y_offset: i32,
    pub coverage: Vec<u8>,
}

/// Subpixel (LCD) rasterized glyph — 3 coverage values per pixel (R, G, B).
#[derive(Debug, Clone)]
pub struct SubpixelBitmap {
    pub width: usize,
    pub height: usize,
    pub x_offset: i32,
    pub y_offset: i32,
    /// Coverage data: 3 bytes per pixel (R, G, B), row-major, width*3 per row.
    pub coverage: Vec<u8>,
}

/// Number of sub-scanlines per pixel row for vertical anti-aliasing.
/// 8 is sufficient when combined with fractional horizontal coverage.
const SUB_SCANLINES: usize = 8;

/// Inverse of SUB_SCANLINES as f32, precomputed.
const INV_SUB: f32 = 1.0 / SUB_SCANLINES as f32;

/// An x-crossing with its winding direction.
struct Crossing {
    x: f32,
    /// +1 for downward edge (y0 < y1), -1 for upward edge (y0 > y1)
    dir: i32,
}

/// Rasterize a set of line segments (flattened outline) into a coverage bitmap.
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
    let mut crossings: Vec<Crossing> = Vec::with_capacity(32);
    let mut pixel_cov = vec![0.0f32; width];
    let width_f = width as f32;

    for row in 0..height {
        // Reset per-row coverage accumulator
        for v in pixel_cov.iter_mut() { *v = 0.0; }

        for sub in 0..SUB_SCANLINES {
            let y = row as f32 + (sub as f32 + 0.5) * INV_SUB;

            // Find all x-crossings with winding direction at this y
            crossings.clear();
            for seg in segments {
                if let Some(crossing) = intersect_scanline(seg, y) {
                    crossings.push(crossing);
                }
            }

            if crossings.is_empty() {
                continue;
            }

            crossings.sort_unstable_by(|a, b| {
                a.x.partial_cmp(&b.x).unwrap_or(core::cmp::Ordering::Equal)
            });

            // Walk crossings, tracking winding number to find filled spans.
            // Each span contributes fractional horizontal coverage to pixels.
            let mut winding: i32 = 0;
            let mut fill_start: f32 = 0.0;

            for c in &crossings {
                let prev_winding = winding;
                winding += c.dir;

                if prev_winding == 0 && winding != 0 {
                    // Entering filled region
                    fill_start = c.x;
                } else if prev_winding != 0 && winding == 0 {
                    // Leaving filled region — add coverage for span [fill_start, c.x]
                    let span_left = fill_start.max(0.0);
                    let span_right = c.x.min(width_f);
                    if span_right > span_left {
                        add_span_coverage(&mut pixel_cov, span_left, span_right, width);
                    }
                }
            }

            // Handle unclosed fill (winding != 0 at end of scanline)
            if winding != 0 {
                let span_left = fill_start.max(0.0);
                let span_right = width_f;
                if span_right > span_left {
                    add_span_coverage(&mut pixel_cov, span_left, span_right, width);
                }
            }
        }

        // Convert accumulated coverage to 0-255
        let row_offset = row * width;
        for px in 0..width {
            // pixel_cov[px] is the sum of fractional widths across all sub-scanlines.
            // Divide by SUB_SCANLINES to normalize to [0.0, 1.0], then scale to [0, 255].
            let c = pixel_cov[px] * INV_SUB * 255.0;
            coverage[row_offset + px] = if c >= 255.0 { 255 } else if c <= 0.0 { 0 } else { c as u8 };
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

/// Add fractional horizontal coverage for a filled span [left, right] to the pixel buffer.
/// For pixels fully inside the span, adds 1.0. For edge pixels, adds the fraction covered.
#[inline]
fn add_span_coverage(pixel_cov: &mut [f32], left: f32, right: f32, width: usize) {
    let px_start = left as usize;
    let px_end = ((ceil(right) as usize).min(width)).max(px_start);

    if px_start == px_end {
        return;
    }

    if px_start + 1 == px_end {
        // Span fits within a single pixel
        pixel_cov[px_start] += right - left;
        return;
    }

    // Left edge pixel: partial coverage
    let left_frac = (px_start + 1) as f32 - left;
    pixel_cov[px_start] += left_frac;

    // Fully covered interior pixels
    for px in (px_start + 1)..(px_end - 1) {
        pixel_cov[px] += 1.0;
    }

    // Right edge pixel: partial coverage
    let right_frac = right - (px_end - 1) as f32;
    if px_end - 1 < width {
        pixel_cov[px_end - 1] += right_frac;
    }
}

/// Find x-intersection of a line segment with a horizontal scanline at y,
/// along with the winding direction of the edge.
fn intersect_scanline(seg: &LineSegment, y: f32) -> Option<Crossing> {
    let y0 = seg.y0;
    let y1 = seg.y1;

    // Segment must cross the scanline (not just touch)
    if (y0 < y && y1 < y) || (y0 >= y && y1 >= y) {
        return None;
    }

    let dy = y1 - y0;
    let abs_dy = if dy < 0.0 { -dy } else { dy };
    if abs_dy < 1e-10 {
        return None;
    }

    let t = (y - y0) / dy;
    let x = seg.x0 + t * (seg.x1 - seg.x0);

    // Direction: +1 if edge goes downward (y increases), -1 if upward
    let dir = if y1 > y0 { 1 } else { -1 };

    Some(Crossing { x, dir })
}

/// Rasterize with LCD subpixel rendering (horizontal RGB striping).
///
/// Rasterizes at 3x horizontal resolution, then maps each triplet of sub-pixel
/// columns to R, G, B coverage values for a single output pixel. This gives
/// ~3x effective horizontal resolution for text on LCD displays.
pub fn rasterize_subpixel(
    segments: &[LineSegment],
    width: usize,
    height: usize,
    x_offset: i32,
    y_offset: i32,
) -> SubpixelBitmap {
    if width == 0 || height == 0 || segments.is_empty() {
        return SubpixelBitmap {
            width,
            height,
            x_offset,
            y_offset,
            coverage: vec![0; width * height * 3],
        };
    }

    // Rasterize at 3x horizontal resolution
    let wide = width * 3;
    let mut coverage_rgb = vec![0u8; width * height * 3];
    let mut crossings: Vec<Crossing> = Vec::with_capacity(32);
    let mut pixel_cov = vec![0.0f32; wide];
    let wide_f = wide as f32;

    // Scale segments to 3x horizontal
    let scaled: Vec<LineSegment> = segments.iter().map(|s| LineSegment {
        x0: s.x0 * 3.0,
        y0: s.y0,
        x1: s.x1 * 3.0,
        y1: s.y1,
    }).collect();

    for row in 0..height {
        for v in pixel_cov.iter_mut() { *v = 0.0; }

        for sub in 0..SUB_SCANLINES {
            let y = row as f32 + (sub as f32 + 0.5) * INV_SUB;

            crossings.clear();
            for seg in &scaled {
                if let Some(crossing) = intersect_scanline(seg, y) {
                    crossings.push(crossing);
                }
            }

            if crossings.is_empty() {
                continue;
            }

            crossings.sort_unstable_by(|a, b| {
                a.x.partial_cmp(&b.x).unwrap_or(core::cmp::Ordering::Equal)
            });

            let mut winding: i32 = 0;
            let mut fill_start: f32 = 0.0;

            for c in &crossings {
                let prev_winding = winding;
                winding += c.dir;

                if prev_winding == 0 && winding != 0 {
                    fill_start = c.x;
                } else if prev_winding != 0 && winding == 0 {
                    let span_left = fill_start.max(0.0);
                    let span_right = c.x.min(wide_f);
                    if span_right > span_left {
                        add_span_coverage(&mut pixel_cov, span_left, span_right, wide);
                    }
                }
            }

            if winding != 0 {
                let span_left = fill_start.max(0.0);
                let span_right = wide_f;
                if span_right > span_left {
                    add_span_coverage(&mut pixel_cov, span_left, span_right, wide);
                }
            }
        }

        // Map 3x sub-pixel columns to RGB triplets per output pixel
        let row_offset = row * width * 3;
        for px in 0..width {
            let r_cov = pixel_cov[px * 3] * INV_SUB * 255.0;
            let g_cov = pixel_cov[px * 3 + 1] * INV_SUB * 255.0;
            let b_cov = pixel_cov[px * 3 + 2] * INV_SUB * 255.0;
            coverage_rgb[row_offset + px * 3] = r_cov.min(255.0).max(0.0) as u8;
            coverage_rgb[row_offset + px * 3 + 1] = g_cov.min(255.0).max(0.0) as u8;
            coverage_rgb[row_offset + px * 3 + 2] = b_cov.min(255.0).max(0.0) as u8;
        }
    }

    SubpixelBitmap {
        width,
        height,
        x_offset,
        y_offset,
        coverage: coverage_rgb,
    }
}
