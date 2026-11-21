//! Implementation-defined root contracts (E24 coordinator rulings 458 and 465).
use crate::filesystem_root::RootFs;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CookieEncoding {
    ByteOffsets,
    DirectoryIndexes,
}

#[derive(Clone, Copy)]
pub struct Expectations {
    pub reuses_inode_numbers: bool,
    pub cache_block_bytes: u64,
    pub statfs_magic: u64,
    pub cookie_encoding: CookieEncoding,
}

pub fn for_root(root: RootFs) -> Expectations {
    match root {
        RootFs::Ext2 => Expectations {
            reuses_inode_numbers: true,
            cache_block_bytes: 512,
            statfs_magic: 0xef53,
            cookie_encoding: CookieEncoding::ByteOffsets,
        },
        RootFs::Breenfs => Expectations {
            // docs/fs/breenfs-design.md 2.3 and 2.9: monotonic inode IDs.
            reuses_inode_numbers: false,
            // Design 1.2 and 2: verified native buffers, 4096 bytes per block.
            cache_block_bytes: 4096,
            // Design 2.1: superblock format identity.
            statfs_magic: u64::from_le_bytes(*b"BREENFSS"),
            // Design 2.3 DIR_INDEX; ledger 465 specifies the VFS encoding.
            cookie_encoding: CookieEncoding::DirectoryIndexes,
        },
    }
}

impl Expectations {
    pub fn directory_size(self, ext2_bytes: u64, entries: u64) -> u64 {
        // docs/fs/breenfs-design.md 2.3: size counts stored names, without dots.
        if self.reuses_inode_numbers { ext2_bytes } else { entries }
    }

    pub fn blocks(self, data: u64, pointers: u64, sectors_per_block: u64) -> u64 {
        // Design 2.3 and invariant F7: nblocks sums mapped extent blocks.
        (data + if self.reuses_inode_numbers { pointers } else { 0 }) * sectors_per_block
    }

    pub fn cookies_match(self, cookies: &[u64], block: u64, entries: u64) -> bool {
        match self.cookie_encoding {
            CookieEncoding::ByteOffsets => cookies.last().is_some_and(|last|
                cookies.iter().any(|cookie| *cookie > block && cookie < last)),
            CookieEncoding::DirectoryIndexes => {
                // Design 2.3: indexes start at 2; returned cookie is index+1.
                cookies.len() as u64 == entries + 2
                    && cookies.starts_with(&[1, 2])
                    && cookies.windows(2).all(|pair| pair[0] < pair[1])
                    && cookies[2..].iter().all(|cookie| (3..=entries + 2).contains(cookie))
            }
        }
    }

    pub fn available_blocks(self, free: u64, total: u64, outstanding: u64) -> Option<u64> {
        if self.reuses_inode_numbers { return Some(free); }
        // Design 2.8: total/200, clamped to 16 MiB..1 GiB, in 4096-byte blocks.
        let reserve = (total / 200).clamp(4096, 262144);
        free.checked_sub(reserve)?.checked_sub(outstanding)
    }

    pub fn inode_capacity(self, files: u64, free: u64) -> bool {
        // Design 2.3/2.9 has monotonic IDs, with no fixed inode pool (ledger 465).
        if self.reuses_inode_numbers { files > 0 && free <= files }
        else { files == 0 && free == 0 }
    }
}
