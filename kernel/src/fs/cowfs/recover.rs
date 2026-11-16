//! Eight-slot recovery and abandoned publication cleanup (design 2.5).
use super::{
    btree::{self, NodeStore},
    commit::{self, DeviceStore, Error},
    format::{
        self, BlockPtr, InternalEntry, Key, LeafItemHeader, NodeHeader, Root, Superblock,
        BLOCK_SIZE, NODE_SIZE,
    },
};
use crate::block::BlockDevice;
use alloc::{collections::BTreeSet, vec::Vec};

pub fn recover(device: &dyn BlockDevice, writable: bool) -> Result<Superblock, Error> {
    let total = commit::partition_blocks(device)?;
    let mut slots = Vec::new();
    let mut uuid = None;
    for ring in 0..2 {
        for slot in 0..4 {
            let block = commit::slot_block(total, ring, slot);
            let bytes = commit::read(device, block, BLOCK_SIZE)?;
            if !format::valid_superblock_slot(&bytes, total, ring, slot) {
                continue;
            }
            let sb = Superblock::decode(&bytes)?;
            if uuid.is_some_and(|u| u != sb.fs_uuid) {
                return Err(Error::UuidMismatch);
            }
            uuid = Some(sb.fs_uuid);
            slots.push((block, sb));
        }
    }
    slots.sort_by(|a, b| b.1.generation.cmp(&a.1.generation));
    let mut chosen = None;
    for (_, sb) in &slots {
        match space_items(&DeviceStore(device), sb) {
            Ok(_) => {
                chosen = Some(sb.clone());
                break;
            }
            Err(Error::InvalidGeneration) => continue,
            Err(e) => return Err(e),
        }
    }
    let mut chosen = chosen.ok_or(Error::NoGeneration)?;
    if writable {
        if chosen.ro_compat != 0 {
            return Err(Error::ReadOnly);
        }
        let mut cleared = false;
        for (block, sb) in &slots {
            if sb.generation > chosen.generation {
                commit::write(device, *block, &[0; BLOCK_SIZE])?;
                cleared = true;
            }
        }
        if cleared {
            device.flush().map_err(|_| Error::EIO)?;
        }
        // Resume committed orphan work before exposing a writable mount.
        let roots: Vec<_> = space_items(&DeviceStore(device), &chosen)?
            .into_iter()
            .filter(|(k, _)| k.item_type == format::ROOT)
            .map(|(k, v)| Ok((k.offset, Root::decode(&v)?)))
            .collect::<Result<Vec<_>, Error>>()?;
        for (id, root) in roots {
            if super::ops::items(&DeviceStore(device), root.root)
                .map_err(|_| Error::EIO)?
                .iter()
                .any(|(k, _)| k.item_type == format::ORPHAN)
            {
                let mut files = super::ops::Files::new(DeviceStore(device), &chosen, id)
                    .map_err(|_| Error::EIO)?;
                let mut committer = commit::Committer::new(chosen);
                chosen = files
                    .fsync(device, &mut committer)
                    .map_err(|_| Error::EIO)?;
            }
        }
    }
    Ok(chosen)
}

fn node<S: NodeStore>(
    store: &S,
    sb: &Superblock,
    ptr: BlockPtr,
    tree: u8,
    limit: u64,
) -> Result<(NodeHeader, Vec<u8>), Error> {
    if ptr.level >= 8
        || ptr.generation == 0
        || ptr.generation > limit
        || ptr.addr < 20
        || ptr
            .addr
            .checked_add(4)
            .is_none_or(|n| n > sb.total_blocks - 4)
    {
        return Err(Error::InvalidGeneration);
    }
    let bytes = store.get(ptr.addr).map_err(tree_error)?;
    if bytes.len() != NODE_SIZE {
        return Err(Error::InvalidGeneration);
    }
    let h = NodeHeader::decode(&bytes).map_err(|_| Error::InvalidGeneration)?;
    if (h.addr, h.generation, h.level, h.tree, h.fs_uuid)
        != (ptr.addr, ptr.generation, ptr.level, tree, sb.fs_uuid)
        || ptr.csum != u32::from_le_bytes(bytes[8..12].try_into().unwrap())
    {
        return Err(Error::InvalidGeneration);
    }
    let width = if h.level == 0 { 21 } else { 37 };
    if 48 + usize::from(h.nitems) * width > NODE_SIZE
        || (h.level > 0 && (h.nitems == 0 || h.nitems > 441))
    {
        return Err(Error::InvalidGeneration);
    }
    Ok((h, bytes))
}
/// Walk SPACE to find ROOT pointers. This is mount recovery, not a full scrub:
/// ROOT tree root blocks are checked here; FS descendants are checked on read.
pub(crate) fn space_items<S: NodeStore>(
    store: &S,
    sb: &Superblock,
) -> Result<Vec<(Key, Vec<u8>)>, Error> {
    let mut todo = alloc::vec![(sb.space_root, sb.generation)];
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    while let Some((ptr, limit)) = todo.pop() {
        if !seen.insert(ptr.addr) {
            return Err(Error::InvalidGeneration);
        }
        let (h, bytes) = node(store, sb, ptr, 2, limit)?;
        for i in 0..usize::from(h.nitems) {
            if h.level > 0 {
                let at = 48 + i * 37;
                let e = InternalEntry::decode(&bytes[at..at + 37]).map_err(|_| Error::InvalidGeneration)?;
                todo.push((
                    BlockPtr {
                        addr: e.child_addr,
                        generation: e.child_generation,
                        csum: e.child_csum,
                        level: h.level - 1,
                    },
                    h.generation,
                ));
            } else {
                let at = 48 + i * 21;
                let item = LeafItemHeader::decode(&bytes[at..at + 21]).map_err(|_| Error::InvalidGeneration)?;
                // Point lookup validates payload layout, key order and SPACE
                // ancestor bounds through the existing B-tree reader.
                let value = btree::search(store, sb.space_root, item.key).map_err(tree_error)?.ok_or(Error::InvalidGeneration)?;
                if item.key.item_type == format::ROOT {
                    let r = Root::decode(&value).map_err(|_| Error::InvalidGeneration)?;
                    node(store, sb, r.root, 1, sb.generation)?;
                }
                out.push((item.key, value));
            }
        }
    }
    Ok(out)
}

fn tree_error(error: btree::Error) -> Error {
    match error {
        btree::Error::Store => Error::EIO,
        _ => Error::InvalidGeneration,
    }
}
