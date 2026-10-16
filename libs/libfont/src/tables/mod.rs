//! The TrueType table directory and the tables libfont reads (OpenType specification,
//! "Organization of an OpenType font" and the per-table chapters).

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

/// The table directory of an sfnt file, read in place: a lookup scans its records, so a
/// font's directory costs no allocation.
#[derive(Debug, Clone, Copy)]
pub struct TableDirectory<'a> {
    data: &'a [u8],
    count: usize,
}

impl<'a> TableDirectory<'a> {
    /// Read the directory: a record whose offset or length runs past the file is
    /// `BadTable(tag)`; a directory cut short is `Truncated`.
    pub fn parse(data: &'a [u8]) -> Result<TableDirectory<'a>, FontError> {
        let version = u32_at(data, 0).ok_or(FontError::Truncated)?;
        // TrueType outlines: version 1.0 or Apple's 'true'.
        if version != 0x0001_0000 && version != 0x7472_7565 {
            return Err(FontError::BadTable(*b"sfnt"));
        }
        let count = u16_at(data, 4).ok_or(FontError::Truncated)? as usize;
        if 12 + 16 * count > data.len() {
            return Err(FontError::Truncated);
        }
        let dir = TableDirectory { data, count };
        for i in 0..count {
            work::add(1);
            let (tag, range) = dir.record(i);
            if range.is_none() {
                return Err(FontError::BadTable(tag));
            }
        }
        Ok(dir)
    }

    /// Record `i`'s tag, and its byte range when that lies inside the file.
    fn record(&self, i: usize) -> ([u8; 4], Option<Range<usize>>) {
        let r = 12 + 16 * i;
        let d = self.data;
        let tag = [d[r], d[r + 1], d[r + 2], d[r + 3]];
        let (offset, length) = (u32_at(d, r + 8).unwrap_or(0) as u64, u32_at(d, r + 12).unwrap_or(0) as u64);
        let range = (offset + length <= d.len() as u64).then(|| offset as usize..(offset + length) as usize);
        (tag, range)
    }

    /// The byte range of table `tag`, if the font has it.
    pub fn find(&self, tag: &[u8; 4]) -> Option<Range<usize>> {
        for i in 0..self.count {
            work::add(1);
            let (t, range) = self.record(i);
            if &t == tag {
                return range;
            }
        }
        None
    }

    /// The byte range of a table the font must have.
    pub fn require(&self, tag: &[u8; 4]) -> Result<Range<usize>, FontError> {
        self.find(tag).ok_or(FontError::MissingTable(*tag))
    }
}
