//! Eight-slot recovery and abandoned publication cleanup (design 2.5).
use super::preempt::preemption_point;
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

/// Small failure receipt, kept out of the on-disk and syscall error formats.
pub(crate) struct Failure {
    pub(crate) error: Error,
    pub(crate) stage: &'static str,
    pub(crate) block: Option<u64>,
    pub(crate) expected: &'static str,
    pub(crate) uuids: Option<([u8; 16], [u8; 16])>,
}

impl Failure {
    pub(crate) fn new(error: Error, stage: &'static str, block: impl Into<Option<u64>>, expected: &'static str) -> Self {
        Self { error, stage, block: block.into(), expected, uuids: None }
    }
}

impl core::fmt::Display for Failure {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(out, "stage={} block={:?}", self.stage, self.block)?;
        if let Some((expected, found)) = self.uuids {
            write!(out, " expected_uuid={:032x} found_uuid={:032x}",
                u128::from_be_bytes(expected), u128::from_be_bytes(found))
        } else {
            write!(out, " expected={} found={:?}", self.expected, self.error)
        }
    }
}

pub fn recover(device: &dyn BlockDevice, writable: bool) -> Result<Superblock, Error> {
    recover_detailed(device, writable).map_err(|failure| failure.error)
}

pub(crate) fn recover_detailed(device: &dyn BlockDevice, writable: bool) -> Result<Superblock, Failure> {
    let total = commit::partition_blocks(device)
        .map_err(|e| Failure::new(e, "geometry", None, "supported-sector-size-and-length"))?;
    let mut slots = Vec::new();
    let mut invalid = Vec::new();
    let mut uuid = None;
    for ring in 0..2 {
        for slot in 0..4 {
            let block = commit::slot_block(total, ring, slot);
            let bytes = commit::read(device, block, BLOCK_SIZE)
                .map_err(|e| Failure::new(e, "superblock-read", block, "readable-slot"))?;
            if !format::valid_superblock_slot(&bytes, total, ring, slot) {
                if bytes.iter().any(|&b| b != 0) {
                    invalid.push((block, slot));
                }
                continue;
            }
            let sb = Superblock::decode(&bytes)
                .map_err(|e| Failure::new(e.into(), "superblock-decode", block, "valid-slot"))?;
            if let Some(expected) = uuid.filter(|&u| u != sb.fs_uuid) {
                let mut failure = Failure::new(Error::UuidMismatch, "superblock-uuid", block, "matching-uuid");
                failure.uuids = Some((expected, sb.fs_uuid));
                return Err(failure);
            }
            uuid = Some(sb.fs_uuid);
            slots.push((block, sb));
        }
    }
    slots.sort_by(|a, b| b.1.generation.cmp(&a.1.generation));
    let mut chosen = None;
    let mut rejected = invalid.first().map(|&(block, _)| block);
    for (block, sb) in &slots {
        match space_items(&DeviceStore(device), sb) {
            Ok(_) => {
                chosen = Some((*block, sb.clone()));
                break;
            }
            Err(Error::InvalidGeneration) => rejected = Some(sb.space_root.addr),
            Err(e) => return Err(Failure::new(e, "space-tree", sb.space_root.addr, "readable-roots")),
        }
    }
    let (chosen_block, mut chosen) = chosen.ok_or_else(|| Failure::new(Error::NoGeneration, "generation", rejected, "valid-generation"))?;
    if writable {
        if chosen.ro_compat != 0 {
            return Err(Failure::new(Error::ReadOnly, "writable-features", chosen_block, "writable-features"));
        }
        let mut changed = false;
        let write = |block, bytes: &[u8]| commit::write(device, block, bytes)
            .map_err(|e| Failure::new(e, "superblock-repair", block, "writable-slot"));
        // Torn publication can leave garbage in an otherwise unused slot.
        // Once M is validated, discard these unusable copies too. M's slot
        // is restored directly below, without an intervening erasure.
        for (block, slot) in invalid {
            if slot != chosen.slot {
                write(block, &[0; BLOCK_SIZE])?;
                changed = true;
            }
        }
        for (block, sb) in &slots {
            if sb.generation > chosen.generation {
                write(*block, &[0; BLOCK_SIZE])?;
                changed = true;
            }
        }
        // A crash during publication can leave M in just one ring. Restore its
        // companion before orphan reclaim or the caller can publish a commit.
        for ring in 0..2 {
            let mut copy = chosen.clone();
            copy.ring = ring;
            if !slots.iter().any(|(_, sb)| *sb == copy) {
                write(commit::slot_block(total, ring, chosen.slot), &copy.encode())?;
                changed = true;
            }
        }
        if changed {
            device.flush().map_err(|_| Failure::new(Error::EIO, "superblock-flush", None, "durable-repair"))?;
        }
        // Resume committed orphan work before exposing a writable mount.
        let space = chosen.space_root.addr;
        let roots: Vec<_> = space_items(&DeviceStore(device), &chosen)
            .map_err(|e| Failure::new(e, "orphan-roots", space, "readable-roots"))?
            .into_iter()
            .filter(|(k, _)| k.item_type == format::ROOT)
            .map(|(k, v)| Root::decode(&v).map(|root| (k.offset, root)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| Failure::new(e.into(), "orphan-roots", space, "valid-root-item"))?;
        for (id, root) in roots {
            if root.flags & 1 != 0 { continue; }
            // Descendants outside the ORPHAN range are verified on demand.
            // Do not turn mount recovery into a whole-filesystem scrub.
            let orphan_failure = |stage, expected| Failure::new(Error::EIO, stage, root.root.addr, expected);
            if !btree::range(
                &DeviceStore(device), root.root,
                super::ops::key(super::ops::ORPHAN_OBJECT, format::ORPHAN, 0),
                super::ops::key(super::ops::ORPHAN_OBJECT, format::ORPHAN, u64::MAX),
            ).map_err(|_| orphan_failure("orphan-scan", "readable-orphan-range"))?.is_empty()
            {
                let mut files = super::ops::Files::new(DeviceStore(device), &chosen, id, true)
                    .map_err(|_| orphan_failure("orphan-open", "valid-files-view"))?;
                let mut committer = commit::Committer::new(chosen);
                chosen = files
                    .fsync(device, &mut committer)
                    .map_err(|_| orphan_failure("orphan-commit", "durable-reclaim"))?;
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
        preemption_point();
        if !seen.insert(ptr.addr) {
            return Err(Error::InvalidGeneration);
        }
        let (h, bytes) = node(store, sb, ptr, 2, limit)?;
        for i in 0..usize::from(h.nitems) {
            preemption_point();
            if h.level > 0 {
                let at = 48 + i * 37;
                let e = InternalEntry::decode(&bytes[at..at + 37])
                    .map_err(|_| Error::InvalidGeneration)?;
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
                let item = LeafItemHeader::decode(&bytes[at..at + 21])
                    .map_err(|_| Error::InvalidGeneration)?;
                // Point lookup validates payload layout, key order and SPACE
                // ancestor bounds through the existing B-tree reader.
                let value = btree::search(store, sb.space_root, item.key)
                    .map_err(tree_error)?
                    .ok_or(Error::InvalidGeneration)?;
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
