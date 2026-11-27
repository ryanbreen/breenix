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
    poisoned: PageVec<()>,
    bindings: Vec<BindingRec>,
    populating: Vec<Claim>,
    writable_shared: usize,
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
#[derive(Clone, Copy, PartialEq, Eq)]
struct PhysFrame(usize);
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
            dirty: false, gen: 0, written: None })
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
                    poisoned: PageVec {
                        entries: Vec::new(),
                    },
                    bindings: Vec::new(),
                    populating: Vec::new(),
                    writable_shared: 0,
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
    let io = node.filesystem.mapping_io(&node).unwrap();
    if populate {
        let bytes = io.read(0, length.min(io.size()) as usize).unwrap();
        let mut inner = node.map.state.inner.lock();
        for (index, bytes) in bytes.chunks(4096).enumerate() {
            let mut page = CachePage::allocate().unwrap();
            page._storage[..bytes.len()].copy_from_slice(bytes);
            inner.pages.try_insert(index as u64, page).unwrap();
        }
        node.map.refresh(&inner);
    }
    let id = publish_host(&node, &*io, length, shared, writable);
    HostBinding(Arc::new(HostBindingOwner { node, id }))
}
/// Publish a read-only private binding through the transfer that production
/// `populate` returned, as mmap's `map_file` does after population.
pub fn bind_populated(node: Arc<VNode>, io: &dyn MappingIo, length: u64) -> HostBinding {
    let id = publish_host(&node, io, length, false, false);
    HostBinding(Arc::new(HostBindingOwner { node, id }))
}
fn publish_host(node: &VNode, io: &dyn MappingIo, length: u64, shared: bool, writable: bool) -> u64 {
    let mut id = 0;
    io.publish(&mut |_| {
        let mut inner = node.map.state.inner.lock();
        id = inner.bindings.iter().map(|r| r.id).max().unwrap_or(0) + 1;
        inner.bindings.push(BindingRec { id, pgoff: 0, pages: pages(length), shared, writable,
            parked: Vec::new(), leaves: Vec::new() });
        inner.writable_shared += usize::from(shared && writable);
        Ok(())
    }).unwrap();
    id
}
impl HostBinding {
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
        if index >= pages(inner.mapped_size) || inner.poisoned.contains(index) {
            return Err(HostFault::Sigbus);
        }
        let slot = inner.bindings.iter().position(|r| r.id == self.0.id).unwrap();
        assert!(inner.bindings[slot].covers(index));
        assert!(store.is_none() || inner.bindings[slot].writable);
        // Absent pages in these fixtures are holes, already read as 0 by
        // the production MappingIo, or pages made valid by a bound extension.
        inner.pages.try_insert(index, CachePage::allocate().unwrap()).unwrap();
        let shared = inner.bindings[slot].shared;
        let cache = inner.pages.get_mut(index).unwrap();
        let frame = cache.frame;
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
