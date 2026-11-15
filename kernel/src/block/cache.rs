//! Shared block cache ordering core (cowfs design section 1.2).
//!
//! Admission is FIFO per BlockDevice::identity. A flush takes a place in that
//! queue before waiting for earlier I/O, so later writes cannot pass it while
//! it drains or while the device flush is open. Device calls and sleeping are
//! outside the state lock. Reads use the same queue to keep cached copies coherent.
//!
//! Mount integration, Hold groups and the kernel BlockedOnIO wait provider
//! belong to the following buffer-cache units.

use crate::block::{BlockDevice, BlockError};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::Deref;
use core::sync::atomic::{AtomicUsize, Ordering};
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

const MIN_BYTES: usize = 4 * 1024 * 1024;
const MAX_BYTES: usize = 1024 * 1024 * 1024;
type Key = (usize, u64);

/// Memory policy from cowfs design section 1.2, in bytes.
pub fn bound_for_memory(memory: usize) -> usize {
    (memory / 8).clamp(MIN_BYTES, MAX_BYTES)
}

// The charge outlives the cache key when a caller retains a pinned snapshot.
struct Charge {
    resident: Arc<AtomicUsize>,
    size: usize,
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.resident.fetch_sub(self.size, Ordering::Relaxed);
    }
}

/// An immutable buffer snapshot. Holding its Arc pins the bytes against eviction.
pub struct Buffer {
    bytes: Vec<u8>,
    charge: Charge,
}

impl Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes[..self.charge.size]
    }
}

enum BufferState {
    Clean { buffer: Arc<Buffer>, used: u64 },
    InFlight,
    Failed,
}

struct Entry {
    status: BufferState,
    generation: u64,
    waiters: usize,
}

struct State {
    buffers: BTreeMap<Key, Entry>,
    next_generation: u64,
    lru: BTreeMap<u64, Key>,
    recency: u64,
    devices: BTreeMap<usize, Queue>,
    sequence: u64,
    counters: Counters,
}

impl State {
    fn remove(&mut self, key: Key) {
        if let Some(Entry {
            status: BufferState::Clean { used, .. },
            ..
        }) = self.buffers.remove(&key)
        {
            self.lru.remove(&used);
        }
    }

    fn touch(&mut self, key: Key) {
        if let Some(Entry {
            status: BufferState::Clean { used, .. },
            ..
        }) = self.buffers.get_mut(&key)
        {
            self.lru.remove(used);
            self.recency = self
                .recency
                .checked_add(1)
                .expect("cache recency exhausted");
            *used = self.recency;
            self.lru.insert(*used, key);
        }
    }

    fn reserve(
        &mut self,
        size: usize,
        bound: usize,
        resident: &Arc<AtomicUsize>,
    ) -> Option<Charge> {
        if size > bound {
            return None;
        }
        while resident.load(Ordering::Relaxed) > bound - size {
            let victim = self
                .lru
                .values()
                .find(|key| {
                    matches!(self.buffers.get(key), Some(Entry {
                        status: BufferState::Clean { buffer, .. }, waiters: 0, ..
                    }) if Arc::strong_count(buffer) == 1)
                })
                .copied()?;
            self.remove(victim);
        }
        resident.fetch_add(size, Ordering::Relaxed);
        Some(Charge {
            resident: resident.clone(),
            size,
        })
    }
}

/// claim-lint:ok: Caller contract for cache identity and access, docs/fs/cowfs-design.md section 1.2.
/// One cache is shared by its device users; wrappers must preserve identity and
/// all their I/O must pass through it. Identities must remain stable and must not
/// be reused for another device during this cache's lifetime.
pub struct BufferCache<W: Wait> {
    state: Mutex<State>,
    wait: W,
    bound: usize,
    resident: Arc<AtomicUsize>,
}

impl<W: Wait> BufferCache<W> {
    pub fn new(wait: W) -> Self {
        Self::with_capacity(wait, bound_for_memory(0))
    }

    /// Use bound_for_memory(total_bytes) for the system cache. Explicit smaller
    /// budgets also support constrained users and host execution of eviction.
    pub fn with_capacity(wait: W, bound: usize) -> Self {
        Self {
            state: Mutex::new(State {
                buffers: BTreeMap::new(),
                next_generation: 0,
                lru: BTreeMap::new(),
                recency: 0,
                devices: BTreeMap::new(),
                sequence: 0,
                counters: Counters::default(),
            }),
            wait,
            bound,
            resident: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Includes in-flight reservations and snapshots detached by writes.
    pub fn resident_bytes(&self) -> usize {
        self.resident.load(Ordering::Relaxed)
    }

    /// Pin a clean resident snapshot without issuing I/O. This is also an LRU
    /// access. A later write may replace the key but cannot mutate this snapshot.
    pub fn pin(&self, device: &dyn BlockDevice, block: u64) -> Option<Arc<Buffer>> {
        self.with(|state| {
            state.touch((device.identity(), block));
            match state.buffers.get(&(device.identity(), block)) {
                Some(Entry {
                    status: BufferState::Clean { buffer, .. },
                    ..
                }) => Some(buffer.clone()),
                _ => None,
            }
        })
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

    fn begin(&self, key: Key, size: usize) -> Option<Charge> {
        self.with(|state| {
            state.remove(key);
            let charge = state.reserve(size, self.bound, &self.resident);
            state.next_generation = state
                .next_generation
                .checked_add(1)
                .expect("cache generation exhausted");
            state.buffers.insert(
                key,
                Entry {
                    status: BufferState::InFlight,
                    generation: state.next_generation,
                    waiters: 0,
                },
            );
            charge
        })
    }

    // Admission still serializes the device when pressure prevents retaining a
    // copy. A completed read/write can succeed without a cache allocation.
    fn finish(
        &self,
        key: Key,
        bytes: &[u8],
        charge: Option<Charge>,
        result: Result<(), BlockError>,
    ) {
        let mut copy = Vec::new();
        let buffer =
            if result.is_ok() && charge.is_some() && copy.try_reserve_exact(bytes.len()).is_ok() {
                copy.extend_from_slice(bytes);
                Some(Arc::new(Buffer {
                    bytes: copy,
                    charge: charge.unwrap(),
                }))
            } else {
                None
            };
        self.with(|state| {
            let entry = state.buffers.get_mut(&key).expect("admitted cache request");
            match buffer {
                Some(buffer) => {
                    entry.status = BufferState::Clean { buffer, used: 0 };
                    state.touch(key);
                }
                None => {
                    // A failed request publishes no bytes, including on verify
                    // refusal. A later queued reader must issue its own read.
                    entry.status = BufferState::Failed;
                    state.remove(key);
                }
            }
        });
    }

    /// Read a native device block, or its clean copy.
    pub fn read(
        &self,
        device: &dyn BlockDevice,
        block: u64,
        bytes: &mut [u8],
    ) -> Result<(), BlockError> {
        self.read_verified(device, block, bytes, |_| true)
    }

    /// Verify the owner's native-block bytes on a miss before publishing Clean.
    /// The callback runs outside the state lock. Owners sharing an identity must
    /// agree on verification; hits reuse the previously accepted copy.
    pub fn read_verified(
        &self,
        device: &dyn BlockDevice,
        block: u64,
        bytes: &mut [u8],
        verify: impl FnOnce(&[u8]) -> bool,
    ) -> Result<(), BlockError> {
        let size = Self::size(device, block, bytes.len())?;
        let key = (device.identity(), block);
        // Pin this key while queued, including the InFlight -> Clean handoff.
        // The generation prevents a write ahead of us from transferring that pin
        // onto replacement bytes. Device admission still defines read/write order.
        let generation = self.with(|state| {
            state.buffers.get_mut(&key).map(|entry| {
                entry.waiters += 1;
                entry.generation
            })
        });
        let _admission = self.enter(key.0);
        let hit = self.with(|state| {
            if let Some(entry) = state.buffers.get_mut(&key) {
                if generation == Some(entry.generation) {
                    entry.waiters -= 1;
                }
            }
            state.touch(key);
            if let Some(Entry {
                status: BufferState::Clean { buffer, .. },
                ..
            }) = state.buffers.get(&key)
            {
                bytes[..size].copy_from_slice(buffer);
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
        let charge = self.begin(key, size);
        self.count(|c| &mut c.device_reads);
        let result = device.read_block(block, &mut bytes[..size]).and_then(|()| {
            if verify(&bytes[..size]) {
                Ok(())
            } else {
                Err(BlockError::IoError)
            }
        });
        self.finish(key, &bytes[..size], charge, result);
        result
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
                let charge = self.begin(key, size);
                self.count(|c| &mut c.device_writes);
                let result = device.write_block(block, &bytes[..size]);
                self.finish(key, &bytes[..size], charge, result);
                result
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
