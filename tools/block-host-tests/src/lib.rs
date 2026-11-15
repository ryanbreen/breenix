//! Host execution of the shared block cache, with a recording synchronous device.
extern crate alloc;

// Host fixture for the BlockDevice surface used by cache.rs, following ext2-host-tests.
pub mod block {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum BlockError {
        IoError,
        OutOfBounds,
    }

    pub trait BlockDevice: Send + Sync {
        fn identity(&self) -> usize {
            self as *const Self as *const () as usize
        }
        fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError>;
        fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError>;
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

#[cfg(test)]
mod cache;
#[cfg(test)]
mod recording;
