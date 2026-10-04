//! Resident private file pages. Lock order: filesystem guard, process manager,
//! inode map, hidden-page custody, frame ledger. Disk reads never hold map/PM.
#[cfg(not(target_arch = "x86_64"))]
use super::arch_stub::{Page, PageTableFlags, PhysFrame, Size4KiB, VirtAddr};
use crate::fs::ext2::{live_inode::FileHandle, Ext2Fs};
use crate::memory::{
    process_memory::{ProcessPageTable, COW_FLAG},
    vma::{Protection, Vma},
};
use crate::process::{Process, ProcessId};
use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::{
    structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB},
    VirtAddr,
};

static NEXT_BINDING: AtomicU64 = AtomicU64::new(1);
const PAGE_SIZE: u64 = 4096;

#[derive(Debug)]
pub struct MapState {
    inner: Mutex<MapInner>,
    resident: AtomicUsize,
    pub(crate) eviction_pending: AtomicBool,
}
#[derive(Debug)]
struct MapInner {
    mapped_size: u64,
    pages: BTreeMap<u64, CachePage>,
    bindings: Vec<BindingRec>,
}
#[derive(Debug, Clone, Copy)]
struct BindingRec {
    id: u64,
    pid: ProcessId,
    root: u64,
    va: u64,
    pages: u64,
    pgoff: u64,
    prot: Protection,
}
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
            super::frame_allocator::allocate_frame().ok_or("Out of memory for file mapping")?;
        match Self::retain(frame) {
            Ok(page) => {
                unsafe {
                    core::ptr::write_bytes(page.ptr(), 0, 4096);
                }
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
}
impl Drop for CachePage {
    fn drop(&mut self) {
        if super::frame_metadata::frame_decref(self.frame) {
            super::frame_allocator::deallocate_leaf_frame(self.frame);
        }
    }
}
#[derive(Debug)]
pub struct Binding {
    rec: BindingRec,
    handle: FileHandle,
    // PROT_NONE must retain detached private copies, including across fork.
    hidden: Mutex<BTreeMap<u64, CachePage>>,
}
impl Drop for Binding {
    fn drop(&mut self) {
        let map = &self.handle.object.map;
        {
            map.inner
                .lock()
                .bindings
                .retain(|rec| rec.id != self.rec.id);
        }
        map.eviction_pending.store(true, Ordering::Release);
        crate::fs::ext2::request_map_eviction();
    }
}
impl MapState {
    pub fn new(size: u64) -> Self {
        Self {
            inner: Mutex::new(MapInner {
                mapped_size: size,
                pages: BTreeMap::new(),
                bindings: Vec::new(),
            }),
            resident: AtomicUsize::new(0),
            eviction_pending: AtomicBool::new(false),
        }
    }
    pub fn is_empty(&self) -> bool {
        let inner = self.inner.lock();
        inner.pages.is_empty() && inner.bindings.is_empty()
    }
    pub(crate) fn clear(&self) {
        let mut inner = self.inner.lock();
        assert!(inner.bindings.is_empty());
        inner.pages.clear();
        self.resident.store(0, Ordering::Release);
    }
    /// FS write guard excludes population and mutation while retiring pages.
    pub(crate) fn evict(&self, budget: &mut usize) -> bool {
        let mut inner = self.inner.lock();
        let indices: Vec<_> = inner
            .pages
            .keys()
            .copied()
            .filter(|index| {
                !inner
                    .bindings
                    .iter()
                    .any(|rec| *index >= rec.pgoff && *index - rec.pgoff < rec.pages)
            })
            .take(*budget)
            .collect();
        for index in indices {
            inner.pages.remove(&index);
            *budget -= 1;
        }
        self.resident.store(inner.pages.len(), Ordering::Release);
        let more = inner.pages.keys().any(|index| {
            !inner
                .bindings
                .iter()
                .any(|rec| *index >= rec.pgoff && *index - rec.pgoff < rec.pages)
        });
        self.eviction_pending.store(more, Ordering::Release);
        more
    }
    pub(crate) fn overlay_write(&self, offset: u64, bytes: &[u8]) {
        if self.resident.load(Ordering::Acquire) == 0 {
            return;
        }
        let inner = self.inner.lock();
        let end = offset + bytes.len() as u64;
        for (&index, page) in inner
            .pages
            .range(offset / PAGE_SIZE..end.div_ceil(PAGE_SIZE))
        {
            let start = offset.max(index * PAGE_SIZE);
            let stop = end.min((index + 1) * PAGE_SIZE);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr().add((start - offset) as usize),
                    page.ptr().add((start % PAGE_SIZE) as usize),
                    (stop - start) as usize,
                );
            }
        }
    }
}

fn flags(prot: Protection, cow: bool) -> PageTableFlags {
    let mut flags = crate::syscall::memory_common::prot_to_page_flags(prot);
    if !prot.contains(Protection::EXEC) {
        flags.insert(PageTableFlags::NO_EXECUTE);
    }
    if cow {
        flags.remove(PageTableFlags::WRITABLE);
        if prot.contains(Protection::WRITE) {
            flags.insert(COW_FLAG);
        }
    }
    flags
}
fn page_at(rec: BindingRec, index: u64) -> Page<Size4KiB> {
    Page::containing_address(VirtAddr::new(rec.va + (index - rec.pgoff) * PAGE_SIZE))
}
fn revoke(pt: &mut ProcessPageTable, page: Page<Size4KiB>) {
    if let Ok(leaf) = pt.unmap_page_deferred(page) {
        leaf.flush().release();
    }
}

/// PM + inode map held. New descriptors use pre-reserved hierarchy/custody.
fn reconcile(
    pt: &mut ProcessPageTable,
    rec: BindingRec,
    inner: &MapInner,
    hidden: &mut BTreeMap<u64, CachePage>,
) -> Result<(), &'static str> {
    hidden.retain(|index, _| *index < inner.mapped_size.div_ceil(PAGE_SIZE));
    for index in rec.pgoff..rec.pgoff + rec.pages {
        let page = page_at(rec, index);
        let present = pt.get_page_info(page);
        if index >= inner.mapped_size.div_ceil(PAGE_SIZE) || rec.prot == Protection::NONE {
            if let Some((frame, _)) = present {
                if index < inner.mapped_size.div_ceil(PAGE_SIZE)
                    && inner
                        .pages
                        .get(&index)
                        .is_some_and(|cache| cache.frame != frame)
                {
                    hidden.insert(index, CachePage::retain(frame)?);
                }
                revoke(pt, page);
            }
        } else if present.is_none() {
            let cache = inner
                .pages
                .get(&index)
                .ok_or("Missing resident file page")?;
            let frame = hidden.get(&index).map_or(cache.frame, |copy| copy.frame);
            let cow = frame == cache.frame || super::frame_metadata::frame_is_shared(frame);
            pt.map_file_page(page, frame, flags(rec.prot, cow))?;
            hidden.remove(&index);
            crate::syscall::memory_common::flush_tlb(page.start_address());
        }
    }
    Ok(())
}

/// Read missing cache pages with the FS guard held, before registering a VMA.
pub(crate) fn populate(
    handle: &FileHandle,
    fs: &Ext2Fs,
    pgoff: u64,
    pages: u64,
) -> Result<(), &'static str> {
    let ino = handle.verify(fs)?;
    let inode = fs.read_inode(ino)?;
    let map = &handle.object.map;
    let size = inode.size();
    let mut prepared = Vec::new();
    for index in pgoff..(pgoff + pages).min(size.div_ceil(PAGE_SIZE)) {
        if map.inner.lock().pages.contains_key(&index) {
            continue;
        }
        let page = CachePage::allocate()?;
        let bytes = fs.read_file_range(&inode, index * PAGE_SIZE, PAGE_SIZE as usize)?;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), page.ptr(), bytes.len());
        }
        prepared.push((index, page));
    }
    let mut inner = map.inner.lock();
    inner.mapped_size = size;
    for (index, page) in prepared {
        inner.pages.entry(index).or_insert(page);
    }
    map.resident.store(inner.pages.len(), Ordering::Release);
    map.eviction_pending.store(true, Ordering::Release);
    drop(inner);
    crate::fs::ext2::request_map_eviction();
    Ok(())
}

pub(crate) fn install(
    handle: FileHandle,
    pid: ProcessId,
    pt: &mut ProcessPageTable,
    vma: &mut Vma,
    pgoff: u64,
) -> Result<(), &'static str> {
    let binding = register(handle, pid, pt, vma, pgoff)?;
    let rec = binding.rec;
    let result = {
        let inner = binding.handle.object.map.inner.lock();
        reconcile(pt, rec, &inner, &mut binding.hidden.lock())
    };
    if let Err(error) = result {
        for index in rec.pgoff..rec.pgoff + rec.pages {
            revoke(pt, page_at(rec, index));
        }
        pt.release_file_reservation(rec.pages as usize);
        return Err(error);
    }
    vma.backing = Some(binding.clone());
    pt.file_bindings.push(binding);
    Ok(())
}

fn register(
    handle: FileHandle,
    pid: ProcessId,
    pt: &mut ProcessPageTable,
    vma: &Vma,
    pgoff: u64,
) -> Result<Arc<Binding>, &'static str> {
    let pages = vma.size() / PAGE_SIZE;
    pt.file_bindings
        .try_reserve(1)
        .map_err(|_| "Out of memory for file binding")?;
    pt.reserve_file_range(vma.start.as_u64(), pages as usize)?;
    let id = NEXT_BINDING
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| "File binding identity exhausted")?;
    let rec = BindingRec {
        id,
        pid,
        root: pt.level_4_frame().start_address().as_u64(),
        va: vma.start.as_u64(),
        pages,
        pgoff,
        prot: vma.prot,
    };
    let binding = Arc::new(Binding {
        rec,
        handle,
        hidden: Mutex::new(BTreeMap::new()),
    });
    let map = &binding.handle.object.map;
    {
        let mut inner = map.inner.lock();
        if inner.bindings.try_reserve(1).is_err() {
            drop(inner);
            pt.release_file_reservation(pages as usize);
            return Err("Out of memory for file reverse map");
        }
        inner.bindings.push(rec);
    }
    Ok(binding)
}

pub(crate) fn remove(pt: &mut ProcessPageTable, binding: &Arc<Binding>) {
    pt.file_bindings
        .retain(|item| item.rec.id != binding.rec.id);
    pt.release_file_reservation(binding.rec.pages as usize);
}

pub(crate) fn protect(
    pt: &mut ProcessPageTable,
    binding: &Arc<Binding>,
    prot: Protection,
) -> Result<(), &'static str> {
    let map = &binding.handle.object.map;
    let mut inner = map.inner.lock();
    let mut rec = inner
        .bindings
        .iter()
        .find(|rec| rec.id == binding.rec.id)
        .copied()
        .ok_or("Missing file binding")?;
    rec.prot = prot;
    for index in rec.pgoff..rec.pgoff + rec.pages {
        let page = page_at(rec, index);
        if let Some((frame, _)) = pt.get_page_info(page) {
            if prot != Protection::NONE {
                let cow = inner
                    .pages
                    .get(&index)
                    .is_some_and(|cache| cache.frame == frame)
                    || super::frame_metadata::frame_is_shared(frame);
                pt.update_page_flags(page, flags(prot, cow))?;
                crate::syscall::memory_common::flush_tlb(page.start_address());
            }
        }
    }
    reconcile(pt, rec, &inner, &mut binding.hidden.lock())?;
    inner
        .bindings
        .iter_mut()
        .find(|item| item.id == rec.id)
        .expect("registered binding")
        .prot = prot;
    Ok(())
}

pub(crate) fn fork_vmas(
    vmas: &[Vma],
    pid: ProcessId,
    pt: &mut ProcessPageTable,
) -> Result<Vec<Vma>, &'static str> {
    let mut result = Vec::new();
    result
        .try_reserve(vmas.len())
        .map_err(|_| "Out of memory for child VMAs")?;
    for parent in vmas {
        let mut child = Vma::new(parent.start, parent.end, parent.prot, parent.flags);
        if let Some(binding) = &parent.backing {
            let child_binding =
                register(binding.handle.clone(), pid, pt, &child, binding.rec.pgoff)?;
            for (&index, page) in binding.hidden.lock().iter() {
                child_binding
                    .hidden
                    .lock()
                    .insert(index, CachePage::retain(page.frame)?);
            }
            child.backing = Some(child_binding.clone());
            pt.file_bindings.push(child_binding);
        }
        result.push(child);
    }
    Ok(result)
}

pub(crate) fn reconcile_process(process: &mut Process) -> Result<(), &'static str> {
    if let Some(pt) = process.page_table.as_mut() {
        for binding in pt.file_bindings.clone() {
            let inner = binding.handle.object.map.inner.lock();
            if let Some(rec) = inner
                .bindings
                .iter()
                .find(|rec| rec.id == binding.rec.id)
                .copied()
            {
                assert_eq!(rec.pid, process.id);
                assert_eq!(rec.root, pt.level_4_frame().start_address().as_u64());
                reconcile(pt, rec, &inner, &mut binding.hidden.lock())?;
            }
        }
    }
    Ok(())
}
