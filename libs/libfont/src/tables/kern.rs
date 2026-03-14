//! `kern` table: kerning pairs (optional).

use crate::reader::{read_u16_at, read_i16_at};

pub struct KernTable<'a> {
    data: &'a [u8],
    num_pairs: u16,
    pairs_offset: usize,
}

impl<'a> KernTable<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        // Version 0 format
        let version = read_u16_at(data, 0);
        if version != 0 {
            return None;
        }
        let n_tables = read_u16_at(data, 2);
        if n_tables == 0 || data.len() < 18 {
            return None;
        }

        // Parse first subtable
        let subtable_offset = 4;
        // Skip version (u16), length (u16)
        let coverage = read_u16_at(data, subtable_offset + 4);
        // We only support format 0 (horizontal kerning)
        let format = coverage >> 8;
        if format != 0 {
            return None;
        }
        // Check it's horizontal and not cross-stream
        if coverage & 0x01 == 0 || coverage & 0x04 != 0 {
            return None;
        }

        let num_pairs = read_u16_at(data, subtable_offset + 6);
        let pairs_offset = subtable_offset + 14; // after nPairs, searchRange, entrySelector, rangeShift

        Some(Self {
            data,
            num_pairs,
            pairs_offset,
        })
    }

    pub fn kern_value(&self, left: u16, right: u16) -> i16 {
        let key = ((left as u32) << 16) | (right as u32);

        // Binary search
        let mut lo = 0u32;
        let mut hi = self.num_pairs as u32;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let offset = self.pairs_offset + mid as usize * 6;
            if offset + 6 > self.data.len() {
                return 0;
            }
            let left_g = read_u16_at(self.data, offset);
            let right_g = read_u16_at(self.data, offset + 2);
            let pair_key = ((left_g as u32) << 16) | (right_g as u32);

            if key < pair_key {
                hi = mid;
            } else if key > pair_key {
                lo = mid + 1;
            } else {
                return read_i16_at(self.data, offset + 4);
            }
        }
        0
    }
}
