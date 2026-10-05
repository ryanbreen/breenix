//! The block device a mounted ext2 filesystem writes through.
//!
//! When an operation leaves the tree inconsistent and cannot undo it, the
//! mount stops writing: every later write fails, so nothing builds on the
//! damage, and reads continue. This is ext2's errors=remount-ro behaviour; the
//! superblock records the error so a checker examines the filesystem before
//! its next writable use.

use crate::block::{BlockDevice, BlockError};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

pub(super) struct MountDevice {
    inner: Box<dyn BlockDevice>,
    failed: Arc<AtomicBool>,
}

impl MountDevice {
    /// Wrap `inner`; setting `failed` stops every later write.
    pub(super) fn new(inner: Box<dyn BlockDevice>, failed: Arc<AtomicBool>) -> Self {
        Self { inner, failed }
    }
}

impl BlockDevice for MountDevice {
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.inner.read_block(block_num, buf)
    }

    fn read_blocks(
        &self,
        start_block: u64,
        block_count: usize,
        buf: &mut [u8],
    ) -> Result<(), BlockError> {
        self.inner.read_blocks(start_block, block_count, buf)
    }

    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError> {
        if self.failed.load(Ordering::Acquire) {
            return Err(BlockError::IoError);
        }
        self.inner.write_block(block_num, buf)
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn num_blocks(&self) -> u64 {
        self.inner.num_blocks()
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.inner.flush()
    }
}
