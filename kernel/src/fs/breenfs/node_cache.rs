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

/// A token naming the running thread; 0 only on hosts with no VFS. A no-wait
/// pass (E2 design 7.2) belongs to one thread: the commit worker reads the
/// frozen generation through the same mount's stores, with no mount lock, while
/// a foreground pass is running, and it must keep waiting on the device.
pub(crate) fn thread_token() -> usize {
    #[cfg(all(any(target_os = "none", target_os = "breenix"), not(test), target_arch = "x86_64"))]
    {
        crate::per_cpu::current_thread_id_lock_free()
            .and_then(|id| usize::try_from(id).ok())
            .map_or(usize::MAX, |id| id.wrapping_add(1).max(1))
    }
    #[cfg(all(any(target_os = "none", target_os = "breenix"), not(test), target_arch = "aarch64"))]
    {
        match crate::per_cpu_aarch64::current_thread_ptr() as usize {
            0 => usize::MAX,
            thread => thread,
        }
    }
    #[cfg(test)]
    {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        std::thread_local! {
            static TOKEN: usize = NEXT.fetch_add(1, Ordering::Relaxed);
        }
        TOKEN.with(|token| *token)
    }
    // Host builds without the VFS adapter run no passes.
    #[cfg(all(not(test), not(any(target_os = "none", target_os = "breenix"))))]
    {
        0
    }
}

/// The running no-wait pass of one foreground operation (E2 design 7.2).
/// While the thread that owns it holds its mount's state lock, the mount's
/// stores do not wait on the device: a node that is not in memory is
/// recorded and the read fails with `ReadCause::Deferred`. The operation then
/// reads the recorded nodes with the lock released into its `Held` set, which
/// it lends to each later pass, so that pass finds them even if the caches
/// evicted them.
#[derive(Default)]
struct Pass {
    owner: usize,
    held: Held,
    wanted: Vec<BlockPtr>,
    wanted_raw: Vec<(u64, u64)>,
}

/// The nodes one operation read with its mount lock released. It belongs to
/// the operation, not the mount: operations in flight at once each keep theirs.
#[derive(Default)]
pub struct Held {
    nodes: Vec<((u64, u64), Arc<Node>)>,
    raw: Vec<(u64, Vec<u8>)>,
}
impl Held {
    /// Keep a node read with the lock released for the operation's later passes.
    pub fn hold(&mut self, ptr: BlockPtr, node: Arc<Node>) -> Result<(), Error> {
        let key = (ptr.addr, ptr.generation);
        if let Err(index) = self.nodes.binary_search_by_key(&key, |(key, _)| *key) {
            self.nodes.try_reserve(1).map_err(|_| allocation(ptr.addr))?;
            self.nodes.insert(index, (key, node));
        }
        Ok(())
    }
    /// Keep raw node bytes read with the lock released (`NodeStore::get`).
    pub fn hold_raw(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), Error> {
        if !self.raw.iter().any(|(held, _)| *held == addr) {
            self.raw.try_reserve(1).map_err(|_| allocation(addr))?;
            self.raw.push((addr, bytes));
        }
        Ok(())
    }
    fn node(&self, ptr: BlockPtr) -> Option<Arc<Node>> {
        let index = self.nodes.binary_search_by_key(&(ptr.addr, ptr.generation), |(key, _)| *key).ok()?;
        Some(self.nodes[index].1.clone())
    }
    fn bytes(&self, addr: u64) -> Option<Vec<u8>> {
        let (_, bytes) = self.raw.iter().find(|(held, _)| *held == addr)?;
        let mut copy = Vec::new();
        copy.try_reserve_exact(bytes.len()).ok()?;
        copy.extend_from_slice(bytes);
        Some(copy)
    }
}

/// The reads one pass could not make without the device.
#[derive(Default)]
pub struct Wanted {
    pub nodes: Vec<BlockPtr>,
    /// Raw node reads (`NodeStore::get`): the address and the generation
    /// ceiling of the store that asked.
    pub raw: Vec<(u64, u64)>,
}
impl Wanted {
    /// Add `other`'s reads to these.
    pub fn append(&mut self, other: Wanted) -> Result<(), Error> {
        self.nodes.try_reserve(other.nodes.len()).map_err(|_| allocation(0))?;
        self.raw.try_reserve(other.raw.len()).map_err(|_| allocation(0))?;
        self.nodes.extend(other.nodes);
        self.raw.extend(other.raw);
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.raw.is_empty()
    }
    /// The newest generation these reads name.
    pub fn ceiling(&self) -> u64 {
        self.nodes.iter().map(|ptr| ptr.generation)
            .chain(self.raw.iter().map(|&(_, ceiling)| ceiling))
            .max().unwrap_or(0)
    }
}

fn allocation(addr: u64) -> Error {
    Error::Read(ReadFailure { cause: ReadCause::Allocation, addr })
}

/// A cache of verified durable decodings scoped to one mount.
pub struct DecodedNodes {
    limit: usize,
    state: Mutex<DecodedState>,
    pass: Mutex<Pass>,
    // The owner of the running pass, read on a node miss without `pass`.
    owner: AtomicUsize,
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
            pass: Mutex::default(),
            owner: AtomicUsize::new(0),
        }
    }

    /// Start a no-wait pass on the current thread, lending it the
    /// operation's `held` nodes. The caller holds the mount's state lock
    /// until `end_pass`, so passes of one mount do not overlap.
    pub fn begin_pass(&self, held: Held) {
        let token = thread_token();
        let mut pass = self.pass.lock();
        pass.owner = token;
        pass.held = held;
        self.owner.store(token, Ordering::Release);
    }
    /// End the current pass: return the operation's held nodes and what the
    /// pass could not read from memory.
    pub fn end_pass(&self) -> (Wanted, Held) {
        self.owner.store(0, Ordering::Release);
        let mut pass = self.pass.lock();
        pass.owner = 0;
        let wanted = Wanted {
            nodes: core::mem::take(&mut pass.wanted),
            raw: core::mem::take(&mut pass.wanted_raw),
        };
        (wanted, core::mem::take(&mut pass.held))
    }
    fn in_pass(&self) -> bool {
        let owner = self.owner.load(Ordering::Acquire);
        owner != 0 && owner == thread_token()
    }
    fn held(&self, ptr: BlockPtr) -> Option<Arc<Node>> {
        self.pass.lock().held.node(ptr)
    }
    fn held_raw(&self, addr: u64) -> Option<Vec<u8>> {
        self.pass.lock().held.bytes(addr)
    }
    /// Record a read the pass could not make from memory.
    fn defer(&self, ptr: Option<BlockPtr>, addr: u64, ceiling: u64) -> Error {
        let mut pass = self.pass.lock();
        let recorded = match ptr {
            Some(ptr) if !pass.wanted.contains(&ptr) => {
                pass.wanted.try_reserve(1).map(|()| pass.wanted.push(ptr))
            }
            None if !pass.wanted_raw.contains(&(addr, ceiling)) => {
                pass.wanted_raw.try_reserve(1).map(|()| pass.wanted_raw.push((addr, ceiling)))
            }
            _ => Ok(()),
        };
        match recorded {
            Ok(()) => Error::Read(ReadFailure { cause: ReadCause::Deferred, addr }),
            Err(_) => allocation(addr),
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
                identity: view_identity(),
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
                identity: view_identity(),
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
                identity: view_identity(),
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
        if self.decoded.in_pass() {
            // No-wait pass (design 7.2): not BufferCache::read_verified,
            // whose admission ticket queues even a hit behind another
            // thread's in-flight request (ruling DEV-53(3)). A resident copy
            // was verified when the cache accepted it.
            if let Some(node) = self.decoded.held(ptr) {
                return Ok(node);
            }
            let Some(buffer) = self.cache.pin(self.device.as_ref(), ptr.addr) else {
                return Err(self.decoded.defer(Some(ptr), ptr.addr, ptr.generation));
            };
            let node = btree::decode_verified_node(&buffer, ptr).map_err(|error| error.at_node(ptr.addr))?;
            self.decoded.insert(ptr, &node);
            return Ok(node);
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
        if self.decoded.in_pass() {
            if let Some(held) = self.decoded.held_raw(addr) {
                return Ok(held);
            }
            let Some(buffer) = self.cache.pin(self.device.as_ref(), addr) else {
                return Err(self.decoded.defer(None, addr, self.generation));
            };
            bytes.copy_from_slice(&buffer[..NODE_SIZE]);
            return Ok(bytes);
        }
        read_verified_node(&self.cache, &self.device, &self.fs_uuid, self.generation, addr, bytes)
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

/// Read node `addr` through `cache` into `bytes` (NODE_SIZE), verifying
/// UUID, address, generation (at most `generation`) and CRC on a miss.
fn read_verified_node<W: Wait>(
    cache: &BufferCache<W>,
    device: &NodeDevice<'_>,
    fs_uuid: &[u8; 16],
    generation: u64,
    addr: u64,
    mut bytes: Vec<u8>,
) -> Result<Vec<u8>, Error> {
    let verification_failed = core::cell::Cell::new(false);
    cache
        .read_verified(device, addr, &mut bytes, |node| {
            // decode checks the magic and CRC-32C of bytes 12..16383.
            let valid = NodeHeader::decode(node).is_ok_and(|header| {
                header.addr == addr
                    && header.generation > 0
                    && header.generation <= generation
                    && header.fs_uuid == *fs_uuid
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

impl<'a, W: Wait> CachedNodeStore<'a, W> {
    /// A handle that reads into this view's caches with no mount lock held
    /// (E2 design 7.2 step 2), for nodes of generations up to `ceiling`.
    pub fn fetcher(&self, ceiling: u64) -> NodeFetcher<'a, W> {
        NodeFetcher {
            cache: self.cache.clone(),
            device: NodeDevice {
                device: self.device.device.clone(),
                total_blocks: self.device.total_blocks,
                identity: self.device.identity,
            },
            fs_uuid: self.fs_uuid,
            generation: ceiling.max(self.generation),
            view_generation: self.generation,
            decoded: self.decoded.clone(),
        }
    }
}

/// Reads a pass's misses with the mount lock released into the view that
/// recorded them: its buffer-cache keyspace and the mount's decoded cache, as
/// the view's own reads would. It reads the device outside the cache's
/// admission queue, which is keyed by the view: a caller reading through the
/// view under the state lock must not queue behind this read (ruling
/// DEV-53(3)). The reader's generation pin keeps writes off these blocks.
/// The view's own drop retires its keyspace; dropping a fetcher leaves it.
pub struct NodeFetcher<'a, W: Wait> {
    cache: Retained<'a, BufferCache<W>>,
    device: NodeDevice<'a>,
    fs_uuid: [u8; 16],
    generation: u64,
    // Only nodes the view itself may read enter its keyspace.
    view_generation: u64,
    decoded: Arc<DecodedNodes>,
}
impl<W: Wait> NodeFetcher<'_, W> {
    pub fn read_node(&self, ptr: BlockPtr) -> Result<Arc<Node>, Error> {
        if ptr.generation == 0 || ptr.generation > self.generation {
            return Err(Error::InvalidNode.at_node(ptr.addr));
        }
        if let Some(node) = self.decoded.get(ptr) {
            return node;
        }
        let node = btree::decode_verified_node(&self.get(ptr.addr)?, ptr)
            .map_err(|error| error.at_node(ptr.addr))?;
        self.decoded.insert(ptr, &node);
        Ok(node)
    }
    pub fn get(&self, addr: u64) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(NODE_SIZE).map_err(|_| allocation(addr))?;
        bytes.resize(NODE_SIZE, 0);
        if let Some(buffer) = self.cache.pin(&self.device, addr) {
            bytes.copy_from_slice(&buffer[..NODE_SIZE]);
            return Ok(bytes);
        }
        self.device.read_block(addr, &mut bytes)
            .map_err(|error| Error::Read(ReadFailure { cause: error.into(), addr }))?;
        // decode checks the magic and CRC-32C of bytes 12..16383.
        let header = NodeHeader::decode(&bytes).ok().filter(|header| {
            header.addr == addr
                && header.generation > 0
                && header.generation <= self.generation
                && header.fs_uuid == self.fs_uuid
        }).ok_or(Error::Read(ReadFailure { cause: ReadCause::Verification, addr }))?;
        if header.generation <= self.view_generation {
            self.cache.adopt_clean(&self.device, addr, &bytes);
        }
        Ok(bytes)
    }
}

// A view's buffer-cache identity: unique in the process, and odd, so it cannot
// equal a device's identity, which is an aligned address. A dropped view's
// address can be reused; its identity is not.
static NEXT_VIEW: AtomicUsize = AtomicUsize::new(0);
fn view_identity() -> usize {
    (NEXT_VIEW.fetch_add(1, Ordering::Relaxed) << 1) | 1
}

// Keys retain the format's 4 KiB start address, including nodes not aligned to
// four blocks. Only read_block is exposed to the store; mutation is refused.
struct NodeDevice<'a> {
    device: Retained<'a, dyn BlockDevice + 'a>,
    total_blocks: u64,
    identity: usize,
}
impl BlockDevice for NodeDevice<'_> {
    fn identity(&self) -> usize {
        self.identity
    }
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
impl<T: ?Sized> Clone for Retained<'_, T> {
    fn clone(&self) -> Self {
        match self {
            Self::Borrowed(value) => Self::Borrowed(value),
            Self::Shared(value) => Self::Shared(value.clone()),
        }
    }
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
