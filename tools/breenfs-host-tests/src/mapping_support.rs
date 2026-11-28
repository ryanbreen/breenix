// Pinned host frames for the production file_map writeback entry points.
use crate::{
    errno::Errno,
    vfs::{FileSystem, MappingIo, MappingRead, VNode},
};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;
const PAGE_SIZE: u64 = 4096;
const WRITEBACK_PAGES: usize = 64;
pub const EVICT_BUDGET: usize = 64;

#[derive(Clone)]
pub struct MapState {
    state: Arc<MapShared>,
    pub mapped_size: HostMappedSize,
}
struct MapShared {
    filesystem: Arc<dyn FileSystem>,
    inner: Mutex<MapInner>,
    resident: AtomicBool,
    dirty: AtomicBool,
    writeback_pending: AtomicBool,
    eviction_pending: AtomicBool,
    writeback_requests: AtomicU64,
    syncers: AtomicUsize,
}
struct MapInner {
    mapped_size: u64,
    pages: PageVec<CachePage>,
    holes: PageVec<()>,
    poisoned: PageVec<()>,
    bindings: Vec<BindingRec>,
    populating: Vec<Claim>,
    writable_shared: usize,
    exec_loads: usize,
    write_opens: usize,
    writeback_cursor: u64,
    writeback_epoch: u64,
}
// PageVec and mutation algorithms are extracted from production file_map.rs.
struct BindingRec {
    id: u64,
    pgoff: u64,
    pages: u64,
    shared: bool,
    writable: bool,
    /// Shared write authority: mapped from a descriptor open for writing.
    may_write: bool,
    /// One of a running image's PT_LOADs (E3-3).
    exec: bool,
    parked: Vec<(u64, CachePage)>,
    leaves: Vec<(u64, PhysFrame)>,
}
impl BindingRec {
    fn writable_shared(&self) -> bool { self.shared && self.writable }
    fn covers(&self, index: u64) -> bool {
        self.pgoff <= index && index - self.pgoff < self.pages
    }
}
// Keep the existing fixture's EOF observer, reading the real MapInner state.
#[derive(Clone)]
pub struct HostMappedSize(Arc<MapShared>);
impl HostMappedSize {
    pub fn load(&self, _: Ordering) -> u64 { self.0.inner.lock().mapped_size }
}
impl MapInner {
    // Host frames are data only; there is no guest instruction cache to sync.
    fn sync_if_executable(&self, _: u64) {}
}
// Host frames are data only; there is no guest instruction cache to sync.
fn sync_executable(_: PhysFrame) {}
/// A host page's address stands for its frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysFrame(pub usize);
impl PhysFrame {
    fn start_address(self) -> Self {
        self
    }
    fn as_u64(self) -> u64 {
        self.0 as u64
    }
}
struct CachePage {
    frame: PhysFrame,
    _storage: Box<[u8; 4096]>,
    dirty: bool,
    gen: u64,
    written: Option<WritebackReceipt>,
    icache_clean: bool,
}
std::thread_local! {
    static ALLOCATION_HOOK: core::cell::RefCell<(usize, Option<Box<dyn FnMut(usize)>>)> =
        core::cell::RefCell::new((0, None));
}
/// Run `hook(n)` before this thread's nth cache-frame allocation from now on,
/// so a test can mutate a file between population's page reads. An empty
/// option clears the hook.
pub fn on_cache_allocation(hook: Option<Box<dyn FnMut(usize)>>) {
    ALLOCATION_HOOK.with(|slot| *slot.borrow_mut() = (0, hook));
}
impl CachePage {
    fn allocate() -> Result<Self, &'static str> {
        let hook = ALLOCATION_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            slot.0 += 1;
            let count = slot.0;
            slot.1.take().map(|hook| (count, hook))
        });
        if let Some((count, mut hook)) = hook {
            hook(count);
            ALLOCATION_HOOK.with(|slot| {
                let mut slot = slot.borrow_mut();
                if slot.1.is_none() {
                    slot.1 = Some(hook);
                }
            });
        }
        let storage = Box::new([0; 4096]);
        Ok(Self { frame: PhysFrame(storage.as_ptr() as usize), _storage: storage,
            dirty: false, gen: 0, written: None, icache_clean: false })
    }
    fn ptr(&self) -> *mut u8 { self.frame.0 as *mut u8 }
    fn zero_from(&self, offset: u64) {
        assert!(offset < PAGE_SIZE);
        unsafe { core::ptr::write_bytes(self.ptr().add(offset as usize), 0, (PAGE_SIZE - offset) as usize) };
    }
}
impl MapState {
    pub fn new(size: u64, _epoch: u64, filesystem: Arc<dyn FileSystem>) -> Result<Self, Errno> {
        let state = Arc::new(MapShared {
                filesystem,
                inner: Mutex::new(MapInner {
                    mapped_size: size,
                    pages: PageVec {
                        entries: Vec::new(),
                    },
                    holes: PageVec {
                        entries: Vec::new(),
                    },
                    poisoned: PageVec {
                        entries: Vec::new(),
                    },
                    bindings: Vec::new(),
                    populating: Vec::new(),
                    writable_shared: 0,
                    exec_loads: 0,
                    write_opens: 0,
                    writeback_cursor: 0,
                    writeback_epoch: 0,
                }),
                resident: AtomicBool::new(false),
                dirty: AtomicBool::new(false),
                writeback_pending: AtomicBool::new(false),
                eviction_pending: AtomicBool::new(false),
                writeback_requests: AtomicU64::new(0),
                syncers: AtomicUsize::new(0),
            });
        Ok(Self { mapped_size: HostMappedSize(state.clone()), state })
    }
    pub fn cache_counts(&self) -> (usize, usize, usize) {
        let inner = self.state.inner.lock();
        (inner.pages.len(), inner.pages.entries.iter().filter(|(_, p)| p.dirty).count(),
            inner.bindings.len())
    }
    pub fn may_change_exec_image(&self) -> bool {
        self.has_dirty()
    }
    /// Model stores into a shared writable binding; snapshotting and copying
    /// these frames below is done by file_map.rs, not by the test's caller.
    pub fn shared_stores(&self, bytes: &[u8], writable: bool) {
        let mut inner = self.state.inner.lock();
        inner.pages.entries.clear();
        for (index, bytes) in bytes.chunks(4096).enumerate() {
            let mut storage = Box::new([0; 4096]);
            storage[..bytes.len()].copy_from_slice(bytes);
            let frame = PhysFrame(storage.as_ptr() as usize);
            inner.pages.entries.push((
                index as u64,
                CachePage {
                    frame,
                    _storage: storage,
                    dirty: true,
                    gen: 1,
                    written: None,
                    icache_clean: false,
                },
            ));
        }
        inner.writable_shared = usize::from(writable);
        inner.bindings = if writable {
            alloc::vec![BindingRec {
                id: 1,
                pgoff: 0,
                pages: bytes.len().div_ceil(4096) as u64,
                shared: true,
                writable: true,
                may_write: true,
                exec: false,
                parked: Vec::new(),
                leaves: Vec::new(),
            }]
        } else {
            Vec::new()
        };
        self.state
            .eviction_pending
            .store(!writable, Ordering::Release);
        self.refresh(&inner);
    }
}
include!(concat!(env!("OUT_DIR"), "/mapping_entries.rs"));

// Only page-table publication/revocation and user accesses are modeled here.
// Overlay, prepare/commit, EOF transition and writeback above are production.
impl MapState {
    fn revoke(&self, first: u64, last: u64, aliases: bool) {
        let mut inner = self.state.inner.lock();
        let MapInner { pages, bindings, .. } = &mut *inner;
        for rec in bindings {
            rec.leaves.retain(|(index, frame)| {
                if *index < first || *index >= last { return true; }
                aliases && pages.get(*index).is_some_and(|p| p.dirty || p.frame != *frame)
            });
        }
    }
}
#[derive(Clone)]
pub struct HostBinding(Arc<HostBindingOwner>);
struct HostBindingOwner { node: Arc<VNode>, id: u64 }
#[derive(Debug, PartialEq, Eq)]
pub enum HostFault { Sigbus }
pub fn bind_host(node: Arc<VNode>, length: u64, shared: bool, writable: bool, populate: bool) -> HostBinding {
    try_bind_host(node, length, shared, writable, populate).unwrap()
}
/// mmap of the file, which the filesystem may refuse for shared write
/// authority (paired generations DEV-44).
pub fn try_bind_host(
    node: Arc<VNode>, length: u64, shared: bool, writable: bool, populate: bool,
) -> Result<HostBinding, Errno> {
    let io = node.filesystem.mapping_io(&node).unwrap();
    let bytes = io.read(0, length.min(io.size()) as usize).unwrap();
    let mut inner = node.map.state.inner.lock();
    for (index, bytes) in bytes.chunks(4096).enumerate() {
        let index = index as u64;
        if populate {
            let mut page = CachePage::allocate().unwrap();
            page._storage[..bytes.len()].copy_from_slice(bytes);
            inner.pages.try_insert(index, page).unwrap();
            inner.holes.remove(index);
        } else if !inner.pages.contains(index) && bytes.iter().all(|byte| *byte == 0) {
            // An unpopulated fixture binding stands for the sparse pages of
            // a file: the pages that read as 00 are recorded as Holes, as a
            // fill reports them (E3-4), and the others stay absent.
            inner.holes.try_insert(index, ()).unwrap();
        }
    }
    node.map.refresh(&inner);
    drop(inner);
    let id = publish_host(&node, &*io, length, shared, writable)?;
    Ok(HostBinding(Arc::new(HostBindingOwner { node, id })))
}
/// Publish a read-only private binding through the transfer that production
/// `populate` returned, as mmap's `map_file` does after population.
pub fn bind_populated(node: Arc<VNode>, io: &dyn MappingIo, length: u64) -> HostBinding {
    let id = publish_host(&node, io, length, false, false).unwrap();
    HostBinding(Arc::new(HostBindingOwner { node, id }))
}
/// A read-only private binding of the first `length` bytes, registered in the
/// map directly, for a filesystem whose host build has no mapping transfer
/// (ext2). It covers no cached page; use it for pages past EOF.
pub fn bind_unpopulated(node: Arc<VNode>, length: u64) -> HostBinding {
    let mut inner = node.map.state.inner.lock();
    let id = inner.bindings.iter().map(|r| r.id).max().unwrap_or(0) + 1;
    inner.bindings.push(BindingRec { id, pgoff: 0, pages: pages(length), shared: false,
        writable: false, may_write: false, exec: false, parked: Vec::new(), leaves: Vec::new() });
    drop(inner);
    HostBinding(Arc::new(HostBindingOwner { node, id }))
}
/// A shared read-only binding of the first `length` bytes mapped from a
/// descriptor open for writing: it keeps write authority, so mprotect can
/// make it writable after the descriptor closes (E3-3, design 10.3).
pub fn bind_shared_authority(node: Arc<VNode>, length: u64) -> HostBinding {
    // mmap populates the range before it binds (production `populate`), and
    // refuses a running image before it replaces anything.
    let _population = populate(&node, 0, pages(length)).unwrap_or_else(|_| panic!("population failed"));
    node.map.refuse_text_busy(true, true).unwrap();
    let mut inner = node.map.state.inner.lock();
    let id = inner.bindings.iter().map(|r| r.id).max().unwrap_or(0) + 1;
    inner.bindings.push(BindingRec { id, pgoff: 0, pages: pages(length), shared: true,
        writable: false, may_write: true, exec: false, parked: Vec::new(), leaves: Vec::new() });
    drop(inner);
    HostBinding(Arc::new(HostBindingOwner { node, id }))
}
// Page-table and VMA stand-ins for the production `ExecImage::map` and
// `entry_flags`. The host table records the entries exec installs; the flag
// bits are the x86-64 ones the kernel's names stand for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageTableFlags(u64);
impl PageTableFlags {
    pub const PRESENT: Self = Self(1);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const USER_ACCESSIBLE: Self = Self(1 << 2);
    pub const BIT_9: Self = Self(1 << 9);
    pub const NO_EXECUTE: Self = Self(1 << 63);
    pub fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
    pub fn insert(&mut self, other: Self) { self.0 |= other.0; }
    pub fn remove(&mut self, other: Self) { self.0 &= !other.0; }
}
impl core::ops::BitOr for PageTableFlags {
    type Output = Self;
    fn bitor(self, other: Self) -> Self { Self(self.0 | other.0) }
}
impl core::ops::BitOrAssign for PageTableFlags {
    fn bitor_assign(&mut self, other: Self) { self.0 |= other.0; }
}
/// The kernel's copy-on-write marker (`process_memory::COW_FLAG`).
pub const COW_FLAG: PageTableFlags = PageTableFlags::BIT_9;
/// mmap protection bits (PROT_READ 1, PROT_WRITE 2, PROT_EXEC 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Protection(u32);
impl Protection {
    pub const WRITE: Self = Self(2);
    pub const EXEC: Self = Self(4);
    pub fn from_bits_truncate(bits: u32) -> Self { Self(bits & 7) }
    pub fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
}
#[derive(Clone, Copy)]
pub struct MmapFlags(u32);
impl MmapFlags {
    pub const PRIVATE: Self = Self(2);
    pub fn bits(self) -> u32 { self.0 }
}
#[derive(Clone, Copy)]
pub struct VirtAddr(u64);
impl VirtAddr {
    pub fn new(address: u64) -> Self { Self(address) }
    pub fn as_u64(self) -> u64 { self.0 }
}
pub struct Page(u64);
impl Page {
    pub fn containing_address(address: VirtAddr) -> Self { Self(address.0 & !(PAGE_SIZE - 1)) }
}
pub struct ProcessId;
impl ProcessId {
    pub fn new(_: u64) -> Self { Self }
}
/// A VMA, and the binding that maps its file pages.
pub struct Vma {
    pub start: VirtAddr,
    pub end: VirtAddr,
    pub prot: Protection,
    pub flags: MmapFlags,
    pub backing: Option<HostBinding>,
}
impl Vma {
    pub fn new(start: VirtAddr, end: VirtAddr, prot: Protection, flags: MmapFlags) -> Self {
        Self { start, end, prot, flags, backing: None }
    }
    pub fn size(&self) -> u64 { self.end.0 - self.start.0 }
}
/// The entries installed into an unpublished table, by virtual page.
#[derive(Default)]
pub struct ProcessPageTable {
    pub entries: alloc::collections::BTreeMap<u64, (PhysFrame, PageTableFlags)>,
}
impl ProcessPageTable {
    pub fn map_page(&mut self, page: Page, frame: PhysFrame, flags: PageTableFlags) -> Result<(), &'static str> {
        match self.entries.insert(page.0, (frame, flags)) {
            None => Ok(()),
            Some(_) => Err("Page already mapped"),
        }
    }
}
/// The kernel `bind`'s binding record for `vma`, after the production
/// running-image refusal (`MapInner::refuse_text_busy`). Shared write claims
/// (DEV-44) are not modeled, because `ExecImage::map` makes private bindings.
#[allow(clippy::too_many_arguments)]
pub fn bind(
    handle: Arc<VNode>,
    _: ProcessId,
    _: &ProcessPageTable,
    vma: &Vma,
    pgoff: u64,
    may_write: bool,
    _: Option<core::convert::Infallible>,
    exec: bool,
) -> Result<HostBinding, &'static str> {
    let shared = vma.flags.bits() & 3 != 2;
    let mut inner = handle.map.state.inner.lock();
    inner.refuse_text_busy(shared, may_write)?;
    let id = inner.bindings.iter().map(|r| r.id).max().unwrap_or(0) + 1;
    inner.bindings.push(BindingRec { id, pgoff, pages: vma.size() / PAGE_SIZE, shared,
        writable: shared && vma.prot.contains(Protection::WRITE), may_write, exec,
        parked: Vec::new(), leaves: Vec::new() });
    drop(inner);
    Ok(HostBinding(Arc::new(HostBindingOwner { node: handle, id })))
}
fn publish_host(node: &VNode, io: &dyn MappingIo, length: u64, shared: bool, writable: bool) -> Result<u64, Errno> {
    let mut id = 0;
    io.publish(&mut |_| {
        let mut inner = node.map.state.inner.lock();
        if shared && writable {
            inner.gain_writable_shared(node).map_err(|_| Errno(crate::syscall_errno::EBUSY))?;
        }
        id = inner.bindings.iter().map(|r| r.id).max().unwrap_or(0) + 1;
        inner.bindings.push(BindingRec { id, pgoff: 0, pages: pages(length), shared, writable,
            may_write: writable, exec: false, parked: Vec::new(), leaves: Vec::new() });
        Ok(())
    })?;
    Ok(id)
}
impl HostBinding {
    /// mprotect adding PROT_WRITE to this binding.
    pub fn protect_write(&self) -> Result<(), Errno> {
        let mut inner = self.0.node.map.state.inner.lock();
        let slot = inner.bindings.iter().position(|r| r.id == self.0.id).unwrap();
        if inner.bindings[slot].writable {
            return Ok(());
        }
        if inner.bindings[slot].shared {
            inner.gain_writable_shared(&self.0.node).map_err(|_| Errno(crate::syscall_errno::EBUSY))?;
        }
        inner.bindings[slot].writable = true;
        Ok(())
    }
    pub fn load(&self, offset: u64) -> Result<u8, HostFault> {
        self.access(offset, None)
    }
    pub fn store(&self, offset: u64, byte: u8) -> Result<(), HostFault> {
        self.access(offset, Some(byte)).map(|_| ())
    }
    pub fn has_leaf(&self, index: u64) -> bool {
        let inner = self.0.node.map.state.inner.lock();
        inner.bindings.iter().find(|rec| rec.id == self.0.id).unwrap()
            .leaves.iter().any(|(i, _)| *i == index)
    }
    fn access(&self, offset: u64, store: Option<u8>) -> Result<u8, HostFault> {
        let map = &self.0.node.map;
        let mut inner = map.state.inner.lock();
        let index = offset / PAGE_SIZE;
        if index >= pages(inner.mapped_size) {
            return Err(HostFault::Sigbus);
        }
        let slot = inner.bindings.iter().position(|r| r.id == self.0.id).unwrap();
        assert!(inner.bindings[slot].covers(index));
        assert!(store.is_none() || inner.bindings[slot].writable);
        // The page a fault maps is production's choice (`fault_frame`, which
        // `resolve_page` calls); only the page-table install is modeled.
        let frame = inner.fault_frame(index).map_err(|_| HostFault::Sigbus)?;
        let shared = inner.bindings[slot].shared;
        let cache = inner.pages.get_mut(index).unwrap();
        if store.is_some() && shared {
            cache.dirty = true;
            cache.gen += 1;
        }
        let rec = &mut inner.bindings[slot];
        let frame = if shared || store.is_none() && !rec.parked.iter().any(|(i, _)| *i == index) {
            frame
        } else {
            if !rec.parked.iter().any(|(i, _)| *i == index) {
                let page = CachePage::allocate().unwrap();
                unsafe { core::ptr::copy_nonoverlapping(frame.0 as *const u8, page.ptr(), 4096) };
                rec.parked.push((index, page));
            }
            rec.parked.iter().find(|(i, _)| *i == index).unwrap().1.frame
        };
        if !rec.leaves.iter().any(|(i, _)| *i == index) { rec.leaves.push((index, frame)); }
        let ptr = (frame.0 as *mut u8).wrapping_add((offset % PAGE_SIZE) as usize);
        if let Some(byte) = store { unsafe { ptr.write(byte) }; }
        let byte = unsafe { ptr.read() };
        map.refresh(&inner);
        Ok(byte)
    }
}
