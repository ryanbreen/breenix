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
    collections::VecDeque,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::ops::Deref;
use spin::Mutex;

const DECODED_NODES: usize = 64;

/// A generation's node reads. Exact parent-pointer checks remain in btree.
///
/// The private device view makes a whole node one cache entry and verifies it
/// before publishing it Clean.
/// Its boxed address is a stable, mount-private cache identity, distinct from
/// raw sector readers and mounts with different UUID/generation limits. Dropping
/// the store discards its entries before that identity can be reused.
pub struct CachedNodeStore<'a, W: Wait> {
    cache: Retained<'a, BufferCache<W>>,
    device: Box<NodeDevice<'a>>,
    fs_uuid: [u8; 16],
    generation: u64,
    // The mount owns the generation being published. Once it is durable the
    // mount drops it, its buffers are freed and its nodes are read from the
    // device (E24 coordinator ledger line 309, C3).
    pending: Option<Weak<commit::PreparedCommit>>,
    // Entries belong to this immutable generation view. Bound decoded-node
    // allocations independently of disk size.
    decoded: Mutex<VecDeque<(BlockPtr, Arc<Node>)>>,
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
            decoded: Mutex::new(VecDeque::new()),
        }
    }
    /// Borrow the system-wide cache while retaining the mount's device.
    pub fn with_device(
        cache: &'a BufferCache<W>,
        device: Arc<dyn BlockDevice>,
        durable: &Superblock,
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
            decoded: Mutex::new(VecDeque::new()),
        }
    }
    /// Retain the mount's device and shared cache without self-references.
    pub fn new_owned(
        cache: Arc<BufferCache<W>>,
        device: Arc<dyn BlockDevice>,
        durable: &Superblock,
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
            decoded: Mutex::new(VecDeque::new()),
        }
    }
}

impl<W: Wait> NodeStore for CachedNodeStore<'_, W> {
    fn read_node(&self, ptr: BlockPtr) -> Result<Arc<Node>, Error> {
        {
            let mut cache = self.decoded.lock();
            if let Some(at) = cache.iter().position(|(key, _)| *key == ptr) {
                let entry = cache.remove(at).ok_or(Error::Store)?;
                let node = entry.1.clone();
                cache.push_back(entry);
                return Ok(node);
            }
        }
        // I/O and decoding run outside the decoded-cache lock. get verifies
        // UUID/address/generation through the shared buffer cache; decode_node
        // verifies the exact pointer, CRC and complete node layout.
        let node = btree::decode_node(&self.get(ptr.addr)?, ptr)
            .map_err(|error| error.at_node(ptr.addr))?;
        let mut cache = self.decoded.lock();
        if cache.len() == DECODED_NODES {
            cache.pop_front();
        }
        cache.try_reserve(1).map_err(|_| Error::Read(ReadFailure {
            cause: ReadCause::Allocation, addr: ptr.addr,
        }))?;
        cache.push_back((ptr, node.clone()));
        Ok(node)
    }
    fn get(&self, addr: u64) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(NODE_SIZE)
            .map_err(|_| Error::Read(ReadFailure { cause: ReadCause::Allocation, addr }))?;
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
            .map_err(|error| Error::Read(ReadFailure {
                cause: if verification_failed.get() { ReadCause::Verification }
                    else if error == BlockError::OutOfMemory { ReadCause::Allocation }
                    else if let BlockError::VirtioRead(failure) = error { ReadCause::Driver(failure) }
                    else { ReadCause::Device },
                addr,
            }))?;
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
