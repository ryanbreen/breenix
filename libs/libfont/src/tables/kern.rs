//! `kern`: the legacy kerning table's horizontal format 0 pairs (optional; OpenType
//! specification, "kern" chapter). A read past the table or past a subtable's own length is
//! `Truncated` for the font; a table of another version (Apple's) gives no kerning.
//!
//! A subtable's length field is 16 bits, so a format 0 subtable of more than 10,920 pairs
//! cannot state its size: its length field holds the size modulo 65,536 (PT Serif's single
//! subtable of 20,958 pairs gives 60,226). Such a subtable's pairs fix its size.

use alloc::vec::Vec;
use core::ops::Range;

use crate::error::FontError;
use crate::reader::{i16_at, u16_at};
use crate::work;

/// One horizontal, non-minimum, non-cross-stream format 0 subtable with at least one pair.
#[derive(Debug, Clone, Copy)]
struct Subtable {
    /// The pair array (6 bytes each), absolute in the font file.
    pairs: u32,
    count: u16,
    /// Coverage bit 3: this subtable's value replaces the sum so far instead of adding.
    replace: bool,
}

/// The subtables kerning reads, in table order.
#[derive(Debug, Clone)]
pub struct KernTable {
    subtables: Vec<Subtable>,
}

const HORIZONTAL: u16 = 0x01;
const MINIMUM: u16 = 0x02;
const CROSS_STREAM: u16 = 0x04;
const OVERRIDE: u16 = 0x08;

impl KernTable {
    /// `table` is the kern table's range in `file`. `None` for a table with no subtable
    /// kerning reads.
    pub fn parse(file: &[u8], table: Range<usize>) -> Result<Option<KernTable>, FontError> {
        let d = file.get(table.clone()).ok_or(FontError::Truncated)?;
        if u16_at(d, 0).ok_or(FontError::Truncated)? != 0 {
            return Ok(None);
        }
        let n_tables = u16_at(d, 2).ok_or(FontError::Truncated)? as usize;
        let mut subtables = Vec::new();
        let mut at = 4usize;
        for _ in 0..n_tables {
            work::add(1);
            // Each subtable: version, length (header included), coverage, then its data.
            let length = u16_at(d, at + 2).ok_or(FontError::Truncated)? as usize;
            let coverage = u16_at(d, at + 4).ok_or(FontError::Truncated)?;
            if length < 6 {
                return Err(FontError::BadTable(*b"kern"));
            }
            let format = coverage >> 8;
            let mut size = length;
            if format == 0 {
                // The pairs follow nPairs, searchRange, entrySelector and rangeShift.
                let count = u16_at(d, at + 6).ok_or(FontError::Truncated)? as usize;
                let need = 14 + 6 * count;
                if need > length {
                    if need % 65_536 != length {
                        return Err(FontError::Truncated);
                    }
                    size = need;
                }
            }
            d.get(at..at + size).ok_or(FontError::Truncated)?;
            if format == 0 && coverage & (HORIZONTAL | MINIMUM | CROSS_STREAM) == HORIZONTAL {
                let count = u16_at(d, at + 6).ok_or(FontError::Truncated)?;
                if count > 0 {
                    let pairs = (table.start + at + 14) as u32;
                    subtables.push(Subtable { pairs, count, replace: coverage & OVERRIDE != 0 });
                }
            }
            at += size;
        }
        Ok((!subtables.is_empty()).then_some(KernTable { subtables }))
    }

    /// The kerning value for the pair, in font units: each subtable holding the pair adds
    /// its value, or replaces the sum so far when it is an override subtable.
    pub fn value(&self, file: &[u8], left: u16, right: u16) -> i32 {
        let mut sum = 0i32;
        for s in &self.subtables {
            work::add(1);
            if let Some(v) = search(file, s, left, right) {
                sum = if s.replace { v as i32 } else { sum + v as i32 };
            }
        }
        sum
    }
}

/// The pair's value in subtable `s`, by binary search of its sorted pairs.
fn search(file: &[u8], s: &Subtable, left: u16, right: u16) -> Option<i16> {
    let key = ((left as u32) << 16) | right as u32;
    let (mut lo, mut hi) = (0usize, s.count as usize);
    while lo < hi {
        work::add(1);
        let mid = (lo + hi) / 2;
        let o = s.pairs as usize + 6 * mid;
        let k = ((u16_at(file, o)? as u32) << 16) | u16_at(file, o + 2)? as u32;
        if key < k {
            hi = mid;
        } else if key > k {
            lo = mid + 1;
        } else {
            return i16_at(file, o + 4);
        }
    }
    None
}
