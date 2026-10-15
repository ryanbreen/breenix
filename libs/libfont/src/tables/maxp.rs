//! `maxp`: glyph count and outline limits.

use crate::error::FontError;
use crate::reader::{u16_at, u32_at};

#[derive(Debug, Clone, Copy)]
pub struct MaxpTable {
    pub num_glyphs: u16,
    /// Points in a simple glyph (version 1.0 only; 65,535 otherwise).
    pub max_points: u16,
    /// Points in a compound glyph after expansion (version 1.0 only).
    pub max_composite_points: u16,
}

impl MaxpTable {
    pub fn parse(d: &[u8]) -> Result<MaxpTable, FontError> {
        let version = u32_at(d, 0).ok_or(FontError::Truncated)?;
        let num_glyphs = u16_at(d, 4).ok_or(FontError::Truncated)?;
        match version {
            0x0000_5000 => Ok(MaxpTable { num_glyphs, max_points: u16::MAX, max_composite_points: u16::MAX }),
            0x0001_0000 => {
                if d.len() < 32 {
                    return Err(FontError::Truncated);
                }
                Ok(MaxpTable {
                    num_glyphs,
                    max_points: u16_at(d, 6).ok_or(FontError::Truncated)?,
                    max_composite_points: u16_at(d, 10).ok_or(FontError::Truncated)?,
                })
            }
            _ => Err(FontError::BadTable(*b"maxp")),
        }
    }
}
