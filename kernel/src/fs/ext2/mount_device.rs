//! The block device a mounted ext2 filesystem writes through.
//!
//! When an operation leaves the tree inconsistent and cannot undo it, the
//! mount stops writing: every later write fails, so nothing builds on the
//! damage, and reads continue. This is ext2's errors=remount-ro behaviour; the
//! superblock records the error so a checker examines the filesystem before
//! its next writable use.
//!
//! This adapter joins the system cache as a WriteThrough group.

use crate::block::{
    cache::WriteGroup,
    cache_kernel::{self, SystemCache},
    BlockDevice, BlockError,
};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

pub struct MountDevice {
    inner: Box<dyn BlockDevice>,
    failed: Arc<AtomicBool>,
    read_only: AtomicBool,
    cache: Option<&'static SystemCache>,
}

impl MountDevice {
    /// Fault-injection mounts pass cached=false so device faults remain visible.
    pub(super) fn new(inner: Box<dyn BlockDevice>, failed: Arc<AtomicBool>, cached: bool) -> Self {
        let cache = cached.then(cache_kernel::system_cache);
        let read_only = inner.is_read_only();
        Self {
            inner,
            failed,
            read_only: AtomicBool::new(read_only),
            cache,
        }
    }

    /// The device itself, past the copy: for checks of what reached the disk.
    pub(super) fn uncached(&self) -> &dyn BlockDevice {
        cache_kernel::assert_io_context();
        self.inner.as_ref()
    }

    pub(super) fn remount_read_only(&self) {
        self.read_only.store(true, Ordering::Release);
    }

    fn writable(&self) -> Result<(), BlockError> {
        if self.is_read_only() || self.failed.load(Ordering::Acquire) {
            return Err(BlockError::IoError);
        }
        Ok(())
    }

    fn span(&self, start: u64, count: usize, len: usize) -> Result<usize, BlockError> {
        let size = self.inner.block_size();
        let bytes = count.checked_mul(size).ok_or(BlockError::IoError)?;
        if size == 0 || len < bytes {
            return Err(BlockError::IoError);
        }
        if start
            .checked_add(count as u64)
            .ok_or(BlockError::OutOfBounds)?
            > self.inner.num_blocks()
        {
            return Err(BlockError::OutOfBounds);
        }
        Ok(bytes)
    }
}

impl Drop for MountDevice {
    fn drop(&mut self) {
        if let Some(cache) = self.cache {
            cache.discard_clean(self.inner.as_ref());
        }
    }
}

impl BlockDevice for MountDevice {
    fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::Acquire) || self.inner.is_read_only()
    }
    fn identity(&self) -> usize {
        self.inner.identity()
    }
    fn hardware_disk(&self) -> Option<crate::block::HardwareDisk> {
        self.inner.hardware_disk()
    }

    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        cache_kernel::assert_io_context();
        self.span(block, 1, buf.len())?;
        match self.cache {
            Some(cache) => cache.read(self.inner.as_ref(), block, buf),
            None => self.inner.read_block(block, buf),
        }
    }

    fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
        cache_kernel::assert_io_context();
        self.span(start, count, buf.len())?;
        match self.cache {
            Some(cache) if count != 0 => cache.read_blocks(self.inner.as_ref(), start, count, buf),
            Some(_) => Ok(()),
            None => self.inner.read_blocks(start, count, buf),
        }
    }

    fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.writable()?;
        cache_kernel::assert_io_context();
        self.span(block, 1, buf.len())?;
        match self.cache {
            Some(cache) => cache.write(self.inner.as_ref(), WriteGroup::WriteThrough, block, buf),
            None => self.inner.write_block(block, buf),
        }
    }

    fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
        self.writable()?;
        cache_kernel::assert_io_context();
        let bytes = self.span(start, count, buf.len())?;
        if self.cache.is_none() {
            return self.inner.write_blocks(start, count, buf);
        }
        let size = self.inner.block_size();
        for (i, chunk) in buf[..bytes].chunks_exact(size).enumerate() {
            self.write_block(start + i as u64, chunk)?;
        }
        Ok(())
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }
    fn num_blocks(&self) -> u64 {
        self.inner.num_blocks()
    }

    fn flush(&self) -> Result<(), BlockError> {
        cache_kernel::assert_io_context();
        match self.cache {
            Some(cache) => cache.flush(self.inner.as_ref()),
            None => self.inner.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct ReadCalls {
        single: AtomicUsize,
        multiple: AtomicUsize,
    }

    struct CountingDisk(Arc<ReadCalls>);

    impl BlockDevice for CountingDisk {
        fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            self.0.single.fetch_add(1, Ordering::Relaxed);
            buf.fill(block as u8);
            Ok(())
        }

        fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
            self.0.multiple.fetch_add(1, Ordering::Relaxed);
            for (i, block) in buf.chunks_exact_mut(self.block_size()).take(count).enumerate() {
                block.fill((start + i as u64) as u8);
            }
            Ok(())
        }

        fn write_block(&self, _block: u64, _buf: &[u8]) -> Result<(), BlockError> {
            Err(BlockError::IoError)
        }

        fn block_size(&self) -> usize { 512 }
        fn num_blocks(&self) -> u64 { 32 }
        fn flush(&self) -> Result<(), BlockError> { Ok(()) }
    }

    #[test]
    fn cached_mount_keeps_an_ext2_block_read_contiguous() {
        let calls = Arc::new(ReadCalls::default());
        let disk = MountDevice::new(Box::new(CountingDisk(calls.clone())), Arc::new(AtomicBool::new(false)), true);
        let mut bytes = [0; 4096];
        super::super::file::read_ext2_block(&disk, 1, bytes.len(), &mut bytes).unwrap();
        for (i, sector) in bytes.chunks_exact(512).enumerate() {
            assert!(sector.iter().all(|&b| b == (i + 8) as u8));
        }
        assert_eq!(calls.single.load(Ordering::Relaxed), 0);
        assert_eq!(calls.multiple.load(Ordering::Relaxed), 1);
        let mut cached = [0; 4096];
        super::super::file::read_ext2_block(&disk, 1, cached.len(), &mut cached).unwrap();
        assert_eq!(cached, bytes);
        assert_eq!(calls.single.load(Ordering::Relaxed), 0);
        assert_eq!(calls.multiple.load(Ordering::Relaxed), 1);
    }

    struct ReadOnlyDisk(Arc<AtomicUsize>);
    impl BlockDevice for ReadOnlyDisk {
        fn is_read_only(&self) -> bool { true }
        fn read_block(&self, _block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            buf.fill(0x5a);
            Ok(())
        }
        fn write_block(&self, _block: u64, _buf: &[u8]) -> Result<(), BlockError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn block_size(&self) -> usize { 512 }
        fn num_blocks(&self) -> u64 { 4 }
        fn flush(&self) -> Result<(), BlockError> { Ok(()) }
    }

    #[test]
    fn read_only_mount_refuses_writes_before_backend_or_cache_changes() {
        let writes = Arc::new(AtomicUsize::new(0));
        let disk = MountDevice::new(Box::new(ReadOnlyDisk(writes.clone())), Arc::new(AtomicBool::new(false)), true);
        assert!(disk.is_read_only());
        let mut bytes = [0; 512];
        disk.read_block(0, &mut bytes).unwrap();
        assert_eq!(disk.write_block(0, &[1; 512]), Err(BlockError::IoError));
        assert_eq!(disk.write_blocks(0, 2, &[2; 1024]), Err(BlockError::IoError));
        assert_eq!(writes.load(Ordering::Relaxed), 0);
        disk.uncached().read_block(0, &mut bytes).unwrap();
        assert_eq!(bytes, [0x5a; 512]);
        disk.read_block(0, &mut bytes).unwrap();
        assert_eq!(bytes, [0x5a; 512]);
    }
}
