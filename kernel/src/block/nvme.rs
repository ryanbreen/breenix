//! NVMe Block Device Wrapper
//!
//! Implements the BlockDevice trait for an NVMe namespace (x86-64).

use super::{BlockDevice, BlockError};
use crate::drivers::nvme::{self, NvmeController};
use alloc::sync::Arc;

/// The namespace of one attached NVMe controller.
pub struct NvmeBlockDevice {
    controller: Arc<NvmeController>,
    // Discovery fixes one active namespace per controller for this boot.
    device_index: usize,
}

impl NvmeBlockDevice {
    /// The namespace of the controller at `index`, in PCI order.
    pub fn new(index: usize) -> Option<Self> {
        Some(NvmeBlockDevice {
            controller: nvme::controller(index)?,
            device_index: index,
        })
    }
}

impl BlockDevice for NvmeBlockDevice {
    fn identity(&self) -> usize {
        // The index identifies the controller and its fixed selected namespace.
        0x4e56_0000_0000_0000 | self.device_index
    }

    fn hardware_disk(&self) -> Option<super::HardwareDisk> {
        Some(super::HardwareDisk::Nvme(Arc::as_ptr(&self.controller) as usize))
    }

    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(block_num, 1, buf)
    }

    fn read_blocks(
        &self,
        start_block: u64,
        block_count: usize,
        buf: &mut [u8],
    ) -> Result<(), BlockError> {
        let size = self.block_size();
        let len = block_count
            .checked_mul(size)
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
            let count = (block_count - done).min(self.controller.blocks_per_command());
            let offset = done * size;
            self.controller
                .read_sectors(
                    start_block + done as u64,
                    &mut buf[offset..offset + count * size],
                )
                .map_err(BlockError::from)?;
            done += count;
        }
        Ok(())
    }

    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError> {
        let size = self.block_size();
        if buf.len() < size {
            return Err(BlockError::IoError);
        }
        if block_num >= self.num_blocks() {
            return Err(BlockError::OutOfBounds);
        }
        self.controller
            .write_sectors(block_num, &buf[..size])
            .map_err(BlockError::from)
    }

    fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
        let size = self.block_size();
        let bytes = count.checked_mul(size).ok_or(BlockError::IoError)?;
        let buf = buf.get(..bytes).ok_or(BlockError::IoError)?;
        if start.checked_add(count as u64).is_none_or(|end| end > self.num_blocks()) {
            return Err(BlockError::OutOfBounds);
        }
        if count == 0 {
            return Ok(());
        }
        let disk = super::HardwareDisk::Nvme(Arc::as_ptr(&self.controller) as usize);
        if !super::hardware_write_allowed(disk, start, count) {
            return Err(BlockError::IoError);
        }
        let blocks_per_command = self.controller.blocks_per_command();
        for (index, chunk) in buf.chunks(blocks_per_command * size).enumerate() {
            let lba = start + (index * blocks_per_command) as u64;
            self.controller.write_sectors(lba, chunk).map_err(BlockError::from)?;
        }
        Ok(())
    }

    fn max_single_write_bytes(&self) -> usize {
        self.controller.blocks_per_command() * self.block_size()
    }

    fn block_size(&self) -> usize {
        self.controller.logical_bytes()
    }

    fn num_blocks(&self) -> u64 {
        self.controller.capacity()
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.controller.flush().map_err(BlockError::from)
    }
}
