//! Block Device Abstraction Layer
//!
//! Provides a generic interface for block devices, allowing filesystems to work
//! with different underlying storage implementations (VirtIO, AHCI, etc.) through
//! a common trait.

use core::fmt;

#[cfg(target_arch = "x86_64")]
pub mod nvme;
pub mod virtio;

/// Enumerate usable x86 disks in backend order: VirtIO block devices, AHCI
/// SATA disks, then NVMe namespaces. Consumers identify disk contents,
/// rather than relying on PCI slots or a fixed index shared across transports.
#[cfg(target_arch = "x86_64")]
pub fn devices() -> alloc::vec::Vec<alloc::boxed::Box<dyn BlockDevice>> {
    let mut devices: alloc::vec::Vec<alloc::boxed::Box<dyn BlockDevice>> = alloc::vec::Vec::new();
    let mut index = 0;
    while let Some(device) = virtio::VirtioBlockWrapper::new(index) {
        devices.push(alloc::boxed::Box::new(device));
        index += 1;
    }
    for index in 0..crate::drivers::ahci::sata_device_count() {
        if let Some(device) = crate::drivers::ahci::get_block_device_by_index(index) {
            devices.push(alloc::boxed::Box::new(device));
        }
    }
    for index in 0..crate::drivers::nvme::controller_count() {
        if let Some(device) = nvme::NvmeBlockDevice::new(index) {
            devices.push(alloc::boxed::Box::new(device));
        }
    }
    devices
}

/// The x86-64 disk at `index` in `devices()` order.
///
/// The x86 launcher attaches every hardware profile's disks to one kind of
/// controller in a fixed order (0 = UEFI boot image, 1 = test binaries,
/// 2 = ext2 root, 3 = optional home disk), so the index names the same disk
/// whichever controller carries it. The root and test disks are found by
/// content through `devices()`; the home disk is an ext2 filesystem like the
/// root, so it is found by its place in that order.
#[cfg(target_arch = "x86_64")]
pub fn disk(index: usize) -> Option<alloc::boxed::Box<dyn BlockDevice>> {
    devices().into_iter().nth(index)
}

/// Generic block device interface
///
/// This trait provides a uniform interface for block-based storage devices.
/// Block sizes are device-specific (typically 512 bytes for raw sectors,
/// but filesystems may use 1024, 2048, or 4096 byte blocks).
#[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
pub trait BlockDevice: Send + Sync {
    /// Read a block into the provided buffer
    ///
    /// # Arguments
    /// * `block_num` - The block number to read (0-indexed)
    /// * `buf` - Buffer to read into (must be at least `block_size()` bytes)
    ///
    /// # Errors
    /// Returns `BlockError::OutOfBounds` if block_num >= num_blocks()
    /// Returns `BlockError::IoError` if the read operation fails
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Read consecutive blocks into the provided buffer.
    ///
    /// The default implementation preserves compatibility for simple block
    /// devices. Drivers with native multi-block commands should override this
    /// to avoid issuing one hardware command per sector.
    fn read_blocks(
        &self,
        start_block: u64,
        block_count: usize,
        buf: &mut [u8],
    ) -> Result<(), BlockError> {
        let block_size = self.block_size();
        if buf.len() < block_count.saturating_mul(block_size) {
            return Err(BlockError::IoError);
        }

        for i in 0..block_count {
            self.read_block(
                start_block + i as u64,
                &mut buf[i * block_size..(i + 1) * block_size],
            )?;
        }

        Ok(())
    }

    /// Write a block from the provided buffer
    ///
    /// # Arguments
    /// * `block_num` - The block number to write (0-indexed)
    /// * `buf` - Buffer to write from (must be at least `block_size()` bytes)
    ///
    /// # Errors
    /// Returns `BlockError::OutOfBounds` if block_num >= num_blocks()
    /// Returns `BlockError::IoError` if the write operation fails
    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError>;

    /// Get the block size in bytes
    ///
    /// This is the native block size for this device. For raw sector devices,
    /// this is typically 512 bytes. Filesystems may use larger block sizes
    /// (1024, 2048, 4096) and perform multiple sector reads/writes as needed.
    fn block_size(&self) -> usize;

    /// Get the total number of blocks on the device
    fn num_blocks(&self) -> u64;

    /// Flush any cached writes to persistent storage
    ///
    /// This ensures all pending writes are committed to the physical device.
    /// Implementations that don't cache writes may return Ok(()) immediately.
    fn flush(&self) -> Result<(), BlockError>;
}

/// Errors that can occur during block device operations
#[allow(dead_code)] // Part of public block device API, will be used by ext2 filesystem
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// I/O error occurred during operation
    IoError,
    /// Block number is out of bounds
    OutOfBounds,
    /// Device is not ready or not responding
    DeviceNotReady,
    /// Operation timed out
    Timeout,
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockError::IoError => write!(f, "I/O error"),
            BlockError::OutOfBounds => write!(f, "block number out of bounds"),
            BlockError::DeviceNotReady => write!(f, "device not ready"),
            BlockError::Timeout => write!(f, "operation timed out"),
        }
    }
}

impl From<&'static str> for BlockError {
    fn from(s: &'static str) -> Self {
        // Map common error strings from VirtIO driver to BlockError variants
        match s {
            "Sector out of range" | "Start sector out of range" => BlockError::OutOfBounds,
            "Read request timed out" | "Write request timed out" => BlockError::Timeout,
            "Block device not initialized" => BlockError::DeviceNotReady,
            _ => BlockError::IoError,
        }
    }
}
