//! `kern`: the legacy kerning table's horizontal format 0 pairs (optional). A table libfont
//! cannot read gives no kerning.

use core::ops::Range;

use crate::reader::{i16_at, u16_at};
use crate::work;

/// The pairs of the first horizontal, non-minimum, non-cross-stream format 0 subtable.
#[derive(Debug, Clone)]
pub struct KernTable {
    /// The pair array (6 bytes each), absolute in the font file.
    pairs: Range<usize>,
}

impl KernTable {
    /// `table` is the kern table's range in `file`.
    pub fn parse(file: &[u8], table: Range<usize>) -> Option<KernTable> {
        let d = file.get(table.clone())?;
        if u16_at(d, 0)? != 0 {
            return None;
        }
        let n_tables = u16_at(d, 2)? as usize;
        let mut at = 4usize;
        work::add(n_tables as u64);
        for _ in 0..n_tables {
            let length = u16_at(d, at + 2)? as usize;
            let coverage = u16_at(d, at + 4)?;
            let format = coverage >> 8;
            if format == 0 && coverage & 0x01 != 0 && coverage & 0x02 == 0 && coverage & 0x04 == 0 {
                let n = u16_at(d, at + 6)? as usize;
                let start = at + 14;
                let end = start.checked_add(6 * n)?;
                if end > d.len() {
                    return None;
                }
                return Some(KernTable { pairs: table.start + start..table.start + end });
            }
            if length < 6 {
                return None;
            }
            at = at.checked_add(length)?;
        }
        None
    }

    /// The kerning value for the pair, in font units (0 when absent).
    pub fn value(&self, file: &[u8], left: u16, right: u16) -> i16 {
        let key = ((left as u32) << 16) | right as u32;
        let n = (self.pairs.end - self.pairs.start) / 6;
        let (mut lo, mut hi) = (0usize, n);
        while lo < hi {
            work::add(1);
            let mid = (lo + hi) / 2;
            let o = self.pairs.start + 6 * mid;
            let (Some(l), Some(r)) = (u16_at(file, o), u16_at(file, o + 2)) else { return 0 };
            let k = ((l as u32) << 16) | r as u32;
            if key < k {
                hi = mid;
            } else if key > k {
                lo = mid + 1;
            } else {
                return i16_at(file, o + 4).unwrap_or(0);
            }
        }
        0
    }
}
