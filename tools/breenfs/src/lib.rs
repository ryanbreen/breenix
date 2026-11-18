#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[path = "../../../libs/libbreenix/src/crypto/sha256.rs"]
pub mod sha256;

#[path = "../../../kernel/src/fs/breenfs/btree.rs"]
pub mod btree;

#[path = "../../../kernel/src/fs/breenfs/format.rs"]
pub mod format;

#[path = "../../../kernel/src/fs/breenfs/crc32c.rs"]
pub mod crc32c;
#[path = "../../../kernel/src/fs/breenfs/siphash.rs"]
pub mod siphash;

#[path = "../../../kernel/src/fs/breenfs/commit.rs"]
pub mod commit;
#[path = "../../../kernel/src/fs/breenfs/ops.rs"]
pub mod ops;
#[path = "../../../kernel/src/fs/breenfs/recover.rs"]
pub mod recover;
#[path = "../../../kernel/src/fs/breenfs/reserve.rs"]
pub mod reserve;
#[path = "../../../kernel/src/fs/breenfs/space.rs"]
pub mod space;
#[path = "../../../kernel/src/fs/breenfs/txn.rs"]
pub mod txn;

#[path = "../../../kernel/src/block/read_failure.rs"]
pub mod read_failure;

/// Synchronous device surface used by the shared no_std writer.
pub mod block {
    pub use crate::read_failure;
    use read_failure::VirtioReadFailure;
    pub use crate::buffer_cache as cache;
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BlockError {
        IoError,
        VirtioRead(VirtioReadFailure),
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

#[cfg(test)]
mod tests {
    mod crc32c {
        #[test]
        fn rfc3720_vector() {
            use crate::crc32c::{checksum, Crc32c};
            assert_eq!(checksum(b"123456789"), 0xe306_9283);
            assert_eq!(checksum(&[0; 32]), 0x8a91_36aa);
            assert_eq!(checksum(&[0xff; 32]), 0x62a8_ab43);
            let mut crc = Crc32c::new();
            crc.update(b"1234");
            crc.update(b"");
            crc.update(b"56789");
            assert_eq!(crc.finish(), 0xe306_9283);
        }
    }

    mod siphash {
        #[test]
        fn reference_vector() {
            let key = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
            let message = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];
            assert_eq!(crate::siphash::hash(&key, &message), 0xa129_ca61_49be_45e5);
        }
    }
}

#[path = "../../../kernel/src/block/cache.rs"]
pub mod buffer_cache;
pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}

#[path = "../../../kernel/src/fs/breenfs/preempt.rs"]
pub mod preempt;
