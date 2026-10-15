//! `loca`: where each glyph's record lies in `glyf`.

use crate::error::FontError;
use crate::reader::{u16_at, u32_at};
use crate::work;

#[derive(Debug, Clone, Copy)]
pub struct LocaTable<'a> {
    data: &'a [u8],
    long: bool,
}

impl<'a> LocaTable<'a> {
    pub fn new(data: &'a [u8], index_to_loc_format: i16) -> LocaTable<'a> {
        LocaTable { data, long: index_to_loc_format == 1 }
    }

    /// Check the table holds exactly `num_glyphs + 1` entries (`GlyphCountMismatch`) and
    /// that they never decrease (`BadLoca`).
    pub fn validate(&self, num_glyphs: u16) -> Result<(), FontError> {
        let entries = num_glyphs as usize + 1;
        if self.data.len() != entries * if self.long { 4 } else { 2 } {
            return Err(FontError::GlyphCountMismatch);
        }
        work::add(entries as u64);
        let mut prev = 0u32;
        for i in 0..entries {
            let o = self.entry(i).ok_or(FontError::Truncated)?;
            if o < prev {
                return Err(FontError::BadLoca);
            }
            prev = o;
        }
        Ok(())
    }

    /// Entry `i` as a byte offset into `glyf`.
    pub fn entry(&self, i: usize) -> Option<u32> {
        if self.long { u32_at(self.data, 4 * i) } else { u16_at(self.data, 2 * i).map(|v| v as u32 * 2) }
    }
}
