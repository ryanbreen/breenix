//! Scanline coverage rasterizer: line segments to coverage bitmaps
//! (docs/gui/breengel.md section 7).
//!
//! Non-zero winding with fractional horizontal coverage: each pixel row is sampled on 8
//! sub-scanlines; on each, the crossings of every segment are sorted (a counted merge
//! sort) and each filled span adds the fraction of each pixel it covers.

use alloc::vec;
use alloc::vec::Vec;

use crate::error::FontError;
use crate::float::ceil;
use crate::outline::LineSegment;
use crate::work;

/// A rasterized glyph: one coverage byte per pixel, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphBitmap {
    pub width: usize,
    pub height: usize,
    pub x_offset: i32,
    pub y_offset: i32,
    pub coverage: Vec<u8>,
}

/// An LCD glyph: three coverage bytes (R, G, B) per pixel, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubpixelBitmap {
    pub width: usize,
    pub height: usize,
    pub x_offset: i32,
    pub y_offset: i32,
    pub coverage: Vec<u8>,
}

const SUB_SCANLINES: usize = 8;
const INV_SUB: f32 = 1.0 / SUB_SCANLINES as f32;

/// Section 7's step bound for rasterizing `e` segments into `h` rows of `c` sampled
/// columns: `E + H × (10C + 8E × (3 + ⌈log₂(E + 1)⌉) + 17) + 64`.
pub fn raster_bound(e: usize, h: usize, c: usize) -> u128 {
    let (e, h, c) = (e as u128, h as u128, c as u128);
    let log = (128 - (e + 1).leading_zeros() as u128) - if (e + 1).is_power_of_two() { 1 } else { 0 };
    e + h * (10 * c + 8 * e * (3 + log) + 17) + 64
}

/// Glyphs whose bound exceeds this are refused before any raster loop.
pub const MAX_RASTER_STEPS: u128 = 1 << 28;

#[derive(Clone, Copy)]
struct Crossing {
    x: f32,
    dir: i32,
}

/// Sort `v` by x with a stable bottom-up merge sort that alternates between `v` and `tmp`,
/// and return the buffer holding the result. Each pass is one loop over the `n` output
/// slots, moving to the next pair of runs at each run boundary, so a pass counts `1 + n`
/// steps.
fn merge_sort<'a>(v: &'a mut [Crossing], tmp: &'a mut Vec<Crossing>) -> &'a [Crossing] {
    let n = v.len();
    if n < 2 {
        return v;
    }
    tmp.clear();
    tmp.extend_from_slice(v);
    let (mut width, mut in_v) = (1, true);
    while width < n {
        work::add(1 + n as u64);
        let (src, dst): (&[Crossing], &mut [Crossing]) = if in_v { (&*v, &mut tmp[..]) } else { (&tmp[..], &mut *v) };
        // The runs src[lo..mid] and src[mid..hi] merge into dst[lo..hi].
        let (mut mid, mut hi) = (width.min(n), (2 * width).min(n));
        let (mut i, mut j) = (0, mid);
        for (k, slot) in dst.iter_mut().enumerate() {
            if k == hi {
                mid = (hi + width).min(n);
                (i, j) = (hi, mid);
                hi = (hi + 2 * width).min(n);
            }
            if j >= hi || (i < mid && src[i].x <= src[j].x) {
                *slot = src[i];
                i += 1;
            } else {
                *slot = src[j];
                j += 1;
            }
        }
        in_v = !in_v;
        width *= 2;
    }
    if in_v { v } else { tmp }
}

/// Coverage sums for `columns` columns, `rows` rows of the segments (already in bitmap
/// coordinates). Each value is the covered fraction of the pixel summed over the 8
/// sub-scanlines.
fn accumulate(segments: &[LineSegment], columns: usize, rows: usize, mut emit: impl FnMut(usize, &[f32])) {
    let mut crossings: Vec<Crossing> = Vec::with_capacity(segments.len());
    let mut scratch: Vec<Crossing> = Vec::with_capacity(segments.len());
    let mut cov = vec![0.0f32; columns];
    let width_f = columns as f32;
    for row in 0..rows {
        work::add(1 + columns as u64);
        for v in cov.iter_mut() {
            *v = 0.0;
        }
        for sub in 0..SUB_SCANLINES {
            work::add(1 + segments.len() as u64);
            let y = row as f32 + (sub as f32 + 0.5) * INV_SUB;
            crossings.clear();
            for s in segments {
                if (s.y0 < y && s.y1 < y) || (s.y0 >= y && s.y1 >= y) {
                    continue;
                }
                let dy = s.y1 - s.y0;
                if dy == 0.0 {
                    continue;
                }
                let t = (y - s.y0) / dy;
                crossings.push(Crossing { x: s.x0 + t * (s.x1 - s.x0), dir: if s.y1 > s.y0 { 1 } else { -1 } });
            }
            if crossings.is_empty() {
                continue;
            }
            let sorted = merge_sort(&mut crossings, &mut scratch);
            work::add(sorted.len() as u64);
            let (mut winding, mut fill_start) = (0i32, 0.0f32);
            for c in sorted {
                let before = winding;
                winding += c.dir;
                if before == 0 && winding != 0 {
                    fill_start = c.x;
                } else if before != 0 && winding == 0 {
                    span(&mut cov, fill_start.max(0.0), c.x.min(width_f));
                }
            }
            if winding != 0 {
                span(&mut cov, fill_start.max(0.0), width_f);
            }
        }
        work::add(columns as u64);
        emit(row, &cov);
    }
}

/// Add the covered fraction of each pixel of the span [left, right).
fn span(cov: &mut [f32], left: f32, right: f32) {
    if right <= left {
        return;
    }
    let width = cov.len();
    let start = left as usize;
    let end = ((ceil(right) as usize).min(width)).max(start);
    if start >= end {
        return;
    }
    work::add((end - start) as u64);
    if start + 1 == end {
        cov[start] += right - left;
        return;
    }
    cov[start] += (start + 1) as f32 - left;
    for v in &mut cov[start + 1..end - 1] {
        *v += 1.0;
    }
    cov[end - 1] += right - (end - 1) as f32;
}

#[inline]
fn to_byte(c: f32) -> u8 {
    let c = c * INV_SUB * 255.0;
    if c >= 255.0 { 255 } else if c <= 0.0 { 0 } else { c as u8 }
}

/// Rasterize `segments` into a `width × height` coverage bitmap. A glyph whose step bound
/// exceeds `MAX_RASTER_STEPS` is refused with `GlyphTooExpensive` before any raster loop.
pub fn rasterize(segments: &[LineSegment], width: usize, height: usize, x_offset: i32, y_offset: i32)
    -> Result<GlyphBitmap, FontError> {
    if raster_bound(segments.len(), height, width) > MAX_RASTER_STEPS {
        return Err(FontError::GlyphTooExpensive);
    }
    let mut coverage = vec![0u8; width * height];
    if !segments.is_empty() && width > 0 {
        accumulate(segments, width, height, |row, cov| {
            for (dst, &c) in coverage[row * width..(row + 1) * width].iter_mut().zip(cov) {
                *dst = to_byte(c);
            }
        });
    }
    Ok(GlyphBitmap { width, height, x_offset, y_offset, coverage })
}

/// Rasterize with LCD subpixel rendering: three sampled columns per pixel, mapped to R, G
/// and B coverage.
pub fn rasterize_subpixel(segments: &[LineSegment], width: usize, height: usize, x_offset: i32, y_offset: i32)
    -> Result<SubpixelBitmap, FontError> {
    let wide = width * 3;
    if raster_bound(segments.len(), height, wide) > MAX_RASTER_STEPS {
        return Err(FontError::GlyphTooExpensive);
    }
    work::add(segments.len() as u64);
    let scaled: Vec<LineSegment> = segments.iter().map(|s| LineSegment { x0: s.x0 * 3.0, y0: s.y0, x1: s.x1 * 3.0, y1: s.y1 }).collect();
    let mut coverage = vec![0u8; wide * height];
    if !scaled.is_empty() && width > 0 {
        accumulate(&scaled, wide, height, |row, cov| {
            for (dst, &c) in coverage[row * wide..(row + 1) * wide].iter_mut().zip(cov) {
                *dst = to_byte(c);
            }
        });
    }
    Ok(SubpixelBitmap { width, height, x_offset, y_offset, coverage })
}
