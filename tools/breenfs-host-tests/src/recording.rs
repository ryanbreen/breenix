//! Device oracle for breenfs-format section 3's writer rules.
use crate::{
    block::{BlockDevice, BlockError},
    btree::{self, NodeStore},
    crc32c,
    format::{
        self, BlockPtr, Extent, InternalEntry, LeafItemHeader, NodeHeader, Ref, Root, Superblock,
        BLOCK_SIZE, DEAD, EXTENT, NODE_SIZE, REF, ROOT,
    },
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use spin::Mutex;

pub const SECTOR_SIZE: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Write {
        sector: u64,
        bytes: [u8; SECTOR_SIZE],
    },
    Flush,
}

struct Image<'a>(&'a [u8]);
impl NodeStore for Image<'_> {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        let start = usize::try_from(addr)
            .ok()
            .and_then(|a| a.checked_mul(BLOCK_SIZE))
            .ok_or(btree::Error::Store)?;
        let end = start.checked_add(NODE_SIZE).ok_or(btree::Error::Store)?;
        self.0
            .get(start..end)
            .map(|b| b.to_vec())
            .ok_or(btree::Error::Store)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
        Err(btree::Error::Store)
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        Err(btree::Error::Store)
    }
}

// Derive a commit's ownership by walking its encoded roots.
struct Walk<'a> {
    image: Image<'a>,
    sb: &'a Superblock,
    nodes: BTreeSet<u64>,
    live: BTreeSet<u64>,
}
impl Walk<'_> {
    fn range(&mut self, start: u64, length: u64) -> Result<(), BlockError> {
        let end = start.checked_add(length).ok_or(BlockError::IoError)?;
        if start < 20 || length == 0 || end > self.sb.total_blocks - 4 {
            return Err(BlockError::IoError);
        }
        self.live.extend(start..end);
        Ok(())
    }

    fn tree(
        &mut self,
        root: BlockPtr,
        ptr: BlockPtr,
        tree: u8,
        limit: u64,
    ) -> Result<(), BlockError> {
        if ptr.level >= 8 || ptr.generation == 0 || ptr.generation > limit {
            return Err(BlockError::IoError);
        }
        self.range(ptr.addr, 4)?;
        let bytes = self.image.get(ptr.addr).map_err(|_| BlockError::IoError)?;
        let h = NodeHeader::decode(&bytes).map_err(|_| BlockError::IoError)?;
        if (h.addr, h.generation, h.level, h.tree, h.fs_uuid)
            != (ptr.addr, ptr.generation, ptr.level, tree, self.sb.fs_uuid)
            || ptr.csum != u32::from_le_bytes(bytes[8..12].try_into().unwrap())
        {
            return Err(BlockError::IoError);
        }
        if !self.nodes.insert(ptr.addr) {
            return Ok(());
        }
        let width = if h.level == 0 { 21 } else { 37 };
        if 48 + usize::from(h.nitems) * width > NODE_SIZE {
            return Err(BlockError::IoError);
        }
        if h.nitems == 0 {
            if h.level != 0 || ptr != root {
                return Err(BlockError::IoError);
            }
        } else {
            // Searching the leaf's first key from its tree root invokes the
            // kernel reader's node layout, key ordering and ancestor checks.
            let key = format::Key::decode(&bytes[48..65]).map_err(|_| BlockError::IoError)?;
            btree::search(&self.image, root, key).map_err(|_| BlockError::IoError)?;
        }
        for i in 0..usize::from(h.nitems) {
            let lo = 48 + i * width;
            if h.level > 0 {
                let child = InternalEntry::decode(&bytes[lo..lo + width])
                    .map_err(|_| BlockError::IoError)?;
                self.tree(
                    root,
                    BlockPtr {
                        addr: child.child_addr,
                        generation: child.child_generation,
                        csum: child.child_csum,
                        level: h.level - 1,
                    },
                    tree,
                    h.generation,
                )?;
                continue;
            }
            let item =
                LeafItemHeader::decode(&bytes[lo..lo + width]).map_err(|_| BlockError::IoError)?;
            let lo = usize::from(item.data_off);
            let value = bytes
                .get(lo..lo + usize::from(item.data_len))
                .ok_or(BlockError::IoError)?;
            match (tree, item.key.item_type) {
                (2, ROOT) => {
                    let r = Root::decode(value).map_err(|_| BlockError::IoError)?;
                    self.tree(r.root, r.root, 1, self.sb.generation)?;
                }
                (2, DEAD) => {
                    let addr = item.key.object;
                    let bytes = self.image.get(addr).map_err(|_| BlockError::IoError)?;
                    let dead = NodeHeader::decode(&bytes).map_err(|_| BlockError::IoError)?;
                    let ptr = BlockPtr {
                        addr,
                        generation: dead.generation,
                        level: dead.level,
                        csum: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
                    };
                    self.tree(ptr, ptr, 1, self.sb.generation)?;
                }
                (1, EXTENT) => {
                    let e = Extent::decode(value).map_err(|_| BlockError::IoError)?;
                    if !(1..=256).contains(&e.phys_len)
                        || e.num_blocks == 0
                        || u64::from(e.extent_offset) + u64::from(e.num_blocks)
                            > u64::from(e.phys_len)
                    {
                        return Err(BlockError::IoError);
                    }
                    // Refcounts own the physical allocation, including pieces
                    // outside an extent's current logical slice.
                    self.range(e.phys_start, u64::from(e.phys_len))?;
                }
                (2, REF) => {
                    let r = Ref::decode(value).map_err(|_| BlockError::IoError)?;
                    if r.kind == 3 {
                        if !(1..=256).contains(&item.key.offset)
                            || r.crcs.len() as u64 != item.key.offset
                        {
                            return Err(BlockError::IoError);
                        }
                        self.range(item.key.object, item.key.offset)?;
                        for (i, expected) in r.crcs.iter().enumerate() {
                            let lo = (item.key.object as usize + i) * BLOCK_SIZE;
                            if crc32c::checksum(&self.image.0[lo..lo + BLOCK_SIZE]) != *expected {
                                return Err(BlockError::IoError);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Commit {
    sb: Superblock,
    live: BTreeSet<u64>,
}
impl Commit {
    fn read(image: &[u8], sb: Superblock) -> Result<Self, BlockError> {
        let mut walk = Walk {
            image: Image(image),
            sb: &sb,
            nodes: BTreeSet::new(),
            live: BTreeSet::new(),
        };
        walk.tree(sb.space_root, sb.space_root, 2, sb.generation)?;
        Ok(Self {
            live: walk.live,
            sb,
        })
    }
}

struct State {
    image: Vec<u8>,
    durable: Vec<u8>,
    events: Vec<Event>,
    commits: BTreeMap<u64, Commit>,
    dirty: BTreeSet<u64>,
    touched_slots: BTreeSet<u64>,
    pending: BTreeMap<u64, Commit>,
    breach: Option<&'static str>,
}
impl State {
    fn healthy(&self) {
        if let Some(rule) = self.breach {
            panic!("{rule}: recording run already failed");
        }
    }
    fn fail(&mut self, rule: &'static str) -> ! {
        self.breach = Some(rule);
        panic!("{rule}: breenfs recording device writer-rule breach");
    }
    fn total(&self) -> u64 {
        self.image.len() as u64 / BLOCK_SIZE as u64
    }
    fn slot(&self, block: u64) -> Option<(u8, u8)> {
        if (16..20).contains(&block) {
            Some((0, (block - 16) as u8))
        } else if (self.total() - 4..self.total()).contains(&block) {
            Some((1, (block - (self.total() - 4)) as u8))
        } else {
            None
        }
    }
    fn superblock(&self, block: u64) -> Option<Superblock> {
        let (ring, slot) = self.slot(block)?;
        let lo = block as usize * BLOCK_SIZE;
        let bytes = &self.image[lo..lo + BLOCK_SIZE];
        if !format::valid_superblock_slot(bytes, self.total(), ring, slot) {
            return None;
        }
        let sb = Superblock::decode(bytes).ok()?;
        (sb.total_blocks == self.total()).then_some(sb)
    }
    fn keep(&mut self, commit: Commit) -> Result<(), BlockError> {
        if let Some(old) = self.commits.get(&commit.sb.generation) {
            let normalized = Superblock {
                ring: old.sb.ring,
                ..commit.sb.clone()
            };
            if normalized != old.sb {
                return Err(BlockError::IoError);
            }
        }
        self.commits.insert(commit.sb.generation, commit);
        while self.commits.len() > 2 {
            self.commits.pop_first();
        }
        Ok(())
    }
}

/// Synchronous 512-byte device with volatile writes and flush-delimited images.
/// Rule breaches panic and latch the run as failed; bounds errors return an I/O
/// result. Durability observers intentionally remain readable after a breach.
pub struct RecordingDevice {
    state: Mutex<State>,
}
impl RecordingDevice {
    /// The initial image is already durable (normally a breenfs-mkfs output).
    pub fn new(image: Vec<u8>) -> Result<Self, BlockError> {
        if image.len() % BLOCK_SIZE != 0 || image.len() / BLOCK_SIZE < 16384 {
            return Err(BlockError::IoError);
        }
        let mut state = State {
            durable: image.clone(),
            image,
            events: Vec::new(),
            commits: BTreeMap::new(),
            dirty: BTreeSet::new(),
            touched_slots: BTreeSet::new(),
            pending: BTreeMap::new(),
            breach: None,
        };
        let mut uuid = None;
        for block in (16..20).chain(state.total() - 4..state.total()) {
            if let Some(sb) = state.superblock(block) {
                if uuid.is_some_and(|id| id != sb.fs_uuid) {
                    return Err(BlockError::IoError);
                }
                uuid = Some(sb.fs_uuid);
                // This immutable byte image has no device read errors: failure
                // here means invalid contents, not transient storage I/O.
                if let Ok(commit) = Commit::read(&state.durable, sb) {
                    state.keep(commit)?;
                }
            }
        }
        if state.commits.is_empty() {
            return Err(BlockError::IoError);
        }
        Ok(Self {
            state: Mutex::new(state),
        })
    }
    pub fn events(&self) -> Vec<Event> {
        self.state.lock().events.clone()
    }
    pub fn durable_image(&self) -> Vec<u8> {
        self.state.lock().durable.clone()
    }
    pub fn durable_generations(&self) -> Vec<u64> {
        self.state.lock().commits.keys().copied().collect()
    }
}
impl BlockDevice for RecordingDevice {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let state = self.state.lock();
        state.healthy();
        if block >= state.image.len() as u64 / SECTOR_SIZE as u64 {
            return Err(BlockError::OutOfBounds);
        }
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::IoError);
        }
        let lo = block as usize * SECTOR_SIZE;
        buf[..SECTOR_SIZE].copy_from_slice(&state.image[lo..lo + SECTOR_SIZE]);
        Ok(())
    }
    fn write_block(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        let mut s = self.state.lock();
        s.healthy();
        if sector >= s.image.len() as u64 / SECTOR_SIZE as u64 {
            return Err(BlockError::OutOfBounds);
        }
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::IoError);
        }
        let bytes: [u8; SECTOR_SIZE] = buf[..SECTOR_SIZE].try_into().unwrap();
        s.events.push(Event::Write { sector, bytes });
        let block = sector / (BLOCK_SIZE / SECTOR_SIZE) as u64;
        if s.commits.values().any(|c| c.live.contains(&block)) {
            s.fail("W1");
        }
        if s.slot(block).is_some() {
            if !s.dirty.is_empty() {
                s.fail("W2");
            }
        } else if s.pending.values().any(|c| c.live.contains(&block)) {
            s.fail("W2");
        }
        let lo = sector as usize * SECTOR_SIZE;
        s.image[lo..lo + SECTOR_SIZE].copy_from_slice(&bytes);
        if s.slot(block).is_some() {
            s.touched_slots.insert(block);
            s.pending.remove(&block);
            if let Some(sb) = s.superblock(block) {
                if sb.fs_uuid != s.commits.last_key_value().unwrap().1.sb.fs_uuid {
                    s.fail("W2");
                }
                // Trees and data must already exist in the pre-superblock
                // durable image, not just in the volatile device cache.
                let commit = Commit::read(&s.durable, sb).unwrap_or_else(|_| s.fail("W2"));
                s.pending.insert(block, commit);
            }
        } else {
            s.dirty.insert(sector);
        }
        Ok(())
    }
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }
    fn num_blocks(&self) -> u64 {
        self.state.lock().image.len() as u64 / SECTOR_SIZE as u64
    }
    fn flush(&self) -> Result<(), BlockError> {
        let mut s = self.state.lock();
        s.healthy();
        s.events.push(Event::Flush);
        // The mutex serializes writes and flushes through their completion.
        // Publish a commit only here, after the superblock's following flush.
        let pending = core::mem::take(&mut s.pending);
        for commit in pending.into_values() {
            s.keep(commit).unwrap_or_else(|_| s.fail("W2"));
        }
        for sector in core::mem::take(&mut s.dirty) {
            let lo = sector as usize * SECTOR_SIZE;
            let bytes: [u8; SECTOR_SIZE] = s.image[lo..lo + SECTOR_SIZE].try_into().unwrap();
            s.durable[lo..lo + SECTOR_SIZE].copy_from_slice(&bytes);
        }
        for block in core::mem::take(&mut s.touched_slots) {
            let lo = block as usize * BLOCK_SIZE;
            let bytes: [u8; BLOCK_SIZE] = s.image[lo..lo + BLOCK_SIZE].try_into().unwrap();
            s.durable[lo..lo + BLOCK_SIZE].copy_from_slice(&bytes);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "recording_tests.rs"]
mod tests;
