//! `hhea`: the horizontal header.

use crate::error::FontError;
use crate::reader::{i16_at, u16_at};

#[derive(Debug, Clone, Copy)]
pub struct HheaTable {
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub num_h_metrics: u16,
}

impl HheaTable {
    pub fn parse(d: &[u8]) -> Result<HheaTable, FontError> {
        if d.len() < 36 {
            return Err(FontError::Truncated);
        }
        let t = FontError::Truncated;
        Ok(HheaTable {
            ascender: i16_at(d, 4).ok_or(t)?,
            descender: i16_at(d, 6).ok_or(t)?,
            line_gap: i16_at(d, 8).ok_or(t)?,
            num_h_metrics: u16_at(d, 34).ok_or(t)?,
        })
    }
}
