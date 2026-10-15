//! The TrueType table directory and the tables libfont reads (OpenType specification,
//! "Organization of an OpenType font" and the per-table chapters).

use alloc::vec::Vec;
use core::ops::Range;

use crate::error::FontError;
use crate::reader::{u16_at, u32_at};
use crate::work;

pub mod cmap;
pub mod glyf;
pub mod head;
pub mod hhea;
pub mod hmtx;
pub mod kern;
pub mod loca;
pub mod maxp;

/// One table record: its tag and its byte range in the file.
#[derive(Debug, Clone)]
pub struct TableRecord {
    pub tag: [u8; 4],
    pub range: Range<usize>,
}

/// The table directory of an sfnt file.
#[derive(Debug, Clone)]
pub struct TableDirectory {
    pub records: Vec<TableRecord>,
}

impl TableDirectory {
    /// Read the directory: a record whose offset or length runs past the file is
    /// `BadTable(tag)`; a directory cut short is `Truncated`.
    pub fn parse(data: &[u8]) -> Result<TableDirectory, FontError> {
        let version = u32_at(data, 0).ok_or(FontError::Truncated)?;
        // TrueType outlines: version 1.0 or Apple's 'true'.
        if version != 0x0001_0000 && version != 0x7472_7565 {
            return Err(FontError::BadTable(*b"sfnt"));
        }
        let n = u16_at(data, 4).ok_or(FontError::Truncated)? as usize;
        if 12 + 16 * n > data.len() {
            return Err(FontError::Truncated);
        }
        let mut records = Vec::with_capacity(n);
        work::add(n as u64);
        for i in 0..n {
            let r = 12 + 16 * i;
            let tag = [data[r], data[r + 1], data[r + 2], data[r + 3]];
            let offset = u32_at(data, r + 8).ok_or(FontError::Truncated)? as u64;
            let length = u32_at(data, r + 12).ok_or(FontError::Truncated)? as u64;
            if offset + length > data.len() as u64 {
                return Err(FontError::BadTable(tag));
            }
            records.push(TableRecord { tag, range: offset as usize..(offset + length) as usize });
        }
        Ok(TableDirectory { records })
    }

    /// The byte range of table `tag`, if the font has it.
    pub fn find(&self, tag: &[u8; 4]) -> Option<Range<usize>> {
        work::add(self.records.len() as u64);
        self.records.iter().find(|r| &r.tag == tag).map(|r| r.range.clone())
    }

    /// The byte range of a table the font must have.
    pub fn require(&self, tag: &[u8; 4]) -> Result<Range<usize>, FontError> {
        self.find(tag).ok_or(FontError::MissingTable(*tag))
    }
}
