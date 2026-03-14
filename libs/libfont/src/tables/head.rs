//! `head` table: font header — units_per_em, index_to_loc_format.

use alloc::string::String;
use crate::reader::Reader;

#[derive(Debug, Clone, Copy)]
pub struct HeadTable {
    pub units_per_em: u16,
    pub index_to_loc_format: i16,
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
}

impl HeadTable {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(data);
        r.skip(4)?; // majorVersion, minorVersion
        r.skip(4)?; // fontRevision (Fixed)
        r.skip(4)?; // checksumAdjustment
        let magic = r.read_u32()?;
        if magic != 0x5F0F3CF5 {
            return Err(String::from("invalid head table magic"));
        }
        r.skip(2)?; // flags
        let units_per_em = r.read_u16()?;
        r.skip(8)?; // created (LONGDATETIME)
        r.skip(8)?; // modified (LONGDATETIME)
        let x_min = r.read_i16()?;
        let y_min = r.read_i16()?;
        let x_max = r.read_i16()?;
        let y_max = r.read_i16()?;
        r.skip(2)?; // macStyle
        r.skip(2)?; // lowestRecPPEM
        r.skip(2)?; // fontDirectionHint
        let index_to_loc_format = r.read_i16()?;

        Ok(Self {
            units_per_em,
            index_to_loc_format,
            x_min,
            y_min,
            x_max,
            y_max,
        })
    }
}
