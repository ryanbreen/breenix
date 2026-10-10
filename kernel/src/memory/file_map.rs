//! File mappings with shared dirty-page custody.
//!
//! Each live inode carries a cache of resident file pages and a reverse map of
//! the bindings (one per file VMA) that may map them. Page-table entries are
//! installed only by faults, from the cache. Everything that runs after the
//! disk has changed only removes entries, so no step there can fail for want
//! of memory once the new size is on disk.
//!
//! Invariant: for every binding and every file page index `i` in its range
//! below the mapped page count, `i` is resident in the cache, or `i` is
//! poisoned (its bytes are unknown until re-read), or the file's bytes for `i`
//! are zero (a page made valid by extension and not written since). A fault
//! therefore never needs I/O: it maps the resident page, inserts and maps a
//! zero page, or raises SIGBUS for a poisoned page.
//!
//! Allocation: every hook that runs after the disk changes (commit, failed
//! write invalidation, the EOF transition and revocation) uses capacity the
//! write or resize reserved before it, so none of them allocates. The cache
//! and the poison set are sorted vectors for that reason: inserting into
//! reserved capacity, draining a range and truncating never allocate.
//!
//! Lock order: ext2 mount guard, PROCESS_MANAGER, live-inode table, `MapState`,
//! frame ledger and allocator. Signal-frame delivery also takes `MapState`
//! under PROCESS_MANAGER with interrupts masked. The inode table may take
//! `MapState` without PM.
//! No disk I/O runs under PROCESS_MANAGER or `MapState`. No path
//! takes an ext2 guard with PROCESS_MANAGER held: faults never touch the
//! filesystem, and mmap releases PROCESS_MANAGER before taking the mount guard.

#[cfg(not(target_arch = "x86_64"))]
use super::arch_stub::{Page, PageTableFlags, PhysFrame, Size4KiB, VirtAddr};
use crate::fs::ext2::{live_inode::FileHandle, Ext2Fs};
use crate::memory::{
    process_memory::{ProcessPageTable, Revoked, COW_FLAG},
    vma::{MmapFlags, Protection, Vma},
};
use crate::process::{Process, ProcessId, ProcessManager};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::{
    structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB},
    VirtAddr,
};

const PAGE_SIZE: u64 = 4096;
/// File pages one PROCESS_MANAGER section revokes, all in one binding. A
/// section also passes over at most this many bindings that miss the range.
const REVOKE_WINDOW: u64 = 64;
/// Unbound cache pages one eviction pass retires.
pub(crate) const EVICT_BUDGET: usize = 64;
/// Pages copied by one writeback batch.
const WRITEBACK_PAGES: usize = 64;

pub(crate) fn writeback_buffer() -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    let capacity = WRITEBACK_PAGES * PAGE_SIZE as usize;
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| "Out of memory for writeback")?;
    bytes.resize(capacity, 0);
    Ok(bytes)
}

/// Synchronous writeback walks the requested range once, reusing a bounded
/// copy buffer and releasing the mount guard between batches. A live writer
/// can keep earlier pages dirty without causing this call to loop over them.
pub(crate) fn sync_range(
    handle: &FileHandle,
    first: u64,
    last: u64,
) -> Result<(), &'static str> {
    let mut bytes = writeback_buffer()?;
    sync_range_with_buffer(handle, first, last, &mut bytes)
}

pub(crate) fn sync_range_with_buffer(
    handle: &FileHandle,
    mut first: u64,
    last: u64,
    bytes: &mut [u8],
) -> Result<(), &'static str> {
    let mut wrote = false;
    loop {
        let next = {
            let mut guard = crate::fs::ext2::write_mount(handle.object.mount)?;
            let fs = guard.as_mut().ok_or("Missing mount")?;
            let ino = handle.verify(fs)?;
            fs.check_shrink(ino)?;
            let next =
                handle
                    .object
                    .map
                    .writeback(fs, ino, first, last, WRITEBACK_PAGES, bytes)?;
            if next.is_none() && !wrote {
                fs.sync()?;
            }
            next
        };
        let Some(next) = next else {
            return Ok(());
        };
        wrote = true;
        if next >= last {
            return Ok(());
        }
        first = next;
    }
}

const SIGBUS: u32 = crate::signal::constants::SIGBUS;
const SIGSEGV: u32 = crate::signal::constants::SIGSEGV;

static NEXT_BINDING: AtomicU64 = AtomicU64::new(1);

fn pages(size: u64) -> u64 {
    size.div_ceil(PAGE_SIZE)
}

#[derive(Debug)]
pub struct MapState {
    inner: Mutex<MapInner>,
    eviction_pending: AtomicBool,
    resident: AtomicBool,
    dirty: AtomicBool,
    writeback_pending: AtomicBool,
    writeback_requests: AtomicU64,
}

#[derive(Debug)]
struct MapInner {
    /// The EOF the bindings honour. Equal to the inode's published size
    /// except while a size transition is between publication and this update.
    mapped_size: u64,
    pages: PageVec<CachePage>,
    /// Bound page indices whose file bytes are unknown after a failed mutation.
    poisoned: PageVec<()>,
    /// Sorted by id: ids are issued under this lock, in increasing order.
    bindings: Vec<BindingRec>,
    writable_shared: usize,
    writeback_cursor: u64,
    /// The request generation the traversal at `writeback_cursor` began
    /// under. A traversal that ends under a newer one starts over.
    writeback_epoch: u64,
}

/// Values keyed by file page index, sorted by index.
#[derive(Debug)]
struct PageVec<T> {
    entries: Vec<(u64, T)>,
}

impl<T> PageVec<T> {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn find(&self, index: u64) -> Result<usize, usize> {
        self.entries.binary_search_by_key(&index, |(key, _)| *key)
    }

    fn contains(&self, index: u64) -> bool {
        self.find(index).is_ok()
    }

    fn get_mut(&mut self, index: u64) -> Option<&mut T> {
        self.find(index).ok().map(|slot| &mut self.entries[slot].1)
    }

    fn get(&self, index: u64) -> Option<&T> {
        self.find(index).ok().map(|slot| &self.entries[slot].1)
    }

    /// Slot of the first entry at or after `index`.
    fn lower_bound(&self, index: u64) -> usize {
        self.entries.partition_point(|(key, _)| *key < index)
    }

    fn try_reserve(&mut self, additional: usize) -> Result<(), &'static str> {
        self.entries
            .try_reserve(additional)
            .map_err(|_| "Out of memory for file mapping")
    }

    fn spare(&self) -> usize {
        self.entries.capacity() - self.entries.len()
    }

    /// Insert, reserving room first; fails with the map unchanged. An index
    /// already present keeps its value.
    fn try_insert(&mut self, index: u64, value: T) -> Result<(), &'static str> {
        if let Err(slot) = self.find(index) {
            self.try_reserve(1)?;
            self.entries.insert(slot, (index, value));
        }
        Ok(())
    }

    fn remove(&mut self, index: u64) -> Option<T> {
        self.find(index)
            .ok()
            .map(|slot| self.entries.remove(slot).1)
    }

    /// Entries with index in `[first, last)`.
    fn range(&self, first: u64, last: u64) -> &[(u64, T)] {
        let low = self.lower_bound(first);
        let high = self.lower_bound(last).max(low);
        &self.entries[low..high]
    }

    /// Drop the entries at and after `index`, in place.
    fn truncate_from(&mut self, index: u64) {
        let slot = self.lower_bound(index);
        self.entries.truncate(slot);
    }

    fn retain(&mut self, mut keep: impl FnMut(u64) -> bool) {
        self.entries.retain(|(index, _)| keep(*index));
    }

    fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.entries.iter().map(|(index, _)| *index)
    }

    /// Merge `new`, sorted and unique, using reserved capacity. Indices
    /// already present keep their value.
    fn merge_reserved(&mut self, mut new: Vec<(u64, T)>) {
        new.retain(|(index, _)| !self.contains(*index));
        debug_assert!(self.spare() >= new.len());
        self.entries.append(&mut new);
        self.entries.sort_unstable_by_key(|(index, _)| *index);
    }
}

/// Call `run` for each maximal run `[low, high)` of page indices in
/// `[first, last)` that some binding covers, in increasing order.
fn bound_runs(bindings: &[BindingRec], first: u64, last: u64, mut run: impl FnMut(u64, u64)) {
    let mut cursor = first;
    while cursor < last {
        // The run starts at the lowest covered index at or after the cursor
        // and extends over every binding that overlaps or abuts it.
        let Some(low) = bindings
            .iter()
            .filter(|rec| rec.pgoff + rec.pages > cursor && rec.pgoff < last)
            .map(|rec| rec.pgoff.max(cursor))
            .min()
        else {
            return;
        };
        let mut high = low;
        loop {
            let reach = bindings
                .iter()
                .filter(|rec| rec.pgoff <= high && rec.pgoff + rec.pages > high)
                .map(|rec| rec.pgoff + rec.pages)
                .max();
            match reach {
                Some(end) => high = end.min(last),
                None => break,
            }
            if high == last {
                break;
            }
        }
        run(low, high);
        cursor = high;
    }
}

#[derive(Debug)]
struct BindingRec {
    id: u64,
    pid: ProcessId,
    /// `ProcessPageTable::address_space` of the table the binding maps into.
    space: u64,
    va: u64,
    pages: u64,
    pgoff: u64,
    prot: Protection,
    shared: bool,
    may_write: bool,
    /// Private copies whose entries mprotect removed, sorted by page index.
    /// The next fault on that index maps the copy again.
    parked: Vec<(u64, CachePage)>,
}

impl BindingRec {
    fn writable_shared(&self) -> bool {
        self.shared && self.prot.contains(Protection::WRITE)
    }

    fn covers(&self, index: u64) -> bool {
        index >= self.pgoff && index - self.pgoff < self.pages
    }

    fn page(&self, index: u64) -> Page<Size4KiB> {
        Page::containing_address(VirtAddr::new(self.va + (index - self.pgoff) * PAGE_SIZE))
    }

    fn index(&self, address: u64) -> u64 {
        self.pgoff + (address - self.va) / PAGE_SIZE
    }
}

/// One frame reference held by the cache or by a parked private copy.
#[derive(Debug)]
struct CachePage {
    frame: PhysFrame,
    dirty: bool,
    gen: u64,
}

impl CachePage {
    fn retain(frame: PhysFrame) -> Result<Self, &'static str> {
        super::frame_allocator::acquire_leaf_mapping(frame)?;
        Ok(Self {
            frame,
            dirty: false,
            gen: 0,
        })
    }

    fn allocate() -> Result<Self, &'static str> {
        let frame =
            super::frame_allocator::allocate_frame().ok_or("Out of memory for file page")?;
        match Self::retain(frame) {
            Ok(page) => {
                // SAFETY: the frame was just allocated and is reached only here.
                unsafe { core::ptr::write_bytes(page.ptr(), 0, PAGE_SIZE as usize) };
                Ok(page)
            }
            Err(error) => {
                super::frame_allocator::deallocate_frame(frame);
                Err(error)
            }
        }
    }

    fn ptr(&self) -> *mut u8 {
        (super::physical_memory_offset().as_u64() + self.frame.start_address().as_u64()) as *mut u8
    }

    /// Zero the bytes at and after `offset` within the page.
    fn zero_from(&self, offset: u64) {
        // SAFETY: the cache owns a reference to the frame; offset < PAGE_SIZE.
        unsafe {
            core::ptr::write_bytes(
                self.ptr().add(offset as usize),
                0,
                (PAGE_SIZE - offset) as usize,
            )
        };
    }
}

impl Drop for CachePage {
    fn drop(&mut self) {
        if super::frame_metadata::frame_decref(self.frame) {
            super::frame_allocator::deallocate_leaf_frame(self.frame);
        }
    }
}

/// The file side of one file VMA. Exec and exit transfer the VMA to its
/// retired page table. A committed exec releases its old bindings; exit
/// retains them until root retirement. Drop queues work without filesystem I/O.
#[derive(Debug)]
pub struct Binding {
    id: u64,
    handle: FileHandle,
}

impl Drop for Binding {
    fn drop(&mut self) {
        let map = &self.handle.object.map;
        let (removed, cached) = {
            let mut inner = map.inner.lock();
            let removed = inner
                .bindings
                .iter()
                .position(|rec| rec.id == self.id)
                .map(|slot| inner.bindings.remove(slot));
            if removed.as_ref().is_some_and(BindingRec::writable_shared) {
                inner.writable_shared -= 1;
            }
            (
                removed,
                !inner.pages.is_empty() || !inner.poisoned.is_empty(),
            )
        };
        // Pages this binding covered may now be unbound: ask the ext2 service
        // to retire them. Parked copies release their frames here, outside
        // the map lock.
        if cached {
            map.request_writeback();
            map.eviction_pending.store(true, Ordering::Release);
            crate::fs::ext2::request_map_eviction();
        }
        drop(removed);
    }
}

/// A new binding id. Called under the map lock of the inode the binding
/// joins, so each inode's bindings are pushed in increasing id order.
fn next_binding() -> u64 {
    NEXT_BINDING.fetch_add(1, Ordering::Relaxed)
}

#[cfg(target_arch = "aarch64")]
fn sync_executable(frame: PhysFrame) {
    let address = super::physical_memory_offset().as_u64() + frame.start_address().as_u64();
    // SAFETY: `address` is the HHDM alias of a frame this map holds.
    unsafe { crate::arch_impl::aarch64::cache::sync_user_page(address) };
}

#[cfg(target_arch = "x86_64")]
fn sync_executable(_: PhysFrame) {}

impl MapInner {
    fn bound(&self, index: u64) -> bool {
        self.bindings.iter().any(|rec| rec.covers(index))
    }

    /// Cache bytes changed through the HHDM alias: make them visible to
    /// instruction fetch through any executable binding.
    fn sync_if_executable(&self, index: u64) {
        if let Some(page) = self.pages.get(index) {
            if self
                .bindings
                .iter()
                .any(|rec| rec.prot.contains(Protection::EXEC) && rec.covers(index))
            {
                sync_executable(page.frame);
            }
        }
    }

    /// Visit, in increasing order, the pages of `[first, last)` that a
    /// binding covers and the cache neither holds nor has poisoned, and
    /// return how many there are.
    fn holes(&self, first: u64, last: u64, mut visit: impl FnMut(u64)) -> usize {
        let mut holes = 0usize;
        bound_runs(&self.bindings, first, last, |low, high| {
            for index in low..high {
                if !self.pages.contains(index) && !self.poisoned.contains(index) {
                    visit(index);
                    holes += 1;
                }
            }
        });
        holes
    }

    /// Room in the poison set for every bound page of `[first, last)` that
    /// is not yet poisoned. Bindings made later without the filesystem guard
    /// come from fork and VMA splits, which cover only pages already bound,
    /// so the room still suffices when `poison` runs after the disk changes.
    fn reserve_poison(&mut self, first: u64, last: u64) -> Result<(), &'static str> {
        let mut wanted = 0usize;
        bound_runs(&self.bindings, first, last, |low, high| {
            wanted += (high - low) as usize - self.poisoned.range(low, high).len();
        });
        if wanted > self.poisoned.spare() {
            self.poisoned.try_reserve(wanted)?;
        }
        Ok(())
    }

    /// Poison the bound pages of `[first, last)`, in room `reserve_poison`
    /// made.
    fn poison(&mut self, first: u64, last: u64) {
        let MapInner {
            poisoned,
            bindings,
            pages,
            ..
        } = self;
        let sorted = poisoned.len();
        bound_runs(bindings, first, last, |low, high| {
            for index in low..high {
                let present = poisoned.entries[..sorted]
                    .binary_search_by_key(&index, |(key, _)| *key)
                    .is_ok();
                if !present
                    && !pages.get(index).is_some_and(|page| page.dirty)
                    && poisoned.spare() > 0
                {
                    poisoned.entries.push((index, ()));
                }
            }
        });
        poisoned.entries.sort_unstable_by_key(|(index, _)| *index);
    }

    fn rec_mut(&mut self, id: u64) -> Option<&mut BindingRec> {
        self.bindings.iter_mut().find(|rec| rec.id == id)
    }
}

impl MapState {
    pub fn new(size: u64) -> Self {
        Self {
            inner: Mutex::new(MapInner {
                mapped_size: size,
                pages: PageVec::new(),
                poisoned: PageVec::new(),
                bindings: Vec::new(),
                writable_shared: 0,
                writeback_cursor: 0,
                writeback_epoch: 0,
            }),
            eviction_pending: AtomicBool::new(false),
            resident: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            writeback_pending: AtomicBool::new(false),
            writeback_requests: AtomicU64::new(0),
        }
    }

    fn refresh(&self, inner: &MapInner) {
        self.resident.store(
            !inner.pages.is_empty() || !inner.poisoned.is_empty(),
            Ordering::Release,
        );
        self.dirty.store(
            inner.pages.entries.iter().any(|(_, page)| page.dirty),
            Ordering::Release,
        );
    }

    pub(crate) fn nonempty(&self) -> bool {
        let inner = self.inner.lock();
        !inner.pages.is_empty() || !inner.poisoned.is_empty() || !inner.bindings.is_empty()
    }

    pub(crate) fn has_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    pub(crate) fn request_writeback(&self) {
        if self.has_dirty() {
            self.writeback_requests.fetch_add(1, Ordering::AcqRel);
            self.writeback_pending.store(true, Ordering::Release);
            crate::fs::ext2::request_map_eviction();
        }
    }

    pub(crate) fn writeback_pending(&self) -> bool {
        self.writeback_pending.load(Ordering::Acquire)
    }

    /// Overlay under the filesystem guard, which prevents cache removal and
    /// disk transitions. The map lock serializes fault insertion and lookup.
    pub(crate) fn overlay(&self, offset: u64, bytes: &mut [u8]) -> Result<(), &'static str> {
        if bytes.is_empty() || !self.resident.load(Ordering::Acquire) {
            return Ok(());
        }
        let end = offset + bytes.len() as u64;
        let inner = self.inner.lock();
        if !inner
            .poisoned
            .range(offset / PAGE_SIZE, pages(end))
            .is_empty()
        {
            return Err("Unknown mapped file bytes");
        }
        for (index, page) in inner.pages.range(offset / PAGE_SIZE, pages(end)) {
            let start = offset.max(index * PAGE_SIZE);
            let stop = end.min((index + 1) * PAGE_SIZE);
            // SAFETY: the filesystem guard pins the frame; both slices are in bounds.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    page.ptr().add((start % PAGE_SIZE) as usize),
                    bytes.as_mut_ptr().add((start - offset) as usize),
                    (stop - start) as usize,
                );
            }
        }
        Ok(())
    }

    /// One bounded snapshot, contiguous writes, then a device flush. Errors
    /// leave the snapshot dirty. No PTEs are changed here.
    pub(crate) fn writeback(
        &self,
        fs: &mut Ext2Fs,
        ino: u32,
        first: u64,
        last: u64,
        limit: usize,
        bytes: &mut [u8],
    ) -> Result<Option<u64>, &'static str> {
        let limit = limit
            .min(WRITEBACK_PAGES)
            .min(bytes.len() / PAGE_SIZE as usize);
        let result = self.writeback_snapshot(fs, ino, first, last, limit, bytes);
        if result.is_err() {
            self.request_writeback();
        }
        result
    }

    fn writeback_snapshot(
        &self,
        fs: &mut Ext2Fs,
        ino: u32,
        first: u64,
        last: u64,
        limit: usize,
        bytes: &mut [u8],
    ) -> Result<Option<u64>, &'static str> {
        let (snapshot, writable) = {
            let inner = self.inner.lock();
            let count = inner
                .pages
                .range(first, last)
                .iter()
                .filter(|(_, page)| page.dirty)
                .take(limit)
                .count();
            let mut snapshot = Vec::new();
            snapshot
                .try_reserve(count)
                .map_err(|_| "Out of memory for writeback")?;
            for (index, page) in inner
                .pages
                .range(first, last)
                .iter()
                .filter(|(_, page)| page.dirty)
                .take(limit)
            {
                snapshot.push((*index, page.frame, page.gen));
            }
            (snapshot, inner.writable_shared)
        };
        if snapshot.is_empty() {
            return Ok(None);
        }
        fs.check_shrink(ino)?;
        let size = fs.read_inode(ino)?.size();
        let mut cursor = 0;
        while cursor < snapshot.len() {
            let start = cursor;
            cursor += 1;
            while cursor < snapshot.len() && snapshot[cursor].0 == snapshot[cursor - 1].0 + 1 {
                cursor += 1;
            }
            let offset = snapshot[start].0 * PAGE_SIZE;
            let length =
                ((cursor - start) * PAGE_SIZE as usize).min(size.saturating_sub(offset) as usize);
            if length == 0 {
                continue;
            }
            for (slot, (_, frame, _)) in snapshot[start..cursor].iter().enumerate() {
                let at = slot * PAGE_SIZE as usize;
                if at >= length {
                    break;
                }
                let ptr = (super::physical_memory_offset().as_u64()
                    + frame.start_address().as_u64()) as *const u8;
                // SAFETY: the filesystem write guard excludes frame removal.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        ptr,
                        bytes.as_mut_ptr().add(at),
                        (PAGE_SIZE as usize).min(length - at),
                    );
                }
            }
            let written = fs.write_mapped_range(ino, offset, &bytes[..length])?;
            if written != length {
                return Err("Short mapped writeback");
            }
        }
        fs.sync()?;
        let mut inner = self.inner.lock();
        if writable == 0 && inner.writable_shared == 0 {
            for (index, _, gen) in &snapshot {
                if let Some(page) = inner.pages.get_mut(*index) {
                    if page.gen == *gen {
                        page.dirty = false;
                    }
                }
            }
        }
        self.refresh(&inner);
        Ok(snapshot.last().map(|(index, _, _)| index + 1))
    }

    /// The service processes at most the remaining page budget and resumes
    /// after its snapshot, even when a live writer keeps those pages dirty.
    /// A request that arrives mid-traversal may concern pages already
    /// passed, so the traversal that ends under it is followed by a full one.
    pub(crate) fn service_writeback(
        &self,
        fs: &mut Ext2Fs,
        ino: u32,
        budget: &mut usize,
    ) -> Result<bool, &'static str> {
        if !self.writeback_pending() {
            return Ok(false);
        }
        if *budget == 0 {
            return Ok(true);
        }
        let request = self.writeback_requests.load(Ordering::Acquire);
        let (first, count) = {
            let mut inner = self.inner.lock();
            let first = inner.writeback_cursor;
            if first == 0 {
                inner.writeback_epoch = request;
            }
            let count = inner
                .pages
                .range(first, u64::MAX)
                .iter()
                .filter(|(_, page)| page.dirty)
                .take(*budget)
                .count();
            (first, count)
        };
        let mut bytes = writeback_buffer()?;
        let next = self.writeback(fs, ino, first, u64::MAX, *budget, &mut bytes)?;
        *budget -= count;
        let mut inner = self.inner.lock();
        let next = next.unwrap_or(u64::MAX);
        let more = inner
            .pages
            .range(next, u64::MAX)
            .iter()
            .any(|(_, page)| page.dirty);
        inner.writeback_cursor = if more { next } else { 0 };
        let epoch = inner.writeback_epoch;
        drop(inner);
        self.writeback_pending.store(more, Ordering::Release);
        // Publish idle before checking the request generation. A request on
        // either side of this check either changes the generation or sets the
        // flag after it; neither wake can be lost.
        if self.writeback_requests.load(Ordering::Acquire) != epoch {
            self.writeback_pending.store(true, Ordering::Release);
            return Ok(true);
        }
        Ok(more)
    }

    /// The caller observed `size` under the filesystem guard. Without a
    /// binding nothing maps the cache, so a size this state never saw only
    /// discards cached pages.
    pub(crate) fn resync_unbound(&self, size: u64) {
        let stale = {
            let mut inner = self.inner.lock();
            if inner.mapped_size == size
                || !inner.bindings.is_empty()
                || inner.pages.entries.iter().any(|(_, page)| page.dirty)
            {
                return;
            }
            inner.mapped_size = size;
            self.resident.store(false, Ordering::Release);
            inner.poisoned = PageVec::new();
            core::mem::replace(&mut inner.pages, PageVec::new())
        };
        drop(stale);
    }

    /// mmap failed after `populate`: pages it read that no binding covers
    /// are left for the ext2 service to retire.
    pub(crate) fn abandon_population(&self) {
        let unbound = {
            let inner = self.inner.lock();
            let unbound = inner.pages.keys().any(|index| !inner.bound(index));
            unbound
        };
        if unbound {
            self.eviction_pending.store(true, Ordering::Release);
            crate::fs::ext2::request_map_eviction();
        }
    }

    pub(crate) fn eviction_pending(&self) -> bool {
        self.eviction_pending.load(Ordering::Acquire)
    }

    /// Retire cache pages no binding covers. The filesystem write guard
    /// excludes population and every mutation hook.
    pub(crate) fn evict(&self, budget: &mut usize) -> bool {
        let mut inner = self.inner.lock();
        let MapInner {
            pages,
            poisoned,
            bindings,
            ..
        } = &mut *inner;
        let unbound = |index: u64| !bindings.iter().any(|rec| rec.covers(index));
        poisoned.retain(|index| !unbound(index));
        // Retired frames are released in place: the frame allocator follows
        // the map lock in the lock order.
        pages.entries.retain(|(index, page)| {
            if *budget == 0 || !unbound(*index) || page.dirty {
                return true;
            }
            *budget -= 1;
            false
        });
        let more = pages
            .entries
            .iter()
            .any(|(index, page)| unbound(*index) && !page.dirty);
        self.resident
            .store(!pages.is_empty() || !poisoned.is_empty(), Ordering::Release);
        self.eviction_pending.store(more, Ordering::Release);
        more
    }

    /// Before a write of `len` bytes at `offset`, while the disk is
    /// unchanged: put zero frames in the cache for the bound holes the write
    /// fills, so `commit_write` only copies bytes, and reserve the poison
    /// room `invalidate` needs if the write fails. A hole's file bytes are
    /// zero (see the module invariant), so its zero frame is already
    /// correct; past EOF no fault maps it.
    pub(crate) fn prepare_write(&self, offset: u64, len: usize) -> Result<(), &'static str> {
        let end = offset.checked_add(len as u64).ok_or("File too large")?;
        let (first, last) = (offset / PAGE_SIZE, pages(end));
        let wanted = {
            let mut inner = self.inner.lock();
            if inner.bindings.is_empty() {
                return Ok(());
            }
            inner.reserve_poison(first, last)?;
            inner.holes(first, last, |_| {})
        };
        if wanted == 0 {
            return Ok(());
        }
        let mut prepared = Vec::new();
        prepared
            .try_reserve(wanted)
            .map_err(|_| "Out of memory for file mapping")?;
        for _ in 0..wanted {
            prepared.push((0, CachePage::allocate()?));
        }
        let mut inner = self.inner.lock();
        inner.pages.try_reserve(prepared.len())?;
        // The lock was released while frames were allocated. Faults may have
        // filled holes meanwhile, and nothing makes new ones, so the holes
        // left are at most `wanted`; the frames left over are released.
        let mut filled = 0;
        inner.holes(first, last, |index| {
            if let Some(slot) = prepared.get_mut(filled) {
                slot.0 = index;
                filled += 1;
            }
        });
        prepared.truncate(filled);
        inner.pages.merge_reserved(prepared);
        self.resident.store(
            !inner.pages.is_empty() || !inner.poisoned.is_empty(),
            Ordering::Release,
        );
        Ok(())
    }

    /// After a successful write of `bytes` at `offset`: the cache takes the
    /// written bytes, so every binding that maps a cache page sees them.
    /// Copies into pages already in the cache; allocates nothing.
    pub(crate) fn commit_write(&self, offset: u64, bytes: &[u8]) {
        let end = offset + bytes.len() as u64;
        let inner = self.inner.lock();
        for (index, page) in inner.pages.range(offset / PAGE_SIZE, pages(end)) {
            let start = offset.max(index * PAGE_SIZE);
            let stop = end.min((index + 1) * PAGE_SIZE);
            // SAFETY: the cache holds the frame; the range lies within the
            // page and within `bytes`.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr().add((start - offset) as usize),
                    page.ptr().add((start % PAGE_SIZE) as usize),
                    (stop - start) as usize,
                );
            }
            inner.sync_if_executable(*index);
        }
    }

    /// Before a resize from `before` to `length`: reserve the poison room
    /// `invalidate` needs for the boundary page if the resize fails.
    pub(crate) fn prepare_resize(&self, before: u64, length: u64) -> Result<(), &'static str> {
        let boundary = before.min(length);
        self.inner
            .lock()
            .reserve_poison(boundary / PAGE_SIZE, pages(boundary.saturating_add(1)))
    }

    /// A mutation failed after it may have changed the bytes of `[offset,
    /// offset + len)`. Poison and revoke clean pages whose disk bytes are
    /// unknown. Dirty pages hold mapping stores in custody and remain mapped
    /// for retry. Private copies stay. Uses reserved room; allocates nothing.
    pub(crate) fn invalidate(&self, offset: u64, len: u64) {
        let first = offset / PAGE_SIZE;
        let last = pages(offset.saturating_add(len));
        let cached = {
            let mut inner = self.inner.lock();
            // Dirty cache pages are authoritative even if the disk mutation
            // failed; poison only pages that lack dirty custody.
            inner.poison(first, last);
            self.resident.store(
                !inner.pages.is_empty() || !inner.poisoned.is_empty(),
                Ordering::Release,
            );
            !inner.pages.range(first, last).is_empty()
        };
        if cached {
            self.revoke(first, last, true);
            let mut inner = self.inner.lock();
            inner
                .pages
                .entries
                .retain(|(index, page)| *index < first || *index >= last || page.dirty);
            self.refresh(&inner);
        }
    }

    /// Bring the cache to `size`, the EOF ext2 published, on success or
    /// failure of the mutation. Growth only moves the EOF: the new pages are
    /// holes, which faults fill with zero pages. A shrink drops the cache,
    /// poison and parked copies past the new EOF, in place, and removes every
    /// entry there. Allocates nothing.
    pub(crate) fn transition(&self, size: u64) {
        let old = {
            let mut inner = self.inner.lock();
            let old = inner.mapped_size;
            if size == old {
                return;
            }
            inner.mapped_size = size;
            let boundary = old.min(size);
            if boundary % PAGE_SIZE != 0 {
                if let Some(page) = inner.pages.get(boundary / PAGE_SIZE) {
                    page.zero_from(boundary % PAGE_SIZE);
                }
                inner.sync_if_executable(boundary / PAGE_SIZE);
            }
            if size > old {
                return;
            }
            let keep = pages(size);
            for rec in inner.bindings.iter_mut() {
                let cut = rec.parked.partition_point(|(index, _)| *index < keep);
                rec.parked.truncate(cut);
            }
            // Entries hold their own frame references, so the cache's may go
            // first. Bytes past EOF read as zero once the file grows again.
            inner.pages.truncate_from(keep);
            inner.poisoned.truncate_from(keep);
            self.refresh(&inner);
            old
        };
        self.revoke(pages(size), pages(old), false);
    }

    /// Re-read poisoned pages that a binding covers and that lie within EOF.
    /// Runs under the filesystem write guard, after a mutation. A page that
    /// still cannot be read, or for which no memory is left, stays poisoned
    /// and faults on it raise SIGBUS.
    pub(crate) fn repair(&self, fs: &Ext2Fs, ino: u32) {
        {
            let mut inner = self.inner.lock();
            if inner.poisoned.is_empty() {
                return;
            }
            let MapInner {
                poisoned, bindings, ..
            } = &mut *inner;
            poisoned.retain(|index| bindings.iter().any(|rec| rec.covers(index)));
            if poisoned.is_empty() {
                return;
            }
        }
        let Ok(inode) = fs.read_inode(ino) else {
            return;
        };
        let mut cursor = 0;
        loop {
            let next = {
                let inner = self.inner.lock();
                let valid = pages(inner.mapped_size);
                inner
                    .poisoned
                    .range(cursor, valid)
                    .first()
                    .map(|(index, _)| *index)
            };
            let Some(index) = next else {
                return;
            };
            cursor = index + 1;
            let Ok(page) = CachePage::allocate() else {
                return;
            };
            let Ok(bytes) = fs.read_file_range(&inode, index * PAGE_SIZE, PAGE_SIZE as usize)
            else {
                continue;
            };
            // SAFETY: a fresh cache frame; `bytes` is at most one page.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), page.ptr(), bytes.len()) };
            let mut inner = self.inner.lock();
            if inner.poisoned.contains(index)
                && !inner.pages.contains(index)
                && inner.pages.try_insert(index, page).is_ok()
            {
                self.resident.store(true, Ordering::Release);
                inner.poisoned.remove(index);
            }
        }
    }

    /// Remove the entries for file pages `[first, last)` in every binding
    /// whose address space is live. With `aliases`, only entries that map
    /// the cache page at their index. Bindings are visited in id order, one
    /// at a time: a PROCESS_MANAGER section revokes at most `REVOKE_WINDOW`
    /// pages of one binding and passes over at most `REVOKE_WINDOW` bindings
    /// that miss the range. A binding created between sections has a larger
    /// id than the cursor, so a later section covers it. Allocates nothing.
    fn revoke(&self, first: u64, last: u64, aliases: bool) {
        // The next section resumes at binding `id`, file page `resume`.
        let (mut id, mut resume) = (0u64, first);
        loop {
            let mut guard = crate::process::manager();
            let Some(manager) = guard.as_mut() else {
                return;
            };
            let inner = self.inner.lock();
            let mut slot = inner.bindings.partition_point(|rec| rec.id < id);
            let mut passed = 0;
            let (rec, low, high) = loop {
                let Some(rec) = inner.bindings.get(slot) else {
                    return;
                };
                let from = if rec.id == id { resume } else { first };
                let (low, high) = (from.max(rec.pgoff), last.min(rec.pgoff + rec.pages));
                if low < high {
                    break (rec, low, high);
                }
                slot += 1;
                passed += 1;
                if passed == REVOKE_WINDOW {
                    break (rec, high, high);
                }
            };
            let stop = high.min(low + REVOKE_WINDOW);
            if low < stop {
                if let Some(pt) = live_table(manager, rec) {
                    let end = rec.page(stop - 1).start_address().as_u64() + PAGE_SIZE;
                    let mut from = rec.page(low).start_address().as_u64();
                    while let Some(page) = pt.next_mapped_page(from, end) {
                        from = page.start_address().as_u64() + PAGE_SIZE;
                        if aliases {
                            let index = rec.index(page.start_address().as_u64());
                            if inner.pages.get(index).is_some_and(|page| page.dirty) {
                                continue;
                            }
                            let mapped = pt.get_page_info(page).map(|(frame, _)| frame);
                            let cached = inner.pages.get(index).map(|page| page.frame);
                            if mapped.is_none() || cached != mapped {
                                continue;
                            }
                        }
                        revoke_entry(pt, page);
                    }
                }
            }
            if stop < high {
                (id, resume) = (rec.id, stop);
            } else {
                (id, resume) = (rec.id + 1, first);
            }
        }
    }
}

/// The table a binding maps into, if it is still its process's live table.
/// A binding whose process exec'd or exited no longer matches: nothing runs
/// in its old table, and the table holds its own frame references.
fn live_table<'a>(
    manager: &'a mut ProcessManager,
    rec: &BindingRec,
) -> Option<&'a mut ProcessPageTable> {
    manager
        .get_process_mut(rec.pid)?
        .page_table
        .as_deref_mut()
        .filter(|pt| pt.address_space() == rec.space)
}

/// Remove one entry. Returns false when its descriptor and custody record
/// disagreed; the descriptor is cleared and flushed either way. The flush
/// reaches every online CPU (`memory::tlb::flush_page`): ARM64 broadcasts in
/// hardware, x86 shoots down by NMI.
fn revoke_entry(pt: &mut ProcessPageTable, page: Page<Size4KiB>) -> bool {
    match pt.revoke_page(page) {
        Ok(Revoked::Absent) => true,
        Ok(Revoked::Leaf(leaf)) => {
            leaf.flush().release();
            true
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// mmap, munmap, mprotect, fork
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MapError {
    NoMemory,
    Io,
}

/// Read the pages of `[first, first + count)` below EOF that the cache does
/// not hold. Runs under the mount's read guard, with no other lock held.
pub(crate) fn populate(
    handle: &FileHandle,
    fs: &Ext2Fs,
    first: u64,
    count: u64,
) -> Result<(), MapError> {
    let ino = handle.verify(fs).map_err(|_| MapError::Io)?;
    let map = &handle.object.map;
    let size = handle.object.size.load(Ordering::Acquire);
    let last = first.saturating_add(count).min(pages(size));
    let wanted = {
        let inner = map.inner.lock();
        (first..last)
            .filter(|index| !inner.pages.contains(*index))
            .count()
    };
    if wanted == 0 {
        return Ok(());
    }
    let inode = fs.read_inode(ino).map_err(|_| MapError::Io)?;
    let mut read = Vec::new();
    read.try_reserve(wanted).map_err(|_| MapError::NoMemory)?;
    for index in first..last {
        if map.inner.lock().pages.contains(index) {
            continue;
        }
        if read.len() == read.capacity() {
            read.try_reserve(1).map_err(|_| MapError::NoMemory)?;
        }
        let page = CachePage::allocate().map_err(|_| MapError::NoMemory)?;
        let bytes = fs
            .read_file_range(&inode, index * PAGE_SIZE, PAGE_SIZE as usize)
            .map_err(|_| MapError::Io)?;
        // SAFETY: a fresh cache frame; `bytes` is at most one page.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), page.ptr(), bytes.len()) };
        read.push((index, page));
    }
    let mut inner = map.inner.lock();
    inner
        .pages
        .try_reserve(read.len())
        .map_err(|_| MapError::NoMemory)?;
    for (index, _) in &read {
        inner.poisoned.remove(*index);
    }
    inner.pages.merge_reserved(read);
    map.resident.store(
        !inner.pages.is_empty() || !inner.poisoned.is_empty(),
        Ordering::Release,
    );
    Ok(())
}

/// Register a binding for a new file VMA. PROCESS_MANAGER held, and the
/// mount's read guard still held from `populate`, so no mutation has run since.
pub(crate) fn bind(
    handle: FileHandle,
    pid: ProcessId,
    pt: &ProcessPageTable,
    vma: &Vma,
    pgoff: u64,
    may_write: bool,
) -> Result<Binding, &'static str> {
    let id = {
        let mut inner = handle.object.map.inner.lock();
        inner
            .bindings
            .try_reserve(1)
            .map_err(|_| "Out of memory for file binding")?;
        let id = next_binding();
        let shared = vma.flags.bits() & 3 != 2;
        if shared && vma.prot.contains(Protection::WRITE) {
            inner.writable_shared += 1;
        }
        inner.bindings.push(BindingRec {
            id,
            pid,
            space: pt.address_space(),
            va: vma.start.as_u64(),
            pages: vma.size() / PAGE_SIZE,
            pgoff,
            prot: vma.prot,
            shared: vma.flags.bits() & 3 != 2,
            may_write,
            parked: Vec::new(),
        });
        id
    };
    Ok(Binding { id, handle })
}

/// Children's VMAs for fork. Each file VMA gets its own binding in the child's
/// address space, sharing the parent's parked copies.
pub(crate) fn fork_vmas(
    vmas: &[Vma],
    pid: ProcessId,
    pt: &ProcessPageTable,
) -> Result<Vec<Vma>, &'static str> {
    let mut result = Vec::new();
    result
        .try_reserve(vmas.len())
        .map_err(|_| "Out of memory for child VMAs")?;
    for parent in vmas {
        // Another thread is still installing this shared mapping's pages, and
        // the child would not share the ones installed after the fork. A
        // private mapping's pages not yet installed are zero, as the child's
        // first touch of them makes them.
        if parent.reservation != 0 && !parent.flags.contains(MmapFlags::PRIVATE) {
            return Err("Shared mapping still being installed");
        }
        let mut child = Vma::new(parent.start, parent.end, parent.prot, parent.flags);
        if let Some(binding) = &parent.backing {
            child.backing = Some(binding.fork(pid, pt)?);
        }
        result.push(child);
    }
    Ok(result)
}

impl Binding {
    /// mincore includes cached file pages even before this VA faults them in.
    pub(crate) fn resident(&self, address: u64) -> bool {
        let inner = self.handle.object.map.inner.lock();
        let Some(rec) = inner.bindings.iter().find(|rec| rec.id == self.id) else {
            return false;
        };
        let index = rec.index(address);
        inner.pages.get(index).is_some()
            || rec.parked.binary_search_by_key(&index, |(index, _)| *index).is_ok()
    }

    /// VMAs bound this virtual range before msync's filesystem work begins.
    pub(crate) fn sync_range(
        &self,
        start: u64,
        end: u64,
    ) -> Result<(FileHandle, u64, u64), &'static str> {
        let inner = self.handle.object.map.inner.lock();
        let rec = inner
            .bindings
            .iter()
            .find(|rec| rec.id == self.id)
            .ok_or("Missing file binding")?;
        Ok((self.handle.clone(), rec.index(start), rec.index(end)))
    }

    fn fork(&self, pid: ProcessId, pt: &ProcessPageTable) -> Result<Binding, &'static str> {
        let mut inner = self.handle.object.map.inner.lock();
        inner
            .bindings
            .try_reserve(1)
            .map_err(|_| "Out of memory for file binding")?;
        let parent = inner
            .bindings
            .iter()
            .find(|rec| rec.id == self.id)
            .ok_or("File binding missing from its reverse map")?;
        let mut parked = Vec::new();
        parked
            .try_reserve(parent.parked.len())
            .map_err(|_| "Out of memory for file binding")?;
        for (index, page) in &parent.parked {
            parked.push((*index, CachePage::retain(page.frame)?));
        }
        let id = next_binding();
        let rec = BindingRec {
            id,
            pid,
            space: pt.address_space(),
            va: parent.va,
            pages: parent.pages,
            pgoff: parent.pgoff,
            prot: parent.prot,
            shared: parent.shared,
            may_write: parent.may_write,
            parked,
        };
        if rec.writable_shared() {
            inner.writable_shared += 1;
        }
        inner.bindings.push(rec);
        drop(inner);
        Ok(Binding {
            id,
            handle: self.handle.clone(),
        })
    }
}

/// Index of the file VMA that is exactly `[start, end)`, after splitting the
/// VMA containing it at `start`, at `end`, or at both. The range must lie
/// within one file VMA; ranges that span VMAs return EINVAL, as for anonymous
/// memory. The isolated VMA's binding gets room to park `extra_parked` more
/// private copies. Everything that can fail runs before the first change, so
/// on error the VMAs and bindings are as they were. PROCESS_MANAGER held.
pub(crate) fn isolate(
    vmas: &mut Vec<Vma>,
    start: u64,
    end: u64,
    extra_parked: usize,
) -> Result<Option<usize>, &'static str> {
    let Some(index) = vmas
        .iter()
        .position(|vma| vma.start.as_u64() <= start && start < vma.end.as_u64())
    else {
        return Ok(None);
    };
    let (vma_start, vma_end, prot, flags) = {
        let vma = &vmas[index];
        (vma.start.as_u64(), vma.end.as_u64(), vma.prot, vma.flags)
    };
    let Some(binding) = vmas[index].backing.as_ref() else {
        return Ok(None);
    };
    if end > vma_end {
        return Err("Range spans more than one VMA");
    }
    if start % PAGE_SIZE != 0 || end % PAGE_SIZE != 0 {
        return Err("Split point outside the VMA");
    }
    // The new VMAs, in address order, each starting at a split point.
    let cuts: &[u64] = match (start > vma_start, end < vma_end) {
        (false, false) => &[],
        (true, false) => &[start],
        (false, true) => &[end],
        (true, true) => &[start, end],
    };
    let (head_id, handle) = (binding.id, binding.handle.clone());
    vmas.try_reserve(cuts.len())
        .map_err(|_| "Out of memory for VMA split")?;
    let mut inner = handle.object.map.inner.lock();
    inner
        .bindings
        .try_reserve(cuts.len())
        .map_err(|_| "Out of memory for VMA split")?;
    let head = inner
        .rec_mut(head_id)
        .ok_or("File binding missing from its reverse map")?;
    let index_of = |at: u64| head.pgoff + (at - head.va) / PAGE_SIZE;
    // Room for each new binding's parked copies, and for the isolated one's
    // extra copies: the first new binding when the range starts after the
    // VMA, else the head.
    let mut parked: [Vec<(u64, CachePage)>; 2] = [Vec::new(), Vec::new()];
    for (slot, &at) in cuts.iter().enumerate() {
        let low = index_of(at);
        let high = cuts.get(slot + 1).map_or(u64::MAX, |&next| index_of(next));
        let mut count = head
            .parked
            .iter()
            .filter(|(page, _)| (low..high).contains(page))
            .count();
        if slot == 0 && at == start {
            count += extra_parked;
        }
        parked[slot]
            .try_reserve(count)
            .map_err(|_| "Out of memory for VMA split")?;
    }
    if start == vma_start {
        head.parked
            .try_reserve(extra_parked)
            .map_err(|_| "Out of memory for VMA split")?;
    }
    // Commit: nothing below allocates or fails. The last split is made first,
    // so each one moves a suffix of the head's parked copies.
    let mut tails: [Option<BindingRec>; 2] = [None, None];
    for (slot, &at) in cuts.iter().enumerate().rev() {
        let pages = (at - head.va) / PAGE_SIZE;
        let cut = head
            .parked
            .partition_point(|(page, _)| *page < head.pgoff + pages);
        let mut moved = core::mem::take(&mut parked[slot]);
        moved.extend(head.parked.drain(cut..));
        tails[slot] = Some(BindingRec {
            id: 0,
            pid: head.pid,
            space: head.space,
            va: at,
            pages: head.pages - pages,
            pgoff: head.pgoff + pages,
            prot: head.prot,
            shared: head.shared,
            may_write: head.may_write,
            parked: moved,
        });
        head.pages = pages;
    }
    if let Some(&first) = cuts.first() {
        vmas[index].end = VirtAddr::new(first);
    }
    for (slot, &at) in cuts.iter().enumerate() {
        let Some(mut rec) = tails[slot].take() else {
            continue;
        };
        rec.id = next_binding();
        let next = cuts.get(slot + 1).copied().unwrap_or(vma_end);
        let mut vma = Vma::new(VirtAddr::new(at), VirtAddr::new(next), prot, flags);
        vma.backing = Some(Binding {
            id: rec.id,
            handle: handle.clone(),
        });
        if rec.writable_shared() {
            inner.writable_shared += 1;
        }
        inner.bindings.push(rec);
        vmas.insert(index + 1 + slot, vma);
    }
    drop(inner);
    // The range is the VMA that starts at `start`.
    Ok(Some(if start > vma_start { index + 1 } else { index }))
}

/// munmap of the whole file VMA `vmas[index]`. If an entry cannot be removed,
/// the VMA and its binding are kept, so a later shrink still covers the range.
pub(crate) fn unmap(
    vmas: &mut Vec<Vma>,
    index: usize,
    pt: &mut ProcessPageTable,
) -> Result<(), &'static str> {
    let vma = &vmas[index];
    let (start, end) = (vma.start.as_u64(), vma.end.as_u64());
    let mut clean = true;
    let mut from = start;
    while let Some(page) = pt.next_mapped_page(from, end) {
        from = page.start_address().as_u64() + PAGE_SIZE;
        clean &= revoke_entry(pt, page);
    }
    if !clean {
        return Err("File mapping entry custody disagreed");
    }
    drop(vmas.remove(index));
    Ok(())
}

/// mprotect of `[start, end)`, which must lie within one file VMA; returns
/// `Ok(false)` when `start` is not in a file VMA. The range is split out and
/// every entry in it removed, and faults install them again under the new
/// protection; a private copy is parked in its binding instead of being
/// lost. Everything that can fail, the private copies' references and the
/// split with room to park them, runs before the first change, so on error
/// the PTEs, bindings and VMAs are as they were. If an entry's custody
/// disagrees, its descriptor is still cleared, the old protection is kept and
/// the error is returned, as munmap does. PROCESS_MANAGER held.
pub(crate) fn protect(
    vmas: &mut Vec<Vma>,
    pt: &mut ProcessPageTable,
    start: u64,
    end: u64,
    prot: Protection,
) -> Result<bool, &'static str> {
    let Some(binding) = vmas
        .iter()
        .find(|vma| vma.start.as_u64() <= start && start < vma.end.as_u64())
        .and_then(|vma| vma.backing.as_ref())
    else {
        return Ok(false);
    };
    let handle = binding.handle.clone();
    // Private copies are the entries that do not map their cache page.
    let mut copies = Vec::new();
    {
        let inner = handle.object.map.inner.lock();
        let rec = inner
            .bindings
            .iter()
            .find(|rec| rec.id == binding.id)
            .ok_or("File binding missing from its reverse map")?;
        if rec.shared && prot.contains(Protection::WRITE) && !rec.may_write {
            return Err("Permission denied");
        }
        let mut from = start;
        while let Some(page) = pt.next_mapped_page(from, end) {
            from = page.start_address().as_u64() + PAGE_SIZE;
            let index = rec.index(page.start_address().as_u64());
            if let Some((frame, _)) = pt.get_page_info(page) {
                if inner.pages.get(index).map(|page| page.frame) != Some(frame) {
                    copies
                        .try_reserve(1)
                        .map_err(|_| "Out of memory for mprotect")?;
                    copies.push((index, CachePage::retain(frame)?));
                }
            }
        }
    }
    let index = isolate(vmas, start, end, copies.len())?.ok_or("Not a file VMA")?;
    let binding = vmas[index].backing.as_ref().ok_or("Not a file VMA")?;
    // Commit: nothing below allocates. PROCESS_MANAGER is held throughout,
    // so no fault observes the binding and the VMA apart.
    let mut inner = binding.handle.object.map.inner.lock();
    let valid = pages(inner.mapped_size);
    let rec = inner
        .rec_mut(binding.id)
        .ok_or("File binding missing from its reverse map")?;
    let mut clean = true;
    let mut from = start;
    while let Some(page) = pt.next_mapped_page(from, end) {
        from = page.start_address().as_u64() + PAGE_SIZE;
        clean &= revoke_entry(pt, page);
    }
    // Every entry is gone either way, so the copies are parked for the next
    // fault to map again. A shrink may have dropped the parked copies past
    // its EOF since they were taken; those are released.
    for (index, page) in copies {
        if index < valid {
            let slot = rec.parked.partition_point(|(parked, _)| *parked < index);
            rec.parked.insert(slot, (index, page));
        }
    }
    if !clean {
        return Err("File mapping entry custody disagreed");
    }
    let was_writable = rec.writable_shared();
    rec.prot = prot;
    let now_writable = rec.writable_shared();
    if now_writable && !was_writable {
        inner.writable_shared += 1;
    }
    if was_writable && !now_writable {
        inner.writable_shared -= 1;
    }
    drop(inner);
    binding.handle.object.map.request_writeback();
    vmas[index].prot = prot;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Faults
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    Write,
    Execute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FaultOutcome {
    /// Not a file mapping, or an entry the copy-on-write path owns.
    NotFile,
    /// The access can be retried.
    Resolved,
    /// The access cannot complete; a user access raises this signal.
    Signal(u32),
}

/// As on Linux, a writable mapping is also readable.
fn permits(prot: Protection, access: Access) -> bool {
    match access {
        Access::Read => prot.contains(Protection::READ) || prot.contains(Protection::WRITE),
        Access::Write => prot.contains(Protection::WRITE),
        Access::Execute => prot.contains(Protection::EXEC),
    }
}

fn entry_permits(flags: PageTableFlags, access: Access) -> bool {
    flags.contains(PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE)
        && match access {
            Access::Read => true,
            Access::Write => flags.contains(PageTableFlags::WRITABLE),
            Access::Execute => !flags.contains(PageTableFlags::NO_EXECUTE),
        }
}

/// Entry flags for a private file page. The cache's frame is never writable;
/// a writable mapping of it is copy-on-write.
fn entry_flags(prot: Protection, writable: bool) -> PageTableFlags {
    let mut flags = crate::syscall::memory_common::prot_to_page_flags(prot);
    if !prot.contains(Protection::EXEC) {
        flags.insert(PageTableFlags::NO_EXECUTE);
    }
    if !writable {
        flags.remove(PageTableFlags::WRITABLE);
        if prot.contains(Protection::WRITE) {
            flags.insert(COW_FLAG);
        }
    }
    flags
}

/// Resolve a fault at `address` in `process`'s address space. PROCESS_MANAGER
/// held. The access is classified against the VMA's protection before EOF.
pub(crate) fn resolve_fault(process: &mut Process, address: u64, access: Access) -> FaultOutcome {
    let Some(table) = process.page_table.as_deref_mut() else {
        return FaultOutcome::NotFile;
    };
    resolve_page(table, &process.vmas, address, access)
}

/// Resolve through the owned table and VMA bindings without acquiring PM or
/// accessing a user virtual address. Signal-frame installation also uses this
/// for CLONE_VM threads, whose table and bindings belong to another row.
/// PROCESS_MANAGER is held; the file cache supplies the page without disk I/O.
pub(crate) fn resolve_page(
    pt: &mut ProcessPageTable,
    vmas: &[Vma],
    address: u64,
    access: Access,
) -> FaultOutcome {
    let Some(vma) = vmas
        .iter()
        .find(|vma| vma.start.as_u64() <= address && address < vma.end.as_u64())
    else {
        return FaultOutcome::NotFile;
    };
    let Some(binding) = vma.backing.as_ref() else {
        return FaultOutcome::NotFile;
    };
    if !permits(vma.prot, access) {
        return FaultOutcome::Signal(SIGSEGV);
    }
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(address));
    if let Some((_, flags)) = pt.get_page_info(page) {
        if entry_permits(flags, access) {
            // Installed by a racing fault after this one was taken.
            crate::syscall::memory_common::flush_tlb(page.start_address());
            return FaultOutcome::Resolved;
        }
        return FaultOutcome::NotFile;
    }
    let object = &binding.handle.object;
    let Some(mut inner) = object.map.inner.try_lock() else {
        // Delivery runs with interrupts masked. A contended cache requests a
        // retry, just like the in-progress size transition below.
        return FaultOutcome::Resolved;
    };
    let mapped_size = inner.mapped_size;
    let Some(rec) = inner.rec_mut(binding.id) else {
        return FaultOutcome::NotFile;
    };
    // The live VMA and binding must belong to this table, including when
    // delivery reached it through a CLONE_VM sibling. Never publish a leaf
    // from a retired or differently protected binding.
    if rec.space != pt.address_space()
        || address < rec.va
        || (address - rec.va) / PAGE_SIZE >= rec.pages
        || !permits(rec.prot, access)
    {
        return FaultOutcome::Signal(SIGSEGV);
    }
    let index = rec.index(address);
    if index >= pages(mapped_size) {
        // ext2 has published a larger size that the transition has not yet
        // applied: retry until it has.
        if index < pages(object.size.load(Ordering::Acquire)) {
            return FaultOutcome::Resolved;
        }
        return FaultOutcome::Signal(SIGBUS);
    }
    let prot = rec.prot;
    let shared = rec.shared;
    if let Ok(slot) = rec.parked.binary_search_by_key(&index, |(index, _)| *index) {
        let frame = rec.parked[slot].1.frame;
        let exclusive = !super::frame_metadata::frame_is_shared(frame);
        if prot.contains(Protection::EXEC) {
            sync_executable(frame);
        }
        if pt
            .map_page(page, frame, entry_flags(prot, exclusive))
            .is_err()
        {
            return FaultOutcome::Signal(SIGBUS);
        }
        // The entry now holds its own reference.
        let parked = rec.parked.remove(slot);
        drop(inner);
        drop(parked);
        return FaultOutcome::Resolved;
    }
    if inner.poisoned.contains(index) {
        return FaultOutcome::Signal(SIGBUS);
    }
    let frame = match inner.pages.get(index) {
        Some(page) => page.frame,
        None => match CachePage::allocate() {
            // A hole: its file bytes are zero (see the module invariant).
            Ok(page) => {
                let frame = page.frame;
                if inner.pages.try_insert(index, page).is_err() {
                    return FaultOutcome::Signal(SIGBUS);
                }
                frame
            }
            Err(_) => return FaultOutcome::Signal(SIGBUS),
        },
    };
    object.map.resident.store(true, Ordering::Release);
    if prot.contains(Protection::EXEC) {
        sync_executable(frame);
    }
    if shared && prot.contains(Protection::WRITE) {
        let cached = inner.pages.get_mut(index).expect("resident file page");
        cached.dirty = true;
        cached.gen = cached.gen.wrapping_add(1);
        object.map.dirty.store(true, Ordering::Release);
    }
    if pt.map_page(page, frame, entry_flags(prot, shared)).is_err() {
        return FaultOutcome::Signal(SIGBUS);
    }
    FaultOutcome::Resolved
}

/// Resolve a user-address fault taken under the page table rooted at `root`,
/// and raise the signal of an unresolvable user-mode access in the faulting
/// thread's process. PROCESS_MANAGER held.
pub(crate) fn handle_fault(
    manager: &mut ProcessManager,
    root: u64,
    address: u64,
    access: Access,
    user_thread: Option<u64>,
) -> FaultOutcome {
    let outcome = match manager.find_process_by_cr3_mut(root) {
        Some((_, owner)) => resolve_fault(owner, address, access),
        None => return FaultOutcome::NotFile,
    };
    if let (FaultOutcome::Signal(signal), Some(tid)) = (outcome, user_thread) {
        let target = match manager.find_process_by_thread_mut(tid) {
            Some((_, process)) => Some(process),
            None => manager
                .find_process_by_cr3_mut(root)
                .map(|(_, owner)| owner),
        };
        if let Some(process) = target {
            // A mapping that forbids the access, or a page past the file's end.
            let code = if signal == SIGSEGV {
                crate::signal::constants::SEGV_ACCERR
            } else {
                crate::signal::constants::BUS_ADRERR
            };
            process
                .signals
                .force_signal(signal, crate::signal::types::SigInfo::fault(code, address));
        }
    }
    outcome
}

/// errno for a failed file write: one that could not allocate cache pages for
/// the mapped holes it fills is ENOMEM, with the file unchanged; anything else
/// is EIO.
pub(crate) fn mutation_errno(error: &'static str) -> u64 {
    if error.starts_with("Out of memory") {
        crate::syscall::errno::ENOMEM as u64
    } else {
        crate::syscall::errno::EIO as u64
    }
}
