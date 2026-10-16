//! `hmtx`: advances and left side bearings. Glyphs past the last long metric take its
//! advance and their own bearing from the trailing array.

use crate::error::FontError;
use crate::reader::{i16_at, u16_at};

pub struct HmtxTable<'a> {
    data: &'a [u8],
    num_h_metrics: u16,
}

impl<'a> HmtxTable<'a> {
    pub fn new(data: &'a [u8], num_h_metrics: u16) -> HmtxTable<'a> {
        HmtxTable { data, num_h_metrics }
    }

    /// `numberOfHMetrics` of 0 or above `numGlyphs`, or a table shorter than
    /// `4n + 2(numGlyphs − n)` bytes, is `BadHmtx`.
    pub fn validate(&self, num_glyphs: u16) -> Result<(), FontError> {
        let (n, g) = (self.num_h_metrics as usize, num_glyphs as usize);
        if n == 0 || n > g || self.data.len() < 4 * n + 2 * (g - n) {
            return Err(FontError::BadHmtx);
        }
        Ok(())
    }

    pub fn advance_width(&self, glyph: u16) -> u16 {
        if self.num_h_metrics == 0 {
            return 0;
        }
        let i = glyph.min(self.num_h_metrics - 1) as usize;
        u16_at(self.data, 4 * i).unwrap_or(0)
    }

    pub fn left_side_bearing(&self, glyph: u16) -> i16 {
        let n = self.num_h_metrics as usize;
        let at = if (glyph as usize) < n { 4 * glyph as usize + 2 } else { 4 * n + 2 * (glyph as usize - n) };
        i16_at(self.data, at).unwrap_or(0)
    }
}
