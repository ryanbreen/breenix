//! Shared block cache ordering core (cowfs design section 1.2).
//!
//! Admission is FIFO per BlockDevice::identity. A flush takes a place in that
//! queue before waiting for earlier I/O, so later writes cannot pass it while
//! it drains or while the device flush is open. Device calls and sleeping are
//! outside the state lock. Reads use the same queue to keep cached copies coherent.
//!
//! Mount integration, Hold groups, verification, bounded eviction and the kernel
//! BlockedOnIO wait provider belong to the following buffer-cache units.

use crate::block::{BlockDevice, BlockError};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

/// A generation-based sleep/wake interface, supplied by the caller.
///
/// `wait(observed)` must return immediately if a wake happened after `observed`
/// was sampled. Otherwise it sleeps until a wake (spurious wakes are allowed).
/// This closes the predicate-check-to-sleep race without holding the cache lock.
/// The kernel provider must sleep BlockedOnIO and release PROCESS_MANAGER before
/// returning; neither a device call nor this trait runs under the cache lock.
pub trait Wait: Send + Sync {
    fn generation(&self) -> u64;
    fn wait(&self, observed: u64);
    fn wake_all(&self);
}

/// The synchronous group used by ext2. Deferred Hold groups are not exposed yet.
#[derive(Clone, Copy, Debug)]
pub enum WriteGroup {
    WriteThrough,
}

/// Requests sent to the device, including failed requests. The sequence is
/// assigned immediately before calling the driver, from this cache's one clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counter {
    pub count: u64,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub hits: Counter,
    pub misses: Counter,
    pub device_reads: Counter,
    pub device_writes: Counter,
    pub flushes: Counter,
}

#[derive(Default)]
struct Queue {
    next: u64,
    completed: u64,
}

struct State {
    buffers: BTreeMap<(usize, u64), Vec<u8>>,
    devices: BTreeMap<usize, Queue>,
    sequence: u64,
    counters: Counters,
}

/// claim-lint:ok: Caller contract for cache identity and access, docs/fs/cowfs-design.md section 1.2.
/// One cache is shared by its device users; wrappers must preserve identity and
/// all their I/O must pass through it. Identities must remain stable and must not
/// be reused for another device during this cache's lifetime.
pub struct BufferCache<W: Wait> {
    state: Mutex<State>,
    wait: W,
}

impl<W: Wait> BufferCache<W> {
    pub fn new(wait: W) -> Self {
        Self {
            state: Mutex::new(State {
                buffers: BTreeMap::new(),
                devices: BTreeMap::new(),
                sequence: 0,
                counters: Counters::default(),
            }),
            wait,
        }
    }

    // claim-lint:ok: kernel/src/block/cache.rs enter waits after with returns; read/write/flush call the driver outside with.
    // The state lock is short and never held over I/O or a sleep. Masking
    // interrupts prevents a preempting local thread spinning on its owner.
    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        crate::arch_without_interrupts(|| f(&mut self.state.lock()))
    }

    fn enter(&self, device: usize) -> Admission<'_, W> {
        let ticket = self.with(|state| {
            let queue = state.devices.entry(device).or_default();
            let ticket = queue.next;
            queue.next = queue
                .next
                .checked_add(1)
                .expect("block cache ticket exhausted");
            ticket
        });
        loop {
            let observed = self.wait.generation();
            if self.with(|state| state.devices[&device].completed == ticket) {
                return Admission {
                    cache: self,
                    device,
                };
            }
            self.wait.wait(observed);
        }
    }

    fn count(&self, select: impl FnOnce(&mut Counters) -> &mut Counter) {
        self.with(|state| {
            state.sequence = state
                .sequence
                .checked_add(1)
                .expect("block cache sequence exhausted");
            let counter = select(&mut state.counters);
            counter.count += 1;
            counter.sequence = state.sequence;
        });
    }

    fn size(device: &dyn BlockDevice, block: u64, len: usize) -> Result<usize, BlockError> {
        if block >= device.num_blocks() {
            return Err(BlockError::OutOfBounds);
        }
        let size = device.block_size();
        if size == 0 || len < size {
            return Err(BlockError::IoError);
        }
        Ok(size)
    }

    // A copy is optional: memory pressure must not turn completed device I/O
    // into an error. The bound and eviction policy are added before integration.
    fn remember(&self, key: (usize, u64), bytes: &[u8]) {
        let mut copy = Vec::new();
        if copy.try_reserve_exact(bytes.len()).is_ok() {
            copy.extend_from_slice(bytes);
            self.with(|state| state.buffers.insert(key, copy));
        }
    }

    /// Read a native device block, or its clean copy.
    pub fn read(
        &self,
        device: &dyn BlockDevice,
        block: u64,
        bytes: &mut [u8],
    ) -> Result<(), BlockError> {
        let size = Self::size(device, block, bytes.len())?;
        let key = (device.identity(), block);
        let _admission = self.enter(key.0);
        let hit = self.with(|state| {
            if let Some(copy) = state.buffers.get(&key) {
                bytes[..size].copy_from_slice(copy);
                true
            } else {
                false
            }
        });
        if hit {
            self.count(|c| &mut c.hits);
            return Ok(());
        }
        self.count(|c| &mut c.misses);
        self.count(|c| &mut c.device_reads);
        device.read_block(block, &mut bytes[..size])?;
        self.remember(key, &bytes[..size]);
        Ok(())
    }

    /// WriteThrough completes the device write before returning. Invalidate the
    /// old copy first, including on failure: a failed write may have changed media.
    pub fn write(
        &self,
        device: &dyn BlockDevice,
        group: WriteGroup,
        block: u64,
        bytes: &[u8],
    ) -> Result<(), BlockError> {
        let size = Self::size(device, block, bytes.len())?;
        let key = (device.identity(), block);
        let _admission = self.enter(key.0);
        match group {
            WriteGroup::WriteThrough => {
                self.with(|state| state.buffers.remove(&key));
                self.count(|c| &mut c.device_writes);
                device.write_block(block, &bytes[..size])?;
                self.remember(key, &bytes[..size]);
                Ok(())
            }
        }
    }

    /// Reserve the barrier before draining earlier requests, then wait for the
    /// synchronous device flush. Admission drops on errors as well as success.
    pub fn flush(&self, device: &dyn BlockDevice) -> Result<(), BlockError> {
        let _admission = self.enter(device.identity());
        self.count(|c| &mut c.flushes);
        device.flush()
    }

    pub fn counters(&self) -> Counters {
        self.with(|state| state.counters)
    }
}

struct Admission<'a, W: Wait> {
    cache: &'a BufferCache<W>,
    device: usize,
}

impl<W: Wait> Drop for Admission<'_, W> {
    fn drop(&mut self) {
        self.cache
            .with(|state| state.devices.get_mut(&self.device).unwrap().completed += 1);
        self.cache.wait.wake_all();
    }
}
