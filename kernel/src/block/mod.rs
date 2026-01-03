//! Block Device Abstraction Layer
//!
//! Provides a generic interface for block devices, allowing filesystems to work
//! with different underlying storage implementations (VirtIO, AHCI, etc.) through
//! a common trait.

use core::fmt;

pub mod virtio;

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
