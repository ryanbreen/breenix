//! In-memory block device for WASM
//!
//! Provides a `BlockDevice` implementation backed by a `Vec<u8>`,
//! suitable for hosting an ext2 filesystem in the browser.

use breenix_core::block::{BlockDevice, BlockError};
use spin::Mutex;

/// In-memory block device backed by a flat byte array.
///
/// Uses `spin::Mutex` for interior mutability so that `write_block(&self, ...)`
/// can mutate the backing store, matching the `BlockDevice` trait signature.
pub struct MemBlockDevice {
    data: Mutex<Vec<u8>>,
    block_size: usize,
    num_blocks: u64,
}

impl MemBlockDevice {
    /// Create a new in-memory block device with the given size and block size.
    ///
    /// The `total_size` is rounded down to the nearest multiple of `block_size`.
    pub fn new(total_size: usize, block_size: usize) -> Self {
        let usable = (total_size / block_size) * block_size;
        Self {
            data: Mutex::new(vec![0u8; usable]),
            block_size,
            num_blocks: (usable / block_size) as u64,
        }
    }

    /// Create from existing data (e.g., an embedded disk image).
    ///
    /// The data length should be a multiple of `block_size`; any trailing
    /// bytes beyond the last full block are ignored for block addressing.
    pub fn from_bytes(data: Vec<u8>, block_size: usize) -> Self {
        let num_blocks = (data.len() / block_size) as u64;
        Self {
            data: Mutex::new(data),
            block_size,
            num_blocks,
        }
    }
}

impl BlockDevice for MemBlockDevice {
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if block_num >= self.num_blocks {
            return Err(BlockError::OutOfBounds);
        }
        if buf.len() < self.block_size {
            return Err(BlockError::IoError);
        }

        let offset = block_num as usize * self.block_size;
        let end = offset + self.block_size;
        let data = self.data.lock();

        if end > data.len() {
            return Err(BlockError::OutOfBounds);
        }

        buf[..self.block_size].copy_from_slice(&data[offset..end]);
        Ok(())
    }

    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError> {
        if block_num >= self.num_blocks {
            return Err(BlockError::OutOfBounds);
        }
        if buf.len() < self.block_size {
            return Err(BlockError::IoError);
        }

        let offset = block_num as usize * self.block_size;
        let end = offset + self.block_size;
        let mut data = self.data.lock();

        if end > data.len() {
            return Err(BlockError::OutOfBounds);
        }

        data[offset..end].copy_from_slice(&buf[..self.block_size]);
        Ok(())
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn num_blocks(&self) -> u64 {
        self.num_blocks
    }

    fn flush(&self) -> Result<(), BlockError> {
        // In-memory device has no persistent storage to flush.
        Ok(())
    }
}
