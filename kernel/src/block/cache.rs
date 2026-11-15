//! Shared block cache ordering core (cowfs design section 1.2).
//!
//! Admission is FIFO per BlockDevice::identity. A flush takes a place in that
//! queue before waiting for earlier I/O, so later writes cannot pass it while
//! it drains or while the device flush is open. Device calls and sleeping are
//! outside the state lock. Reads use the same queue to keep cached copies coherent.
//!
//! The kernel wait provider and system-wide instance live in cache_kernel.rs.

use crate::block::{BlockDevice, BlockError};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::Deref;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

/// A generation-based sleep/wake interface, supplied by the caller.
///
/// `wait(observed)` must return immediately if a wake happened after `observed`
/// was sampled. Otherwise it sleeps until a wake (spurious wakes are allowed).
/// This closes the predicate-check-to-sleep race without holding the cache lock.
/// The kernel provider sleeps BlockedOnIO and returns with scheduler guards
/// released; neither a device call nor this trait runs under the cache lock.
pub trait Wait: Send + Sync {
    fn generation(&self) -> u64;
    fn wait(&self, observed: u64);
    fn wake_all(&self);
}

/// WriteThrough writes synchronously; Hold retains bytes until explicit submit.
#[derive(Clone)]
pub enum WriteGroup {
    WriteThrough,
    Hold(Arc<HoldGroup>),
}

/// A device-bound transaction owner. The callback asks its commit worker to run;
/// it runs outside cache locks and device admission and may reenter the cache.
pub struct HoldGroup {
    device: usize,
    request_commit: Arc<dyn Fn() + Send + Sync>,
    requested: AtomicBool,
}

impl WriteGroup {
    pub fn hold(device: &dyn BlockDevice, request_commit: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::Hold(Arc::new(HoldGroup {
            device: device.identity(),
            request_commit,
            requested: AtomicBool::new(false),
        }))
    }
}

/// Observable state of the bytes at a device/block key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferStatus {
    Clean,
    Dirty,
    InFlight,
    Failed,
}

/// Completion of a submitted group. BlockDevice is synchronous, so submit drives
/// the device writes and returns a completed Batch. No flush is implied: the
/// owner waits on the batch, then calls flush for its durability boundary.
pub struct Batch {
    result: Result<(), BlockError>,
}

impl Batch {
    pub fn wait(&self) -> Result<(), BlockError> {
        self.result
    }
}

/// A count and its last change on this cache's shared sequence clock.
/// Device request counters advance on completion, including failed requests.
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
    pub pending_writes: Counter,
    pub flushes: Counter,
}

#[derive(Default)]
struct Queue {
    next: u64,
    completed: u64,
}

const MIN_BYTES: usize = 4 * 1024 * 1024;
// Every supported kernel has at least a 64 MiB heap. Cache allocations share
// that heap with the rest of the kernel, irrespective of physical RAM size.
const MAX_BYTES: usize = 64 * 1024 * 1024 / 8;
/// Conservative per-snapshot allowance for Arc/Buffer, both BTreeMap indexes,
/// their sparsely occupied nodes, and allocator alignment. Detached pins keep
/// this charge too; index removal therefore cannot prematurely free the budget.
pub const BUFFER_OVERHEAD: usize = 1280;
type Key = (usize, u64);

/// RAM policy, additionally capped to one eighth of the smallest kernel heap.
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
        &self.bytes[..self.charge.size - BUFFER_OVERHEAD]
    }
}

enum BufferState {
    Clean {
        buffer: Arc<Buffer>,
        used: u64,
    },
    Dirty {
        buffer: Arc<Buffer>,
        group: Arc<HoldGroup>,
    },
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
        let size = size.checked_add(BUFFER_OVERHEAD)?;
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

    /// Charged heap budget: data and metadata, including in-flight reservations
    /// and snapshots detached by writes.
    pub fn resident_bytes(&self) -> usize {
        self.resident.load(Ordering::Relaxed)
    }

    /// Pin a resident snapshot without issuing I/O. Clean bytes get an LRU
    /// access. A later write may replace the key but cannot mutate this snapshot.
    pub fn pin(&self, device: &dyn BlockDevice, block: u64) -> Option<Arc<Buffer>> {
        self.with(|state| {
            state.touch((device.identity(), block));
            match state.buffers.get(&(device.identity(), block)) {
                Some(Entry {
                    status: BufferState::Clean { buffer, .. } | BufferState::Dirty { buffer, .. },
                    ..
                }) => Some(buffer.clone()),
                _ => None,
            }
        })
    }

    pub fn status(&self, device: &dyn BlockDevice, block: u64) -> Option<BufferStatus> {
        self.with(|state| {
            state
                .buffers
                .get(&(device.identity(), block))
                .map(|entry| match entry.status {
                    BufferState::Clean { .. } => BufferStatus::Clean,
                    BufferState::Dirty { .. } => BufferStatus::Dirty,
                    BufferState::InFlight => BufferStatus::InFlight,
                    BufferState::Failed => BufferStatus::Failed,
                })
        })
    }

    fn request_commits(&self) {
        let holders = self.with(|state| {
            state
                .buffers
                .values()
                .filter_map(|entry| match &entry.status {
                    BufferState::Dirty { group, .. }
                        if !group.requested.swap(true, Ordering::Relaxed) =>
                    {
                        Some(group.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        for holder in holders {
            (holder.request_commit)();
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

    fn device_write(
        &self,
        device: &dyn BlockDevice,
        block: u64,
        bytes: &[u8],
    ) -> Result<(), BlockError> {
        self.count(|c| &mut c.pending_writes);
        let result = device.write_block(block, bytes);
        self.with(|state| {
            state.sequence = state
                .sequence
                .checked_add(1)
                .expect("block cache sequence exhausted");
            state.counters.device_writes.count += 1;
            state.counters.device_writes.sequence = state.sequence;
            state.counters.pending_writes.count -= 1;
            state.counters.pending_writes.sequence = state.sequence;
        });
        result
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
            let charge = state.reserve(size, self.bound, &self.resident)?;
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
            Some(charge)
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
        // No reservation means uncached I/O: do not allocate an index entry or
        // a snapshot. Device admission still serializes the operation.
        let Some(charge) = charge else { return };
        let mut copy = Vec::new();
        let buffer =
            if result.is_ok() && copy.try_reserve_exact(bytes.len()).is_ok() {
                copy.extend_from_slice(bytes);
                Some(Arc::new(Buffer {
                    bytes: copy,
                    charge,
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
            match state.buffers.get(&key).map(|entry| &entry.status) {
                Some(BufferState::Clean { buffer, .. } | BufferState::Dirty { buffer, .. }) => {
                    bytes[..size].copy_from_slice(buffer);
                    Ok(true)
                }
                Some(BufferState::Failed) => Err(BlockError::IoError),
                _ => Ok(false),
            }
        })?;
        if hit {
            self.count(|c| &mut c.hits);
            return Ok(());
        }
        self.count(|c| &mut c.misses);
        let charge = self.begin(key, size);
        let result = device.read_block(block, &mut bytes[..size]);
        self.count(|c| &mut c.device_reads);
        let result = result.and_then(|()| {
            if verify(&bytes[..size]) {
                Ok(())
            } else {
                Err(BlockError::IoError)
            }
        });
        let pressure = charge.is_none();
        self.finish(key, &bytes[..size], charge, result);
        drop(_admission);
        if pressure {
            self.request_commits();
        }
        result
    }

    /// WriteThrough completes the device write before returning. Hold publishes
    /// Dirty(group) bytes without writing media. A full dirty cache asks its
    /// holders for a commit and returns OutOfMemory; the caller may retry after
    /// that commit. Existing dirty bytes survive a refused write.
    pub fn write(
        &self,
        device: &dyn BlockDevice,
        group: WriteGroup,
        block: u64,
        bytes: &[u8],
    ) -> Result<(), BlockError> {
        let size = Self::size(device, block, bytes.len())?;
        let key = (device.identity(), block);
        let admission = self.enter(key.0);
        let (result, pressure) = match group {
            WriteGroup::WriteThrough => {
                // A different writer cannot discard an unsubmitted transaction.
                if self.with(|state| {
                    matches!(
                        state.buffers.get(&key),
                        Some(Entry {
                            status: BufferState::Dirty { .. },
                            ..
                        })
                    )
                }) {
                    return Err(BlockError::IoError);
                }
                let charge = self.begin(key, size);
                let pressure = charge.is_none();
                let result = self.device_write(device, block, &bytes[..size]);
                self.finish(key, &bytes[..size], charge, result);
                (result, pressure)
            }
            WriteGroup::Hold(group) => {
                let result = self.stage(key, group, &bytes[..size]);
                (result, result == Err(BlockError::OutOfMemory))
            }
        };
        drop(admission);
        if pressure {
            self.request_commits();
        }
        result
    }

    fn stage(&self, key: Key, group: Arc<HoldGroup>, bytes: &[u8]) -> Result<(), BlockError> {
        if group.device != key.0 {
            return Err(BlockError::IoError);
        }
        // A private dirty snapshot can be overwritten without another charge.
        let reused = self.with(|state| {
            if let Some(Entry {
                status:
                    BufferState::Dirty {
                        buffer,
                        group: owner,
                    },
                ..
            }) = state.buffers.get_mut(&key)
            {
                if !Arc::ptr_eq(owner, &group) {
                    return Err(BlockError::IoError);
                }
                if let Some(buffer) = Arc::get_mut(buffer) {
                    buffer.bytes.copy_from_slice(bytes);
                    return Ok(true);
                }
            }
            Ok(false)
        })?;
        if reused {
            return Ok(());
        }
        // Evict before allocating the new payload, including for Hold writes.
        let charge = self.with(|state| state.reserve(bytes.len(), self.bound, &self.resident))
            .ok_or(BlockError::OutOfMemory)?;
        let mut copy = Vec::new();
        copy.try_reserve_exact(bytes.len())
            .map_err(|_| BlockError::OutOfMemory)?;
        copy.extend_from_slice(bytes);
        self.with(|state| {
            state.remove(key);
            state.next_generation = state
                .next_generation
                .checked_add(1)
                .expect("cache generation exhausted");
            state.buffers.insert(
                key,
                Entry {
                    status: BufferState::Dirty {
                        buffer: Arc::new(Buffer {
                            bytes: copy,
                            charge,
                        }),
                        group,
                    },
                    generation: state.next_generation,
                    waiters: 0,
                },
            );
            Ok(())
        })
    }

    /// Submit this device's dirty group as one FIFO operation. A flush queued
    /// behind it cannot split the batch. A failed block becomes Failed and the
    /// remaining blocks are still completed before the Batch reports IoError.
    pub fn submit(&self, device: &dyn BlockDevice, group: &WriteGroup) -> Batch {
        Batch {
            result: self.submit_group(device, group),
        }
    }

    fn submit_group(&self, device: &dyn BlockDevice, group: &WriteGroup) -> Result<(), BlockError> {
        let group = match group {
            WriteGroup::Hold(group) if group.device == device.identity() => group,
            _ => return Err(BlockError::IoError),
        };
        let _admission = self.enter(device.identity());
        let buffers = self.with(|state| {
            let mut buffers = Vec::new();
            let belongs = |entry: &Entry| {
                matches!(&entry.status,
                BufferState::Dirty { group: owner, .. } if Arc::ptr_eq(owner, group))
            };
            let count = state
                .buffers
                .iter()
                .filter(|(key, entry)| key.0 == group.device && belongs(entry))
                .count();
            buffers
                .try_reserve_exact(count)
                .map_err(|_| BlockError::OutOfMemory)?;
            for (&key, entry) in state.buffers.iter_mut() {
                if key.0 == group.device && belongs(entry) {
                    if let BufferState::Dirty { buffer, .. } = &entry.status {
                        buffers.push((key, buffer.clone()));
                    }
                    entry.status = BufferState::InFlight;
                }
            }
            group.requested.store(false, Ordering::Relaxed);
            Ok::<_, BlockError>(buffers)
        })?;
        let mut result = Ok(());
        for (key, buffer) in buffers {
            let written = self.device_write(device, key.1, &buffer);
            self.with(|state| {
                let entry = state.buffers.get_mut(&key).expect("submitted cache buffer");
                if written.is_ok() {
                    entry.status = BufferState::Clean { buffer, used: 0 };
                    state.touch(key);
                } else {
                    entry.status = BufferState::Failed;
                }
            });
            if written.is_err() {
                result = Err(BlockError::IoError);
            }
        }
        result
    }

    /// Reserve the barrier before draining earlier requests, then wait for the
    /// synchronous device flush. Admission drops on errors as well as success.
    pub fn flush(&self, device: &dyn BlockDevice) -> Result<(), BlockError> {
        let _admission = self.enter(device.identity());
        let result = device.flush();
        self.count(|c| &mut c.flushes);
        result
    }

    /// Retire clean copies when a mounted backend is dropped. This prevents a
    /// later backend allocated at the same identity from seeing old media bytes.
    /// In-flight and transaction-owned buffers keep their completion state.
    pub fn discard_clean(&self, device: &dyn BlockDevice) {
        let identity = device.identity();
        self.with(|state| {
            state.lru.retain(|_, key| key.0 != identity);
            state.buffers.retain(|key, entry| {
                key.0 != identity || !matches!(entry.status, BufferState::Clean { .. })
            });
        });
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
