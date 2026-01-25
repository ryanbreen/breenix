//! VirtIO Block Device Wrapper
//!
//! Implements the BlockDevice trait for VirtIO block devices, providing
//! a generic interface to the VirtIO-specific driver.

use super::{BlockDevice, BlockError};

// Use PCI-based VirtIO block driver on x86_64, MMIO-based on ARM64
#[cfg(target_arch = "x86_64")]
use crate::drivers::virtio::block::{get_device_by_index, VirtioBlockDevice, SECTOR_SIZE};
#[cfg(target_arch = "aarch64")]
use crate::drivers::virtio::block_mmio as block_driver;

#[cfg(target_arch = "x86_64")]
use alloc::sync::Arc;

/// Wrapper for VirtIO block device that implements the generic BlockDevice trait
///
/// This wrapper provides a unified interface to the VirtIO block driver,
/// handling device selection and error translation.

// x86_64 version: uses PCI-based VirtIO with device instances
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
pub struct VirtioBlockWrapper {
    /// Reference to the underlying VirtIO device
    device: Arc<VirtioBlockDevice>,
    /// Block size for this wrapper (always 512 bytes for VirtIO)
    block_size: usize,
}

#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "x86_64")]
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

// ARM64 version: uses MMIO-based VirtIO with module-level interface
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
pub struct VirtioBlockWrapper {
    /// Block size for this wrapper (always 512 bytes for VirtIO)
    block_size: usize,
    /// Device index (only one device supported on ARM64 currently)
    device_index: usize,
}

#[cfg(target_arch = "aarch64")]
impl VirtioBlockWrapper {
    /// Create a new VirtIO block device wrapper
    ///
    /// # Arguments
    /// * `device_index` - Index of the VirtIO block device to wrap (only 0 supported)
    ///
    /// # Returns
    /// `Some(VirtioBlockWrapper)` if the device exists, `None` otherwise
    #[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
    pub fn new(device_index: usize) -> Option<Self> {
        // On ARM64, we only support device index 0 (the primary device)
        if device_index != 0 {
            return None;
        }

        // Check if the block device is initialized by trying to get capacity
        if block_driver::capacity().is_none() {
            return None;
        }

        Some(VirtioBlockWrapper {
            block_size: block_driver::SECTOR_SIZE,
            device_index,
        })
    }

    /// Get the primary (first) VirtIO block device
    #[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
    pub fn primary() -> Option<Self> {
        Self::new(0)
    }
}

#[cfg(target_arch = "aarch64")]
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

        // The MMIO driver expects a fixed-size buffer
        let mut sector_buf = [0u8; 512];
        block_driver::read_sector(block_num, &mut sector_buf)
            .map_err(|_| BlockError::IoError)?;
        buf[..512].copy_from_slice(&sector_buf);
        Ok(())
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

        // The MMIO driver expects a fixed-size buffer
        let mut sector_buf = [0u8; 512];
        sector_buf.copy_from_slice(&buf[..512]);
        block_driver::write_sector(block_num, &sector_buf)
            .map_err(|_| BlockError::IoError)
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn num_blocks(&self) -> u64 {
        // VirtIO reports capacity in sectors (512-byte blocks)
        block_driver::capacity().unwrap_or(0)
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

    #[cfg(target_arch = "x86_64")]
    log::info!("Block device subsystem initialized");
    #[cfg(target_arch = "aarch64")]
    crate::serial_println!("[block] Block device subsystem initialized");

    Ok(())
}

#[cfg(all(test, target_arch = "x86_64"))]
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
