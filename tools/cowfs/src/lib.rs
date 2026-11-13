#![no_std]

extern crate alloc;

#[path = "../../../kernel/src/fs/cowfs/btree.rs"]
pub mod btree;

#[path = "../../../kernel/src/fs/cowfs/format.rs"]
pub mod format;

#[path = "../../../kernel/src/fs/cowfs/crc32c.rs"]
pub mod crc32c;
#[path = "../../../kernel/src/fs/cowfs/siphash.rs"]
pub mod siphash;

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
