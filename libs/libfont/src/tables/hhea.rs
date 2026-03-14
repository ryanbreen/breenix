//! `hhea` table: horizontal header — ascender, descender, line_gap, num_h_metrics.

use alloc::string::String;
use crate::reader::Reader;

#[derive(Debug, Clone, Copy)]
pub struct HheaTable {
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub num_h_metrics: u16,
}

impl HheaTable {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(data);
        r.skip(4)?; // majorVersion, minorVersion
        let ascender = r.read_i16()?;
        let descender = r.read_i16()?;
        let line_gap = r.read_i16()?;
        r.skip(2)?; // advanceWidthMax
        r.skip(2)?; // minLeftSideBearing
        r.skip(2)?; // minRightSideBearing
        r.skip(2)?; // xMaxExtent
        r.skip(2)?; // caretSlopeRise
        r.skip(2)?; // caretSlopeRun
        r.skip(2)?; // caretOffset
        r.skip(8)?; // 4 reserved i16s
        r.skip(2)?; // metricDataFormat
        let num_h_metrics = r.read_u16()?;

        Ok(Self {
            ascender,
            descender,
            line_gap,
            num_h_metrics,
        })
    }
}
