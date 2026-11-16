#![no_std]

extern crate alloc;

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
#[path = "../../../kernel/src/fs/breenfs/space.rs"]
pub mod space;
#[path = "../../../kernel/src/fs/breenfs/txn.rs"]
pub mod txn;

/// Synchronous device surface used by the shared no_std writer.
pub mod block {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum BlockError {
        IoError,
        OutOfBounds,
    }
    pub trait BlockDevice: Send + Sync {
        fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError>;
        fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError>;
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
