//! `loca` table: glyph data offset table (short/long format).

use crate::reader::{read_u16_at, read_u32_at};

pub struct LocaTable<'a> {
    data: &'a [u8],
    is_long: bool,
}

impl<'a> LocaTable<'a> {
    pub fn new(data: &'a [u8], index_to_loc_format: i16) -> Self {
        Self {
            data,
            is_long: index_to_loc_format != 0,
        }
    }

    pub fn glyph_offset(&self, glyph_index: u16) -> Option<u32> {
        let idx = glyph_index as usize;
        if self.is_long {
            let byte_offset = idx * 4;
            if byte_offset + 8 > self.data.len() {
                return None;
            }
            let offset = read_u32_at(self.data, byte_offset);
            let next = read_u32_at(self.data, byte_offset + 4);
            if offset == next {
                None // empty glyph (e.g., space)
            } else {
                Some(offset)
            }
        } else {
            let byte_offset = idx * 2;
            if byte_offset + 4 > self.data.len() {
                return None;
            }
            let offset = read_u16_at(self.data, byte_offset) as u32 * 2;
            let next = read_u16_at(self.data, byte_offset + 2) as u32 * 2;
            if offset == next {
                None
            } else {
                Some(offset)
            }
        }
    }
}
