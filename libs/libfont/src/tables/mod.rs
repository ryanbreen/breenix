//! TrueType table directory parser and tag-based table lookup.

use alloc::string::String;
use crate::reader::Reader;

pub mod head;
pub mod hhea;
pub mod hmtx;
pub mod maxp;
pub mod cmap;
pub mod loca;
pub mod glyf;
pub mod kern;

#[derive(Debug, Clone, Copy)]
pub struct TableRecord {
    pub tag: [u8; 4],
    pub offset: u32,
    pub length: u32,
}

#[derive(Debug, Clone)]
pub struct TableDirectory {
    pub num_tables: u16,
    pub records: alloc::vec::Vec<TableRecord>,
}

impl TableDirectory {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(data);
        let sfversion = r.read_u32()?;
        // Accept TrueType (0x00010000) and OpenType with TrueType outlines ('true')
        if sfversion != 0x00010000 && sfversion != 0x74727565 {
            return Err(String::from("not a TrueType font"));
        }
        let num_tables = r.read_u16()?;
        r.skip(6)?; // searchRange, entrySelector, rangeShift

        let mut records = alloc::vec::Vec::with_capacity(num_tables as usize);
        for _ in 0..num_tables {
            let tag = r.read_tag()?;
            r.skip(4)?; // checksum
            let offset = r.read_u32()?;
            let length = r.read_u32()?;
            records.push(TableRecord { tag, offset, length });
        }

        Ok(Self { num_tables, records })
    }

    pub fn find_table(&self, tag: &[u8; 4]) -> Option<&TableRecord> {
        self.records.iter().find(|r| &r.tag == tag)
    }

    pub fn table_data<'a>(&self, data: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
        let rec = self.find_table(tag)?;
        let start = rec.offset as usize;
        let end = start + rec.length as usize;
        if end <= data.len() {
            Some(&data[start..end])
        } else {
            None
        }
    }

    pub fn table_range(&self, data_len: usize, tag: &[u8; 4]) -> Option<core::ops::Range<usize>> {
        let rec = self.find_table(tag)?;
        let start = rec.offset as usize;
        let end = start + rec.length as usize;
        if end <= data_len {
            Some(start..end)
        } else {
            None
        }
    }
}
