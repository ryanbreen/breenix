//! The testing profile's immutable ext2 device, independent of host disk layout.

use super::{BlockDevice, BlockError};

static IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/readonly-ext2.img"));

pub struct ReadOnlyFixture;

impl BlockDevice for ReadOnlyFixture {
    fn is_read_only(&self) -> bool { true }

    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let offset = usize::try_from(block).ok().and_then(|n| n.checked_mul(512))
            .ok_or(BlockError::OutOfBounds)?;
        let end = offset.checked_add(512).ok_or(BlockError::OutOfBounds)?;
        let bytes = IMAGE.get(offset..end).ok_or(BlockError::OutOfBounds)?;
        let output = buf.get_mut(..512).ok_or(BlockError::IoError)?;
        output.copy_from_slice(bytes);
        Ok(())
    }

    fn write_block(&self, _block: u64, _buf: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::IoError)
    }

    fn block_size(&self) -> usize { 512 }
    fn num_blocks(&self) -> u64 { (IMAGE.len() / 512) as u64 }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}
