//! A node (format section 2.2) read from the image and compared with the pointer that names
//! it: magic, CRC, own address, generation, level, fs_uuid and tree.

use std::io;

use super::image::Image;
use super::raw::{tree_name, u16_at, u32_at, u64_at, Key, FIRST_DATA_BLOCK, NODE, NODE_BLOCKS};

const MAGIC: &[u8; 8] = b"CoWFS-ND";
pub const HEADER: usize = 48;

/// What a pointer says about the node it names. A DEAD item names only an address and tree.
pub struct Expect<'a> {
    pub addr: u64,
    pub tree: u8,
    pub generation: Option<u64>,
    pub csum: Option<u32>,
    /// The expected level, and the invariant a mismatch breaks: N1 for a BlockPtr's level,
    /// N3 for an internal entry's child (whose level is the parent's minus one).
    pub level: Option<(u8, &'static str)>,
    pub uuid: &'a [u8; 16],
}

pub struct Node {
    pub raw: Vec<u8>,
    pub tree: u8,
    pub level: u8,
    pub nitems: usize,
    pub generation: u64,
    pub csum: u32,
}

/// One way a node fails its pointer, with the invariant it breaks. `fatal` means the node's
/// bytes cannot be trusted (or are another node's), so its contents are not read.
pub struct Mismatch {
    pub id: &'static str,
    pub detail: String,
    pub fatal: bool,
}

/// The node area is blocks 20 to total-5; a node is four consecutive blocks inside it.
pub fn in_node_area(addr: u64, total_blocks: u64) -> bool {
    addr >= FIRST_DATA_BLOCK && addr.checked_add(NODE_BLOCKS).is_some_and(|end| end <= total_blocks - 4)
}

pub fn read(image: &Image, addr: u64) -> io::Result<Node> {
    let raw = image.read(addr, NODE_BLOCKS)?;
    Ok(Node {
        tree: raw[12],
        level: raw[13],
        nitems: u16_at(&raw, 14) as usize,
        generation: u64_at(&raw, 40),
        csum: u32_at(&raw, 8),
        raw,
    })
}

pub fn compare(node: &Node, expect: &Expect<'_>) -> Vec<Mismatch> {
    let mut out = Vec::new();
    let mut push = |id, detail: String, fatal| out.push(Mismatch { id, detail, fatal });
    if &node.raw[0..8] != MAGIC {
        push("N1", "bad node magic".into(), true);
        return out;
    }
    let computed = cowfs::crc32c::checksum(&node.raw[12..NODE]);
    if node.csum != computed {
        push("N1", format!("node CRC {:#010x}, computed {computed:#010x}", node.csum), true);
    }
    let own = u64_at(&node.raw, 32);
    if own != expect.addr {
        push("N1", format!("node records address {own}"), true);
    }
    if &node.raw[16..32] != expect.uuid {
        push("N1", "node fs_uuid differs from the superblock's".into(), true);
    }
    if node.tree != expect.tree {
        push("N1", format!("{} tree node where a {} node belongs", tree_name(node.tree), tree_name(expect.tree)), true);
    }
    if let Some(generation) = expect.generation {
        if node.generation != generation {
            push("N1", format!("node generation {}, pointer says {generation}", node.generation), false);
        }
    }
    if let Some(csum) = expect.csum {
        if node.csum != csum {
            push("N1", format!("node CRC {:#010x}, pointer says {csum:#010x}", node.csum), false);
        }
    }
    if let Some((level, id)) = expect.level {
        if node.level != level {
            push(id, format!("node level {}, expected {level}", node.level), false);
        }
    }
    out
}

pub const LEAF_ITEM: usize = 21;
pub const INTERNAL_ENTRY: usize = 37;
pub const MAX_INTERNAL_ENTRIES: usize = 441;

pub struct LeafItem {
    pub key: Key,
    pub data_off: usize,
    pub data_len: usize,
}

impl LeafItem {
    /// The payload, when it lies inside the node.
    pub fn payload<'n>(&self, node: &'n Node) -> Option<&'n [u8]> {
        node.raw.get(self.data_off..self.data_off.checked_add(self.data_len)?)
    }
}

pub struct Entry {
    pub key: Key,
    pub child: u64,
    pub generation: u64,
    pub csum: u32,
}

/// The item headers that fit in the node, at most `nitems` of them.
pub fn leaf_items(node: &Node) -> Vec<LeafItem> {
    let fit = (NODE - HEADER) / LEAF_ITEM;
    (0..node.nitems.min(fit))
        .map(|i| {
            let h = HEADER + LEAF_ITEM * i;
            LeafItem {
                key: Key::parse(&node.raw, h),
                data_off: u16_at(&node.raw, h + 17) as usize,
                data_len: u16_at(&node.raw, h + 19) as usize,
            }
        })
        .collect()
}

/// The internal entries, at most `nitems` and at most 441 of them.
pub fn entries(node: &Node) -> Vec<Entry> {
    (0..node.nitems.min(MAX_INTERNAL_ENTRIES))
        .map(|i| {
            let e = HEADER + INTERNAL_ENTRY * i;
            Entry {
                key: Key::parse(&node.raw, e),
                child: u64_at(&node.raw, e + 17),
                generation: u64_at(&node.raw, e + 25),
                csum: u32_at(&node.raw, e + 33),
            }
        })
        .collect()
}
