//! `cmap`: codepoint to glyph mapping through a Unicode format 12 or format 4 subtable
//! (OpenType specification, "cmap" chapter). Every Unicode subtable of format 4 or 12 is
//! validated when the font is parsed; a malformed one rejects the font with `BadCmap`.

use core::ops::Range;

use crate::error::FontError;
use crate::reader::{u16_at, u32_at};
use crate::work;

/// The subtable lookups use, absolute in the font file.
#[derive(Debug, Clone, Copy)]
pub enum CmapTable {
    /// No Unicode subtable libfont reads: every codepoint is unmapped.
    None,
    Format4 { at: usize, seg_count: usize },
    Format12 { at: usize, groups: usize },
}

fn is_unicode(platform: u16, encoding: u16) -> bool {
    platform == 0 || (platform == 3 && (encoding == 1 || encoding == 10))
}

impl CmapTable {
    /// `table` is the cmap table's range in `file`.
    pub fn parse(file: &[u8], table: Range<usize>) -> Result<CmapTable, FontError> {
        let d = &file[table.clone()];
        let n = u16_at(d, 2).ok_or(FontError::Truncated)? as usize;
        if 4 + 8 * n > d.len() {
            return Err(FontError::Truncated);
        }
        let (mut best4, mut best12) = (None, None);
        work::add(n as u64);
        for i in 0..n {
            let r = 4 + 8 * i;
            let (platform, encoding) = (u16_at(d, r).unwrap_or(0), u16_at(d, r + 2).unwrap_or(0));
            let off = u32_at(d, r + 4).ok_or(FontError::Truncated)? as usize;
            if !is_unicode(platform, encoding) {
                continue;
            }
            match u16_at(d, off).ok_or(FontError::Truncated)? {
                4 => {
                    let seg_count = validate4(d, off)?;
                    best4.get_or_insert(CmapTable::Format4 { at: table.start + off, seg_count });
                }
                12 => {
                    let groups = validate12(d, off)?;
                    best12.get_or_insert(CmapTable::Format12 { at: table.start + off, groups });
                }
                _ => {}
            }
        }
        Ok(best12.or(best4).unwrap_or(CmapTable::None))
    }

    /// The glyph for `cp`, or 0 when it is unmapped.
    pub fn lookup(&self, file: &[u8], cp: u32) -> u16 {
        match *self {
            CmapTable::None => 0,
            CmapTable::Format4 { at, seg_count } => lookup4(file, at, seg_count, cp),
            CmapTable::Format12 { at, groups } => lookup12(file, at, groups, cp),
        }
    }
}

/// The subtable `d[off..off + length]`, or `Truncated` when it runs past the table.
fn subtable(d: &[u8], off: usize, length: usize) -> Result<&[u8], FontError> {
    d.get(off..off.checked_add(length).ok_or(FontError::Truncated)?).ok_or(FontError::Truncated)
}

/// Format 4: `segCountX2` even, end codes strictly increasing and ending at 0xFFFF, and every
/// `idRangeOffset` inside the subtable. Returns the segment count.
fn validate4(d: &[u8], off: usize) -> Result<usize, FontError> {
    let length = u16_at(d, off + 2).ok_or(FontError::Truncated)? as usize;
    let s = subtable(d, off, length)?;
    let seg_x2 = u16_at(s, 6).ok_or(FontError::BadCmap)? as usize;
    if seg_x2 % 2 != 0 || seg_x2 == 0 {
        return Err(FontError::BadCmap);
    }
    let segs = seg_x2 / 2;
    if 16 + 4 * seg_x2 > s.len() {
        return Err(FontError::BadCmap);
    }
    let (ends, starts, ranges) = (14, 16 + seg_x2, 16 + 3 * seg_x2);
    work::add(segs as u64);
    let mut prev_end: Option<u16> = None;
    for i in 0..segs {
        let end = u16_at(s, ends + 2 * i).ok_or(FontError::BadCmap)?;
        if prev_end.is_some_and(|p| end <= p) {
            return Err(FontError::BadCmap);
        }
        prev_end = Some(end);
        let start = u16_at(s, starts + 2 * i).ok_or(FontError::BadCmap)?;
        let ro = u16_at(s, ranges + 2 * i).ok_or(FontError::BadCmap)? as usize;
        if ro != 0 && start <= end {
            // The glyph id for `end` is read at ro_pos + ro + 2 (end − start).
            let last = ranges + 2 * i + ro + 2 * (end - start) as usize;
            if last + 2 > s.len() {
                return Err(FontError::BadCmap);
            }
        }
    }
    if prev_end != Some(0xFFFF) {
        return Err(FontError::BadCmap);
    }
    Ok(segs)
}

/// Format 12: groups sorted by start, not overlapping, each with start ≤ end. Returns the
/// group count.
fn validate12(d: &[u8], off: usize) -> Result<usize, FontError> {
    let length = u32_at(d, off + 4).ok_or(FontError::Truncated)? as usize;
    let s = subtable(d, off, length)?;
    let n = u32_at(s, 12).ok_or(FontError::BadCmap)? as usize;
    if n.checked_mul(12).and_then(|b| b.checked_add(16)).is_none_or(|b| b > s.len()) {
        return Err(FontError::BadCmap);
    }
    work::add(n as u64);
    let mut prev_end: Option<u32> = None;
    for i in 0..n {
        let g = 16 + 12 * i;
        let (start, end) = (u32_at(s, g).ok_or(FontError::BadCmap)?, u32_at(s, g + 4).ok_or(FontError::BadCmap)?);
        if start > end || prev_end.is_some_and(|p| start <= p) {
            return Err(FontError::BadCmap);
        }
        prev_end = Some(end);
    }
    Ok(n)
}

fn lookup4(file: &[u8], at: usize, segs: usize, cp: u32) -> u16 {
    if cp > 0xFFFF {
        return 0;
    }
    let cp = cp as u16;
    let seg_x2 = 2 * segs;
    let (ends, starts, deltas, ranges) = (at + 14, at + 16 + seg_x2, at + 16 + 2 * seg_x2, at + 16 + 3 * seg_x2);
    // The first segment whose end code is at least cp.
    let (mut lo, mut hi) = (0usize, segs);
    while lo < hi {
        work::add(1);
        let mid = (lo + hi) / 2;
        if u16_at(file, ends + 2 * mid).unwrap_or(0xFFFF) < cp { lo = mid + 1 } else { hi = mid }
    }
    if lo == segs {
        return 0;
    }
    let i = lo;
    let start = u16_at(file, starts + 2 * i).unwrap_or(0xFFFF);
    if start > cp {
        return 0;
    }
    let delta = u16_at(file, deltas + 2 * i).unwrap_or(0);
    let ro = u16_at(file, ranges + 2 * i).unwrap_or(0) as usize;
    if ro == 0 {
        return cp.wrapping_add(delta);
    }
    match u16_at(file, ranges + 2 * i + ro + 2 * (cp - start) as usize) {
        Some(0) | None => 0,
        Some(g) => g.wrapping_add(delta),
    }
}

fn lookup12(file: &[u8], at: usize, groups: usize, cp: u32) -> u16 {
    let (mut lo, mut hi) = (0usize, groups);
    while lo < hi {
        work::add(1);
        let mid = (lo + hi) / 2;
        let g = at + 16 + 12 * mid;
        let (start, end) = (u32_at(file, g).unwrap_or(u32::MAX), u32_at(file, g + 4).unwrap_or(0));
        if cp < start {
            hi = mid;
        } else if cp > end {
            lo = mid + 1;
        } else {
            let glyph = u32_at(file, g + 8).unwrap_or(0).wrapping_add(cp - start);
            return if glyph > 0xFFFF { 0 } else { glyph as u16 };
        }
    }
    0
}
