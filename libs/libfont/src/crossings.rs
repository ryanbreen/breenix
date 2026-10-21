//! Binary32 sub-scanline crossings for GPU coverage_spans (BreenGL section 8.3).
//! This module produces geometry only, in the frozen rasterizer's operation order.

use alloc::vec::Vec;
use crate::{outline::LineSegment, work};

#[derive(Clone, Copy)]
pub struct Crossing {
    pub x: f32,
    pub direction: i32,
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

/// Reusable scratch for one sorted sub-scanline; storage is proportional to edges,
/// independent of target area. Equal-x crossings retain outline order.
pub struct Crossings {
    crossings: Vec<Crossing>,
    scratch: Vec<Crossing>,
}

impl Crossings {
    pub fn new(edges: usize) -> Self {
        Self { crossings: Vec::with_capacity(edges), scratch: Vec::with_capacity(edges) }
    }

    /// Prepare sub-scanline `sub` (0..8) in pixel row `row`. The caller admits the
    /// glyph's original total raster bound before entering any scanline loop.
    pub fn scanline(&mut self, segments: &[LineSegment], row: usize, sub: usize) -> &[Crossing] {
        assert!(sub < 8);
        work::add(1 + segments.len() as u64);
        let y = row as f32 + (sub as f32 + 0.5) * (1.0 / 8.0);
        self.crossings.clear();
        for s in segments {
            if (s.y0 < y && s.y1 < y) || (s.y0 >= y && s.y1 >= y) {
                continue;
            }
            let dy = s.y1 - s.y0;
            if dy == 0.0 { continue; }
            let t = (y - s.y0) / dy;
            self.crossings.push(Crossing {
                x: s.x0 + t * (s.x1 - s.x0),
                direction: if s.y1 > s.y0 { 1 } else { -1 },
            });
        }
        let sorted = merge_sort(&mut self.crossings, &mut self.scratch);
        work::add(sorted.len() as u64);
        sorted
    }
}
