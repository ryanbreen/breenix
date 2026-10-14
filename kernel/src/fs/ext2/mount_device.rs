//! The block device a mounted ext2 filesystem writes through.
//!
//! When an operation leaves the tree inconsistent and cannot undo it, the
//! mount stops writing: every later write fails, so nothing builds on the
//! damage, and reads continue. This is ext2's errors=remount-ro behaviour; the
//! superblock records the error so a checker examines the filesystem before
//! its next writable use.
//!
//! Every read and write of the mount passes through here, so this is where the
//! mount keeps its write-through copy of recently transferred blocks.

use super::sector_cache::SectorCache;
use crate::block::{BlockDevice, BlockError};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

pub struct MountDevice {
    inner: Box<dyn BlockDevice>,
    failed: Arc<AtomicBool>,
    cache: Option<SectorCache>,
}

impl MountDevice {
    /// Wrap `inner`; setting `failed` stops every later write. With `cached`,
    /// reads are served from a copy of recently transferred blocks when the
    /// memory for one is available.
    pub(super) fn new(inner: Box<dyn BlockDevice>, failed: Arc<AtomicBool>, cached: bool) -> Self {
        let cache = if cached { SectorCache::new(inner.block_size()) } else { None };
        Self { inner, failed, cache }
    }

    /// The device itself, past the copy: for checks of what reached the disk.
    pub(super) fn uncached(&self) -> &dyn BlockDevice {
        self.inner.as_ref()
    }

    fn write(&self, start_block: u64, buf: &[u8], write: impl FnOnce() -> Result<(), BlockError>) -> Result<(), BlockError> {
        if self.failed.load(Ordering::Acquire) {
            return Err(BlockError::IoError);
        }
        let Some(cache) = &self.cache else { return write() };
        let store = cache.begin_write(start_block, buf.len());
        let result = write();
        cache.end_write(store, start_block, result.is_ok().then_some(buf));
        result
    }
}

impl BlockDevice for MountDevice {
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let size = self.inner.block_size();
        if buf.len() < size {
            return Err(BlockError::IoError);
        }
        let Some(cache) = &self.cache else { return self.inner.read_block(block_num, buf) };
        let fill = match cache.lookup(block_num, &mut buf[..size]) {
            Ok(()) => return Ok(()),
            Err(fill) => fill,
        };
        self.inner.read_block(block_num, buf)?;
        cache.fill(fill, block_num, &buf[..size]);
        Ok(())
    }

    fn read_blocks(
        &self,
        start_block: u64,
        block_count: usize,
        buf: &mut [u8],
    ) -> Result<(), BlockError> {
        let bytes = block_count.checked_mul(self.inner.block_size()).ok_or(BlockError::IoError)?;
        if buf.len() < bytes {
            return Err(BlockError::IoError);
        }
        let Some(cache) = &self.cache else { return self.inner.read_blocks(start_block, block_count, buf) };
        let fill = match cache.lookup(start_block, &mut buf[..bytes]) {
            Ok(()) => return Ok(()),
            Err(fill) => fill,
        };
        self.inner.read_blocks(start_block, block_count, buf)?;
        cache.fill(fill, start_block, &buf[..bytes]);
        Ok(())
    }

    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError> {
        let size = self.inner.block_size();
        if buf.len() < size {
            return Err(BlockError::IoError);
        }
        self.write(block_num, &buf[..size], || self.inner.write_block(block_num, buf))
    }

    fn write_blocks(&self, start_block: u64, block_count: usize, buf: &[u8]) -> Result<(), BlockError> {
        let bytes = block_count.checked_mul(self.inner.block_size()).ok_or(BlockError::IoError)?;
        if buf.len() < bytes {
            return Err(BlockError::IoError);
        }
        self.write(start_block, &buf[..bytes], || self.inner.write_blocks(start_block, block_count, buf))
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
