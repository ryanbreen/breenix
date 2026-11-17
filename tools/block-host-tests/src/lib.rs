//! Host execution of the shared block cache, with a recording synchronous device.
extern crate alloc;

// Host fixture for the BlockDevice surface used by cache.rs, following ext2-host-tests.
pub mod block {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum BlockError {
        IoError,
        OutOfMemory,
        OutOfBounds,
    }

    pub trait BlockDevice: Send + Sync {
        fn identity(&self) -> usize {
            self as *const Self as *const () as usize
        }
        fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError>;
        /// Mirrors kernel/src/block/mod.rs: one read_block per block by default.
        fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
            let size = self.block_size();
            if buf.len() < count.saturating_mul(size) {
                return Err(BlockError::IoError);
            }
            for i in 0..count {
                self.read_block(start + i as u64, &mut buf[i * size..(i + 1) * size])?;
            }
            Ok(())
        }
        fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError>;
        /// Mirrors kernel/src/block/mod.rs: one write_block per block by default.
        fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
            let size = self.block_size();
            if buf.len() < count.checked_mul(size).ok_or(BlockError::IoError)? {
                return Err(BlockError::IoError);
            }
            for i in 0..count {
                self.write_block(start + i as u64, &buf[i * size..(i + 1) * size])?;
            }
            Ok(())
        }
        fn block_size(&self) -> usize;
        fn num_blocks(&self) -> u64;
        fn flush(&self) -> Result<(), BlockError>;
    }
}

pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}

#[path = "../../../kernel/src/block/cache.rs"]
pub mod buffer_cache;
pub use buffer_cache as cache;

#[cfg(test)]
#[path = "cache.rs"]
mod cache_tests;
#[cfg(test)]
mod recording;

#[path = "../../../kernel/src/block/cache_report.rs"]
pub mod cache_report;
#[cfg(test)]
mod report;

#[cfg(test)]
#[path = "../../../userspace/programs/src/cache_reread.rs"]
mod cache_reread;
