//! `maxp` table: maximum profile — num_glyphs.

use alloc::string::String;
use crate::reader::Reader;

#[derive(Debug, Clone, Copy)]
pub struct MaxpTable {
    pub num_glyphs: u16,
}

impl MaxpTable {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(data);
        r.skip(4)?; // version
        let num_glyphs = r.read_u16()?;
        Ok(Self { num_glyphs })
    }
}
