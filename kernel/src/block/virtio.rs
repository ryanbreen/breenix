//! VirtIO Block Device Wrapper
//!
//! Implements the BlockDevice trait for VirtIO block devices, providing
//! a generic interface to the VirtIO-specific driver.

use super::{BlockDevice, BlockError};
use crate::drivers::virtio::block::{get_device_by_index, VirtioBlockDevice, SECTOR_SIZE};
use alloc::sync::Arc;

/// Wrapper for VirtIO block device that implements the generic BlockDevice trait
///
/// This wrapper provides a unified interface to the VirtIO block driver,
/// handling device selection and error translation.
#[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
pub struct VirtioBlockWrapper {
    /// Reference to the underlying VirtIO device
    device: Arc<VirtioBlockDevice>,
    /// Block size for this wrapper (always 512 bytes for VirtIO)
    block_size: usize,
}

impl VirtioBlockWrapper {
    /// Create a new VirtIO block device wrapper
    ///
    /// # Arguments
    /// * `device_index` - Index of the VirtIO block device to wrap (0 for primary)
    ///
    /// # Returns
    /// `Some(VirtioBlockWrapper)` if the device exists, `None` otherwise
    #[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
    pub fn new(device_index: usize) -> Option<Self> {
        let device = get_device_by_index(device_index)?;

        Some(VirtioBlockWrapper {
            device,
            block_size: SECTOR_SIZE,
        })
    }

    /// Get the primary (first) VirtIO block device
    #[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
    pub fn primary() -> Option<Self> {
        Self::new(0)
    }
}

impl BlockDevice for VirtioBlockWrapper {
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        // Validate buffer size
        if buf.len() < self.block_size {
            return Err(BlockError::IoError);
        }

        // Validate block number
        if block_num >= self.num_blocks() {
            return Err(BlockError::OutOfBounds);
        }

        // VirtIO uses 512-byte sectors, which matches our block size
        self.device
            .read_sector(block_num, buf)
            .map_err(BlockError::from)
    }

    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError> {
        // Validate buffer size
        if buf.len() < self.block_size {
            return Err(BlockError::IoError);
        }

        // Validate block number
        if block_num >= self.num_blocks() {
            return Err(BlockError::OutOfBounds);
        }

        // VirtIO uses 512-byte sectors, which matches our block size
        self.device
            .write_sector(block_num, buf)
            .map_err(BlockError::from)
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn num_blocks(&self) -> u64 {
        // VirtIO reports capacity in sectors (512-byte blocks)
        self.device.capacity()
    }

    fn flush(&self) -> Result<(), BlockError> {
        // VirtIO driver currently doesn't implement flush
        // Operations are synchronous, so data is already committed
        Ok(())
    }
}

/// Initialize the block device subsystem
///
/// This should be called after VirtIO initialization to make block devices
/// available through the generic BlockDevice interface.
#[allow(dead_code)] // Part of public block device API, will be called during kernel init
pub fn init() -> Result<(), &'static str> {
    // VirtIO block driver is already initialized by drivers::virtio::block::init()
    // This function exists for symmetry and future initialization needs

    // Verify that at least one block device is available
    if VirtioBlockWrapper::primary().is_none() {
        return Err("No VirtIO block device available");
    }

    log::info!("Block device subsystem initialized");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_size() {
        // This test requires a VirtIO device to be initialized
        // In practice, this would be run as part of integration tests
        if let Some(device) = VirtioBlockWrapper::primary() {
            assert_eq!(device.block_size(), SECTOR_SIZE);
        }
    }
}
