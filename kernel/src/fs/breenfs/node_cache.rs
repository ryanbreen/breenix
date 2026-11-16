//! Read-only verified node store (breenfs design sections 1.2 and 2.2).
use super::{
    btree::{Error, NodeStore},
    commit,
    format::{NodeHeader, Superblock, BLOCK_SIZE, NODE_SIZE},
};
use crate::block::{
    cache::{BufferCache, Wait},
    BlockDevice, BlockError,
};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::ops::Deref;

/// A durable mount's node reads. Exact parent-pointer checks remain in btree.
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
}

impl<'a, W: Wait> CachedNodeStore<'a, W> {
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
        }
    }
}

impl<W: Wait> NodeStore for CachedNodeStore<'_, W> {
    fn get(&self, addr: u64) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(NODE_SIZE)
            .map_err(|_| Error::Store)?;
        bytes.resize(NODE_SIZE, 0);
        self.cache
            .read_verified(self.device.as_ref(), addr, &mut bytes, |node| {
                // decode checks the magic and CRC-32C of bytes 12..16383.
                NodeHeader::decode(node).is_ok_and(|header| {
                    header.addr == addr
                        && header.generation > 0
                        && header.generation <= self.generation
                        && header.fs_uuid == self.fs_uuid
                })
            })
            .map_err(|_| Error::Store)?;
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
        for (i, bytes) in bytes.chunks_exact_mut(sector).enumerate() {
            self.device.read_block(start + i as u64, bytes)?;
        }
        Ok(())
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
