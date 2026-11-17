//! Generation preparation and ordered publication (design section 2.5).
//! The mount closes a generation under its lock, then publishes without it.
use super::{
    btree::{self, NodeStore},
    crc32c,
    format::{
        self, BlockPtr, Free, InternalEntry, LeafItemHeader, NodeHeader, Ref, Root, Superblock,
        BLOCK_SIZE, NODE_SIZE,
    },
    txn::Transaction,
};
use crate::block::{
    cache::{BufferCache, Wait, WriteGroup},
    BlockDevice, BlockError,
};
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
    InvalidGeneration,
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
pub struct FsyncWaiter {
    generation: u64,
    status: AtomicU8,
}
impl FsyncWaiter {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn result(&self) -> Option<Result<(), Error>> {
        match self.status.load(Ordering::Acquire) {
            0 => None,
            1 => Some(Ok(())),
            _ => Some(Err(Error::EIO)),
        }
    }
}

pub struct Committer {
    durable: Superblock,
    failed: bool,
    open_generation: u64,
    in_flight: bool,
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
            open_generation: durable.generation.saturating_add(1),
            in_flight: false,
            durable,
            failed: false,
            waiters: Vec::new(),
        }
    }
    pub fn read_only(&self) -> bool {
        self.failed || self.durable.ro_compat != 0
    }
    pub fn waiter(&mut self) -> Arc<FsyncWaiter> {
        let w = Arc::new(FsyncWaiter {
            generation: self.open_generation,
            status: AtomicU8::new(if self.read_only() { 2 } else { 0 }),
        });
        if !self.read_only() {
            self.waiters.push(w.clone());
        }
        w
    }
    /// `data` contains private extents, keyed by their starting breenfs block.
    /// The transaction owns their allocated REF records and per-block CRCs.
    pub fn commit<S: NodeStore>(
        &mut self,
        device: &dyn BlockDevice,
        txn: &mut Transaction<'_, S>,
        data: &BTreeMap<u64, Vec<u8>>,
    ) -> Result<Superblock, Error> {
        self.committing(|mount| {
            let prepared = mount.prepare(device, txn, data)?;
            prepared.write_direct(device, data)?;
            Ok(prepared.sb)
        })
    }
    pub(crate) fn commit_files<S: NodeStore>(
        &mut self,
        device: &dyn BlockDevice,
        files: &mut super::ops::Files<'_, S>,
    ) -> Result<Superblock, Error> {
        self.committing(|mount| {
            let (txn, data) = files.prepare_commit().map_err(|_| Error::EIO)?;
            let prepared = mount.prepare(device, txn, data)?;
            prepared.write_direct(device, data)?;
            Ok(prepared.sb)
        })
    }
    fn committing(
        &mut self,
        operation: impl FnOnce(&Self) -> Result<Superblock, Error>,
    ) -> Result<Superblock, Error> {
        if self.failed {
            return Err(Error::EIO);
        }
        if self.read_only() {
            return Err(Error::ReadOnly);
        }
        let result = operation(self);
        match &result {
            Ok(sb) => {
                self.durable = sb.clone();
                self.open_generation = sb.generation.saturating_add(1);
            }
            Err(_) => self.failed = true,
        }
        let status = if result.is_ok() { 1 } else { 2 };
        for w in self.waiters.drain(..) {
            w.status.store(status, Ordering::Release);
        }
        result
    }
    fn prepare<S: NodeStore>(
        &self,
        device: &dyn BlockDevice,
        txn: &mut Transaction<'_, S>,
        data: &BTreeMap<u64, Vec<u8>>,
    ) -> Result<PreparedCommit, Error> {
        let generation = self.open_generation;
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
        Ok(PreparedCommit {
            sb,
            nodes: filled,
            data: BTreeMap::new(),
            formatting: self.durable.generation == 0,
            waiters: Vec::new(),
        })
    }

    /// Freeze the open generation and transfer only its receipts. A mount may
    /// now open G+1 on the returned immutable nodes, before publishing G.
    pub fn prepare_files<S: NodeStore>(
        &mut self,
        device: &dyn BlockDevice,
        files: &mut super::ops::Files<'_, S>,
    ) -> Result<PreparedCommit, Error> {
        if self.read_only() {
            return Err(Error::EIO);
        }
        if self.in_flight {
            return Err(Error::InvalidGeneration);
        }
        let result = files
            .prepare_commit()
            .map_err(|_| Error::EIO)
            .and_then(|(txn, data)| {
                let mut prepared = self.prepare(device, txn, data)?;
                prepared.data = core::mem::take(data);
                Ok(prepared)
            });
        match result {
            Ok(mut prepared) => {
                self.open_generation = self.open_generation.checked_add(1).ok_or_else(|| {
                    self.fail();
                    Error::EIO
                })?;
                prepared.waiters = core::mem::take(&mut self.waiters);
                self.in_flight = true;
                Ok(prepared)
            }
            Err(e) => {
                self.fail();
                Err(e)
            }
        }
    }
    /// Publish completion under the mount lock. A failed G also fails callers
    /// already waiting for G+1, which can no longer be committed.
    pub fn complete(&mut self, prepared: &PreparedCommit, result: Result<(), Error>) {
        self.in_flight = false;
        let status = if result.is_ok() { 1 } else { 2 };
        if result.is_ok() {
            self.durable = prepared.sb.clone();
        } else {
            self.fail();
        }
        for waiter in &prepared.waiters {
            waiter.status.store(status, Ordering::Release);
        }
    }
    pub fn fail(&mut self) {
        self.failed = true;
        for waiter in self.waiters.drain(..) {
            waiter.status.store(2, Ordering::Release);
        }
    }
}

/// Immutable G, also the backing view from which the adapter opens G+1.
/// Receipts remain pending until the mount observes the publication result.
pub struct PreparedCommit {
    sb: Superblock,
    nodes: BTreeMap<u64, Vec<u8>>,
    data: BTreeMap<u64, Vec<u8>>,
    formatting: bool,
    waiters: Vec<Arc<FsyncWaiter>>,
}
impl PreparedCommit {
    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }
    pub fn dirty_bytes(&self) -> u64 {
        self.data.values().map(|bytes| bytes.len() as u64).sum()
    }
    pub fn node(&self, addr: u64) -> Option<&[u8]> {
        self.nodes.get(&addr).map(Vec::as_slice)
    }
    /// Read a private data block while G is being published.
    pub fn data_block(&self, block: u64) -> Option<&[u8]> {
        let (&start, bytes) = self.data.range(..=block).next_back()?;
        let offset = usize::try_from(block.checked_sub(start)?)
            .ok()?
            .checked_mul(BLOCK_SIZE)?;
        bytes.get(offset..offset.checked_add(BLOCK_SIZE)?)
    }
    fn write_direct(
        &self,
        device: &dyn BlockDevice,
        data: &BTreeMap<u64, Vec<u8>>,
    ) -> Result<(), Error> {
        for (&addr, bytes) in self.nodes.iter().chain(data.iter()) {
            write(device, addr, bytes)?;
        }
        device.flush().map_err(|_| Error::EIO)?;
        self.superblocks(|addr, bytes| write(device, addr, bytes))?;
        device.flush().map_err(|_| Error::EIO)
    }
    /// Hold buffers are submitted in bounded batches when the cache fills.
    /// No superblock is issued until those batches complete and the data flush
    /// succeeds. A publication error returns immediately, without erasure.
    pub fn write_cached<W: Wait>(
        &self,
        cache: &BufferCache<W>,
        device: &dyn BlockDevice,
        group: &WriteGroup,
    ) -> Result<(), Error> {
        if !matches!(group, WriteGroup::Hold(_)) {
            return Err(Error::EIO);
        }
        let sector = device.block_size();
        for (&addr, bytes) in self.nodes.iter().chain(self.data.iter()) {
            for (i, bytes) in bytes.chunks_exact(sector).enumerate() {
                let block = addr * (BLOCK_SIZE / sector) as u64 + i as u64;
                match cache.write(device, group.clone(), block, bytes) {
                    Err(BlockError::OutOfMemory) => {
                        cache.submit(device, group).wait().map_err(|_| Error::EIO)?;
                        cache
                            .write(device, group.clone(), block, bytes)
                            .map_err(|_| Error::EIO)?;
                    }
                    result => result.map_err(|_| Error::EIO)?,
                }
            }
        }
        cache.submit(device, group).wait().map_err(|_| Error::EIO)?;
        cache.flush(device).map_err(|_| Error::EIO)?;
        self.superblocks(|addr, bytes| {
            for (i, bytes) in bytes.chunks_exact(sector).enumerate() {
                cache
                    .write(
                        device,
                        WriteGroup::WriteThrough,
                        addr * (BLOCK_SIZE / sector) as u64 + i as u64,
                        bytes,
                    )
                    .map_err(|_| Error::EIO)?;
            }
            Ok(())
        })?;
        cache.flush(device).map_err(|_| Error::EIO)
    }
    fn superblocks(
        &self,
        mut write: impl FnMut(u64, &[u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.formatting {
            for ring in 0..2 {
                for slot in 0..4 {
                    if slot != self.sb.slot {
                        write(
                            slot_block(self.sb.total_blocks, ring, slot),
                            &[0; BLOCK_SIZE],
                        )?;
                    }
                }
            }
        }
        let mut sb = self.sb.clone();
        for ring in 0..2 {
            sb.ring = ring;
            write(slot_block(sb.total_blocks, ring, sb.slot), &sb.encode())?;
        }
        Ok(())
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
