//! Immutable verified node store (breenfs design sections 1.2 and 2.2).
use super::{
    btree::{self, Error, Node, NodeStore, ReadCause, ReadFailure},
    commit,
    format::{BlockPtr, NodeHeader, Superblock, BLOCK_SIZE, NODE_SIZE},
};
use crate::block::{
    cache::{BufferCache, Wait},
    BlockDevice, BlockError,
};
use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    mem::size_of,
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};
use spin::Mutex;

// Until E1, retained metadata shares the kernel heap. Each class has a
// system-wide ceiling as well as a budget derived from usable physical RAM.
const DECODED_HEAP_CAP: usize = 4 * 1024 * 1024;
const MIN_NODE_CHARGE: usize = NODE_SIZE / 4;
static DECODED_HEAP: CacheBudget = CacheBudget::new();

pub(super) fn metadata_ram_bytes() -> usize {
    #[cfg(any(target_os = "none", target_os = "breenix"))]
    {
        let bytes = crate::memory::frame_allocator::memory_stats().total_bytes;
        usize::try_from(bytes).unwrap_or(usize::MAX)
    }
    #[cfg(not(any(target_os = "none", target_os = "breenix")))]
    {
        // Keep host measurements independent of the runner's memory.
        512 * 1024 * 1024
    }
}

pub(super) struct CacheBudget(AtomicUsize);

impl CacheBudget {
    pub(super) const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    pub(super) fn charge(&'static self, bytes: usize, limit: usize) -> Option<CacheCharge> {
        self.0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|&total| total <= limit)
            })
            .ok()?;
        Some(CacheCharge {
            budget: self,
            bytes,
        })
    }
}

pub(super) struct CacheCharge {
    budget: &'static CacheBudget,
    bytes: usize,
}

impl Drop for CacheCharge {
    fn drop(&mut self) {
        self.budget.0.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// A cache of verified durable decodings scoped to one mount.
pub struct DecodedNodes {
    limit: usize,
    state: Mutex<DecodedState>,
}

#[derive(Default)]
struct DecodedState {
    entries: Vec<DecodedEntry>,
    table: Option<CacheCharge>,
    bytes: usize,
    clock: u64,
}

struct DecodedEntry {
    ptr: BlockPtr,
    node: Arc<Node>,
    used: u64,
    charge: CacheCharge,
}

impl DecodedEntry {
    fn key(&self) -> (u64, u64) {
        (self.ptr.addr, self.ptr.generation)
    }
}

fn decoded_bytes(node: &Node) -> usize {
    // Include Vec capacity, Arc headers and alignment as well as the payloads.
    let bytes = size_of::<Node>()
        + 128
        + node.children.capacity() * size_of::<super::format::InternalEntry>()
        + node.items.capacity() * size_of::<(super::format::Key, Arc<[u8]>)>()
        + node
            .items
            .iter()
            .map(|(_, bytes)| bytes.len().next_multiple_of(16) + 32)
            .sum::<usize>();
    bytes.max(MIN_NODE_CHARGE)
}

impl DecodedNodes {
    pub fn new(ram: usize) -> Self {
        Self {
            limit: (ram / 128).min(DECODED_HEAP_CAP),
            state: Mutex::default(),
        }
    }

    fn get(&self, ptr: BlockPtr) -> Option<Result<Arc<Node>, Error>> {
        let mut state = self.state.lock();
        let index = state
            .entries
            .binary_search_by_key(&(ptr.addr, ptr.generation), DecodedEntry::key)
            .ok()?;
        state.clock = state.clock.wrapping_add(1);
        let clock = state.clock;
        let entry = &mut state.entries[index];
        entry.used = clock;
        // The cache key names the incarnation. An exact pointer still has to
        // match the checksum and level that were verified on admission.
        Some(if entry.ptr == ptr {
            Ok(entry.node.clone())
        } else {
            Err(Error::InvalidNode.at_node(ptr.addr))
        })
    }

    fn insert(&self, ptr: BlockPtr, node: &Arc<Node>) {
        let bytes = decoded_bytes(node);
        let slots = self.limit / (MIN_NODE_CHARGE + size_of::<DecodedEntry>());
        let table_bytes = slots * size_of::<DecodedEntry>();
        if bytes > self.limit - table_bytes || slots == 0 {
            return;
        }
        let mut state = self.state.lock();
        if state.table.is_none() {
            let Some(charge) = DECODED_HEAP.charge(table_bytes, self.limit) else {
                return;
            };
            if state.entries.try_reserve_exact(slots).is_err() {
                return;
            }
            state.table = Some(charge);
        }
        let key = (ptr.addr, ptr.generation);
        if state
            .entries
            .binary_search_by_key(&key, DecodedEntry::key)
            .is_ok()
        {
            return;
        }
        while state.bytes + bytes + table_bytes > self.limit {
            let Some((index, _)) = state
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.used)
            else {
                return;
            };
            state.bytes -= state.entries.remove(index).charge.bytes;
        }
        let Some(charge) = DECODED_HEAP.charge(bytes, self.limit) else {
            return;
        };
        let Err(index) = state.entries.binary_search_by_key(&key, DecodedEntry::key) else {
            return;
        };
        state.clock = state.clock.wrapping_add(1);
        let used = state.clock;
        state.entries.insert(
            index,
            DecodedEntry {
                ptr,
                node: node.clone(),
                used,
                charge,
            },
        );
        state.bytes += bytes;
    }

    pub fn retire_overwritten(&self, prepared: &commit::PreparedCommit) {
        let mut state = self.state.lock();
        let mut released = 0;
        state.entries.retain(|entry| {
            if prepared.overwrites_node(entry.ptr.addr) {
                released += entry.charge.bytes;
                false
            } else {
                true
            }
        });
        state.bytes -= released;
    }

    pub fn clear(&self) {
        *self.state.lock() = DecodedState::default();
    }
}

/// A view with a generation ceiling and optional unpublished buffers.
/// The raw buffer cache remains view-private because its key only records an
/// address. Decoded entries belong to the mount and carry the node generation.
pub struct CachedNodeStore<'a, W: Wait> {
    cache: Retained<'a, BufferCache<W>>,
    device: Box<NodeDevice<'a>>,
    fs_uuid: [u8; 16],
    generation: u64,
    // The mount owns the generation being published. Once it is durable the
    // mount drops it, its buffers are freed and its nodes are read from the
    // device (E24 coordinator ledger line 309, C3).
    pending: Option<Weak<commit::PreparedCommit>>,
    decoded: Arc<DecodedNodes>,
}

impl<'a, W: Wait> CachedNodeStore<'a, W> {
    pub fn with_pending(mut self, pending: &Arc<commit::PreparedCommit>) -> Self {
        self.pending = Some(Arc::downgrade(pending));
        self
    }
    /// `durable` is the superblock selected by read-only recovery. The device
    /// must remain unchanged while this read-only mount is in use.
    pub fn new(
        cache: &'a BufferCache<W>,
        device: &'a dyn BlockDevice,
        durable: &Superblock,
    ) -> Self {
        Self {
            cache: Retained::Borrowed(cache),
            device: Box::new(NodeDevice {
                device: Retained::Borrowed(device),
                total_blocks: durable.total_blocks,
            }),
            fs_uuid: durable.fs_uuid,
            generation: durable.generation,
            pending: None,
            decoded: Arc::new(DecodedNodes::new(metadata_ram_bytes())),
        }
    }
    /// Borrow the system-wide cache while retaining the mount's device.
    pub fn with_device(
        cache: &'a BufferCache<W>,
        device: Arc<dyn BlockDevice>,
        durable: &Superblock,
        decoded: Arc<DecodedNodes>,
    ) -> Self {
        Self {
            cache: Retained::Borrowed(cache),
            device: Box::new(NodeDevice {
                device: Retained::Shared(device),
                total_blocks: durable.total_blocks,
            }),
            fs_uuid: durable.fs_uuid,
            generation: durable.generation,
            pending: None,
            decoded,
        }
    }
    /// Retain the mount's device and shared cache without self-references.
    pub fn new_owned(
        cache: Arc<BufferCache<W>>,
        device: Arc<dyn BlockDevice>,
        durable: &Superblock,
        decoded: Arc<DecodedNodes>,
    ) -> Self {
        Self {
            cache: Retained::Shared(cache),
            device: Box::new(NodeDevice {
                device: Retained::Shared(device),
                total_blocks: durable.total_blocks,
            }),
            fs_uuid: durable.fs_uuid,
            generation: durable.generation,
            pending: None,
            decoded,
        }
    }
}

impl<W: Wait> NodeStore for CachedNodeStore<'_, W> {
    fn read_node(&self, ptr: BlockPtr) -> Result<Arc<Node>, Error> {
        if ptr.generation == 0 || ptr.generation > self.generation {
            return Err(Error::InvalidNode.at_node(ptr.addr));
        }
        if let Some(pending) = self.pending.as_ref().and_then(Weak::upgrade) {
            if let Some(bytes) = pending.node(ptr.addr) {
                // A failed publication can leave these bytes off disk.
                return btree::decode_verified_node(bytes, ptr)
                    .map_err(|error| error.at_node(ptr.addr));
            }
            if pending.overwrites_node(ptr.addr) {
                return Err(Error::InvalidNode.at_node(ptr.addr));
            }
        }
        if let Some(node) = self.decoded.get(ptr) {
            return node;
        }
        // The view's buffer cache verifies UUID/address/generation and CRC.
        // Decode verifies the exact pointer and the complete node layout.
        let node = btree::decode_verified_node(&self.get(ptr.addr)?, ptr)
            .map_err(|error| error.at_node(ptr.addr))?;
        self.decoded.insert(ptr, &node);
        Ok(node)
    }
    fn get(&self, addr: u64) -> Result<Vec<u8>, Error> {
        #[cfg(test)]
        btree::measurement::edit_cost(|c| c.copies += 1);
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(NODE_SIZE).map_err(|_| {
            Error::Read(ReadFailure {
                cause: ReadCause::Allocation,
                addr,
            })
        })?;
        bytes.resize(NODE_SIZE, 0);
        if let Some(pending) = self.pending.as_ref().and_then(Weak::upgrade) {
            if let Some(node) = pending.node(addr) {
                bytes.copy_from_slice(node);
                return Ok(bytes);
            }
        }
        let verification_failed = core::cell::Cell::new(false);
        self.cache
            .read_verified(self.device.as_ref(), addr, &mut bytes, |node| {
                // decode checks the magic and CRC-32C of bytes 12..16383.
                let valid = NodeHeader::decode(node).is_ok_and(|header| {
                    header.addr == addr
                        && header.generation > 0
                        && header.generation <= self.generation
                        && header.fs_uuid == self.fs_uuid
                });
                verification_failed.set(!valid);
                valid
            })
            .map_err(|error| {
                Error::Read(ReadFailure {
                    cause: if verification_failed.get() {
                        ReadCause::Verification
                    } else {
                        error.into()
                    },
                    addr,
                })
            })?;
        Ok(bytes)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), Error> {
        Err(Error::Store)
    }
    fn alloc(&mut self) -> Result<u64, Error> {
        Err(Error::Store)
    }
}

impl<W: Wait> Drop for CachedNodeStore<'_, W> {
    fn drop(&mut self) {
        self.cache.discard_clean(self.device.as_ref());
    }
}

// Keys retain the format's 4 KiB start address, including nodes not aligned to
// four blocks. Only read_block is exposed to the store; mutation is refused.
struct NodeDevice<'a> {
    device: Retained<'a, dyn BlockDevice + 'a>,
    total_blocks: u64,
}
impl BlockDevice for NodeDevice<'_> {
    fn read_block(&self, addr: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let total = commit::partition_blocks(&*self.device).map_err(|_| BlockError::IoError)?;
        if bytes.len() != NODE_SIZE
            || self.total_blocks > total
            || addr < 20
            || addr
                .checked_add(4)
                .is_none_or(|end| end > self.total_blocks.saturating_sub(4))
        {
            return Err(BlockError::OutOfBounds);
        }
        let sector = self.device.block_size();
        let start = addr
            .checked_mul((BLOCK_SIZE / sector) as u64)
            .ok_or(BlockError::OutOfBounds)?;
        // One device request per node miss (E24 ledger 372, #3040).
        self.device.read_blocks(start, NODE_SIZE / sector, bytes)
    }
    fn write_block(&self, _: u64, _: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::IoError)
    }
    fn block_size(&self) -> usize {
        NODE_SIZE
    }
    fn num_blocks(&self) -> u64 {
        // The last permissible start is total_blocks - 8 (before ring B).
        self.total_blocks.saturating_sub(7)
    }
    fn flush(&self) -> Result<(), BlockError> {
        Err(BlockError::IoError)
    }
}

// The borrowed form remains useful for scoped host/core readers; the owned
// form lets an Any + 'static VFS backend retain exactly the same verified store.
enum Retained<'a, T: ?Sized> {
    Borrowed(&'a T),
    Shared(Arc<T>),
}
impl<T: ?Sized> Deref for Retained<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Borrowed(value) => value,
            Self::Shared(value) => value,
        }
    }
}
