//! The image file, read a block range at a time. Any read failure makes the image
//! unreadable, which the checker reports with exit 8.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use super::raw::BLOCK;

pub struct Image {
    file: File,
    /// floor(file bytes / 4096): the partition's block count.
    pub blocks: u64,
}

impl Image {
    pub fn open(path: &Path) -> io::Result<Image> {
        let file = File::open(path)?;
        let bytes = file.metadata()?.len();
        Ok(Image { file, blocks: bytes / BLOCK as u64 })
    }

    /// Reads `count` blocks from `block`; the range must lie inside the image.
    pub fn read(&self, block: u64, count: u64) -> io::Result<Vec<u8>> {
        let end = block.checked_add(count).filter(|&e| e <= self.blocks);
        if end.is_none() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("blocks {block}+{count} past the image")));
        }
        let mut buf = vec![0u8; count as usize * BLOCK];
        let mut file = &self.file;
        file.seek(SeekFrom::Start(block * BLOCK as u64))?;
        file.read_exact(&mut buf)?;
        Ok(buf)
    }
}
