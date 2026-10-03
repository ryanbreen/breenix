//! NVMe Block Device Wrapper
//!
//! Implements the BlockDevice trait for an NVMe namespace (x86-64).

use super::{BlockDevice, BlockError};
use crate::drivers::nvme::{self, NvmeController, SECTOR_SIZE};
use alloc::sync::Arc;

/// Sectors one NVMe command moves (one 4 KiB page).
const SECTORS_PER_COMMAND: usize = 8;

/// The namespace of one attached NVMe controller.
pub struct NvmeBlockDevice {
    controller: Arc<NvmeController>,
}

impl NvmeBlockDevice {
    /// The namespace of the controller at `index`, in PCI order.
    pub fn new(index: usize) -> Option<Self> {
        Some(NvmeBlockDevice {
            controller: nvme::controller(index)?,
        })
    }
}

impl BlockDevice for NvmeBlockDevice {
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::IoError);
        }
        if block_num >= self.num_blocks() {
            return Err(BlockError::OutOfBounds);
        }
        self.controller
            .read_sectors(block_num, &mut buf[..SECTOR_SIZE])
            .map_err(BlockError::from)
    }

    fn read_blocks(
        &self,
        start_block: u64,
        block_count: usize,
        buf: &mut [u8],
    ) -> Result<(), BlockError> {
        let len = block_count
            .checked_mul(SECTOR_SIZE)
            .ok_or(BlockError::IoError)?;
        if buf.len() < len {
            return Err(BlockError::IoError);
        }
        let end = start_block
            .checked_add(block_count as u64)
            .ok_or(BlockError::OutOfBounds)?;
        if end > self.num_blocks() {
            return Err(BlockError::OutOfBounds);
        }

        let mut done = 0;
        while done < block_count {
            let count = (block_count - done).min(SECTORS_PER_COMMAND);
            let offset = done * SECTOR_SIZE;
            self.controller
                .read_sectors(
                    start_block + done as u64,
                    &mut buf[offset..offset + count * SECTOR_SIZE],
                )
                .map_err(BlockError::from)?;
            done += count;
        }
        Ok(())
    }

    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::IoError);
        }
        if block_num >= self.num_blocks() {
            return Err(BlockError::OutOfBounds);
        }
        self.controller
            .write_sectors(block_num, &buf[..SECTOR_SIZE])
            .map_err(BlockError::from)
    }

    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn num_blocks(&self) -> u64 {
        self.controller.capacity()
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.controller.flush().map_err(BlockError::from)
    }
}
