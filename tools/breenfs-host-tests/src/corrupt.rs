//! Committed-image corruption corpus (breenfs design section 5).
use crate::{
    block::{BlockDevice, BlockError},
    crash::Image,
    recording::SECTOR_SIZE,
    Error, Result,
};
use alloc::vec::Vec;
use spin::Mutex;

/// Plain recovery device without the healthy-image recording oracle.
pub struct Device(Mutex<Image>);
pub fn device(image: Image) -> Device {
    Device(Mutex::new(image))
}
impl Device {
    pub fn image(&self) -> Image {
        self.0.lock().clone()
    }
}
impl BlockDevice for Device {
    fn read_block(&self, sector: u64, bytes: &mut [u8]) -> core::result::Result<(), BlockError> {
        let image = self.0.lock();
        if sector >= image.sector_count || bytes.len() != SECTOR_SIZE {
            return Err(BlockError::OutOfBounds);
        }
        bytes.copy_from_slice(image.sectors.get(&sector).unwrap_or(&[0; SECTOR_SIZE]));
        Ok(())
    }
    fn write_block(&self, sector: u64, bytes: &[u8]) -> core::result::Result<(), BlockError> {
        let mut image = self.0.lock();
        if sector >= image.sector_count || bytes.len() != SECTOR_SIZE {
            return Err(BlockError::OutOfBounds);
        }
        let bytes: [u8; SECTOR_SIZE] = bytes.try_into().map_err(|_| BlockError::IoError)?;
        if bytes == [0; SECTOR_SIZE] {
            image.sectors.remove(&sector);
        } else {
            image.sectors.insert(sector, bytes);
        }
        Ok(())
    }
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }
    fn num_blocks(&self) -> u64 {
        self.0.lock().sector_count
    }
    fn flush(&self) -> core::result::Result<(), BlockError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Slot,
    Node { addr: u64 },
    Data,
}

#[derive(Clone, Copy, Debug)]
pub struct Flip {
    pub seed: u64,
    pub operation: Option<usize>,
    pub block: u64,
    /// Bit offset within the flipped 4 KiB block.
    pub bit: usize,
    pub kind: Kind,
}

#[derive(Debug, Default)]
pub struct Report {
    pub committed_images: usize,
    pub slots: usize,
    pub nodes: usize,
    pub data: usize,
}

/// Visit one seeded bit flip per reachable block of the populated formatter
/// image and the committed image after a successful fsync in the fixed streams.
/// File identities and lengths come from the pristine committed image so that
/// an error in a corrupted state walk cannot skip the separate file reads.
pub fn bit_flips(
    _populated: Vec<u8>,
    _empty: Vec<u8>,
    _seeds: &[u64],
    _operations: usize,
    _observe: impl FnMut(&Image, Flip, &[(u64, u64)]) -> Result<()>,
) -> Result<Report> {
    Err(Error::Unimplemented)
}
