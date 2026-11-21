//! Shared block cache ordering core (breenfs design section 1.2).
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

    /// One bounded wait for already-issued I/O. Providers without a timed
    /// wait can decline it; publication has a path that needs no cache space.
    fn wait_for_pressure(&self, _observed: u64) -> PressureWait {
        PressureWait::Unavailable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureWait {
    Progress,
    TimedOut,
    Unavailable,
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
    ready_device: Option<Arc<dyn BlockDevice>>,
    // Serialized by this device's cache admission. The publisher consumes
    // errors from writes that another publisher completed on its behalf.
    assisted_error: Mutex<Option<BlockError>>,
}

impl WriteGroup {
    pub fn hold(device: &dyn BlockDevice, request_commit: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::Hold(Arc::new(HoldGroup {
            device: device.identity(),
            request_commit,
            requested: AtomicBool::new(false),
            ready_device: None,
            assisted_error: Mutex::new(None),
        }))
    }

    /// Stage frozen copy-on-write data that may be submitted by a peer.
    /// The owner still controls the durability barriers and publication.
    pub fn ready_hold(device: Arc<dyn BlockDevice>, request_commit: Arc<dyn Fn() + Send + Sync>) -> Self {
        let Self::Hold(mut group) = Self::hold(device.as_ref(), request_commit) else {
            unreachable!("Hold constructor");
        };
        Arc::get_mut(&mut group).expect("new Hold group").ready_device = Some(device);
        Self::Hold(group)
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
// claim-lint:ok: kernel/src/memory/heap.rs HEAP_SIZE and QEMU_HEAP_SIZE.
// Every supported kernel has at least a 64 MiB heap. Cache allocations share
// that heap with the rest of the kernel, irrespective of physical RAM size.
const MAX_BYTES: usize = 64 * 1024 * 1024 / 8;
/// Conservative per-snapshot allowance for Arc/Buffer, both BTreeMap indexes,
/// their sparsely occupied nodes, and allocator alignment. Detached pins keep
/// this charge too; index removal therefore cannot prematurely free the budget.
pub const BUFFER_OVERHEAD: usize = 1280;
/// The most bytes one coalesced device call carries (E24 ledger 368, #3040).
/// submit and read_blocks join LBA-contiguous blocks into runs of at most this
/// size; a driver may still split a run at its own transfer limit.
pub const RUN_BYTES: usize = 64 * 1024;
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

/// claim-lint:ok: Caller contract for cache identity and access, docs/fs/breenfs-design.md section 1.2.
/// One cache is shared by its device users; wrappers must preserve identity and
/// all their I/O must pass through it. Identities must remain stable and must not
/// be reused for another device during this cache's lifetime.
#[cfg(test)]
pub mod measurement {
    extern crate std;
    std::thread_local! { static DISCARD: core::cell::Cell<u64> = const { core::cell::Cell::new(0) }; }
    pub fn discard_visits() -> u64 { DISCARD.with(|n| n.get()) }
    pub(super) fn discard_visit() { DISCARD.with(|n| n.set(n.get()+1)); }
}

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
        use core::ops::Bound::{Excluded, Unbounded};
        let mut start = Unbounded;
        while let Some((key, holder)) = self.with(|state| {
            state.buffers.range((start, Unbounded)).find_map(|(&key, entry)| {
                if let BufferState::Dirty { group, .. } = &entry.status {
                    if !group.requested.swap(true, Ordering::Relaxed) {
                        return Some((key, group.clone()));
                    }
                }
                None
            })
        }) {
            start = Excluded(key);
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
        self.count_by(1, select);
    }

    fn count_by(&self, n: usize, select: impl FnOnce(&mut Counters) -> &mut Counter) {
        if n == 0 {
            return;
        }
        self.with(|state| {
            state.sequence = state
                .sequence
                .checked_add(1)
                .expect("block cache sequence exhausted");
            let counter = select(&mut state.counters);
            counter.count += n as u64;
            counter.sequence = state.sequence;
        });
    }

    /// Native blocks in one device call: RUN_BYTES, and at least one block.
    fn run_limit(size: usize) -> usize {
        RUN_BYTES.checked_div(size).unwrap_or(1).max(1)
    }

    /// One device request for `count` consecutive blocks. A single block keeps
    /// the backend's write_block path; device_writes counts requests.
    fn device_write(
        &self,
        device: &dyn BlockDevice,
        start: u64,
        count: usize,
        bytes: &[u8],
    ) -> Result<(), BlockError> {
        self.count(|c| &mut c.pending_writes);
        let result = if count == 1 {
            device.write_block(start, bytes)
        } else {
            device.write_blocks(start, count, bytes)
        };
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

    /// Read `count` consecutive native blocks under one admission. Resident
    /// Clean and Dirty copies are served from memory; each LBA-contiguous run
    /// of misses, up to RUN_BYTES, is one device call (E24 ledger 368, #3040).
    /// A failed run leaves its blocks unpublished and ends the read with the
    /// device's error, as a failed single-block miss does. A Failed write
    /// buffer returns IoError without device I/O, as in read_verified.
    pub fn read_blocks(
        &self,
        device: &dyn BlockDevice,
        start: u64,
        count: usize,
        bytes: &mut [u8],
    ) -> Result<(), BlockError> {
        let size = device.block_size();
        let total = count.checked_mul(size).ok_or(BlockError::IoError)?;
        if size == 0 || bytes.len() < total {
            return Err(BlockError::IoError);
        }
        if start
            .checked_add(count as u64)
            .is_none_or(|end| end > device.num_blocks())
        {
            return Err(BlockError::OutOfBounds);
        }
        let identity = device.identity();
        let limit = Self::run_limit(size);
        let admission = self.enter(identity);
        let mut pressure = false;
        let mut result = Ok(());
        let mut i = 0;
        while i < count {
            // Copy the resident prefix, then measure the miss run behind it.
            let scan = self.with(|state| {
                let mut hits = 0;
                while i + hits < count {
                    let key = (identity, start + (i + hits) as u64);
                    state.touch(key);
                    match state.buffers.get(&key).map(|entry| &entry.status) {
                        Some(BufferState::Clean { buffer, .. } | BufferState::Dirty { buffer, .. }) => {
                            bytes[(i + hits) * size..][..size].copy_from_slice(buffer);
                            hits += 1;
                        }
                        Some(BufferState::Failed) => return Err(BlockError::IoError),
                        _ => break,
                    }
                }
                let first = i + hits;
                let mut misses = 0;
                while first + misses < count
                    && misses < limit
                    && !matches!(
                        state
                            .buffers
                            .get(&(identity, start + (first + misses) as u64))
                            .map(|entry| &entry.status),
                        Some(
                            BufferState::Clean { .. }
                                | BufferState::Dirty { .. }
                                | BufferState::Failed
                        )
                    )
                {
                    misses += 1;
                }
                Ok((hits, misses))
            });
            let (hits, misses) = match scan {
                Ok(scan) => scan,
                Err(error) => {
                    result = Err(error);
                    break;
                }
            };
            self.count_by(hits, |c| &mut c.hits);
            i += hits;
            if misses == 0 {
                continue;
            }
            self.count_by(misses, |c| &mut c.misses);
            let block = start + i as u64;
            let run = &mut bytes[i * size..(i + misses) * size];
            let read = if misses == 1 {
                device.read_block(block, run)
            } else {
                device.read_blocks(block, misses, run)
            };
            self.count(|c| &mut c.device_reads);
            if let Err(error) = read {
                result = Err(error);
                break;
            }
            for k in 0..misses {
                let key = (identity, block + k as u64);
                let charge = self.begin(key, size);
                pressure |= charge.is_none();
                self.finish(key, &bytes[(i + k) * size..][..size], charge, Ok(()));
            }
            i += misses;
        }
        drop(admission);
        if pressure {
            self.request_commits();
        }
        result
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
        // Only completed device reads reach verification. Return the original
        // block error on failure, including a refusal before submission.
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
                let result = self.device_write(device, block, 1, &bytes[..size]);
                self.finish(key, &bytes[..size], charge, result);
                (result, pressure)
            }
            WriteGroup::Hold(group) => {
                if group.device != key.0 {
                    return Err(BlockError::IoError);
                }
                let previous = *group.assisted_error.lock();
                let result = match previous {
                    Some(error) => Err(error),
                    None => self.stage(key, group, &bytes[..size]),
                };
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
        let assisted_error = *group.assisted_error.lock();
        if let Some(error) = assisted_error {
            return Err(error);
        }
        self.submit_admitted(device, group)
    }

    /// Under pressure, help one ready peer without queueing behind that peer.
    /// The caller holds no cache admission. The helper takes only the peer's
    /// admission and leaves its mount state and barriers to the peer.
    pub fn relieve_publication_pressure(&self, publisher: &dyn BlockDevice) -> PressureWait {
        let identity = publisher.identity();
        let observed = self.wait.generation();
        let (ready, in_flight) = self.with(|state| {
            let mut in_flight = false;
            for (&(device, _), entry) in &state.buffers {
                if device == identity {
                    continue;
                }
                match &entry.status {
                    BufferState::Dirty { group, .. } if group.ready_device.is_some() => {
                        let queue = state.devices.get_mut(&device).expect("staged cache device");
                        if queue.next == queue.completed {
                            queue.next = queue.next.checked_add(1).expect("block cache ticket exhausted");
                            return (Some(group.clone()), in_flight);
                        }
                    }
                    BufferState::InFlight => in_flight = true,
                    _ => {}
                }
            }
            (None, in_flight)
        });
        if let Some(group) = ready {
            let _admission = Admission { cache: self, device: group.device };
            let device = group.ready_device.as_ref().expect("ready cache group");
            if let Err(error) = self.submit_admitted(device.as_ref(), &group) {
                *group.assisted_error.lock() = Some(error);
            }
            PressureWait::Progress
        } else if in_flight {
            self.wait.wait_for_pressure(observed)
        } else {
            PressureWait::Unavailable
        }
    }

    fn submit_admitted(&self, device: &dyn BlockDevice, group: &HoldGroup) -> Result<(), BlockError> {
        // Staged buffers are device.block_size() bytes each (stage via size()).
        let size = device.block_size();
        let limit = Self::run_limit(size);
        let belongs = |entry: &Entry| {
            matches!(&entry.status, BufferState::Dirty { group: owner, .. } if core::ptr::eq(owner.as_ref(), group))
        };
        let workspace = self.with(|state| {
            let mut buffers = Vec::new();
            let count = state
                .buffers
                .iter()
                .filter(|(key, entry)| key.0 == group.device && belongs(entry))
                .count();
            buffers
                .try_reserve_exact(count)
                .map_err(|_| BlockError::OutOfMemory)?;
            // One run's contiguous copy, reserved before any entry changes state.
            let mut scratch = Vec::new();
            if count > 1 {
                scratch
                    .try_reserve_exact(count.min(limit) * size)
                    .map_err(|_| BlockError::OutOfMemory)?;
            }
            for (&key, entry) in state.buffers.iter_mut() {
                if key.0 == group.device && belongs(entry) {
                    if let BufferState::Dirty { buffer, .. } = &entry.status {
                        buffers.push((key, buffer.clone()));
                    }
                    entry.status = BufferState::InFlight;
                }
            }
            Ok::<_, BlockError>((buffers, scratch))
        });
        group.requested.store(false, Ordering::Relaxed);
        // A full heap can also refuse the coalescing workspace. Submit one
        // existing snapshot at a time without allocating that workspace.
        let Ok((buffers, mut scratch)) = workspace else {
            return self.submit_one_at_a_time(device, group.device, belongs);
        };
        // Keys iterate in LBA order, so each run is a maximal contiguous slice
        // of at most `limit` blocks and takes one device call. A failed run
        // marks each of its entries Failed; later runs are still written.
        let mut result = Ok(());
        let mut rest = &buffers[..];
        while let Some((first, _)) = rest.first() {
            let len = 1 + rest
                .windows(2)
                .take(limit - 1)
                .take_while(|pair| pair[0].0 .1.checked_add(1) == Some(pair[1].0 .1))
                .count();
            let (run, tail) = rest.split_at(len);
            rest = tail;
            let written = if len == 1 {
                self.device_write(device, first.1, 1, &run[0].1)
            } else {
                scratch.clear();
                for (_, buffer) in run {
                    scratch.extend_from_slice(buffer);
                }
                self.device_write(device, first.1, len, &scratch)
            };
            self.finish_submitted(run, written);
            result = result.and(written);
        }
        result
    }

    fn submit_one_at_a_time(
        &self,
        device: &dyn BlockDevice,
        identity: usize,
        belongs: impl Fn(&Entry) -> bool,
    ) -> Result<(), BlockError> {
        let mut result = Ok(());
        let mut start = (identity, 0);
        loop {
            let next = self.with(|state| {
                let (&key, entry) = state.buffers.range_mut(start..=(identity, u64::MAX))
                    .find(|(_, entry)| belongs(entry))?;
                let BufferState::Dirty { buffer, .. } =
                    core::mem::replace(&mut entry.status, BufferState::InFlight)
                else { unreachable!("selected dirty buffer") };
                Some((key, buffer))
            });
            let Some(next) = next else { break };
            let block = next.0.1;
            let written = self.device_write(device, block, 1, &next.1);
            self.with(|state| {
                if written.is_ok() {
                    state.remove(next.0);
                } else {
                    state.buffers.get_mut(&next.0).expect("submitted cache buffer").status =
                        BufferState::Failed;
                }
            });
            result = result.and(written);
            let Some(block) = block.checked_add(1) else { break };
            start = (identity, block);
        }
        result
    }

    fn finish_submitted(&self, buffers: &[(Key, Arc<Buffer>)], result: Result<(), BlockError>) {
        self.with(|state| {
            for (key, buffer) in buffers {
                let entry = state.buffers.get_mut(key).expect("submitted cache buffer");
                if result.is_ok() {
                    entry.status = BufferState::Clean { buffer: buffer.clone(), used: 0 };
                    state.touch(*key);
                } else {
                    entry.status = BufferState::Failed;
                }
            }
        });
    }

    /// Submit a prepared range directly when no cache capacity is reclaimable.
    /// Keep device ordering and retire old cache copies under that admission.
    pub fn write_uncached(&self, device: &dyn BlockDevice, start: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let size = device.block_size();
        if size == 0 || bytes.is_empty() || bytes.len() % size != 0 {
            return Err(BlockError::IoError);
        }
        let count = bytes.len() / size;
        if start.checked_add(count as u64).is_none_or(|end| end > device.num_blocks()) {
            return Err(BlockError::OutOfBounds);
        }
        let identity = device.identity();
        let _admission = self.enter(identity);
        self.with(|state| {
            let end = start + count as u64;
            let keys = (identity, start)..(identity, end);
            if state.buffers.range(keys).any(|(_, entry)| matches!(entry.status, BufferState::Dirty { .. })) {
                return Err(BlockError::IoError);
            }
            for block in start..end {
                state.remove((identity, block));
            }
            Ok(())
        })?;
        self.device_write(device, start, count, bytes)
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
            use core::ops::Bound::{Excluded, Included};
            let mut lower = Included((identity, 0));
            let upper = Included((identity, u64::MAX));
            while let Some((&key, entry)) = state.buffers.range((lower, upper)).next() {
                #[cfg(test)] measurement::discard_visit();
                let clean = matches!(entry.status, BufferState::Clean { .. });
                lower = Excluded(key);
                if clean {
                    #[cfg(test)] measurement::discard_visit();
                    // remove also retires this entry's exact LRU key. Neither
                    // index needs a scan over other device identities.
                    state.remove(key);
                }
            }
        });
    }

    /// Invalidate a changed physical range without inspecting unrelated cache
    /// entries. In-flight and transaction-owned entries retain their custody.
    pub fn discard_clean_range(&self, device: &dyn BlockDevice, start: u64, count: u64) {
        self.with(|state| {
            for block in start..start.saturating_add(count) {
                #[cfg(test)] measurement::discard_visit();
                let key = (device.identity(), block);
                if matches!(state.buffers.get(&key), Some(Entry { status: BufferState::Clean { .. }, .. })) {
                    state.remove(key);
                }
            }
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
