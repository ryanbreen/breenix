//! `cmap` table: character-to-glyph mapping.
//!
//! Supports format 4 (BMP) and format 12 (full Unicode).

use alloc::boxed::Box;
use alloc::string::String;
use crate::reader::{Reader, read_u16_at, read_u32_at};

pub struct CmapTable {
    data: Box<[u8]>,
    format: u16,
    subtable_offset: usize,
}

impl CmapTable {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(data);
        r.skip(2)?; // version
        let num_tables = r.read_u16()?;

        // Find the best subtable: prefer format 12 (platform 3, encoding 10),
        // then format 4 (platform 3, encoding 1 — Unicode BMP),
        // then format 4 (platform 0).
        let mut best_offset: Option<u32> = None;
        let mut best_priority = 0u8;

        for _ in 0..num_tables {
            let platform_id = r.read_u16()?;
            let encoding_id = r.read_u16()?;
            let offset = r.read_u32()?;

            let priority = match (platform_id, encoding_id) {
                (3, 10) => 4, // Windows, full Unicode (format 12)
                (0, 4) => 3,  // Unicode, full repertoire
                (3, 1) => 2,  // Windows, Unicode BMP (format 4)
                (0, 3) => 1,  // Unicode, BMP
                _ => 0,
            };

            if priority > best_priority {
                best_priority = priority;
                best_offset = Some(offset);
            }
        }

        let subtable_offset = best_offset
            .ok_or_else(|| String::from("no suitable cmap subtable found"))? as usize;

        if subtable_offset + 2 > data.len() {
            return Err(String::from("cmap subtable offset out of bounds"));
        }

        let format = read_u16_at(data, subtable_offset);
        if format != 4 && format != 12 {
            return Err(String::from("unsupported cmap format"));
        }

        Ok(Self {
            data: Box::from(data),
            format,
            subtable_offset,
        })
    }

    pub fn glyph_index(&self, codepoint: u32) -> u16 {
        match self.format {
            4 => self.lookup_format4(codepoint),
            12 => self.lookup_format12(codepoint),
            _ => 0,
        }
    }

    fn lookup_format4(&self, codepoint: u32) -> u16 {
        if codepoint > 0xFFFF {
            return 0;
        }
        let cp = codepoint as u16;
        let base = self.subtable_offset;
        if base + 6 > self.data.len() {
            return 0;
        }
        let seg_count_x2 = read_u16_at(&self.data, base + 6) as usize;
        let seg_count = seg_count_x2 / 2;

        // Arrays start after the 14-byte format 4 header
        let end_codes_offset = base + 14;
        // +2 for reservedPad
        let start_codes_offset = end_codes_offset + seg_count_x2 + 2;
        let id_delta_offset = start_codes_offset + seg_count_x2;
        let id_range_offset_base = id_delta_offset + seg_count_x2;

        for i in 0..seg_count {
            let end_code = read_u16_at(&self.data, end_codes_offset + i * 2);
            if cp > end_code {
                continue;
            }
            let start_code = read_u16_at(&self.data, start_codes_offset + i * 2);
            if cp < start_code {
                return 0;
            }
            let id_delta = read_u16_at(&self.data, id_delta_offset + i * 2);
            let id_range_offset_pos = id_range_offset_base + i * 2;
            let id_range_offset = read_u16_at(&self.data, id_range_offset_pos);

            if id_range_offset == 0 {
                return cp.wrapping_add(id_delta);
            }

            let glyph_offset = id_range_offset_pos
                + id_range_offset as usize
                + (cp - start_code) as usize * 2;
            if glyph_offset + 2 > self.data.len() {
                return 0;
            }
            let glyph_id = read_u16_at(&self.data, glyph_offset);
            if glyph_id == 0 {
                return 0;
            }
            return glyph_id.wrapping_add(id_delta);
        }
        0
    }

    fn lookup_format12(&self, codepoint: u32) -> u16 {
        let base = self.subtable_offset;
        if base + 16 > self.data.len() {
            return 0;
        }
        // format 12: u16 format, u16 reserved, u32 length, u32 language, u32 numGroups
        let num_groups = read_u32_at(&self.data, base + 12) as usize;
        let groups_offset = base + 16;

        // Binary search over groups
        let mut lo = 0usize;
        let mut hi = num_groups;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let entry = groups_offset + mid * 12;
            if entry + 12 > self.data.len() {
                return 0;
            }
            let start_char = read_u32_at(&self.data, entry);
            let end_char = read_u32_at(&self.data, entry + 4);
            let start_glyph = read_u32_at(&self.data, entry + 8);

            if codepoint < start_char {
                hi = mid;
            } else if codepoint > end_char {
                lo = mid + 1;
            } else {
                return (start_glyph + (codepoint - start_char)) as u16;
            }
        }
        0
    }
}
