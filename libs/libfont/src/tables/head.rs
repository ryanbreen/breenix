//! `head`: the font header.

use crate::error::FontError;
use crate::reader::{i16_at, u16_at};

#[derive(Debug, Clone, Copy)]
pub struct HeadTable {
    pub units_per_em: u16,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
    /// 0 for 16-bit `loca` offsets (halved), 1 for 32-bit.
    pub index_to_loc_format: i16,
}

impl HeadTable {
    pub fn parse(d: &[u8]) -> Result<HeadTable, FontError> {
        if d.len() < 54 {
            return Err(FontError::Truncated);
        }
        let units_per_em = u16_at(d, 18).ok_or(FontError::Truncated)?;
        if units_per_em == 0 || units_per_em > 16_384 {
            return Err(FontError::BadUnitsPerEm);
        }
        let index_to_loc_format = i16_at(d, 50).ok_or(FontError::Truncated)?;
        if index_to_loc_format != 0 && index_to_loc_format != 1 {
            return Err(FontError::BadTable(*b"head"));
        }
        let g = |o| i16_at(d, o).ok_or(FontError::Truncated);
        Ok(HeadTable { units_per_em, x_min: g(36)?, y_min: g(38)?, x_max: g(40)?, y_max: g(42)?, index_to_loc_format })
    }
}
