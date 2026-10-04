//! Private file mappings.
//!
//! Each live inode carries a cache of clean file pages and a reverse map of
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
//! Lock order: ext2 mount guard, PROCESS_MANAGER, `MapState`, frame ledger and
//! allocator. No disk I/O runs under PROCESS_MANAGER or `MapState`. No path
//! takes an ext2 guard with PROCESS_MANAGER held: faults never touch the
//! filesystem, and mmap releases PROCESS_MANAGER before taking the mount guard.

#[cfg(not(target_arch = "x86_64"))]
use super::arch_stub::{Page, PageTableFlags, PhysFrame, Size4KiB, VirtAddr};
use crate::fs::ext2::{live_inode::FileHandle, Ext2Fs};
use crate::memory::{
    process_memory::{ProcessPageTable, Revoked, COW_FLAG},
    vma::{Protection, Vma},
};
use crate::process::{Process, ProcessId, ProcessManager};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::{
    structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB},
    VirtAddr,
};

const PAGE_SIZE: u64 = 4096;
/// File pages one PROCESS_MANAGER section revokes per binding.
const REVOKE_WINDOW: u64 = 64;
/// Unbound cache pages one eviction pass retires.
pub(crate) const EVICT_BUDGET: usize = 64;

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
}

#[derive(Debug)]
struct MapInner {
    /// The EOF the bindings honour. Equal to the inode's published size
    /// except while a size transition is between publication and this update.
    mapped_size: u64,
    pages: BTreeMap<u64, CachePage>,
    /// Page indices whose file bytes are unknown after a failed mutation.
    poisoned: BTreeSet<u64>,
    bindings: Vec<BindingRec>,
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
    /// Private copies whose entries mprotect removed, sorted by page index.
    /// The next fault on that index maps the copy again.
    parked: Vec<(u64, CachePage)>,
}

impl BindingRec {
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
}

impl CachePage {
    fn retain(frame: PhysFrame) -> Result<Self, &'static str> {
        super::frame_allocator::acquire_leaf_mapping(frame)?;
        Ok(Self { frame })
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

/// The file side of one file VMA. Owned by the VMA; dropping it removes the
/// binding from the reverse map.
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
                .map(|slot| inner.bindings.swap_remove(slot));
            (
                removed,
                !inner.pages.is_empty() || !inner.poisoned.is_empty(),
            )
        };
        // Pages this binding covered may now be unbound: ask the ext2 service
        // to retire them. Parked copies release their frames here, outside
        // the map lock.
        if cached {
            map.eviction_pending.store(true, Ordering::Release);
            crate::fs::ext2::request_map_eviction();
        }
        drop(removed);
    }
}

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
        if let Some(page) = self.pages.get(&index) {
            if self
                .bindings
                .iter()
                .any(|rec| rec.prot.contains(Protection::EXEC) && rec.covers(index))
            {
                sync_executable(page.frame);
            }
        }
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
                pages: BTreeMap::new(),
                poisoned: BTreeSet::new(),
                bindings: Vec::new(),
            }),
            eviction_pending: AtomicBool::new(false),
        }
    }

    /// The caller observed `size` under the filesystem guard. Without a
    /// binding nothing maps the cache, so a size this state never saw only
    /// discards cached pages.
    pub(crate) fn resync_unbound(&self, size: u64) {
        let stale = {
            let mut inner = self.inner.lock();
            if inner.mapped_size == size || !inner.bindings.is_empty() {
                return;
            }
            inner.mapped_size = size;
            inner.poisoned.clear();
            core::mem::take(&mut inner.pages)
        };
        drop(stale);
    }

    pub(crate) fn eviction_pending(&self) -> bool {
        self.eviction_pending.load(Ordering::Acquire)
    }

    /// Retire cache pages no binding covers. The filesystem write guard
    /// excludes population and every mutation hook.
    pub(crate) fn evict(&self, budget: &mut usize) -> bool {
        let mut retired = Vec::new();
        let more = {
            let mut inner = self.inner.lock();
            let MapInner {
                pages,
                poisoned,
                bindings,
                ..
            } = &mut *inner;
            let unbound = |index: &u64| !bindings.iter().any(|rec| rec.covers(*index));
            poisoned.retain(|index| !unbound(index));
            let victims: Vec<u64> = pages
                .keys()
                .copied()
                .filter(|index| unbound(index))
                .take(*budget)
                .collect();
            for index in victims {
                if let Some(page) = pages.remove(&index) {
                    retired.push(page);
                    *budget -= 1;
                }
            }
            let more = pages.keys().any(|index| unbound(index));
            self.eviction_pending.store(more, Ordering::Release);
            more
        };
        drop(retired);
        more
    }

    /// Before a write: zero frames for the pages it touches that a binding
    /// covers but the cache does not hold. Fails before the disk changes.
    pub(crate) fn prepare_write(&self, offset: u64, len: usize) -> Result<Prepared, &'static str> {
        let end = offset.checked_add(len as u64).ok_or("File too large")?;
        let wanted: Vec<u64> = {
            let inner = self.inner.lock();
            if inner.bindings.is_empty() {
                return Ok(Prepared { pages: Vec::new() });
            }
            (offset / PAGE_SIZE..pages(end))
                .filter(|index| {
                    !inner.pages.contains_key(index)
                        && !inner.poisoned.contains(index)
                        && inner.bound(*index)
                })
                .collect()
        };
        let mut prepared = Vec::new();
        prepared
            .try_reserve(wanted.len())
            .map_err(|_| "Out of memory for file mapping")?;
        for index in wanted {
            prepared.push((index, CachePage::allocate()?));
        }
        Ok(Prepared { pages: prepared })
    }

    /// After a successful write of `bytes` at `offset`: the cache takes the
    /// written bytes, so every binding that maps a cache page sees them.
    pub(crate) fn commit_write(&self, offset: u64, bytes: &[u8], prepared: Prepared) {
        let end = offset + bytes.len() as u64;
        let mut inner = self.inner.lock();
        for (index, page) in prepared.pages {
            // A fault may have inserted a zero page meanwhile; it is overlaid below.
            if !inner.pages.contains_key(&index) && !inner.poisoned.contains(&index) {
                inner.pages.insert(index, page);
            }
        }
        let touched: Vec<u64> = inner
            .pages
            .range(offset / PAGE_SIZE..pages(end))
            .map(|(&index, _)| index)
            .collect();
        for index in touched {
            let page = &inner.pages[&index];
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
            inner.sync_if_executable(index);
        }
    }

    /// A mutation failed after it may have changed the bytes of `[offset,
    /// offset + len)`. The cache no longer knows them: drop its pages there,
    /// mark the range poisoned and remove every entry that maps a dropped
    /// cache page. Private copies are the process's own bytes and stay.
    pub(crate) fn invalidate(&self, offset: u64, len: u64) {
        let first = offset / PAGE_SIZE;
        let last = pages(offset.saturating_add(len));
        let dropped = {
            let mut inner = self.inner.lock();
            let mut dropped = inner.pages.split_off(&first);
            let mut kept = dropped.split_off(&last);
            inner.pages.append(&mut kept);
            for index in first..last {
                inner.poisoned.insert(index);
            }
            dropped
        };
        if !dropped.is_empty() {
            let frames: BTreeMap<u64, PhysFrame> = dropped
                .iter()
                .map(|(&index, page)| (index, page.frame))
                .collect();
            self.revoke(first, last, Some(&frames));
        }
        drop(dropped);
    }

    /// Bring the cache to `size`, the EOF ext2 published, on success or
    /// failure of the mutation. Growth only moves the EOF: the new pages are
    /// holes, which faults fill with zero pages. A shrink drops the cache and
    /// parked copies past the new EOF and removes every entry there.
    pub(crate) fn transition(&self, size: u64) {
        let (old, dropped) = {
            let mut inner = self.inner.lock();
            let old = inner.mapped_size;
            if size == old {
                return;
            }
            inner.mapped_size = size;
            let boundary = old.min(size);
            if boundary % PAGE_SIZE != 0 {
                if let Some(page) = inner.pages.get(&(boundary / PAGE_SIZE)) {
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
            (old, inner.pages.split_off(&keep))
        };
        // Entries hold their own frame references, so the cache's may go first.
        drop(dropped);
        self.revoke(pages(size), pages(old), None);
    }

    /// Re-read poisoned pages that a binding covers and that lie within EOF.
    /// Runs under the filesystem write guard, after a mutation. A page that
    /// still cannot be read stays poisoned and faults on it raise SIGBUS.
    pub(crate) fn repair(&self, fs: &Ext2Fs, ino: u32) {
        let wanted: Vec<u64> = {
            let mut inner = self.inner.lock();
            if inner.poisoned.is_empty() {
                return;
            }
            let MapInner {
                poisoned, bindings, ..
            } = &mut *inner;
            poisoned.retain(|index| bindings.iter().any(|rec| rec.covers(*index)));
            let valid = pages(inner.mapped_size);
            inner.poisoned.range(..valid).copied().collect()
        };
        if wanted.is_empty() {
            return;
        }
        let Ok(inode) = fs.read_inode(ino) else {
            return;
        };
        for index in wanted {
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
            if inner.poisoned.remove(&index) && !inner.pages.contains_key(&index) {
                inner.pages.insert(index, page);
            }
        }
    }

    /// Remove the entries for file pages `[first, last)` in every binding
    /// whose address space is live, a bounded window per PROCESS_MANAGER
    /// section. With `aliases`, only entries that map the given cache frames.
    /// A binding created between sections is covered by the later windows.
    fn revoke(&self, first: u64, last: u64, aliases: Option<&BTreeMap<u64, PhysFrame>>) {
        let mut cursor = first;
        while cursor < last {
            let mut guard = crate::process::manager();
            let Some(manager) = guard.as_mut() else {
                return;
            };
            let inner = self.inner.lock();
            let Some(start) = inner
                .bindings
                .iter()
                .filter_map(|rec| {
                    let start = cursor.max(rec.pgoff);
                    (start < last.min(rec.pgoff + rec.pages)).then_some(start)
                })
                .min()
            else {
                return;
            };
            let end = last.min(start + REVOKE_WINDOW);
            for rec in &inner.bindings {
                let (low, high) = (start.max(rec.pgoff), end.min(rec.pgoff + rec.pages));
                if low >= high {
                    continue;
                }
                let Some(pt) = live_table(manager, rec) else {
                    continue;
                };
                let stop = rec.page(high - 1).start_address().as_u64() + PAGE_SIZE;
                let mut from = rec.page(low).start_address().as_u64();
                while let Some(page) = pt.next_mapped_page(from, stop) {
                    from = page.start_address().as_u64() + PAGE_SIZE;
                    if let Some(aliases) = aliases {
                        let index = rec.index(page.start_address().as_u64());
                        let mapped = pt.get_page_info(page).map(|(frame, _)| frame);
                        if mapped.is_none() || aliases.get(&index).copied() != mapped {
                            continue;
                        }
                    }
                    revoke_entry(pt, page);
                }
            }
            cursor = end;
        }
    }
}

/// Frames allocated before a write for the bound holes it fills.
pub(crate) struct Prepared {
    pages: Vec<(u64, CachePage)>,
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

/// x86 removes a file page's translation with a local `invlpg`, which is
/// enough only while one CPU is online. x86 brings up no secondary CPUs yet
/// (#814; #629 tracks the online count), and mmap refuses file mappings once a
/// second CPU is online. ARM64 invalidation is broadcast.
pub(crate) fn local_invalidation_suffices() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch_impl::x86_64::smp::cpus_online() == 1
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

/// Remove one entry. Returns false when its descriptor and custody record
/// disagreed; the descriptor is cleared and flushed either way.
fn revoke_entry(pt: &mut ProcessPageTable, page: Page<Size4KiB>) -> bool {
    assert!(
        local_invalidation_suffices(),
        "file mapping revocation needs remote TLB invalidation (#814)"
    );
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
    let missing: Vec<u64> = {
        let inner = map.inner.lock();
        (first..last)
            .filter(|index| !inner.pages.contains_key(index))
            .collect()
    };
    if missing.is_empty() {
        return Ok(());
    }
    let inode = fs.read_inode(ino).map_err(|_| MapError::Io)?;
    let mut read = Vec::new();
    read.try_reserve(missing.len())
        .map_err(|_| MapError::NoMemory)?;
    for index in missing {
        let page = CachePage::allocate().map_err(|_| MapError::NoMemory)?;
        let bytes = fs
            .read_file_range(&inode, index * PAGE_SIZE, PAGE_SIZE as usize)
            .map_err(|_| MapError::Io)?;
        // SAFETY: a fresh cache frame; `bytes` is at most one page.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), page.ptr(), bytes.len()) };
        read.push((index, page));
    }
    let mut inner = map.inner.lock();
    for (index, page) in read {
        inner.poisoned.remove(&index);
        inner.pages.entry(index).or_insert(page);
    }
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
) -> Result<Binding, &'static str> {
    let id = next_binding();
    {
        let mut inner = handle.object.map.inner.lock();
        inner
            .bindings
            .try_reserve(1)
            .map_err(|_| "Out of memory for file binding")?;
        inner.bindings.push(BindingRec {
            id,
            pid,
            space: pt.address_space(),
            va: vma.start.as_u64(),
            pages: vma.size() / PAGE_SIZE,
            pgoff,
            prot: vma.prot,
            parked: Vec::new(),
        });
    }
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
        let mut child = Vma::new(parent.start, parent.end, parent.prot, parent.flags);
        if let Some(binding) = &parent.backing {
            child.backing = Some(binding.fork(pid, pt)?);
        }
        result.push(child);
    }
    Ok(result)
}

impl Binding {
    fn fork(&self, pid: ProcessId, pt: &ProcessPageTable) -> Result<Binding, &'static str> {
        let id = next_binding();
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
        let rec = BindingRec {
            id,
            pid,
            space: pt.address_space(),
            va: parent.va,
            pages: parent.pages,
            pgoff: parent.pgoff,
            prot: parent.prot,
            parked,
        };
        inner.bindings.push(rec);
        drop(inner);
        Ok(Binding {
            id,
            handle: self.handle.clone(),
        })
    }
}

/// Split the file VMA `vmas[index]` so that a VMA starts at `at`. Either
/// completes or changes nothing. PROCESS_MANAGER held.
pub(crate) fn split_vma(vmas: &mut Vec<Vma>, index: usize, at: u64) -> Result<(), &'static str> {
    let (start, end, prot, flags) = {
        let vma = &vmas[index];
        (vma.start.as_u64(), vma.end, vma.prot, vma.flags)
    };
    if at <= start || at >= end.as_u64() || at % PAGE_SIZE != 0 {
        return Err("Split point outside the VMA");
    }
    let (head_id, handle) = match &vmas[index].backing {
        Some(binding) => (binding.id, binding.handle.clone()),
        None => return Err("Not a file VMA"),
    };
    vmas.try_reserve(1)
        .map_err(|_| "Out of memory for VMA split")?;
    let id = next_binding();
    {
        let mut inner = handle.object.map.inner.lock();
        inner
            .bindings
            .try_reserve(1)
            .map_err(|_| "Out of memory for VMA split")?;
        let head = inner
            .rec_mut(head_id)
            .ok_or("File binding missing from its reverse map")?;
        let head_pages = (at - head.va) / PAGE_SIZE;
        let cut = head
            .parked
            .partition_point(|(page, _)| *page < head.pgoff + head_pages);
        let mut parked = Vec::new();
        parked
            .try_reserve(head.parked.len() - cut)
            .map_err(|_| "Out of memory for VMA split")?;
        parked.extend(head.parked.drain(cut..));
        let tail = BindingRec {
            id,
            pid: head.pid,
            space: head.space,
            va: at,
            pages: head.pages - head_pages,
            pgoff: head.pgoff + head_pages,
            prot: head.prot,
            parked,
        };
        head.pages = head_pages;
        inner.bindings.push(tail);
    }
    let mut tail = Vma::new(VirtAddr::new(at), end, prot, flags);
    tail.backing = Some(Binding { id, handle });
    vmas[index].end = VirtAddr::new(at);
    vmas.insert(index + 1, tail);
    Ok(())
}

/// Index of the file VMA that is exactly `[start, end)`, after splitting the
/// VMA containing it. The range must lie within one file VMA; ranges that
/// span VMAs return EINVAL, as for anonymous memory.
pub(crate) fn isolate(
    vmas: &mut Vec<Vma>,
    start: u64,
    end: u64,
) -> Result<Option<usize>, &'static str> {
    let Some(mut index) = vmas
        .iter()
        .position(|vma| vma.start.as_u64() <= start && start < vma.end.as_u64())
    else {
        return Ok(None);
    };
    if vmas[index].backing.is_none() {
        return Ok(None);
    }
    if end > vmas[index].end.as_u64() {
        return Err("Range spans more than one VMA");
    }
    if start > vmas[index].start.as_u64() {
        split_vma(vmas, index, start)?;
        index += 1;
    }
    if end < vmas[index].end.as_u64() {
        split_vma(vmas, index, end)?;
    }
    Ok(Some(index))
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

/// mprotect of the whole file VMA `vmas[index]`. Every entry is removed, and
/// faults install them again under the new protection; a private copy is
/// parked in its binding instead of being lost. Everything that can fail runs
/// before the first change, so on error the PTEs, binding and VMA still agree.
pub(crate) fn protect(
    vmas: &mut [Vma],
    index: usize,
    pt: &mut ProcessPageTable,
    prot: Protection,
) -> Result<(), &'static str> {
    let (start, end) = (vmas[index].start.as_u64(), vmas[index].end.as_u64());
    let binding = vmas[index].backing.as_ref().ok_or("Not a file VMA")?;
    let mut inner = binding.handle.object.map.inner.lock();
    let MapInner {
        pages: cache,
        bindings,
        ..
    } = &mut *inner;
    let rec = bindings
        .iter_mut()
        .find(|rec| rec.id == binding.id)
        .ok_or("File binding missing from its reverse map")?;
    let mut copies = Vec::new();
    let mut from = start;
    while let Some(page) = pt.next_mapped_page(from, end) {
        from = page.start_address().as_u64() + PAGE_SIZE;
        let index = rec.index(page.start_address().as_u64());
        if let Some((frame, _)) = pt.get_page_info(page) {
            if cache.get(&index).map(|page| page.frame) != Some(frame) {
                copies
                    .try_reserve(1)
                    .map_err(|_| "Out of memory for mprotect")?;
                copies.push((index, CachePage::retain(frame)?));
            }
        }
    }
    rec.parked
        .try_reserve(copies.len())
        .map_err(|_| "Out of memory for mprotect")?;
    // Commit: nothing below allocates or fails. PROCESS_MANAGER is held
    // throughout, so no fault observes the binding and the VMA apart.
    rec.prot = prot;
    let mut from = start;
    while let Some(page) = pt.next_mapped_page(from, end) {
        from = page.start_address().as_u64() + PAGE_SIZE;
        revoke_entry(pt, page);
    }
    for (index, page) in copies {
        let slot = rec.parked.partition_point(|(parked, _)| *parked < index);
        rec.parked.insert(slot, (index, page));
    }
    drop(inner);
    vmas[index].prot = prot;
    Ok(())
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
    let Process {
        vmas, page_table, ..
    } = process;
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
    let Some(pt) = page_table.as_deref_mut() else {
        return FaultOutcome::NotFile;
    };
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
    let mut inner = object.map.inner.lock();
    let mapped_size = inner.mapped_size;
    let Some(rec) = inner.rec_mut(binding.id) else {
        return FaultOutcome::NotFile;
    };
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
    if inner.poisoned.contains(&index) {
        return FaultOutcome::Signal(SIGBUS);
    }
    let frame = match inner.pages.get(&index) {
        Some(page) => page.frame,
        None => match CachePage::allocate() {
            // A hole: its file bytes are zero (see the module invariant).
            Ok(page) => {
                let frame = page.frame;
                inner.pages.insert(index, page);
                frame
            }
            Err(_) => return FaultOutcome::Signal(SIGBUS),
        },
    };
    if prot.contains(Protection::EXEC) {
        sync_executable(frame);
    }
    if pt.map_page(page, frame, entry_flags(prot, false)).is_err() {
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
            force_signal(process, signal);
        }
    }
    outcome
}

/// Make a synchronous fault signal pending as Linux's force_sig does: a
/// blocked or ignored signal is unblocked and reset to its default action, so
/// an installed handler runs and otherwise the default action applies. The
/// faulting instruction is retried until the return path delivers it.
fn force_signal(process: &mut Process, signal: u32) {
    let signals = &mut process.signals;
    if signals.is_blocked(signal) || signals.get_handler(signal).is_ignore() {
        signals.unblock_signals(crate::signal::constants::sig_mask(signal));
        signals.set_handler(signal, crate::signal::types::SignalAction::default());
    }
    signals.set_pending(signal);
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
