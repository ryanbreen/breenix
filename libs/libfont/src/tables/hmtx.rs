//! `hmtx` table: per-glyph advance widths and left side bearings.

use crate::reader::{read_i16_at, read_u16_at};

pub struct HmtxTable<'a> {
    data: &'a [u8],
    num_h_metrics: u16,
}

impl<'a> HmtxTable<'a> {
    pub fn new(data: &'a [u8], num_h_metrics: u16) -> Self {
        Self { data, num_h_metrics }
    }

    pub fn advance_width(&self, glyph_index: u16) -> u16 {
        if glyph_index < self.num_h_metrics {
            let offset = glyph_index as usize * 4;
            if offset + 2 <= self.data.len() {
                read_u16_at(self.data, offset)
            } else {
                0
            }
        } else {
            // All glyphs beyond num_h_metrics share the last advance width
            let offset = (self.num_h_metrics as usize - 1) * 4;
            if offset + 2 <= self.data.len() {
                read_u16_at(self.data, offset)
            } else {
                0
            }
        }
    }

    pub fn left_side_bearing(&self, glyph_index: u16) -> i16 {
        if glyph_index < self.num_h_metrics {
            let offset = glyph_index as usize * 4 + 2;
            if offset + 2 <= self.data.len() {
                read_i16_at(self.data, offset)
            } else {
                0
            }
        } else {
            // LSBs for glyphs beyond num_h_metrics are stored after the metrics array
            let base = self.num_h_metrics as usize * 4;
            let idx = (glyph_index - self.num_h_metrics) as usize;
            let offset = base + idx * 2;
            if offset + 2 <= self.data.len() {
                read_i16_at(self.data, offset)
            } else {
                0
            }
        }
    }
}
