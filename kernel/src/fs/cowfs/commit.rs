//! Synchronous transaction publication, cowfs design section 2.5.
//!
//! The caller serializes a mount's commits. This core writes directly to the
//! device; the kernel adapter supplies scheduling and Hold groups in step 24.11.
use super::{
    btree::{self, NodeStore},
    crc32c,
    format::{
        self, BlockPtr, Free, InternalEntry, LeafItemHeader, NodeHeader, Ref, Root, Superblock,
        BLOCK_SIZE, NODE_SIZE,
    },
    txn::Transaction,
};
use crate::block::BlockDevice;
use alloc::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec,
    vec::Vec,
};
use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    EIO,
    UuidMismatch,
    NoGeneration,
    ReadOnly,
}
impl From<format::Error> for Error {
    fn from(_: format::Error) -> Self {
        Self::EIO
    }
}
impl From<btree::Error> for Error {
    fn from(_: btree::Error) -> Self {
        Self::EIO
    }
}

/// Completion receipt for fsync callers joining the current generation.
/// The adapter may park a thread while completion is pending.
pub struct FsyncWaiter(AtomicU8);
impl FsyncWaiter {
    pub fn result(&self) -> Option<Result<(), Error>> {
        match self.0.load(Ordering::Acquire) {
            0 => None,
            1 => Some(Ok(())),
            _ => Some(Err(Error::EIO)),
        }
    }
}

pub struct Committer {
    durable: Superblock,
    failed: bool,
    waiters: Vec<Arc<FsyncWaiter>>,
}
impl Committer {
    /// Publish generation 1 from an unpublished formatter template. Generation
    /// 0 is an in-memory sentinel; publication increments it before encoding.
    pub fn format(mut template: Superblock) -> Self {
        template.generation = 0;
        Self::new(template)
    }
    /// Construct only after writable recovery has cleared abandoned slots.
    pub fn new(durable: Superblock) -> Self {
        Self {
            durable,
            failed: false,
            waiters: Vec::new(),
        }
    }
    pub fn read_only(&self) -> bool {
        self.failed || self.durable.ro_compat != 0
    }
    pub fn waiter(&mut self) -> Arc<FsyncWaiter> {
        let w = Arc::new(FsyncWaiter(AtomicU8::new(if self.read_only() {
            2
        } else {
            0
        })));
        if !self.read_only() {
            self.waiters.push(w.clone());
        }
        w
    }
    /// `data` contains private extents, keyed by their starting cowfs block.
    /// The transaction owns their allocated REF records and per-block CRCs.
    pub fn commit<S: NodeStore>(
        &mut self,
        device: &dyn BlockDevice,
        txn: &mut Transaction<'_, S>,
        data: &BTreeMap<u64, Vec<u8>>,
    ) -> Result<Superblock, Error> {
        if self.failed {
            return Err(Error::EIO);
        }
        if self.read_only() {
            return Err(Error::ReadOnly);
        }
        let result = self.publish(device, txn, data);
        match &result {
            Ok(sb) => self.durable = sb.clone(),
            Err(_) => self.failed = true,
        }
        let status = if result.is_ok() { 1 } else { 2 };
        for w in self.waiters.drain(..) {
            w.0.store(status, Ordering::Release);
        }
        result
    }
    fn publish<S: NodeStore>(
        &self,
        device: &dyn BlockDevice,
        txn: &mut Transaction<'_, S>,
        data: &BTreeMap<u64, Vec<u8>>,
    ) -> Result<Superblock, Error> {
        let generation = self.durable.generation.checked_add(1).ok_or(Error::EIO)?;
        if txn.generation() != generation || self.durable.total_blocks > partition_blocks(device)? {
            return Err(Error::EIO);
        }
        // Step 1: the caller's write lock covers this deferred-space fixpoint.
        // Opening the next in-memory transaction belongs to the adapter.
        let space_root = txn.settle().map_err(|_| Error::EIO)?;
        let mut nodes = txn.writes().clone();
        let mut filled = BTreeMap::new();
        let space_root = fill(
            space_root,
            &mut nodes,
            &mut filled,
            &mut BTreeSet::new(),
            generation,
            self.durable.fs_uuid,
        )?;
        // A private FS node retained as DEAD still owns outgoing references.
        // Write the remaining staged nodes too, after filling their children.
        let remaining: Vec<_> = nodes.keys().copied().collect();
        for addr in remaining {
            if let Some(bytes) = nodes.get(&addr) {
                let ptr = pointer(bytes, NodeHeader::decode(bytes)?);
                fill(
                    ptr,
                    &mut nodes,
                    &mut filled,
                    &mut BTreeSet::new(),
                    generation,
                    self.durable.fs_uuid,
                )?;
            }
        }
        let mut sb = self.durable.clone();
        sb.generation = generation;
        sb.slot = (generation % 4) as u8;
        sb.ring = 0;
        sb.space_root = space_root;
        let view = Prepared {
            base: txn.view(),
            nodes: &filled,
        };
        let entries = super::recover::space_items(&view, &sb)?;
        sb.free_blocks = 0;
        let mut expected_data = BTreeSet::new();
        for (key, bytes) in entries {
            if key.item_type == format::FREE {
                sb.free_blocks = sb
                    .free_blocks
                    .checked_add(Free::decode(&bytes)?.length)
                    .ok_or(Error::EIO)?;
            } else if key.item_type == format::REF {
                let r = Ref::decode(&bytes)?;
                if r.kind == 3 && r.alloc_generation == generation {
                    let bytes = data.get(&key.object).ok_or(Error::EIO)?;
                    if key.offset != r.crcs.len() as u64 || bytes.len() != r.crcs.len() * BLOCK_SIZE
                    {
                        return Err(Error::EIO);
                    }
                    for (chunk, crc) in bytes.chunks_exact(BLOCK_SIZE).zip(r.crcs) {
                        if crc32c::checksum(chunk) != crc {
                            return Err(Error::EIO);
                        }
                    }
                    expected_data.insert(key.object);
                }
            }
        }
        if expected_data.len() != data.len() {
            return Err(Error::EIO);
        }
        if self.durable.generation == 0 && sb.free_blocks < sb.global_reserve {
            return Err(Error::EIO);
        }
        // Validate ranges before issuing step 3, including disjoint node/data
        // extents. Allocation/reuse itself is owned by txn.rs and space.rs.
        let mut end = 20;
        let mut ranges: Vec<_> = filled
            .iter()
            .chain(data.iter())
            .map(|(&addr, b)| (addr, b.len()))
            .collect();
        ranges.sort_unstable();
        for (addr, len) in ranges {
            if len == 0 || len % BLOCK_SIZE != 0 || addr < end {
                return Err(Error::EIO);
            }
            end = addr
                .checked_add((len / BLOCK_SIZE) as u64)
                .ok_or(Error::EIO)?;
            if end > sb.total_blocks - 4 {
                return Err(Error::EIO);
            }
        }
        // Steps 3-4: the synchronous surface waits at each sector completion.
        for (&addr, bytes) in filled.iter().chain(data.iter()) {
            write(device, addr, bytes)?;
        }
        device.flush().map_err(|_| Error::EIO)?;
        // Steps 5-6: return immediately on error. In particular, no rollback
        // writes or erasure can follow a partially published superblock.
        if self.durable.generation == 0 {
            // Image writers overwrite the unused slots too (design 2.5).
            for ring in 0..2 {
                for slot in 0..4 {
                    if slot != sb.slot {
                        write(
                            device,
                            slot_block(sb.total_blocks, ring, slot),
                            &[0; BLOCK_SIZE],
                        )?;
                    }
                }
            }
        }
        for ring in 0..2 {
            sb.ring = ring;
            write(
                device,
                slot_block(sb.total_blocks, ring, sb.slot),
                &sb.encode(),
            )?;
        }
        device.flush().map_err(|_| Error::EIO)?;
        sb.ring = 0;
        Ok(sb)
    }
}
struct Prepared<'a, S> {
    base: S,
    nodes: &'a BTreeMap<u64, Vec<u8>>,
}
impl<S: NodeStore> NodeStore for Prepared<'_, S> {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.nodes
            .get(&addr)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| self.base.get(addr))
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
        Err(btree::Error::Store)
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        Err(btree::Error::Store)
    }
}
fn pointer(bytes: &[u8], h: NodeHeader) -> BlockPtr {
    BlockPtr {
        addr: h.addr,
        generation: h.generation,
        csum: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        level: h.level,
    }
}
// Dependency order is children, FS roots, SPACE leaves, SPACE ancestors.
fn fill(
    ptr: BlockPtr,
    pending: &mut BTreeMap<u64, Vec<u8>>,
    done: &mut BTreeMap<u64, Vec<u8>>,
    visiting: &mut BTreeSet<u64>,
    generation: u64,
    uuid: [u8; 16],
) -> Result<BlockPtr, Error> {
    if let Some(bytes) = done.get(&ptr.addr) {
        return Ok(pointer(bytes, NodeHeader::decode(bytes)?));
    }
    if !visiting.insert(ptr.addr) {
        return Err(Error::EIO);
    }
    let Some(mut bytes) = pending.remove(&ptr.addr) else {
        visiting.remove(&ptr.addr);
        return Ok(ptr);
    };
    let h = NodeHeader::decode(&bytes)?;
    if h.addr != ptr.addr
        || h.generation != generation
        || h.fs_uuid != uuid
        || h.level >= 8
        || h.level != ptr.level
        || !(1..=2).contains(&h.tree)
    {
        return Err(Error::EIO);
    }
    let width = if h.level == 0 { 21 } else { 37 };
    if 48 + usize::from(h.nitems) * width > NODE_SIZE {
        return Err(Error::EIO);
    }
    for i in 0..usize::from(h.nitems) {
        let at = 48 + i * width;
        if h.level > 0 {
            let mut e = InternalEntry::decode(&bytes[at..at + 37])?;
            let p = fill(
                BlockPtr {
                    addr: e.child_addr,
                    generation: e.child_generation,
                    csum: e.child_csum,
                    level: h.level - 1,
                },
                pending,
                done,
                visiting,
                generation,
                uuid,
            )?;
            e.child_addr = p.addr;
            e.child_generation = p.generation;
            e.child_csum = p.csum;
            bytes[at..at + 37].copy_from_slice(&e.encode());
        } else if h.tree == 2 {
            let item = LeafItemHeader::decode(&bytes[at..at + 21])?;
            if item.key.item_type == format::ROOT {
                let lo = usize::from(item.data_off);
                let hi = lo + usize::from(item.data_len);
                let mut r = Root::decode(bytes.get(lo..hi).ok_or(Error::EIO)?)?;
                r.root = fill(r.root, pending, done, visiting, generation, uuid)?;
                bytes[lo..hi].copy_from_slice(&r.encode());
            }
        }
    }
    h.encode(&mut bytes)?;
    let ptr = pointer(&bytes, h);
    done.insert(h.addr, bytes);
    visiting.remove(&h.addr);
    Ok(ptr)
}

pub(crate) fn slot_block(total: u64, ring: u8, slot: u8) -> u64 {
    (if ring == 0 { 16 } else { total - 4 }) + u64::from(slot)
}
pub(crate) fn partition_blocks(device: &dyn BlockDevice) -> Result<u64, Error> {
    let sector = device.block_size();
    if sector == 0 || sector > BLOCK_SIZE || BLOCK_SIZE % sector != 0 {
        return Err(Error::EIO);
    }
    device
        .num_blocks()
        .checked_div((BLOCK_SIZE / sector) as u64)
        .filter(|n| *n >= 16384)
        .ok_or(Error::EIO)
}
pub(crate) fn read(device: &dyn BlockDevice, addr: u64, len: usize) -> Result<Vec<u8>, Error> {
    let total = partition_blocks(device)?;
    let sector = device.block_size();
    if len % BLOCK_SIZE != 0
        || addr
            .checked_add((len / BLOCK_SIZE) as u64)
            .is_none_or(|end| end > total)
    {
        return Err(Error::EIO);
    }
    let start = addr
        .checked_mul((BLOCK_SIZE / sector) as u64)
        .ok_or(Error::EIO)?;
    let mut bytes = vec![0; len];
    for (i, b) in bytes.chunks_exact_mut(sector).enumerate() {
        device
            .read_block(start + i as u64, b)
            .map_err(|_| Error::EIO)?;
    }
    Ok(bytes)
}
pub(crate) fn write(device: &dyn BlockDevice, addr: u64, bytes: &[u8]) -> Result<(), Error> {
    let total = partition_blocks(device)?;
    let sector = device.block_size();
    if bytes.len() % BLOCK_SIZE != 0
        || addr
            .checked_add((bytes.len() / BLOCK_SIZE) as u64)
            .is_none_or(|end| end > total)
    {
        return Err(Error::EIO);
    }
    let start = addr
        .checked_mul((BLOCK_SIZE / sector) as u64)
        .ok_or(Error::EIO)?;
    for (i, b) in bytes.chunks_exact(sector).enumerate() {
        device
            .write_block(start + i as u64, b)
            .map_err(|_| Error::EIO)?;
    }
    Ok(())
}
/// Read-only backing store for a transaction or recovery walk.
pub struct DeviceStore<'a>(pub &'a dyn BlockDevice);
impl NodeStore for DeviceStore<'_> {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        read(self.0, addr, NODE_SIZE).map_err(|_| btree::Error::Store)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
        Err(btree::Error::Store)
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        Err(btree::Error::Store)
    }
}
