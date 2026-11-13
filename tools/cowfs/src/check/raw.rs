//! Little-endian fields, keys and block pointers as docs/fs/cowfs-format.md section 4 lays
//! them out. Callers check a buffer's length before reading a field from it.

use std::cmp::Ordering;
use std::fmt;

pub const BLOCK: usize = 4096;
pub const NODE: usize = 16384;
pub const NODE_BLOCKS: u64 = 4;
/// Blocks 0-15 are reserved and 16-19 hold ring A, so nodes and data start at block 20.
pub const FIRST_DATA_BLOCK: u64 = 20;

pub const TREE_FS: u8 = 1;
pub const TREE_SPACE: u8 = 2;

pub fn bytes<const N: usize>(b: &[u8], off: usize) -> [u8; N] {
    let mut a = [0u8; N];
    a.copy_from_slice(&b[off..off + N]);
    a
}

pub fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(bytes(b, off))
}

pub fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(bytes(b, off))
}

pub fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(bytes(b, off))
}

pub fn all_zero(b: &[u8]) -> bool {
    b.iter().all(|&x| x == 0)
}

/// Key (17 bytes): object u64, type u8, offset u64, ordered by (object, type, offset).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub object: u64,
    pub ty: u8,
    pub offset: u64,
}

impl Key {
    pub fn parse(b: &[u8], off: usize) -> Key {
        Key { object: u64_at(b, off), ty: b[off + 8], offset: u64_at(b, off + 9) }
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Key) -> Ordering {
        (self.object, self.ty, self.offset).cmp(&(other.object, other.ty, other.offset))
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Key) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({:#x}, {:#04x}, {:#x})", self.object, self.ty, self.offset)
    }
}

/// BlockPtr (24 bytes): addr u64, generation u64, csum u32, level u8, 3 pad bytes.
#[derive(Clone, Copy)]
pub struct BlockPtr {
    pub addr: u64,
    pub generation: u64,
    pub csum: u32,
    pub level: u8,
    pub pad_zero: bool,
}

impl BlockPtr {
    pub fn parse(b: &[u8], off: usize) -> BlockPtr {
        BlockPtr {
            addr: u64_at(b, off),
            generation: u64_at(b, off + 8),
            csum: u32_at(b, off + 16),
            level: b[off + 20],
            pad_zero: all_zero(&b[off + 21..off + 24]),
        }
    }
}

pub fn tree_name(tree: u8) -> &'static str {
    match tree {
        TREE_FS => "FS",
        TREE_SPACE => "SPACE",
        _ => "unknown",
    }
}
