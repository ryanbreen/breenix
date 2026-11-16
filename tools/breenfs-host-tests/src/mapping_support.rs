// Pinned host frames for the production file_map writeback entry points.
use crate::{
    errno::Errno,
    vfs::{FileSystem, VNode},
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
    pub mapped_size: Arc<AtomicU64>,
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
    pages: PageVec<CachePage>,
    poisoned: PageVec<()>,
    bindings: Vec<BindingRec>,
    writable_shared: usize,
    writeback_cursor: u64,
    writeback_epoch: u64,
}
struct PageVec<T> {
    entries: Vec<(u64, T)>,
}
struct BindingRec {
    first: u64,
    last: u64,
}
impl BindingRec {
    fn covers(&self, index: u64) -> bool {
        self.first <= index && index < self.last
    }
}
impl<T> PageVec<T> {
    fn retain(&mut self, keep: impl Fn(u64) -> bool) {
        self.entries.retain(|(i, _)| keep(*i));
    }
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    fn range(&self, first: u64, last: u64) -> &[(u64, T)] {
        let start = self.entries.partition_point(|(i, _)| *i < first);
        let end = self.entries.partition_point(|(i, _)| *i < last);
        &self.entries[start..end]
    }
    fn get_mut(&mut self, index: u64) -> Option<&mut T> {
        self.entries
            .iter_mut()
            .find(|(i, _)| *i == index)
            .map(|(_, page)| page)
    }
}
#[derive(Clone, Copy)]
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
    written_gen: Option<u64>,
}
impl MapState {
    pub fn new(size: u64, _epoch: u64, filesystem: Arc<dyn FileSystem>) -> Result<Self, Errno> {
        Ok(Self {
            state: Arc::new(MapShared {
                filesystem,
                inner: Mutex::new(MapInner {
                    pages: PageVec {
                        entries: Vec::new(),
                    },
                    poisoned: PageVec {
                        entries: Vec::new(),
                    },
                    bindings: Vec::new(),
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
            }),
            mapped_size: Arc::new(AtomicU64::new(size)),
        })
    }
    pub fn resync_unbound(&self, size: u64) {
        self.mapped_size.store(size, Ordering::Release);
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
                    written_gen: None,
                },
            ));
        }
        inner.writable_shared = usize::from(writable);
        self.refresh(&inner);
    }
}
include!(concat!(env!("OUT_DIR"), "/mapping_entries.rs"));
