//! BreenFS operations at the VFS boundary (design 1.1, 2.3 and 2.5).
use super::preempt::preemption_point;
#[path = "node_events.rs"]
mod node_events;
use node_events::{Events, NodeEvents, Stamps, TouchedTimes};
#[path = "mutation_cost.rs"]
mod mutation_cost;
use mutation_cost::{Costs, Op as CostOp, Phase as CostPhase};
use super::{
    abi::*,
    scrub::{self, Scrub},
    commit::{Committer, FrozenCommit, PublicationError},
    commit_policy::{CommitPolicy, COMMIT_INTERVAL_NS, DIRTY_BUDGET},
    commit_signal::{monotonic_ns, CommitSignal},
    dirty::DirtyPages,
    format::{self, Inode, Root, Superblock, Time, BLOCK_SIZE},
    mount_lock::{MountGuard, MountLock},
    node_cache::CachedNodeStore,
    ops::{self, Files, ReadError},
    recover,
    reserve::{Estimate, Grant, Ledger, Operation},
    space::PendingSummary,
};
use crate::{
    block::{
        cache::{BufferCache, Wait, WriteGroup},
        BlockDevice, BlockError,
    },
    fs::{attrs::InodeAttrs, errno::Errno, vfs::*},
    syscall::errno::*,
};
use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{cell::Cell, ops::Deref, sync::atomic::{AtomicBool, AtomicU64, Ordering}};
use spin::Mutex;

#[cfg(test)]
pub mod measurement {
    use alloc::vec::Vec;
    std::thread_local! {
        static COMMITS: core::cell::Cell<[u64; 3]> = const { core::cell::Cell::new([0; 3]) };
        static EVENTS: core::cell::RefCell<Option<Vec<Event>>> = const { core::cell::RefCell::new(None) };
        static MATERIALIZED: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
    }
    /// Replayed changes and refreshed inodes since the last take (x11c M2).
    pub fn take_materialized() -> u64 { MATERIALIZED.with(|n| n.replace(0)) }
    pub(super) fn materialized() { MATERIALIZED.with(|n| n.set(n.get() + 1)); }
    // Host-only observations for coordinator E24 ledger 544. Disabled unless
    // the cost replay explicitly takes events; no guest code is generated.
    #[derive(Debug)]
    pub enum Event {
        Refresh { objects: Vec<u64>, visited: Vec<u64>, indexed_at_entry: Vec<u64> },
        Admission { attempts: Vec<Attempt>, pressure: Vec<bool> },
    }
    #[derive(Debug)]
    pub struct Attempt { pub error: Option<i32>, pub refusal: Option<[u64; 5]> }
    pub fn take_events() -> Vec<Event> {
        EVENTS.with(|rows| rows.replace(Some(Vec::new())).unwrap_or_default())
    }
    fn enabled() -> bool { EVENTS.with(|rows| rows.borrow().is_some()) }
    fn record(event: Event) {
        EVENTS.with(|rows| { if let Some(rows) = rows.borrow_mut().as_mut() { rows.push(event); } });
    }
    pub(super) struct Refresh(Option<(Vec<u64>, Vec<u64>, Vec<u64>)>);
    impl Refresh {
        pub(super) fn new(objects: &[u64], is_indexed: impl Fn(u64) -> bool) -> Self {
            Self(enabled().then(|| {
                // Coordinator 551: snapshot indexed membership independently
                // of the subsequent loop that records actual refresh visits.
                let indexed_at_entry = objects.iter().enumerate()
                    .filter(|(index, ino)| !objects[..*index].contains(ino) && is_indexed(**ino))
                    .map(|(_, &ino)| ino).collect();
                (objects.to_vec(), Vec::new(), indexed_at_entry)
            }))
        }
        pub(super) fn visit(&mut self, ino: u64) {
            if let Some((_, visited, _)) = &mut self.0 { visited.push(ino); }
        }
    }
    impl Drop for Refresh {
        fn drop(&mut self) {
            if let Some((objects, visited, indexed_at_entry)) = self.0.take() {
                record(Event::Refresh { objects, visited, indexed_at_entry });
            }
        }
    }
    pub(super) struct Admission(Option<(Vec<Attempt>, Vec<bool>)>);
    impl Admission {
        pub(super) fn new() -> Self {
            Self(enabled().then(|| {
                super::super::reserve::observation::take();
                (Vec::new(), Vec::new())
            }))
        }
        pub(super) fn attempt(&mut self, error: Option<i32>) {
            if let Some((attempts, _)) = &mut self.0 {
                attempts.push(Attempt { error, refusal: super::super::reserve::observation::take() });
            }
        }
        pub(super) fn pressure(&mut self, published: bool) {
            if let Some((_, pressure)) = &mut self.0 { pressure.push(published); }
        }
    }
    impl Drop for Admission {
        fn drop(&mut self) {
            if let Some((attempts, pressure)) = self.0.take() { record(Event::Admission { attempts, pressure }); }
        }
    }
    pub(super) fn prepared(reservation: bool, dirty: bool) {
        COMMITS.with(|cell| { let mut n = cell.get(); n[0] += u64::from(reservation);
            n[1] += u64::from(dirty); cell.set(n); });
    }
    pub(super) fn published() {
        COMMITS.with(|cell| { let mut n = cell.get(); n[2] += 1; cell.set(n); });
    }
    pub fn heap_live() -> isize { super::dirty_heap_bound::live() }
    pub fn take_work() -> [u64; 3] { COMMITS.with(|n| n.replace([0; 3])) }
    pub fn scrub_step<W: crate::block::cache::Wait + 'static>(
        fs: &super::BreenFs<W>, budget: usize,
    ) -> Result<bool, crate::fs::errno::Errno> {
        fs.scrub_pass(budget).map(|(running, _)| running)
    }
    pub fn service<W: crate::block::cache::Wait + 'static>(
        fs: &super::BreenFs<W>,
    ) -> Result<bool, crate::fs::errno::Errno> {
        fs.commit_due(0)
    }
    pub fn service_at<W: crate::block::cache::Wait + 'static>(
        fs: &super::BreenFs<W>, now_ns: u64,
    ) -> Result<(bool, bool), crate::fs::errno::Errno> {
        let observed = fs.inner.signal.worker.epoch();
        fs.commit_due(now_ns).map(|worked| (worked, fs.inner.signal.worker.epoch() != observed))
    }
}

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/failed_mount_teardown.rs"]
mod failed_mount_teardown;
#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/dirty_heap_bound.rs"]
mod dirty_heap_bound;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/mapped_writeback.rs"]
mod mapped_writeback;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/mount_exclusion.rs"]
mod mount_exclusion;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/metadata_admission.rs"]
mod metadata_admission;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/outstanding_grants.rs"]
mod outstanding_grants;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/publication_trigger.rs"]
mod publication_trigger;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/incremental_files_view.rs"]
mod incremental_files_view;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/publication_phases.rs"]
pub(super) mod publication_phases;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/publication_cost.rs"]
pub(super) mod publication_cost;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/metadata_failure.rs"]
mod metadata_failure;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/frozen_generation.rs"]
mod frozen_generation;

#[cfg(test)]
#[path = "../../../../tools/breenfs-host-tests/src/frozen_review.rs"]
mod frozen_review;

// Extended attributes use ABI errno 61 for an absent attribute.
const ENODATA: i32 = 61;

// Shared across cache/wait implementations and both boot and explicit mounts.
// Admission releases the registry lock before recovery I/O.
static WRITABLE_OWNERS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

// The short registry lock protects weak device references, not device I/O.
static MOUNTED_DEVICES: Mutex<Vec<Weak<DeviceMounts>>> = Mutex::new(Vec::new());
struct DeviceMounts {
    identity: usize,
    roots: Mutex<Vec<u64>>,
    // A delete holds this through durable ROOT removal. A constructor cannot
    // select the old on-disk ROOT between the busy check and publication.
    selection: MountLock<()>,
    // Snapshot data reads need current REF checksums. SPACE nodes from their
    // original generation can be reused even while the FS root remains live.
    // Serialize those reads with publication, using sleeping exclusion.
    publication: MountLock<Option<Superblock>>,
}
impl DeviceMounts {
    fn get(identity: usize) -> Result<Arc<Self>, Errno> {
        let mut devices = MOUNTED_DEVICES.lock();
        devices.retain(|device| device.strong_count() != 0);
        if let Some(device) = devices.iter().filter_map(Weak::upgrade)
            .find(|device| device.identity == identity)
        {
            return Ok(device);
        }
        let device = Arc::try_new(Self {
            identity,
            roots: Mutex::new(Vec::new()),
            selection: MountLock::new(()),
            publication: MountLock::new(None),
        }).map_err(|_| Errno(ENOMEM))?;
        devices.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
        devices.push(Arc::downgrade(&device));
        Ok(device)
    }
}
struct MountedRoot {
    device: Arc<DeviceMounts>,
    id: u64,
}
impl MountedRoot {
    fn claim(device: Arc<DeviceMounts>, id: u64) -> Result<Self, Errno> {
        {
            let mut roots = device.roots.lock();
            roots.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
            roots.push(id);
        }
        Ok(Self { device, id })
    }
}
impl Drop for MountedRoot {
    fn drop(&mut self) {
        let mut roots = self.device.roots.lock();
        if let Some(index) = roots.iter().position(|&id| id == self.id) {
            roots.swap_remove(index);
        }
    }
}

struct WritableOwner(usize);
impl WritableOwner {
    fn claim(identity: usize) -> Result<Self, Errno> {
        let mut owners = WRITABLE_OWNERS.lock();
        // Reject an existing writer before allocating or entering recovery.
        if owners.contains(&identity) {
            return Err(Errno(EBUSY));
        }
        owners.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
        owners.push(identity);
        Ok(Self(identity))
    }
}
impl Drop for WritableOwner {
    fn drop(&mut self) {
        let mut owners = WRITABLE_OWNERS.lock();
        if let Some(index) = owners.iter().position(|&identity| identity == self.0) {
            owners.swap_remove(index);
        }
    }
}

/// Clones retain the same mount and transaction exclusion.
pub struct BreenFs<W: Wait + 'static> {
    inner: Arc<Mount<W>>,
}
struct Mount<W: Wait + 'static> {
    events: Arc<NodeEvents>,
    costs: Costs,
    state: MountLock<MountState<W>>,
    device: Arc<dyn BlockDevice>,
    owner: Option<WritableOwner>,
    data: Box<DataBlocks>,
    cache: Cache<W>,
    id: usize,
    root_id: u64,
    mounted_root: MountedRoot,
    default_root: Option<MountedRoot>,
    frozen: bool,
    writable: bool,
    signal: Arc<CommitSignal>,
    scrub_signal: Arc<CommitSignal>,
    // The open generation, read by touch() without the mount exclusion; the
    // freeze advances it (E24 coordinator 601(2)).
    generation: AtomicU64,
    // Install left N+1's replay or N's per-file cleanup to the next holder.
    materialize: AtomicBool,
    mapping_passes: AtomicU64,
    mapping_pages_evicted: AtomicU64,
    mapping_requests_dropped: AtomicU64,
    #[cfg(any(feature = "breenfs_commit_window", feature = "breenfs_corrupt_inject"))]
    commit_faults: Option<super::commit::faults::Faults>,
}
#[cfg(not(test))]
struct ReportCount(Option<usize>);
#[cfg(not(test))]
impl core::fmt::Display for ReportCount {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(count) => write!(f, "{count}"),
            None => f.write_str("unavailable"),
        }
    }
}
enum Cache<W: Wait + 'static> {
    Shared(Arc<BufferCache<W>>),
    Kernel(&'static BufferCache<W>),
}
impl<W: Wait> Cache<W> {
    fn buffers(&self) -> &BufferCache<W> {
        match self {
            Self::Shared(cache) => cache,
            Self::Kernel(cache) => cache,
        }
    }
    fn store(&self, device: Arc<dyn BlockDevice>, sb: &Superblock) -> CachedNodeStore<'static, W> {
        match self {
            Self::Shared(cache) => CachedNodeStore::new_owned(cache.clone(), device, sb),
            Self::Kernel(cache) => CachedNodeStore::with_device(cache, device, sb),
        }
    }
}
enum MountFailure {
    Other(Errno),
    Publication(PublicationError),
}

impl From<Errno> for MountFailure {
    fn from(error: Errno) -> Self { Self::Other(error) }
}

impl From<PublicationError> for MountFailure {
    fn from(error: PublicationError) -> Self { Self::Publication(error) }
}

struct MountState<W: Wait + 'static> {
    files: OpenFiles<W>,
    // N's next view, installed while N+1's scratch changes await replay.
    // Each holder of the exclusion replays a bounded share (x11c M2).
    next_files: Option<Files<'static, CachedNodeStore<'static, W>>>,
    // Installed N's pages, readable until that replay completes: the
    // scratch view does not hold N's drained extents.
    installed: Option<Arc<FrozenPages>>,
    // The scratch view and N's pages, once replaced: the commit worker drops
    // them outside the exclusion.
    stale_views: Option<StaleViews<W>>,
    // Inodes whose NodeState and VNode attributes a published generation
    // changed outside the exclusion: refreshed a bounded share per holder.
    refresh: BTreeSet<u64>,
    committer: Option<Committer>,
    durable: Superblock,
    durable_pending: Option<PendingSummary>,
    failed: bool,
    failure: Option<(&'static str, i32)>,
    metadata_error: Option<super::btree::ReadFailure>,
    read_refusal_reported: bool,
    scrub: Option<Scrub>,
    scrub_state: u32,
    mount: usize,
    closed: bool,
    nodes: BTreeMap<u64, NodeState>,
    events: Arc<NodeEvents>,
    retained_events: Events,
    retained_batches: u64,
    retaining_for_sync: usize,
    reservation_pressure: bool,
    // The open generation's custody (601(2)); freeze moves each whole.
    touches: BTreeMap<u64, Arc<TouchedTimes>>,
    dirty: BTreeMap<u64, GenPages>,
    dirty_nodes: BTreeSet<u64>,
    dirty_bytes: u64,
    // True only before the open transaction's first filesystem change.
    fresh: bool,
    roots: BTreeMap<u64, (Vec<u8>, u32)>,
    dead_remaining: u64,
    skip_reap: bool,
    reap_shutdown: bool,
    inodes: InodeCache,
    policy: CommitPolicy,
    mapping_retry_ns: u64,
    // The frozen generation's pages while it is in flight: readers look
    // here after the open generation and before the published view.
    pending: Option<Arc<FrozenPages>>,
    reservations: SharedLedger,
    grants: Grants,
    metadata_reserved: u64,
    first_write_space: u64,
    // SPACE funded independently of the FS allocations the scratch log owes.
    replay_space: u64,
    drain_growth: u64,
    signal: Arc<CommitSignal>,
}
// Mapping pins are held separately until cache retirement or stopped-service
// teardown. Dirty pages are per generation, in MountState.dirty.
#[derive(Default)]
struct NodeState {
    node: Weak<VNode>,
    links: u32,
    // A linked inode keeps its cache until dirty data is durable and clean
    // pages can be evicted. An orphan is owned only by its remaining users.
    mapped: Option<Arc<VNode>>,
    touched: Option<Arc<TouchedTimes>>,
    // A write admitted in parts (N2) holds other writes and resizes of this
    // file off between its parts.
    splitting: bool,
}
/// One generation's dirty pages for one file (E24 coordinator 601(2)).
#[derive(Default)]
struct GenPages {
    pages: DirtyPages,
    reserved_metadata: u64,
    // This generation truncated the file while an older generation was
    // frozen: that generation's pages from this index on are not its bytes.
    hide_frozen_from: Option<u64>,
}

struct DataGrant {
    grant: Grant,
    metadata: u64,
    first_write_space: u64,
    funded_debt: u64,
    replay_space: Option<u64>,
    drain_growth: u64,
}
struct MetadataGrant {
    grant: Grant,
    replay_space: Option<u64>,
}
impl From<Grant> for MetadataGrant {
    fn from(grant: Grant) -> Self { Self { grant, replay_space: None } }
}
#[derive(Debug)]
struct DeniedGrant {
    charged: u64,
    request: u64,
}
#[derive(Debug)]
enum AdmissionError {
    Denied(DeniedGrant),
    Other(Errno),
}
impl From<Errno> for AdmissionError {
    fn from(error: Errno) -> Self { Self::Other(error) }
}
impl From<AdmissionError> for Errno {
    fn from(error: AdmissionError) -> Self { error.errno() }
}
impl AdmissionError {
    fn errno(&self) -> Errno {
        match self {
            Self::Denied(_) => Errno(ENOSPC),
            Self::Other(error) => *error,
        }
    }
}
/// A frozen generation's pages, immutable and readable without exclusion.
struct FrozenPages {
    nodes: BTreeMap<u64, GenPages>,
    bytes: u64,
}
/// The reservation ledger, shared by foreground admission and the frozen
/// generation's publication (design 2.8: admission counts both generations'
/// grants until install). Each operation is a few comparisons under a spin
/// lock; no caller yields or waits while holding it.
// Changing methods take `&mut self`, as Ledger's do: each holder of a handle
// (the mount state or one frozen generation) changes it as its owner would.
#[derive(Clone)]
struct SharedLedger(Arc<Mutex<Ledger>>);
impl SharedLedger {
    fn outstanding(&self) -> u64 { self.0.lock().outstanding() }
    fn available(&self, free_blocks: u64) -> u64 { self.0.lock().available(free_blocks) }
    fn reserve(&mut self, free_blocks: u64, blocks: u64, operation: Operation)
        -> Result<Grant, DeniedGrant> {
        let mut ledger = self.0.lock();
        ledger.reserve(free_blocks, blocks, operation)
            .map_err(|_| DeniedGrant { charged: ledger.charged(), request: blocks })
    }
    fn settle(&mut self, grant: &mut Grant, blocks: u64) -> Result<(), super::space::Error> {
        self.0.lock().settle(grant, blocks)
    }
    fn release(&mut self, grant: Grant) { self.0.lock().release(grant) }
    fn retire(&mut self, blocks: u64) { self.0.lock().retire(blocks) }
    fn release_retired(&mut self, grant: Grant) { self.0.lock().release_retired(grant) }
}
/// One generation's retained grants and their total, so install can retire
/// the total without walking the list (x11c M3).
#[derive(Default)]
struct Grants {
    list: Vec<Grant>,
    blocks: u64,
}
impl Grants {
    fn try_reserve(&mut self, additional: usize) -> Result<(), alloc::collections::TryReserveError> {
        self.list.try_reserve(additional)
    }
    fn push(&mut self, grant: Grant) {
        self.blocks += grant.blocks();
        self.list.push(grant);
    }
    fn is_empty(&self) -> bool { self.list.is_empty() }
    // The x1 guard (outstanding_grants.rs) plants and removes a grant.
    #[cfg(test)]
    fn pop(&mut self) -> Option<Grant> {
        let grant = self.list.pop()?;
        self.blocks -= grant.blocks();
        Some(grant)
    }
}
impl IntoIterator for Grants {
    type Item = Grant;
    type IntoIter = alloc::vec::IntoIter<Grant>;
    fn into_iter(self) -> Self::IntoIter { self.list.into_iter() }
}
type FilesView<W> = Files<'static, CachedNodeStore<'static, W>>;
type StaleViews<W> = (FilesView<W>, Option<Arc<FrozenPages>>);
/// Replayed changes plus refreshed inodes per holder of the exclusion after an
/// install (x11c M2). A holder's own call records at most a few changes, so
/// the replay still gains on the record.
const MATERIALIZE_STEPS: usize = 8;
/// Maximum detached node notifications processed per retain-open hold (E24 607).
pub(crate) const RETAIN_OPEN_STEPS: usize = 8;
/// The open generation's Files view (E24 coordinator 597/601). Normally it is
/// the mount's transaction. While generation N is frozen, N+1 runs on a
/// scratch fork of N's frozen view (`log` is Some): its changes describe N+1
/// to foreground readers, allocate no real block, and are recorded. After N
/// is installed the log is replayed, in order and with identical results, on
/// N's published view; N's preparation never changes what N+1 reads.
struct OpenFiles<W: Wait + 'static> {
    files: FilesView<W>,
    log: Option<ReplayLog>,
    replay_bounds: Option<ReplayBounds>,
}
/// Records per replay page: 15 × 272 bytes is one 4,080-byte heap block.
const REPLAY_PAGE_RECORDS: usize = 15;
const _: () = assert!(REPLAY_PAGE_RECORDS * core::mem::size_of::<Change>() <= 4096 - 8);
/// N+1's recorded changes, oldest first (E24 coordinator 764, k2). Records
/// live in fixed pages, each its own fallible allocation, so a long frozen
/// window asks the heap for one page at a time instead of doubling a single
/// buffer (17,825,792 bytes at 65,536 records). The page table adds one
/// 32-byte entry per page. Callers reserve before they change the open
/// view: a refused page returns ENOMEM before the view changes, and
/// `push_back` then fills reserved room without allocating.
struct ReplayLog {
    pages: VecDeque<VecDeque<Change>>,
    len: usize,
}
impl ReplayLog {
    const fn new() -> Self { Self { pages: VecDeque::new(), len: 0 } }
    fn len(&self) -> usize { self.len }
    fn is_empty(&self) -> bool { self.len == 0 }
    /// Room for `additional` (at most one page) more records in the last page.
    fn reserve(&mut self, additional: usize) -> Result<(), ops::Error> {
        debug_assert!(additional <= REPLAY_PAGE_RECORDS);
        if self.pages.back().is_some_and(|page| page.capacity() - page.len() >= additional) {
            return Ok(());
        }
        self.pages.try_reserve(1).map_err(|_| ops::Error::ENOMEM)?;
        let mut page = VecDeque::new();
        page.try_reserve_exact(REPLAY_PAGE_RECORDS).map_err(|_| ops::Error::ENOMEM)?;
        self.pages.push_back(page);
        Ok(())
    }
    fn push_back(&mut self, change: Change) {
        let page = self.pages.back_mut().filter(|page| page.len() < page.capacity());
        debug_assert!(page.is_some(), "replay record without a reservation");
        match page {
            Some(page) => page.push_back(change),
            None => {
                let mut page = VecDeque::with_capacity(REPLAY_PAGE_RECORDS);
                page.push_back(change);
                self.pages.push_back(page);
            }
        }
        self.len += 1;
    }
    // Only the last page can be empty: reserve adds a page only after a
    // nonempty one, and a drained earlier page is dropped here.
    fn pop_front(&mut self) -> Option<Change> {
        let page = self.pages.front_mut()?;
        let change = page.pop_front()?;
        if page.is_empty() && self.pages.len() > 1 {
            self.pages.pop_front();
        }
        self.len -= 1;
        Some(change)
    }
}
struct ReplayBounds {
    frozen_root: format::BlockPtr,
    root_growth: Option<super::btree::RootGrowth>,
    growing_edits: u64,
    space_growth_bytes: u64,
}
impl<W: Wait + 'static> Deref for OpenFiles<W> {
    type Target = FilesView<W>;
    fn deref(&self) -> &Self::Target { &self.files }
}
/// One recorded open-generation change, replayed through the same Files call.
enum Change {
    // Fields this change set; the replay keeps the published view's others
    // (N's drained nblocks and applied touches).
    Inode { ino: u64, before: Inode, after: Inode },
    Touch { ino: u64, older: Stamps, open: Stamps },
    Open { ino: u64, open: bool },
    Retain(BTreeSet<u64>),
    Snapshot { name: Vec<u8>, read_only: bool, id: u64 },
    CreateRoot { name: Vec<u8>, at: Time, id: u64 },
    RenameRoot { from: Vec<u8>, to: Vec<u8> },
    DeleteRoot { name: Vec<u8> },
    Setattr { ino: u64, attrs: ops::SetAttr, at: Time },
    Create { dir: u64, name: Vec<u8>, inode: Inode, target: Option<Vec<u8>>, at: Time, ino: u64 },
    Link { ino: u64, dir: u64, name: Vec<u8>, at: Time },
    Remove { dir: u64, name: Vec<u8>, directory: bool, at: Time },
    Rename { from_dir: u64, from: Vec<u8>, to_dir: u64, to: Vec<u8>, at: Time },
    Trim { ino: u64, size: u64 },
    Xattr { ino: u64, name: Vec<u8>, value: Option<Vec<u8>>, at: Time },
}
impl Change {
    fn replay<W: Wait + 'static>(self, files: &mut FilesView<W>) -> Result<(), ops::Error> {
        match self {
            Self::Inode { ino, before, after } => {
                let mut inode = files.stored_inode(ino)?;
                merge_inode(&mut inode, &before, &after);
                files.put_inode(ino, inode)
            }
            Self::Touch { ino, older, open } => apply_inode_touches(files, ino, older, open).map(|_| ()),
            Self::Open { ino, open } => { files.set_inode_open(ino, open); Ok(()) }
            Self::Retain(inodes) => { files.retain_open_inodes(inodes); Ok(()) }
            Self::Snapshot { name, read_only, id } => {
                if files.snapshot(&name, read_only)? == id { Ok(()) } else { Err(ops::Error::EIO) }
            }
            Self::CreateRoot { name, at, id } => {
                if files.create_empty_root(&name, at)? == id { Ok(()) } else { Err(ops::Error::EIO) }
            }
            Self::RenameRoot { from, to } => files.rename_root(&from, &to),
            Self::DeleteRoot { name } => files.delete_root(&name),
            Self::Setattr { ino, attrs, at } => files.setattr(ino, &attrs, at),
            Self::Create { dir, name, inode, target, at, ino } => {
                let created = files.create_node_prepared(dir, &name, inode, target.as_deref(), at, |_, _| Ok(()))?;
                if created == ino { Ok(()) } else { Err(ops::Error::EIO) }
            }
            Self::Link { ino, dir, name, at } => files.link(ino, dir, &name, at),
            Self::Remove { dir, name, directory: true, at } => files.rmdir(dir, &name, at),
            Self::Remove { dir, name, directory: false, at } => files.unlink(dir, &name, at),
            Self::Rename { from_dir, from, to_dir, to, at } => files.rename(from_dir, &from, to_dir, &to, at),
            Self::Trim { ino, size } => {
                // The published view's extents decide nblocks, including N's.
                let nblocks = files.trim_mappings(ino, size)?;
                let mut inode = files.stored_inode(ino)?;
                inode.nblocks = nblocks;
                files.put_inode(ino, inode)
            }
            Self::Xattr { ino, name, value, at } => files.xattr(ino, &name, value.as_deref(), at),
        }
    }
}
// nblocks is derived from extents; only preparation and truncation set it.
fn merge_inode(current: &mut Inode, before: &Inode, after: &Inode) {
    macro_rules! merge {
        ($($field:ident),*) => { $(if after.$field != before.$field { current.$field = after.$field; })* };
    }
    merge!(mode, nlink, uid, gid, size, parent, rdev, next_index, created_generation,
        atime, mtime, ctime, btime);
}
impl<W: Wait + 'static> OpenFiles<W> {
    /// Room for `records` changes, taken before the open view changes. An
    /// operation that records twice reserves both at admission.
    fn reserve(&mut self, records: usize) -> Result<(), ops::Error> {
        self.log.as_mut().map_or(Ok(()), |log| log.reserve(records))
    }
    /// The recorded copy of a name or value, taken before the view changes.
    fn copy(&self, bytes: &[u8]) -> Result<Vec<u8>, ops::Error> {
        let mut copy = Vec::new();
        if self.log.is_some() {
            copy.try_reserve_exact(bytes.len()).map_err(|_| ops::Error::ENOMEM)?;
            copy.extend_from_slice(bytes);
        }
        Ok(copy)
    }
    /// A generation that will not be installed has no view to replay into.
    fn detach_replay(&mut self) -> Option<ReplayLog> {
        self.replay_bounds = None;
        self.log.take()
    }
    fn record(&mut self, change: impl FnOnce() -> Change) {
        let Some(log) = self.log.as_mut() else { return; };
        let change = change();
        if let Some(bounds) = self.replay_bounds.as_mut() {
            match &change {
                Change::Create { target, .. } => {
                    bounds.growing_edits = bounds.growing_edits.saturating_add(3 + u64::from(target.is_some()));
                }
                Change::Inode { .. } | Change::Open { .. } | Change::Retain(_) => (),
                // The tighter bound covers simple creates and first writes.
                // Other mutations can alter their replay paths and debt.
                _ => self.replay_bounds = None,
            }
        }
        log.push_back(change);
    }
    pub(super) fn put_inode(&mut self, ino: u64, inode: Inode) -> Result<(), ops::Error> {
        self.replay_bounds = None;
        self.put_reserved_inode(ino, inode)
    }
    fn put_reserved_inode(&mut self, ino: u64, inode: Inode) -> Result<(), ops::Error> {
        self.reserve(1)?;
        let before = if self.log.is_some() { Some(self.files.stored_inode(ino)?) } else { None };
        self.files.put_inode(ino, inode)?;
        if let Some(before) = before { self.record(|| Change::Inode { ino, before, after: inode }); }
        Ok(())
    }
    fn record_touches(&mut self, ino: u64, older: Stamps, open: Stamps) -> Result<bool, ops::Error> {
        self.reserve(1)?;
        let changed = apply_inode_touches(&mut self.files, ino, older, open)?;
        if changed { self.record(|| Change::Touch { ino, older, open }); }
        Ok(changed)
    }
    fn set_inode_open(&mut self, ino: u64, open: bool) -> Result<(), ops::Error> {
        self.reserve(1)?;
        self.files.set_inode_open(ino, open);
        self.record(|| Change::Open { ino, open });
        Ok(())
    }
    #[cfg(test)]
    fn retain_open_inodes(&mut self, inodes: BTreeSet<u64>) {
        if self.log.is_some() {
            self.reserve(1).expect("host replay page");
            self.record(|| Change::Retain(inodes.clone()));
        }
        self.files.retain_open_inodes(inodes);
    }
    fn snapshot(&mut self, name: &[u8], read_only: bool) -> Result<u64, ops::Error> {
        self.reserve(1)?;
        let copy = self.copy(name)?;
        let id = self.files.snapshot(name, read_only)?;
        self.record(|| Change::Snapshot { name: copy, read_only, id });
        Ok(id)
    }
    fn create_empty_root(&mut self, name: &[u8], at: Time) -> Result<u64, ops::Error> {
        self.reserve(1)?;
        let copy = self.copy(name)?;
        let id = self.files.create_empty_root(name, at)?;
        self.record(|| Change::CreateRoot { name: copy, at, id });
        Ok(id)
    }
    fn rename_root(&mut self, from: &[u8], to: &[u8]) -> Result<(), ops::Error> {
        self.reserve(1)?;
        let from_copy = self.copy(from)?;
        let to_copy = self.copy(to)?;
        self.files.rename_root(from, to)?;
        self.record(|| Change::RenameRoot { from: from_copy, to: to_copy });
        Ok(())
    }
    fn delete_root(&mut self, name: &[u8]) -> Result<(), ops::Error> {
        self.reserve(1)?;
        let copy = self.copy(name)?;
        self.files.delete_root(name)?;
        self.record(|| Change::DeleteRoot { name: copy });
        Ok(())
    }
    fn setattr(&mut self, ino: u64, attrs: &ops::SetAttr, at: Time) -> Result<(), ops::Error> {
        self.reserve(1)?;
        self.files.setattr(ino, attrs, at)?;
        self.record(|| Change::Setattr { ino, attrs: attrs.clone(), at });
        Ok(())
    }
    fn create_node_prepared(
        &mut self,
        dir: u64,
        name: &[u8],
        inode: Inode,
        target: Option<&[u8]>,
        at: Time,
        prepare: impl FnOnce(u64, Inode) -> Result<(), ops::Error>,
    ) -> Result<u64, ops::Error> {
        self.reserve(1)?;
        let copy = self.copy(name)?;
        let target_copy = target.map(|bytes| self.copy(bytes)).transpose()?;
        let ino = self.files.create_node_prepared(dir, name, inode, target, at, prepare)?;
        self.record(|| Change::Create { dir, name: copy, inode, target: target_copy, at, ino });
        Ok(ino)
    }
    fn link(&mut self, ino: u64, dir: u64, name: &[u8], at: Time) -> Result<(), ops::Error> {
        self.reserve(1)?;
        let copy = self.copy(name)?;
        self.files.link(ino, dir, name, at)?;
        self.record(|| Change::Link { ino, dir, name: copy, at });
        Ok(())
    }
    fn rmdir(&mut self, dir: u64, name: &[u8], at: Time) -> Result<(), ops::Error> {
        self.reserve(1)?;
        let copy = self.copy(name)?;
        self.files.rmdir(dir, name, at)?;
        self.record(|| Change::Remove { dir, name: copy, directory: true, at });
        Ok(())
    }
    fn unlink(&mut self, dir: u64, name: &[u8], at: Time) -> Result<(), ops::Error> {
        self.reserve(1)?;
        let copy = self.copy(name)?;
        self.files.unlink(dir, name, at)?;
        self.record(|| Change::Remove { dir, name: copy, directory: false, at });
        Ok(())
    }
    fn rename(&mut self, from_dir: u64, from: &[u8], to_dir: u64, to: &[u8], at: Time)
        -> Result<(), ops::Error> {
        self.reserve(1)?;
        let (from_copy, to_copy) = (self.copy(from)?, self.copy(to)?);
        self.files.rename(from_dir, from, to_dir, to, at)?;
        self.record(|| Change::Rename { from_dir, from: from_copy, to_dir, to: to_copy, at });
        Ok(())
    }
    fn trim_mappings(&mut self, ino: u64, size: u64) -> Result<u64, ops::Error> {
        self.reserve(1)?;
        let nblocks = self.files.trim_mappings(ino, size)?;
        self.record(|| Change::Trim { ino, size });
        Ok(nblocks)
    }
    fn xattr(&mut self, ino: u64, name: &[u8], value: Option<&[u8]>, at: Time) -> Result<(), ops::Error> {
        self.reserve(1)?;
        let copy = self.copy(name)?;
        let value_copy = value.map(|bytes| self.copy(bytes)).transpose()?;
        self.files.xattr(ino, name, value, at)?;
        self.record(|| Change::Xattr { ino, name: copy, value: value_copy, at });
        Ok(())
    }
}
// The VNode pin queues its final release without mount locking or I/O.
struct NodePin<W: Wait + 'static> {
    mount: Arc<Mount<W>>,
    touched: Arc<TouchedTimes>,
}
impl<W: Wait + 'static> Drop for NodePin<W> {
    fn drop(&mut self) { self.mount.events.push(&self.touched); }
}
// Bounded metadata cache; mutation completion invalidates its entries.
struct InodeCache {
    entries: [Option<(u64, Inode)>; 32],
    next: usize,
}
impl<W: Wait> MountState<W> {
    fn read_only(&self) -> bool {
        self.closed || self.failed || self.committer.as_ref().is_none_or(Committer::read_only)
    }
    fn writable(&self) -> Result<(), Errno> {
        if self.read_only() {
            Err(Errno(EROFS))
        } else {
            Ok(())
        }
    }
    /// The open generation: one past the published, and one more while a
    /// frozen generation is in flight (E24 coordinator 601(3)).
    fn open_generation(&self) -> u64 {
        self.durable.generation + 1 + u64::from(self.pending.is_some())
    }
    fn publication_blocked(&self) -> bool {
        self.pending.is_some() || self.retaining_for_sync != 0
    }
    fn apply_touches(&mut self, ino: u64) -> Result<(), Errno> {
        // A timestamp refresh can edit a private tree before returning.
        #[cfg(not(test))]
        let _custody = crate::task::thread::KillCustody::try_enter().ok_or(Errno(EINTR))?;
        preemption_point();
        let Some(touched) = self.nodes.get(&ino).and_then(|entry| entry.touched.as_ref())
        else {
            self.touches.remove(&ino);
            return Ok(());
        };
        let generation = self.open_generation();
        if self.files.log.is_some() && !self.failed
            && self.committer.as_ref().is_some_and(|committer| !committer.read_only())
        {
            // Publication can move a stamp from the atomic slots into the
            // frozen sample, so check the slots first and then the sample.
            if !touched.pending() && !touched.pending_frozen(generation - 1) {
                self.touches.remove(&ino);
                return Ok(());
            }
            // A refused replay record leaves the stamps pending and queued.
            self.files.reserve(1).map_err(errno)?;
        }
        self.touches.remove(&ino);
        let mut open = touched.take(generation);
        let mut older = Stamps::default();
        if self.files.log.is_some() {
            // Scratch was forked before N applied its deferred times. Inherit
            // those once, before this inode's first N+1 observation or edit.
            // This also covers the scratch view used during bounded replay.
            older = touched.inherit_frozen(generation - 1);
        } else {
            let late = touched.take_unsampled(generation - 1);
            open.access = open.access.max(late.access);
            open.modify = open.modify.max(late.modify);
        }
        if (older.is_empty() && open.is_empty())
            || self.failed
            || self.committer.as_ref().is_none_or(Committer::read_only)
        {
            return Ok(());
        }
        match self.files.record_touches(ino, older, open) {
            Ok(false) => Ok(()),
            result => self.edited(&[ino], result).map(|_| ()),
        }
    }
    /// The older generation's pages the open one reads through: frozen N's
    /// while in flight, installed N's until N+1's replay completes.
    fn frozen_pages(&self) -> Option<&Arc<FrozenPages>> {
        self.pending.as_ref().or(self.installed.as_ref())
    }
    /// Page `index` as the open generation sees it: its own dirty page, else
    /// the frozen generation's unless the open one truncated it away.
    fn page(&self, ino: u64, index: u64) -> Option<&[u8]> {
        let open = self.dirty.get(&ino);
        if let Some(page) = open.and_then(|node| node.pages.get(index)) {
            return Some(page);
        }
        if open.and_then(|node| node.hide_frozen_from).is_some_and(|from| index >= from) {
            return None;
        }
        self.frozen_pages()?.nodes.get(&ino)?.pages.get(index)
    }
    /// Page indexes visible through `page`, for st_blocks of unwritten data.
    fn page_indexes(&self, ino: u64) -> BTreeSet<u64> {
        let open = self.dirty.get(&ino);
        let hidden = open.and_then(|node| node.hide_frozen_from).unwrap_or(u64::MAX);
        let frozen = self.frozen_pages().and_then(|p| p.nodes.get(&ino));
        open.into_iter().flat_map(|node| node.pages.indexes())
            .chain(frozen.into_iter().flat_map(|node| node.pages.indexes()).filter(|&i| i < hidden))
            .collect()
    }
    fn inode(&mut self, ino: u64) -> Result<Inode, Errno> {
        self.apply_touches(ino)?;
        // VNode drops do no I/O or mount locking. Refresh the writer's open
        // snapshot here so a closed orphan cannot be opened again by inode.
        // A refused share stays queued; the final-close check below covers it.
        let _ = self.retain_open();
        let inode = if let Some((_, inode)) = self
            .inodes
            .entries
            .iter()
            .flatten()
            .find(|(cached, _)| *cached == ino)
        {
            *inode
        } else {
            let result = self.files.inode(ino);
            let inode = self.observed(result)?;
            let slot = self.inodes.next;
            self.inodes.entries[slot] = Some((ino, inode));
            self.inodes.next = (slot + 1) % self.inodes.entries.len();
            inode
        };
        // A final-close notification may still be in a later retention share.
        if inode.nlink == 0 && self.nodes.get(&ino).is_none_or(|node| node.node.strong_count() == 0) {
            return Err(Errno(ENOENT));
        }
        Ok(inode)
    }
    fn attrs(&mut self, ino: u64) -> Result<NodeAttrs, Errno> {
        let mut inode = self.inode(ino)?;
        let pages = self.page_indexes(ino);
        if !pages.is_empty() {
            let result = self.files.extents(ino);
            let extents = self.observed(result)?;
            inode.nblocks += pages
                .into_iter()
                .filter(|page| {
                    let offset = page * BLOCK_SIZE as u64;
                    !extents.iter().any(|(k, e)| {
                        k.offset <= offset
                            && offset < k.offset + u64::from(e.num_blocks) * BLOCK_SIZE as u64
                    })
                })
                .count() as u64;
        }
        attributes(ino, inode)
    }
    fn directory(&mut self, ino: u64) -> Result<NodeAttrs, Errno> {
        let attrs = self.attrs(ino)?;
        if attrs.kind != NodeKind::Directory {
            return Err(Errno(ENOTDIR));
        }
        Ok(attrs)
    }
    fn regular(&mut self, ino: u64) -> Result<Inode, Errno> {
        let inode = self.inode(ino)?;
        match kind(inode.mode)? {
            NodeKind::Regular => Ok(inode),
            NodeKind::Directory => Err(Errno(EISDIR)),
            _ => Err(Errno(EINVAL)),
        }
    }
    /// One bounded share of the node notifications. ENOMEM means a replay
    /// page was refused; the untaken notifications stay queued for a later
    /// share, which callers other than sync may leave to the next holder.
    fn retain_open(&mut self) -> Result<(), Errno> {
        #[cfg(test)] let _phase = publication_phases::Phase::new("retain_open");
        // Only opens, touches and final releases change this snapshot. The
        // atomic queue owns no VNode and cannot keep an inode open itself.
        let mut batch = core::mem::take(&mut self.retained_events);
        if batch.is_empty() {
            batch = self.events.take();
        }
        let mut result = Ok(());
        for _ in 0..RETAIN_OPEN_STEPS {
            if batch.is_empty() { break; }
            if let Err(error) = self.files.reserve(1) {
                result = Err(errno(error));
                break;
            }
            let Some(times) = batch.next() else { break; };
            preemption_point();
            #[cfg(test)] publication_phases::visit(0);
            let Some(node) = self.nodes.get(&times.ino) else { continue; };
            if node.touched.as_ref().is_none_or(|current| !Arc::ptr_eq(current, &times)) { continue; }
            let open = node.node.strong_count() != 0;
            // Reserved above. Were it refused, requeue the notification.
            if let Err(error) = self.files.set_inode_open(times.ino, open) {
                self.events.push(&times);
                result = Err(errno(error));
                break;
            }
            if times.pending() { self.touches.insert(times.ino, times.clone()); }
            self.prune_node(times.ino);
        }
        if batch.is_empty() {
            self.retained_batches += 1;
        }
        self.retained_events = batch;
        result
    }
    // A generation's pages are custody of that generation, not of NodeState:
    // keep the entry only while the open generation holds pages or a touch.
    fn prune_node(&mut self, ino: u64) {
        let scratch = self.files.log.is_some().then(|| self.open_generation() - 1);
        if self.nodes.get(&ino).is_some_and(|node| node.node.strong_count() == 0
            && !node.splitting && !self.dirty.contains_key(&ino)
            && !node.touched.as_ref().is_some_and(|times| {
                times.pending() || scratch.is_some_and(|generation| times.pending_frozen(generation))
            })) {
            self.nodes.remove(&ino);
            self.touches.remove(&ino);
        }
    }
    /// One bounded share of what an install left (E24 coordinator 601(4),
    /// x11c M2): replay up to MATERIALIZE_STEPS of N+1's recorded changes onto
    /// N's published view, oldest first, then refresh the VNodes and
    /// NodeStates N's preparation changed. Until the replay completes, the
    /// open view stays N+1's scratch view and installed N's pages stay
    /// readable, so a holder sees N+1 as it stands. The replaced scratch
    /// view and N's pages go to the commit worker, which drops them outside
    /// the exclusion. Returns true once the replay and the refresh are done.
    fn materialize(&mut self) -> bool {
        // Replay completes previously accepted edits. A syscall helping with
        // replay must retain their custody through any cold metadata reads.
        #[cfg(not(test))]
        let Some(_custody) = crate::task::thread::KillCustody::try_enter() else {
            return false;
        };
        let mut steps = 0;
        if let Some(next) = self.next_files.as_mut() {
            let log = self.files.log.get_or_insert_with(ReplayLog::new);
            let mut failed = None;
            while steps < MATERIALIZE_STEPS {
                preemption_point();
                let Some(change) = log.pop_front() else { break; };
                steps += 1;
                #[cfg(test)] measurement::materialized();
                if let Err(error) = change.replay(next) {
                    failed = Some(error);
                    break;
                }
            }
            if failed.is_none() && !log.is_empty() {
                return false;
            }
            // Constant moves: N's next view with N+1's changes becomes the
            // open view.
            if let Some(next) = self.next_files.take() {
                let scratch = core::mem::replace(&mut self.files.files, next);
                self.files.log = None;
                self.files.replay_bounds = None;
                self.inodes.entries.fill(None);
                // Each install is preceded by a freeze, which takes this slot.
                self.stale_views = Some((scratch, self.installed.take()));
            }
            if let Some(error) = failed {
                self.record_metadata(error);
                self.fail("replay-open-generation", Errno(EIO));
            }
        }
        while steps < MATERIALIZE_STEPS {
            preemption_point();
            let Some(ino) = self.refresh.pop_first() else { break; };
            steps += 1;
            #[cfg(test)] measurement::materialized();
            if let Some(node) = self.nodes.get(&ino).and_then(|entry| entry.node.upgrade()) {
                if let Ok(inode) = self.files.inode(ino) {
                    node.size.store(inode.size, Ordering::Release);
                    if let Ok(attrs) = attributes(ino, inode) { *node.times.lock() = attrs.times; }
                }
            }
            self.prune_node(ino);
        }
        self.next_files.is_none() && self.refresh.is_empty()
    }
    fn fail(&mut self, branch: &'static str, failure: impl Into<MountFailure>) {
        if self.failure.is_some() { return; }
        let (error, publication) = match failure.into() {
            MountFailure::Other(error) => (error, None),
            MountFailure::Publication(error) => (Errno(EIO), Some(error)),
        };
        self.failure = Some((branch, error.0));
        self.failed = true;
        if let Some(failure) = publication {
            log::error!("[breenfs] failure branch={} errno={} generation={} mount={} substep={:?} block_error={:?}; latched read-only",
                branch, error.0, self.durable.generation, self.mount, failure.step, failure.block_error);
        } else if let Some(failure) = self.metadata_error {
            log::error!("[breenfs] failure branch={} errno={} generation={} metadata_cause={} metadata_node={}; latched read-only",
                branch, error.0, self.durable.generation, failure.cause, failure.addr);
        } else {
            log::error!("[breenfs] failure branch={} errno={} generation={}; latched read-only",
                branch, error.0, self.durable.generation);
        }
        if let Some(committer) = self.committer.as_mut() {
            committer.fail();
        }
        // procfs renders the registry's flags, rather than calling back into
        // the mount under the registry lock. Host adapters have no registry.
        #[cfg(not(test))]
        let _ = crate::fs::vfs::mount::remount_read_only(self.mount);
        self.signal.mappings.store(true, Ordering::Release);
        self.signal.wake_all();
    }
    fn observed<T>(&mut self, result: Result<T, ops::Error>) -> Result<T, Errno> {
        if let Err(error) = result.as_ref() { self.record_metadata(*error); }
        if result.as_ref().is_err_and(|error| latches_on_read(*error)) {
            self.fail("observed-metadata", Errno(EIO));
        }
        result.map_err(errno)
    }
    fn record_metadata(&mut self, error: ops::Error) {
        if self.failure.is_none() {
            self.metadata_error = match error {
                ops::Error::Metadata(failure) => Some(failure),
                _ => None,
            };
            if let Some(failure) = self.metadata_error {
                if matches!(failure.cause, super::btree::ReadCause::Driver(driver) if driver.context_refused())
                    && !self.read_refusal_reported
                {
                    self.read_refusal_reported = true;
                    log::error!("[breenfs] metadata read refused errno={} metadata_cause={} metadata_node={}",
                        EAGAIN, failure.cause, failure.addr);
                }
            }
        }
    }
    fn read_result<T>(&mut self, result: Result<T, ReadError>) -> Result<T, Errno> {
        match result {
            Ok(value) => Ok(value),
            Err(ReadError::Metadata(error)) => self.observed(Err(error)),
            Err(ReadError::Device(error)) => Err(read_cause_errno(error.into())),
            Err(ReadError::Data) => Err(Errno(EIO)),
        }
    }
    fn edited<T>(&mut self, objects: &[u64], result: Result<T, ops::Error>) -> Result<T, Errno> {
        #[cfg(test)]
        let mut refresh = measurement::Refresh::new(objects, |ino| self.nodes.get(&ino).is_some());
        #[cfg(test)]
        super::btree::measurement::refresh_call();
        if result.is_ok() { self.fresh = false; }
        self.inodes.entries.fill(None);
        // A storage/allocation failure may have interrupted a multi-item edit.
        // Do not publish that transaction. Reservation refusals return before
        // mutation via admission; an ENOSPC here is still a bug.
        if let Err(error) = result.as_ref() {
            self.record_metadata(*error);
            match (error, errno(*error)) {
                // Admission expiry here stopped an edit already in custody.
                // Keep the failed generation from being published.
                (ops::Error::Metadata(super::btree::ReadFailure {
                    cause: super::btree::ReadCause::Ahci(
                        crate::block::read_failure::AhciReadError::AdmissionTimeout
                    ), ..
                }), _) => self.fail("edited", Errno(EIO)),
                (_, error @ Errno(EIO | ENOSPC)) => self.fail("edited", error),
                _ => {}
            }
        }
        // Refresh only inodes this edit can change. Closed dirty entries stay
        // indexed until publication, without making later edits scan them.
        for (index, &ino) in objects.iter().enumerate() {
            if objects[..index].contains(&ino) { continue; }
            let Some(entry) = self.nodes.get_mut(&ino) else { continue; };
            #[cfg(test)]
            {
                super::btree::measurement::mutation(7, 1);
                refresh.visit(ino);
            }
            preemption_point();
            if let Some(node) = entry.node.upgrade() {
                if let Ok(inode) = self.files.inode(ino) {
                    entry.links = inode.nlink;
                    node.size.store(inode.size, Ordering::Release);
                    *node.times.lock() = attributes(ino, inode)?.times;
                }
            }
        }
        if result.is_ok() {
            self.signal.worker.wake();
        }
        result.map_err(errno)
    }
    /// Dirty pages plus the private data of the generation being published:
    /// the bytes mutation admission charges against DIRTY_BUDGET.
    fn charged(&self) -> u64 {
        self.dirty_bytes + self.frozen_pages().map_or(0, |p| p.bytes)
    }
    /// Admission already includes the mounted ROOT update. Add any other
    /// accumulated debt that previous grants did not retain, such as COW
    /// from applying an atime before opening or statting a file.
    fn unpaid_metadata(&mut self, paths: &super::txn::ReservationPaths, space: u64) -> Result<(u64, u64), Errno> {
        let root_update = Estimate {
            space_changes: 3,
            space_height: paths.space_height,
            space_nodes: paths.space_nodes,
            ..Estimate::default()
        };
        let root_update = Self::metadata_blocks(root_update, space)?;
        let result = self.metadata_debt(space);
        let debt = self.observed(result)?;
        Ok((debt, debt.saturating_sub(self.metadata_reserved.max(root_update))))
    }
    fn metadata_debt(&self, space: u64) -> Result<u64, ops::Error> {
        if space == 0 { return self.files.metadata_debt(); }
        Self::metadata_blocks(self.files.metadata_estimate()?, space).map_err(|_| ops::Error::ENOSPC)
    }
    fn metadata_blocks(estimate: Estimate, space: u64) -> Result<u64, Errno> {
        let total = estimate.blocks().map_err(|_| Errno(ENOSPC))?;
        if space == 0 { return Ok(total); }
        let fs = Estimate { space_changes: 0, ..estimate }.blocks().map_err(|_| Errno(ENOSPC))?;
        Ok(total.saturating_sub(space).max(fs))
    }
    /// Bound replay from the frozen root and the older generation's SPACE
    /// topology. Scratch-private paths do not imply private installed paths.
    fn replay_cost(&mut self, paths: &super::txn::ReservationPaths) -> Result<Option<(u64, u64)>, Errno> {
        let Some(bounds) = self.files.replay_bounds.as_ref() else { return Ok(None); };
        let Some(nodes) = super::btree::shallow_space_bound(paths.space_height, paths.space_nodes,
            bounds.space_growth_bytes) else { return Ok(None); };
        let growth = bounds.root_growth.map_or_else(|| self.files.root_growth(bounds.frozen_root), Ok);
        let edits = bounds.growing_edits;
        let growth = self.observed(growth)?;
        self.files.replay_bounds.as_mut().expect("scratch replay bounds").root_growth = Some(growth);
        // Keep the installed tree's design 2.8 SPACE allowance in the
        // scratch generation's grants through replay.
        Ok(Some((growth.level_after(edits).max(paths.fs_height), 4 * (2 * nodes + 1))))
    }
    fn reserve_blocks(&mut self, blocks: u64, operation: Operation) -> Result<Grant, AdmissionError> {
        self.reservations.reserve(self.durable.free_blocks, blocks, operation)
            .map_err(AdmissionError::Denied)
    }
    /// Called with mutation admission held, before publishing dirty pages or
    /// inode changes. Clean mapped blocks also need fresh COW allocations.
    fn reserve_data(
        &mut self,
        ino: u64,
        offset: u64,
        length: usize,
        operation: Operation,
        empty_file: bool,
    ) -> Result<DataGrant, AdmissionError> {
        self.nodes.get(&ino).ok_or(Errno(EIO))?;
        // The open generation's pages only: a page copied from a frozen
        // generation is a new page of this one and needs its own block.
        let open = self.dirty.get(&ino);
        let (new_pages, pages, runs) = match open {
            Some(node) => node.pages.reservation_runs(offset, length),
            None => DirtyPages::default().reservation_runs(offset, length),
        };
        let reserved_metadata = open.map_or(0, |node| node.reserved_metadata);
        let first_write = empty_file && offset == 0 && length == 1 && open.is_none();
        let result = self.files.reservation_paths(ino);
        let paths = self.observed(result)?;
        let replay = if first_write { self.replay_cost(&paths)? } else { None };
        let space = replay.map_or(self.replay_space, |(_, space)| space);
        let (funded_debt, unpaid) = self.unpaid_metadata(&paths, space)?;
        if first_write && (self.files.log.is_none() || replay.is_some()) {
            // One data block and one EXTENT without boundary slices. The
            // foreground INODE edit can copy two nodes at each old level
            // below the root and the root itself. The extent may use a
            // different path. The two same-size INODE updates during drain
            // already have a private path, but can each rebalance with an
            // older sibling per level. Any levels grown since admission
            // belong to this transaction. Include a full split cascade.
            let fs_height = replay.map_or(paths.fs_height, |(height, _)| height);
            let fs_nodes = 6 * fs_height + 9;
            let metadata = 4 * fs_nodes;
            // Design 2.8 bounds the SPACE fixpoint for the whole open
            // transaction. Retain the base topology once, plus growth for
            // the added REF; later files share the same pending fixpoint.
            let first_write_space = self.first_write_space.max(
                replay.map_or(4 * (2 * paths.space_nodes + paths.space_height), |(_, space)| space));
            let blocks = (1 + metadata + 4 + (first_write_space - self.first_write_space))
                .checked_add(unpaid).ok_or(Errno(ENOSPC))?;
            self.grants.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
            let grant = self.reserve_blocks(blocks, operation)?;
            return Ok(DataGrant { grant, metadata: metadata + 4, first_write_space, funded_debt,
                replay_space: replay.map(|(_, space)| space), drain_growth: 1 });
        }
        // A run inserts at most 256 data EXTENTs and can retain up to 510
        // one-page slices at its old boundaries (ops::remove_range). Their
        // 45-byte items fit in three nodes' payload space; allow four split
        // paths, their COW siblings, a new root level and the inode edit.
        let fs_levels = runs.checked_add(1)
            .and_then(|n| n.checked_mul(4 * (paths.fs_height + 2)))
            .ok_or(Errno(ENOSPC))?;
        let space_changes = paths.uncopied_fs_nodes.checked_add(fs_levels)
            .and_then(|n| n.checked_add(pages))
            .and_then(|n| n.checked_add(1))
            .and_then(|n| n.checked_mul(3))
            .ok_or(Errno(ENOSPC))?;
        let estimate = Estimate {
            data_blocks: pages,
            uncopied_fs_nodes: paths.uncopied_fs_nodes,
            fs_levels,
            space_changes,
            space_height: paths.space_height,
            space_nodes: paths.space_nodes,
        };
        let metadata = estimate.blocks().map_err(|_| Errno(ENOSPC))? - pages;
        let blocks = new_pages.checked_add(metadata.saturating_sub(reserved_metadata))
            .and_then(|blocks| blocks.checked_add(unpaid))
            .ok_or(Errno(ENOSPC))?;
        if blocks != 0 {
            self.grants.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
        }
        // durable.free_blocks includes the allocations covered by both the
        // publishing generation and the open generation. Do not debit them
        // a second time before asking the ledger for a grant.
        self.reserve_blocks(blocks, operation)
            .map(|grant| DataGrant { grant, metadata, first_write_space: self.first_write_space, funded_debt,
                replay_space: None, drain_growth: pages.saturating_mul(511) })
    }
    fn finish_data(&mut self, ino: u64, reservation: DataGrant, result: Result<(), Errno>) -> Result<(), Errno> {
        let DataGrant { grant, metadata, first_write_space, funded_debt, replay_space, drain_growth } = reservation;
        if result.is_ok() {
            self.nodes.get(&ino).ok_or(Errno(EIO))?;
            self.first_write_space = first_write_space;
            self.drain_growth = self.drain_growth.saturating_add(drain_growth);
            if let Some(space) = replay_space { self.replay_space = space; }
            else { self.files.replay_bounds = None; }
            let node = self.dirty.entry(ino).or_default();
            node.reserved_metadata = node.reserved_metadata.max(metadata);
            self.dirty_nodes.insert(ino);
            if grant.blocks() != 0 { self.grants.push(grant); }
            else { self.reservations.release(grant); }
            // Credit only the existing debt measured and funded at admission.
            self.metadata_reserved = self.metadata_reserved.max(funded_debt);
        } else {
            // DirtyPages prepares its pages before changing the dirty map.
            self.reservations.release(grant);
        }
        result
    }
    fn reserve_create(&mut self, dir: u64, inline: bool) -> Result<MetadataGrant, AdmissionError> {
        if self.files.log.is_some() && self.files.replay_bounds.is_none() {
            return self.reserve_metadata(&[dir], 5, true, false, Operation::Allocate).map(Into::into);
        }
        let result = self.files.reservation_paths(dir);
        let paths = self.observed(result)?;
        if self.files.log.is_some() {
            if let Some((height, space)) = self.replay_cost(&paths)? {
                let current = self.files.metadata_estimate();
                let current = Self::metadata_blocks(self.observed(current)?, space)?;
                let edits = 4 + u64::from(inline);
                let blocks = 4 * (2 * height.max(1) * edits + 3) + space - self.replay_space;
                let blocks = blocks.checked_add(current.saturating_sub(self.metadata_reserved))
                    .ok_or(Errno(ENOSPC))?;
                self.grants.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
                let grant = self.reserve_blocks(blocks, Operation::Allocate)?;
                return Ok(MetadataGrant { grant, replay_space: Some(space) });
            }
            return self.reserve_metadata(&[dir], 5, true, false, Operation::Allocate).map(Into::into);
        }
        // Grow INODE, DIR_ENTRY, DIR_INDEX (and INLINE for a symlink), and
        // replace the parent INODE. Below the original root, an edit can
        // copy the target and either copy a sibling or split the target.
        // The original root copies once if needed and can split once.
        // The new root and both children of that split have room for the
        // remaining create edits, and those nodes belong to this generation.
        let edits = 4 + u64::from(inline);
        let fs_nodes = 2 * paths.fs_height * edits + 2 + u64::from(!paths.private_fs_root);
        // Settlement prices queued orphan cleanup at the resulting root
        // height. Growing a level adds twelve FS nodes per orphan to that
        // estimate and can increase its SPACE charge too.
        let result = self.files.orphan_growth_paths(dir, 3 + usize::from(inline));
        let fs_nodes = fs_nodes.checked_add(self.observed(result)?).ok_or(Errno(ENOSPC))?;
        let estimate = Estimate {
            fs_levels: fs_nodes,
            // Up to two deferred records per FS allocation and the ROOT
            // update. Include both in the remaining-debt calculation.
            space_changes: 6 * fs_nodes + 3,
            space_height: paths.space_height,
            space_nodes: paths.space_nodes,
            ..Estimate::default()
        };
        self.reserve_namespace(estimate, Operation::Allocate).map(Into::into)
    }
    /// Reserve COW paths and structural edits before a namespace mutation.
    /// Each item edit allows a split/merge path, its COW siblings and a new
    /// root. Small calls retain separate item, sibling and root-transition
    /// paths as conservative commit headroom.
    fn reserve_metadata(
        &mut self,
        objects: &[u64],
        edits: u64,
        new_inode: bool,
        orphans: bool,
        operation: Operation,
    ) -> Result<Grant, AdmissionError> {
        let result = self.files.metadata_reservation_paths(objects, new_inode, orphans);
        let paths = self.observed(result)?;
        let fs_levels = edits.max(3).checked_mul(4 * (paths.fs_height + 2))
            .ok_or(Errno(ENOSPC))?;
        let space_changes = paths.uncopied_fs_nodes.checked_add(fs_levels)
            .and_then(|n| n.checked_mul(3)).ok_or(Errno(ENOSPC))?;
        let estimate = Estimate {
            data_blocks: 0,
            uncopied_fs_nodes: paths.uncopied_fs_nodes,
            fs_levels,
            space_changes,
            space_height: paths.space_height,
            space_nodes: paths.space_nodes,
        };
        self.reserve_namespace(estimate, operation)
    }
    fn reserve_namespace(&mut self, estimate: Estimate, operation: Operation) -> Result<Grant, AdmissionError> {
        let result = self.files.metadata_estimate();
        let current = self.observed(result)?;
        let combined = Estimate {
            data_blocks: current.data_blocks,
            fs_levels: estimate.fs_levels.checked_add(current.fs_levels).ok_or(Errno(ENOSPC))?,
            // The namespace estimate already has room for the mounted ROOT
            // update. Apply the SPACE cap to the combined changes once.
            space_changes: estimate.space_changes.checked_add(current.space_changes - 3)
                .ok_or(Errno(ENOSPC))?,
            ..estimate
        };
        let combined = Self::metadata_blocks(combined, self.replay_space)?;
        let standalone = estimate.blocks().map_err(|_| Errno(ENOSPC))?;
        let blocks = standalone.max(combined.saturating_sub(self.metadata_reserved));
        self.grants.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
        self.reserve_blocks(blocks, operation)
    }
    fn finish_metadata<T>(&mut self, objects: &[u64], reservation: impl Into<MetadataGrant>, result: Result<T, ops::Error>) -> Result<T, Errno> {
        let MetadataGrant { mut grant, replay_space } = reservation.into();
        if result.is_ok() {
            // durable.free_blocks still includes private allocations. Keep
            // those and the work publication still owes, once per transaction;
            // return unused worst-case paths (E24 ledger 509(n)).
            let space = replay_space.unwrap_or(self.replay_space);
            let debt = self.metadata_debt(space);
            let debt = match debt {
                Ok(debt) => debt,
                Err(error) => {
                    self.reservations.release(grant);
                    return self.edited(objects, Err(error));
                }
            };
            let retained = debt.saturating_sub(self.metadata_reserved);
            if retained > grant.blocks()
                || (replay_space.is_none() && self.reservations.settle(&mut grant, retained).is_err()) {
                self.reservations.release(grant);
                return self.edited(objects, Err(ops::Error::ENOSPC));
            }
            // The real replay may copy different paths. Keep its FS bound
            // until publication, even if the scratch edit used fewer nodes.
            self.replay_space = space;
            self.first_write_space = self.first_write_space.max(space);
            self.metadata_reserved += retained;
            if grant.blocks() != 0 { self.grants.push(grant); }
            else { self.reservations.release(grant); }
            // E24 coordinator ledger 558(x2): request background publication
            // before retained reservations force foreground admission to flush.
            let trigger = self.durable.free_blocks.saturating_sub(self.durable.global_reserve) / 2;
            if self.reservations.outstanding() > trigger {
                self.signal.requested.store(true, Ordering::Release);
            }
        } else { self.reservations.release(grant); }
        self.edited(objects, result)
    }
}
/// Generation N from freeze to install, owned by its publication (E24
/// coordinator 597/601). Preparation runs on it without mount exclusion and
/// may yield anywhere (ruling 488); foreground state is not reachable from it.
struct FrozenGeneration<W: Wait + 'static> {
    files: FilesView<W>,
    commit: FrozenCommit,
    // Shared with readers through MountState.pending until install.
    pages: Arc<FrozenPages>,
    dirty_nodes: BTreeSet<u64>,
    touches: BTreeMap<u64, Arc<TouchedTimes>>,
    // Inodes whose notifications preparation consumed: refreshed or pruned after install.
    touched: BTreeSet<u64>,
    grants: Grants,
    ledger: SharedLedger,
    reap: bool,
    free_blocks: u64,
}
impl<W: Wait + 'static> FrozenGeneration<W> {
    /// Apply one inode's deferred times from N's slot (601(2)).
    fn apply_touch(&mut self, ino: u64, stamps: &TouchedTimes) -> Result<(), ops::Error> {
        let stamps = stamps.for_publication(self.commit.generation());
        if !stamps.is_empty() {
            apply_inode_touches(&mut self.files, ino, Stamps::default(), stamps)?;
            self.touched.insert(ino);
        }
        Ok(())
    }
    fn drain_dirty(&mut self, device: &dyn BlockDevice) -> Result<(), Errno> {
        #[cfg(test)] let _phase = publication_phases::Phase::new("drain_dirty");
        self.drain_pages(device)
    }
    // Readers keep the frozen pages until install (597): drain a clone, which
    // shares each page, and write it into N's private extents.
    fn drain_pages(&mut self, device: &dyn BlockDevice) -> Result<(), Errno> {
        for &ino in &self.dirty_nodes {
            preemption_point();
            #[cfg(test)] publication_phases::visit(0);
            let Some(node) = self.pages.nodes.get(&ino) else { continue; };
            if node.pages.bytes() == 0 { continue; }
            let mut inode = match self.files.inode(ino) {
                Ok(inode) => inode,
                // Closed orphans are reclaimed by preparation below.
                Err(ops::Error::ENOENT) => continue,
                Err(e) => return Err(errno(e)),
            };
            let mut pages = node.pages.clone();
            let files = &mut self.files;
            pages.write_runs(inode.size, |offset, bytes| {
                files.write(device, ino, offset, bytes, inode.mtime).map_err(errno)?;
                Ok(())
            })?;
            inode.nblocks = self.files.inode(ino).map_err(errno)?.nblocks;
            self.files.put_inode(ino, inode).map_err(errno)?;
        }
        Ok(())
    }
}
fn apply_inode_touches<W: Wait + 'static>(
    files: &mut FilesView<W>, ino: u64, older: Stamps, open: Stamps,
) -> Result<bool, ops::Error> {
    let mut inode = match files.inode(ino) {
        Ok(inode) => inode,
        // A final touch can race the reclaim of a closed orphan.
        Err(ops::Error::ENOENT) => return Ok(false),
        Err(error) => return Err(error),
    };
    // Replay the same touch operations over the installed inode. Inherited
    // stamps already applied there are normally no-ops.
    let mut changed = touched_inode(&mut inode, older);
    changed |= touched_inode(&mut inode, open);
    if changed { files.put_inode(ino, inode)?; }
    Ok(changed)
}
/// Apply coarse touch stamps (seconds + 1; 0 means no stamp). True if changed.
fn touched_inode(inode: &mut Inode, Stamps { access, modify }: Stamps) -> bool {
    let before = *inode;
    if modify != 0 {
        inode.mtime = Time { seconds: (modify - 1) as i64, nanos: 0 };
        inode.ctime = inode.mtime;
    }
    if access != 0 {
        let at = Time { seconds: (access - 1) as i64, nanos: 0 };
        if needs_atime_update(inode, at) {
            inode.atime = at;
        }
    }
    *inode != before
}
impl<W: Wait + 'static> Clone for BreenFs<W> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
impl<W: Wait + 'static> BreenFs<W> {
    /// Recovery is read-only: no abandoned-slot erasure, repair or orphan work.
    pub fn mount(
        device: Arc<dyn BlockDevice>,
        cache: Arc<BufferCache<W>>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Shared(cache), mount, root_id, None, false, false, false)
    }
    /// Writable recovery repairs the rings before opening the first transaction.
    /// The caller starts the per-mount worker through init_service().
    pub fn mount_writable(
        device: Arc<dyn BlockDevice>,
        cache: Arc<BufferCache<W>>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Shared(cache), mount, root_id, None, true, false, false)
    }
    /// Kernel read mounts borrow the system cache.
    pub fn mount_cached(
        device: Arc<dyn BlockDevice>,
        cache: &'static BufferCache<W>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Kernel(cache), mount, root_id, None, false, false, false)
    }
    /// Writable kernel mounts borrow the system cache, as read mounts do.
    pub fn mount_writable_cached(
        device: Arc<dyn BlockDevice>,
        cache: &'static BufferCache<W>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Kernel(cache), mount, root_id, None, true, true, false)
    }
    /// Recover the selected OS and the default tree's data under one writer.
    pub(crate) fn mount_persistent_cached(
        device: Arc<dyn BlockDevice>,
        cache: &'static BufferCache<W>,
        mount: usize,
        name: &[u8],
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Kernel(cache), mount, 1, Some(name), true, true, true)
    }

    pub(crate) fn default_root_inode(&self) -> u64 {
        debug_assert!(self.inner.default_root.is_some());
        ops::DEFAULT_TREE | 2
    }

    /// Pin the selected root after writable recovery. The view reuses the
    /// writer's Files transaction and cache.
    pub fn generation_view(&self, id: usize, name: &[u8]) -> Result<Arc<dyn FileSystem>, Errno> {
        let device = &self.inner.mounted_root.device;
        let _selection = device.selection.lock();
        let state = self.foreground_lock()?;
        state.writable()?;
        let root_id = state.roots.iter().find(|(_, (root, _))| root == name)
            .map(|(&id, _)| id).ok_or(Errno(ENOENT))?;
        let root = Arc::try_new(GenerationRoot {
            mount: id,
            mounted: MountedRoot::claim(device.clone(), root_id)?,
            closed_inodes: Arc::try_new(BTreeSet::new()).map_err(|_| Errno(ENOMEM))?,
        }).map_err(|_| Errno(ENOMEM))?;
        let view = Arc::try_new(GenerationView { writer: self.clone(), root })
            .map_err(|_| Errno(ENOMEM))?;
        Ok(view)
    }
    /// Explicit mounts use the same root-data parser with either cache owner.
    pub fn mount_named(
        device: Arc<dyn BlockDevice>, cache: Arc<BufferCache<W>>, mount: usize,
        data: Option<&[u8]>, writable: bool,
    ) -> Result<Arc<Self>, Errno> {
        let name = super::mount_args::parse_root(data).map_err(|e| Errno(e as i32))?;
        Self::mount_with(device, Cache::Shared(cache), mount, 1, name, writable, false, false)
    }
    pub fn mount_named_cached(
        device: Arc<dyn BlockDevice>, cache: &'static BufferCache<W>, mount: usize,
        data: Option<&[u8]>, writable: bool,
    ) -> Result<Arc<Self>, Errno> {
        let name = super::mount_args::parse_root(data).map_err(|e| Errno(e as i32))?;
        Self::mount_with(device, Cache::Kernel(cache), mount, 1, name, writable, false, false)
    }
    /// Query for snapshot deletion admission; shutdown alone retains custody.
    pub fn check_root_unmounted(&self, root_id: u64) -> Result<(), Errno> {
        if self.inner.mounted_root.device.roots.lock().contains(&root_id) {
            Err(Errno(EBUSY))
        } else {
            Ok(())
        }
    }
    fn mount_with(
        device: Arc<dyn BlockDevice>,
        cache: Cache<W>,
        mount: usize,
        root_id: u64,
        root_name: Option<&[u8]>,
        writable: bool,
        boot: bool,
        with_default: bool,
    ) -> Result<Arc<Self>, Errno> {
        let report = |details: core::fmt::Arguments<'_>| {
            #[cfg(any(target_os = "none", target_os = "breenix"))]
            if boot {
                crate::fs::vfs::root_mount_receipt(Some(device.as_ref()), details);
            }
            #[cfg(not(any(target_os = "none", target_os = "breenix")))]
            let _ = (boot, details);
        };
        // A failed constructor drops this guard, including failures after
        // recovery. Successful mounts retain it through shutdown/restart until
        // unmount releases the last mount reference (including a pinned VNode).
        let owner = if writable {
            Some(WritableOwner::claim(device.identity())?)
        } else {
            None
        };
        let mounts = DeviceMounts::get(device.identity())?;
        let selection = mounts.selection.lock();
        let mut publication = mounts.publication.lock();
        let durable = recover::recover_detailed(device.as_ref(), writable).map_err(|failure| {
            report(format_args!("breenfs {}", failure));
            recovery_errno(failure.error)
        })?;
        // Recovery can publish orphan reclamation even if root selection fails.
        *publication = Some(durable.clone());
        let store = cache.store(device.clone(), &durable);
        let block = durable.space_root.addr;
        let roots = recover::space_items(&store, &durable).map_err(|error| {
            report(format_args!("stage=breenfs-node-cache space_root={} expected=readable-roots found={:?}", block, error));
            recovery_errno(error)
        })?;
        let durable_pending = PendingSummary::from_items(&roots).ok();
        let mut selected = None;
        for (key, bytes) in roots.iter().filter(|(key, _)| key.item_type == format::ROOT) {
            let root = Root::decode(bytes).map_err(|error| {
                report(format_args!("stage=breenfs-root-decode space_root={} expected=ROOT-item found={:?}", block, error));
                Errno(EIO)
            })?;
            if root_name.map_or(key.offset == root_id, |name| root.name == name) {
                selected = Some((key.offset, root.flags & 1 != 0));
                break;
            }
        }
        let (root_id, frozen) = selected.ok_or_else(|| {
            report(format_args!("stage=breenfs-root-select space_root={} expected=root-{} found=absent", block, root_id));
            Errno(ENOENT)
        })?;
        let mut files = Files::new(store, &durable, root_id, writable).map_err(|error| {
            let block = match error {
                ops::Error::Metadata(failure) => failure.addr,
                _ => block,
            };
            report(format_args!("stage=breenfs-files-view block={} expected=valid-files-view found={:?}", block, error));
            errno(error)
        })?;
        let mounted_root = MountedRoot::claim(mounts.clone(), root_id)?;
        let default_root = if with_default {
            files.include_default_tree().map_err(errno)?;
            Some(MountedRoot::claim(mounts.clone(), 1)?)
        } else {
            None
        };
        drop(publication);
        drop(selection);
        let dead_remaining = roots.iter().filter(|(key, _)| key.item_type == format::DEAD).count() as u64;
        let roots = roots.into_iter().filter(|(key, _)| key.item_type == format::ROOT)
            .map(|(key, bytes)| Root::decode(&bytes)
                .map(|root| (key.offset, (root.name, root.flags))))
            .collect::<Result<_, _>>().map_err(|error| {
                report(format_args!("stage=breenfs-root-index space_root={} expected=ROOT-item found={:?}", block, error));
                Errno(EIO)
            })?;
        let committer = writable.then(|| Committer::new(durable.clone()));
        let data = Box::try_new(DataBlocks {
            device: device.clone(),
            blocks: durable.total_blocks,
        })
        .map_err(|_| Errno(ENOMEM))?;
        let signal = Arc::new(CommitSignal::new());
        let events = Arc::try_new(NodeEvents::default()).map_err(|_| Errno(ENOMEM))?;
        let ledger = Arc::try_new(Mutex::new(Ledger::new(durable.global_reserve)))
            .map_err(|_| Errno(ENOMEM))?;
        let generation = AtomicU64::new(durable.generation + 1);
        let inner = Arc::try_new(Mount {
            events: events.clone(),
            costs: Costs::new(),
            state: MountLock::new(MountState {
                files: OpenFiles { files, log: None, replay_bounds: None },
                next_files: None,
                installed: None,
                stale_views: None,
                refresh: BTreeSet::new(),
                committer,
                reservations: SharedLedger(ledger),
                grants: Grants::default(),
                metadata_reserved: 0,
                first_write_space: 0,
                replay_space: 0,
                drain_growth: 0,
                durable,
                durable_pending,
                failed: false,
                failure: None,
                metadata_error: None,
                read_refusal_reported: false,
                scrub: None,
                scrub_state: SCRUB_IDLE,
                mount,
                closed: false,
                nodes: BTreeMap::new(),
                events,
                retained_events: Events::default(),
                retained_batches: 0,
                retaining_for_sync: 0,
                reservation_pressure: false,
                touches: BTreeMap::new(),
                dirty: BTreeMap::new(),
                dirty_nodes: BTreeSet::new(),
                dirty_bytes: 0,
                fresh: true,
                roots,
                dead_remaining,
                skip_reap: false,
                reap_shutdown: false,
                policy: CommitPolicy::new(monotonic_ns()),
                mapping_retry_ns: 0,
                pending: None,
                signal: signal.clone(),
                inodes: InodeCache {
                    entries: [None; 32],
                    next: 0,
                },
            }),
            device,
            owner,
            data,
            cache,
            id: mount,
            root_id,
            mounted_root,
            default_root,
            frozen,
            writable,
            signal,
            scrub_signal: Arc::new(CommitSignal::new()),
            generation,
            materialize: AtomicBool::new(false),
            mapping_passes: AtomicU64::new(0),
            mapping_pages_evicted: AtomicU64::new(0),
            mapping_requests_dropped: AtomicU64::new(0),
            #[cfg(any(feature = "breenfs_commit_window", feature = "breenfs_corrupt_inject"))]
            commit_faults: super::commit::faults::Faults::for_mount(boot && writable),
        })
        .map_err(|_| Errno(ENOMEM))?;
        Arc::try_new(Self { inner }).map_err(|_| Errno(ENOMEM))
    }
    #[cfg(not(test))]
    fn mapping_descriptor_counts<const N: usize>(&self, inodes: &[Option<u64>; N]) -> Option<[(usize, usize); N]> {
        use crate::ipc::fd::{FdKind, FdTable};

        // Scan each shared table once. Report unavailable if the snapshot is
        // contended or exceeds its bounds, so partial counts cannot imply a close.
        const MAX_ROWS: usize = 128;
        const MAX_DESCRIPTIONS: usize = 256;
        let mut descriptions = Vec::new();
        descriptions.try_reserve_exact(MAX_DESCRIPTIONS).ok()?;
        let mut tables: [Option<&FdTable>; MAX_ROWS] = [None; MAX_ROWS];
        let mut table_count = 0;
        let mut counts = [(0, 0); N];
        let manager = crate::process::try_manager()?;
        for (index, (_, process)) in manager.as_ref()?.iter_processes().enumerate() {
            if index == MAX_ROWS { return None; }
            let table = &process.fd_table;
            if tables[..table_count].iter().flatten().any(|known| known.is_same_table(table)) {
                continue;
            }
            tables[table_count] = Some(table);
            table_count += 1;
            for kind in table.kinds() {
                let FdKind::RegularFile(description) = kind else { continue };
                let file = description.try_lock()?;
                if file.mount_id != self.inner.id { continue; }
                let Some(slot) = inodes.iter().position(|ino| *ino == Some(file.inode_num)) else { continue };
                counts[slot].0 += 1;
                let identity = Arc::as_ptr(description);
                if !descriptions.contains(&identity) {
                    if descriptions.len() == MAX_DESCRIPTIONS { return None; }
                    descriptions.push(identity);
                    counts[slot].1 += 1;
                }
            }
        }
        Some(counts)
    }
    fn mutation_lock(&self, op: CostOp) -> MountGuard<'_, MountState<W>> {
        let _wait = self.inner.costs.span(op, CostPhase::Wait);
        if matches!(op, CostOp::Commit) {
            return self.inner.state.lock();
        }
        let _wait = self.inner.costs.span(CostOp::Foreground, CostPhase::MountExclusion);
        self.materialized(self.inner.state.lock())
    }
    fn mutation_admission_lock(&self, op: CostOp) -> Result<MountGuard<'_, MountState<W>>, Errno> {
        let _wait = self.inner.costs.span(op, CostPhase::Wait);
        if matches!(op, CostOp::Writeback) {
            return Ok(self.materialized(self.inner.state.lock_interruptible()?));
        }
        self.foreground_lock()
    }
    fn foreground_lock(&self) -> Result<MountGuard<'_, MountState<W>>, Errno> {
        let _wait = self.inner.costs.span(CostOp::Foreground, CostPhase::MountExclusion);
        Ok(self.materialized(self.inner.state.lock_interruptible()?))
    }

    fn check_durable_roots(&self, generation: Option<u64>) -> Result<(), Errno> {
        // Keep this durable generation pinned against install and reuse while
        // reading it. An in-flight publication still protects its blocks.
        let state = self.inner.state.lock_interruptible()?;
        #[cfg(all(not(test), target_arch = "x86_64"))]
        let progress = match self.inner.device.hardware_disk() {
            Some(crate::block::HardwareDisk::Ahci(port)) => {
                Some((port, crate::drivers::ahci::completed_io(port)))
            }
            _ => None,
        };
        let result = self.probe_durable_roots(&state.durable, generation);
        // A probe starved at admission can defer verification for this round
        // if the same disk completed another command during the probe.
        #[cfg(all(not(test), target_arch = "x86_64"))]
        if result == Err(Errno(ETIMEDOUT)) && progress.is_some_and(|(port, before)| {
            crate::drivers::ahci::completed_io(port) != before
        }) {
            return Ok(());
        }
        result
    }

    fn probe_durable_roots(&self, sb: &Superblock, generation: Option<u64>) -> Result<(), Errno> {
        use super::{btree, commit};

        let device = self.inner.device.as_ref();
        let mut superblock_error = Errno(EIO);
        let has_superblock = (0..2).any(|ring| {
            let slot = commit::slot_block(sb.total_blocks, ring, sb.slot);
            let bytes = match commit::read(device, slot, BLOCK_SIZE) {
                Ok(bytes) => bytes,
                Err(commit::Error::BlockRead(BlockError::AhciRead(
                    crate::block::read_failure::AhciReadError::AdmissionTimeout,
                ))) => {
                    superblock_error = Errno(ETIMEDOUT);
                    return false;
                }
                Err(_) => return false,
            };
            let mut expected = sb.clone();
            expected.ring = ring;
            Superblock::decode(&bytes) == Ok(expected)
        });
        if !has_superblock {
            return Err(superblock_error);
        }

        let store = commit::DeviceStore(device);
        // Each point lookup verifies the on-disk parent pointers and CRCs.
        // Both trees used by the namespace must have valid durable roots.
        let default = self.inner.default_root.as_ref().map(|root| root.id);
        for id in [Some(self.inner.root_id), default, generation].into_iter().flatten() {
            let root = ops::persisted_root(&store, sb, id).map_err(errno)?;
            let bytes = btree::search(&store, root.root, ops::key(2, format::INODE, 0))
                .map_err(|error| errno(error.into()))?
                .ok_or(Errno(EIO))?;
            let inode = Inode::decode(&bytes).map_err(|_| Errno(EIO))?;
            if inode.mode & 0o170000 != 0o040000 || inode.parent != 2 || inode.nlink < 2 {
                return Err(Errno(EIO));
            }
        }
        Ok(())
    }
    // Install sets the flag and the last share clears it, both under the
    // exclusion: the flag is set while replay or refresh remains (x11c M2).
    fn materialized<'a>(&self, mut state: MountGuard<'a, MountState<W>>) -> MountGuard<'a, MountState<W>> {
        if self.inner.materialize.load(Ordering::Acquire) && state.materialize() {
            self.inner.materialize.store(false, Ordering::Release);
        }
        state
    }
    fn mutation_wait(&self, op: CostOp, observed: u64, deadline: Option<u64>) -> Result<(), Errno> {
        let _wait = self.inner.costs.span(op, CostPhase::Wait);
        self.inner.signal.wait(observed, deadline)
    }
    fn check_node(&self, node: &VNode) -> Result<(), Errno> {
        if node.mount != self.inner.id
            || node
                .pin::<NodePin<W>>()
                .is_none_or(|pin| !Arc::ptr_eq(&pin.mount, &self.inner))
        {
            return Err(Errno(EXDEV));
        }
        Ok(())
    }
    fn new_node(&self, ino: u64, inode: Inode, epoch: u64) -> Result<Arc<VNode>, Errno> {
        let fs = Arc::try_new(self.clone()).map_err(|_| Errno(ENOMEM))?;
        let pin = Arc::try_new(NodePin {
            mount: self.inner.clone(),
            touched: Arc::try_new(TouchedTimes::new(ino)).map_err(|_| Errno(ENOMEM))?,
        })
        .map_err(|_| Errno(ENOMEM))?;
        Arc::try_new(VNode::new(
            self.inner.id,
            ino,
            inode.created_generation,
            inode.size,
            attributes(ino, inode)?.times,
            fs,
            epoch,
            pin,
        )?)
        .map_err(|_| Errno(ENOMEM))
    }
    fn open_locked(&self, state: &mut MountState<W>, ino: u64) -> Result<Arc<VNode>, Errno> {
        let inode = state.inode(ino)?;
        if let Some(node) = state.nodes.get(&ino).and_then(|n| n.node.upgrade()) {
            // A completed resize or write can predate this mapping transfer.
            // Faults must see that size before an unbound VNode is mapped.
            node.map.resync_unbound(inode.size);
            return Ok(node);
        }
        // While N is frozen this open is one of N+1's records: refuse it
        // with ENOMEM before any VNode exists if its page is refused.
        state.files.reserve(1).map_err(errno)?;
        let node = self.new_node(ino, inode, state.durable.generation)?;
        let entry = state.nodes.entry(ino).or_default();
        entry.node = Arc::downgrade(&node);
        entry.links = inode.nlink;
        entry.touched = Some(node.pin::<NodePin<W>>().ok_or(Errno(EIO))?.touched.clone());
        state.files.set_inode_open(ino, true).map_err(errno)?;
        // A refused share stays queued for the next holder.
        let _ = state.retain_open();
        Ok(node)
    }
    /// Release the namespace's cache pin after a successful last-link edit.
    /// The returned pin drops outside mount exclusion, where it may free the
    /// entire file cache. Descriptors and bindings retain the orphan until
    /// their own last release.
    fn take_orphan_cache_pin(state: &mut MountState<W>, ino: u64) -> Option<Arc<VNode>> {
        let entry = state.nodes.get_mut(&ino)?;
        if entry.links == 0 { entry.mapped.take() } else { None }
    }
    /// Resident dirty-page bytes charged to this mount, including closed files
    /// awaiting commit. Available to the mount's commit-pressure policy.
    pub fn dirty_bytes(&self) -> u64 {
        let state = self.inner.state.lock();
        state.charged()
    }
    /// Admit `incoming` dirty bytes (E24 ledger 309, C3). When the open
    /// transaction passes half the budget the caller asks for a commit; when
    /// it and the generation being published pass the whole budget, the caller
    /// sleeps, holding no mount lock, until a generation completes. A call
    /// larger than the whole budget is admitted alone, once no other bytes are
    /// charged. write splits its calls at WRITE_PART (N2), so no write admits
    /// more than half the budget and two edge pages at once.
    fn admit(&self, incoming: u64, cost: CostOp) -> Result<MountGuard<'_, MountState<W>>, Errno> {
        loop {
            let observed = self.inner.signal.epoch();
            let state = self.mutation_admission_lock(cost)?;
            state.writable()?;
            if state.dirty_bytes.saturating_add(incoming) > DIRTY_BUDGET / 2 {
                // edited() also wakes the worker under the mount lock.
                self.inner.signal.pressure.store(true, Ordering::Release);
                self.inner.signal.worker.wake();
            }
            let charged = state.charged();
            if charged.saturating_add(incoming) <= DIRTY_BUDGET || charged == 0 {
                return Ok(state);
            }
            drop(state);
            // Admission can run on the commit worker while staging mappings.
            // Publish without servicing mappings recursively; waiting first
            // would ask that same worker to complete its own generation.
            self.inner.signal.requested.store(true, Ordering::Release);
            let published = {
                let _publication = self.inner.costs.span(cost, CostPhase::Publication);
                self.commit_generation(monotonic_ns())?
            };
            if published {
                continue;
            }
            // Another publisher owns the detached generation. Its completion
            // wakes us, with neither mount nor mapping locks held here.
            self.mutation_wait(cost, observed, None)?;
        }
    }
    /// Admission for a change to one file's data. Other callers wait while a
    /// write admitted in parts holds the file (N2), so write(2) stays atomic
    /// with respect to writes and size changes of that file; `Split::Take`
    /// sets that mark and `Split::Held` is its owner's later part.
    fn admit_data(
        &self,
        ino: u64,
        incoming: u64,
        split: Split,
        cost: CostOp,
    ) -> Result<MountGuard<'_, MountState<W>>, Errno> {
        loop {
            // Observed before the mark is read under the mount lock; clearing
            // it bumps the epoch under that lock, so the wake is not lost.
            let observed = self.inner.signal.epoch();
            let mut state = self.admit(incoming, cost)?;
            let marked = state.nodes.get(&ino).is_some_and(|node| node.splitting);
            if split == Split::Held || !marked {
                if split == Split::Take {
                    state.nodes.entry(ino).or_default().splitting = true;
                }
                return Ok(state);
            }
            drop(state);
            self.mutation_wait(cost, observed, None)?;
        }
    }
    /// Retry admission after publishing retained grants and the PENDING ranges
    /// visible at its first refusal (E24 ledger 379(2), 509(n)). Reserve and
    /// validate again under the new lock: publication can change COW estimates, and another caller can
    /// change names, ownership or append offsets while the lock is dropped.
    fn reserve_with_pressure<'a, T>(
        &'a self,
        cost: CostOp,
        mut state: MountGuard<'a, MountState<W>>,
        mut admit: impl FnMut() -> Result<MountGuard<'a, MountState<W>>, Errno>,
        mut reserve: impl FnMut(&mut MountState<W>) -> Result<T, AdmissionError>,
    ) -> Result<(MountGuard<'a, MountState<W>>, T), Errno> {
        #[cfg(test)]
        let mut admission = measurement::Admission::new();
        let create = matches!(cost, CostOp::Create);
        let mut readmit = || {
            if create {
                // The entry lock already gave this create its replay share.
                // Both the scratch view and the materialized view admit edits.
                let _wait = self.inner.costs.span(cost, CostPhase::Wait);
                self.inner.state.lock_interruptible()
            } else {
                admit()
            }
        };
        let mut limit = None;
        loop {
            state.writable()?;
            state.protect_edit()?;
            let result = {
                let _reservation = self.inner.costs.span(cost, CostPhase::Reservation);
                reserve(&mut state)
            };
            #[cfg(test)]
            admission.attempt(result.as_ref().err().map(|error| error.errno().0));
            let denial = match result {
                Ok(grant) => return Ok((state, grant)),
                Err(AdmissionError::Denied(denial)) => denial,
                Err(AdmissionError::Other(error)) => {
                    if error == Errno(ENOSPC) {
                        self.note_refusal(&state, cost, state.durable.generation, None);
                    }
                    return Err(error);
                }
            };
            if limit.is_none() && state.pending.is_some() {
                // A frozen generation's releases and grants settle only at its
                // install. Wait for it before fixing this refusal's bound.
                self.note_refusal(&state, cost, state.durable.generation.saturating_add(1), Some(denial));
                let observed = self.inner.signal.epoch();
                drop(state);
                self.mutation_wait(cost, observed, None)?;
                state = readmit()?;
                continue;
            }
            let last = {
                let _reservation = self.inner.costs.span(cost, CostPhase::Reservation);
                match limit {
                    Some(last) => last,
                    None => {
                        let sb = &state.durable;
                        let promotion = state.durable_pending.ok_or(Errno(EIO))?.promotion_generation()
                            .map_err(|_| Errno(EIO))?;
                        // Capture a finite publication bound on the first refusal.
                        // The open generation can hold admission capacity without
                        // any durable PENDING range (#3064); release its grants too.
                        let retained = (!state.grants.is_empty())
                            .then(|| sb.generation.checked_add(1).ok_or(Errno(ENOSPC)))
                            .transpose()?;
                        let Some(generation) = promotion.into_iter().chain(retained).max() else {
                            self.note_refusal(&state, cost, sb.generation, Some(denial));
                            return Err(Errno(ENOSPC));
                        };
                        // A6 bounds release generations by this superblock's
                        // generation, including the detached publication's view.
                        if generation > sb.generation.saturating_add(2) {
                            return Err(Errno(EIO));
                        }
                        // An already eligible range still needs a publication to
                        // enter durable.free_blocks, which admission uses (design 2.5).
                        let last = generation.max(sb.generation.checked_add(1).ok_or(Errno(ENOSPC))?);
                        limit = Some(last);
                        last
                    }
                }
            };
            self.note_refusal(&state, cost, last, Some(denial));
            if state.durable.generation >= last { return Err(Errno(ENOSPC)); }
            let observed = self.inner.signal.epoch();
            // The worker can finish a backlog without spending another
            // create replay share or waiting on a mapping held by this caller.
            // Writeback still publishes directly: the worker can be its caller.
            if create && (state.next_files.is_some() || !state.refresh.is_empty()) {
                let waiter = state.committer.as_mut().ok_or(Errno(EIO))?
                    .try_waiter(|receipt| Arc::try_new(receipt).ok()).ok_or(Errno(ENOMEM))?;
                #[cfg(test)]
                let requested = !state.reservation_pressure;
                state.reservation_pressure = true;
                drop(state);
                let signal = &self.inner.signal;
                signal.worker.wake();
                loop {
                    let observed = signal.epoch();
                    if let Some(result) = waiter.result() {
                        result.map_err(|_| Errno(EIO))?;
                        break;
                    }
                    #[cfg(not(test))]
                    if !signal.started.load(Ordering::Acquire) { return Err(Errno(EIO)); }
                    self.mutation_wait(cost, observed, None)?;
                }
                #[cfg(test)]
                if requested { admission.pressure(true); }
                state = readmit()?;
                continue;
            }
            // A racing install may introduce a backlog. Stop the inline
            // attempt at that generation and revalidate admission first.
            let through = if create { state.durable.generation + 1 } else { last };
            drop(state);
            // Do not service mappings recursively: a mapping writer can be the
            // caller. Detached publication and its wait hold no mount lock.
            let published = {
                let _publication = self.inner.costs.span(cost, CostPhase::Publication);
                self.commit_generation_up_to(monotonic_ns(), through, true)?
            };
            #[cfg(test)]
            admission.pressure(published);
            if !published {
                self.mutation_wait(cost, observed, None)?;
            }
            state = readmit()?;
        }
    }
    /// E24 coordinator 621(y2): record the denial using the installed
    /// summary and the ledger snapshot taken at the admission decision.
    fn note_refusal(
        &self, state: &MountState<W>, cost: CostOp, release_generation: u64,
        denial: Option<DeniedGrant>,
    ) {
        let sb = &state.durable;
        if !self.inner.costs.refusal_wanted(sb.generation) {
            return;
        }
        let grants_next = state.grants.blocks;
        self.inner.costs.refusal(cost, &mutation_cost::Refusal {
            generation: sb.generation,
            release_generation,
            free: sb.free_blocks,
            reserve: sb.global_reserve,
            pending: state.durable_pending.and_then(PendingSummary::census),
            accounting: denial.map(|d| (d.charged.saturating_sub(grants_next), d.request)),
            grants_next,
        });
    }
    /// Write `bytes` under one admission. The caller admitted them.
    fn write_part(
        &self,
        cost: CostOp,
        state: MountGuard<'_, MountState<W>>,
        node: &VNode,
        req: WriteReq,
        bytes: &[u8],
    ) -> Result<WriteResult, WriteError> {
        let split = if state.nodes.get(&node.ino).is_some_and(|n| n.splitting) {
            Split::Held
        } else { Split::No };
        let (mut state, (mut inode, offset, length, end, grant)) = self.reserve_with_pressure(
            cost, state,
            || self.admit_data(node.ino, touched_bytes(bytes.len()), split, cost),
            |state| {
                let inode = state.regular(node.ino)?;
                let offset = match req.at {
                    WriteAt::Offset(offset) => offset,
                    WriteAt::Append => inode.size,
                };
                if bytes.is_empty() || (req.limit != u64::MAX && offset >= req.limit) {
                    return Ok((inode, offset, 0, offset, None));
                }
                let length = (req.limit - offset).min(bytes.len() as u64) as usize;
                let end = offset.checked_add(length as u64)
                    .filter(|end| *end <= (1 << 63) - 4096).ok_or(Errno(EFBIG))?;
                let grant = if length == 0 { None } else {
                    // The inode record follows the page edit: reserve it first.
                    state.files.reserve(1).map_err(errno)?;
                    Some(state.reserve_data(node.ino, offset, length, Operation::Allocate,
                        inode.size == 0 && inode.nblocks == 0)?)
                };
                Ok((inode, offset, length, end, grant))
            },
        )?;
        let _edit = self.inner.costs.span(cost, CostPhase::Edit);
        let Some(grant) = grant else {
            return if bytes.is_empty() {
                Ok(WriteResult { offset, bytes: 0, limited: false })
            } else { Err(WriteError::FileSizeLimit) };
        };
        // Bound holes need frames before file bytes change. Preparation can
        // allocate; completing a published write only copies into those frames.
        if node.map.prepare_write(offset, length).is_err() {
            state.reservations.release(grant.grant);
            return Err(Errno(ENOMEM).into());
        }
        let result = self.edit_pages(&mut state, node.ino, "write-read-metadata",
            |open, read| open.pages.write(offset, &bytes[..length], read));
        state.finish_data(node.ino, grant, result)?;
        inode.size = inode.size.max(end);
        inode.mtime = now();
        inode.ctime = inode.mtime;
        if req.clear_setid {
            inode.mode &= !content_privilege(inode.mode);
        }
        match state.files.put_reserved_inode(node.ino, inode) {
            Err(error) => {
                let result = state.edited::<()>(&[node.ino], Err(error));
                node.map.transition(node.size.load(Ordering::Acquire));
                node.map.invalidate(offset, length as u64);
                result?;
            }
            Ok(()) => {
                state.fresh = false;
                // This data write changes one inode. Keep its known result
                // instead of rereading the unrelated live VNodes (#3040).
                state.inodes.entries.fill(None);
                state.inodes.entries[0] = Some((node.ino, inode));
                state.inodes.next = 1;
                node.size.store(inode.size, Ordering::Release);
                *node.times.lock() = attributes(node.ino, inode)?.times;
                state.signal.worker.wake();
            }
        }
        // Growth clears the old EOF tail before the new bytes are copied.
        node.map.transition(inode.size);
        node.map.commit_write(offset, &bytes[..length]);
        self.inner.costs.record_write(length);
        Ok(WriteResult {
            offset,
            bytes: length,
            limited: length < bytes.len(),
        })
    }
    /// The parts after the first, which `first` admitted with the mark set.
    /// An error after some bytes are written returns those bytes, as write(2)
    /// reports a partial transfer.
    fn write_parts(
        &self,
        first: MountGuard<'_, MountState<W>>,
        node: &VNode,
        req: WriteReq,
        bytes: &[u8],
    ) -> Result<WriteResult, WriteError> {
        let mut state = Some(first);
        let mut part = req;
        let mut start = None;
        let mut done = 0;
        loop {
            let end = (done + WRITE_PART).min(bytes.len());
            let written = match state.take() {
                Some(guard) => Ok(guard),
                None => self
                    .admit_data(node.ino, touched_bytes(end - done), Split::Held, CostOp::Write)
                    .map_err(WriteError::from),
            }
            .and_then(|guard| self.write_part(CostOp::Write, guard, node, part, &bytes[done..end]));
            let written = match (written, start) {
                (Ok(written), _) => written,
                (Err(error), None) => return Err(error),
                (Err(_), Some(offset)) => {
                    return Ok(WriteResult { offset, bytes: done, limited: false });
                }
            };
            let offset = *start.get_or_insert(written.offset);
            done += written.bytes;
            if written.limited || done == bytes.len() {
                return Ok(WriteResult { offset, bytes: done, limited: written.limited });
            }
            part.at = WriteAt::Offset(written.offset + written.bytes as u64);
        }
    }
    /// One commit-worker pass. Preparation and opening G+1 are serialized;
    /// device publication and completion waits hold no mount lock.
    fn commit_due(&self, now_ns: u64) -> Result<bool, Errno> {
        preemption_point();
        let (retry_ns, reservation_pressure) = {
            let state = self.inner.state.lock();
            (state.mapping_retry_ns, state.reservation_pressure && !state.publication_blocked())
        };
        if reservation_pressure && self.commit_generation(now_ns)? {
            return Ok(true);
        }
        if now_ns >= retry_ns && self.inner.signal.mappings.swap(false, Ordering::AcqRel) {
            match self.service_mappings(now_ns) {
                Ok(worked) => return Ok(worked),
                Err(error) => {
                    let mut state = self.inner.state.lock();
                    // Shutdown can close admission while a service snapshot is in
                    // flight. Its own staged generation owns that final barrier.
                    if error == Errno(ENOMEM) || error == Errno(ENOSPC) {
                        // Keep dirty custody. Defer ENOSPC to the next interval.
                        // Pressure publication wakes this worker and creates new
                        // PENDING metadata even if no data space becomes available;
                        // retrying on that wake would force commits indefinitely.
                        if error == Errno(ENOSPC) {
                            state.mapping_retry_ns = now_ns.saturating_add(COMMIT_INTERVAL_NS);
                        }
                        self.inner.signal.mappings.store(true, Ordering::Release);
                    } else if !state.closed || state.failed {
                        state.fail("service-mappings", error);
                        return Err(error);
                    }
                }
            }
        }
        self.commit_generation(now_ns)
    }
    fn service_mappings(&self, now_ns: u64) -> Result<bool, Errno> {
        self.inner.mapping_passes.fetch_add(1, Ordering::Relaxed);
        // A publication owns copies of staged bytes. Holding a whole
        // snapshot of VNodes across that I/O would keep even closed orphans
        // and their cache pages alive until the publication returns.
        let handles = self.mapped_nodes(|node| Arc::downgrade(&node))?;
        let mut budget = crate::memory::file_map::EVICT_BUDGET;
        let blocked = self.inner.state.lock().publication_blocked();
        // While a foreign publication blocks staging, a retry can spend the
        // whole pass on frozen receipts. Give ready clean pages first use of
        // the budget in that case.
        let mut clean_left = blocked && self.retire_mappings(&handles, &mut budget);
        let mut more = false;
        let mut staged = Vec::new();
        staged.try_reserve(handles.len().min(budget)).map_err(|_| Errno(ENOMEM))?;
        for (index, handle) in handles.iter().enumerate() {
            preemption_point();
            let Some(node) = handle.upgrade() else { continue };
            if !self.inner.state.lock().failed {
                let result = node.map.service_writeback_with(&node, &mut budget, |first, last, generation| {
                    staged.push((index, first, last));
                    if let Some(generation) = generation {
                        let state = self.inner.state.lock();
                        if generation == state.open_generation() {
                            self.inner.signal.requested.store(true, Ordering::Release);
                        }
                    }
                    Ok(())
                });
                match result {
                    Ok(pending) => more |= pending,
                    Err(error) => {
                        // Earlier snapshots advanced their cursors but have no
                        // durable receipt yet. Keep them eligible for retry.
                        for &(index, _, _) in &staged {
                            if let Some(node) = handles[index].upgrade() {
                                node.map.request_writeback();
                            }
                        }
                        return Err(error);
                    }
                }
            }
        }
        #[cfg(test)]
        super::preempt::context::observe(super::preempt::context::Event::MappingsStaged);
        if !blocked {
            clean_left = self.retire_mappings(&handles, &mut budget);
        }
        // Keep the request visible while this pass publishes, including when
        // staging used the budget and eviction has yet to retire any pages.
        if clean_left {
            self.request_eviction();
        }
        // E24 ledger 558(x4): the pass joins its mapping snapshots into one
        // generation. Complete their dirty custody only after that barrier;
        // commit_due must not publish a second generation after this returns.
        let worked = if self.inner.state.lock().failed { false }
            else { self.commit_generation(now_ns)? };
        let mut deferred = false;
        for (index, first, last) in staged {
            preemption_point();
            if let Some(node) = handles[index].upgrade() {
                deferred |= self.complete_mapping(&node, first, last);
            }
        }
        let more_clean = self.retire_mappings(&handles, &mut budget);
        // Dirty receipts wait for the publication, but ready clean work can
        // make progress even before that publication installs.
        if more_clean || (more && !deferred) { self.request_eviction(); }
        if self.inner.state.lock().failed { Err(Errno(EIO)) } else { Ok(worked) }
    }
    fn retire_mappings(&self, handles: &[Weak<VNode>], budget: &mut usize) -> bool {
        let mut more = false;
        for handle in handles {
            preemption_point();
            if let Some(node) = handle.upgrade() {
                more |= self.retire_mapping(&node, budget);
            }
        }
        more
    }
    fn retire_mapping(&self, node: &Arc<VNode>, budget: &mut usize) -> bool {
        let mut more = false;
        // Population and mutation take the mount lock before MapState.
        let mut state = self.inner.state.lock();
        if state.failed {
            if let Some(entry) = state.nodes.get_mut(&node.ino) {
                let internal = 1 + usize::from(entry.mapped.is_some());
                if Arc::strong_count(node) == internal {
                    // No descriptor or binding can observe these pages.
                    // Failed storage cannot write them back; retire them
                    // under the ordinary page budget, reporting EIO below.
                    more |= node.map.discard_unobserved(budget);
                } else if node.map.eviction_pending() {
                    // External handles now own the cache lifetime. Dropping
                    // the final descriptor must not leave a service cycle.
                    entry.mapped = None;
                }
            }
        }
        if node.map.eviction_pending() {
            let before = *budget;
            let clean_left = node.map.evict(budget);
            self.inner.mapping_pages_evicted.fetch_add((before - *budget) as u64, Ordering::Relaxed);
            if clean_left && !node.map.eviction_pending() {
                self.inner.mapping_requests_dropped.fetch_add(1, Ordering::Relaxed);
            }
            more |= clean_left;
        }
        if !node.map.nonempty() {
            if let Some(entry) = state.nodes.get_mut(&node.ino) { entry.mapped = None; }
        }
        more
    }
    fn mapped_nodes<T>(&self, mut capture: impl FnMut(Arc<VNode>) -> T) -> Result<Vec<T>, Errno> {
        #[cfg(test)] let _phase = publication_phases::Phase::new("mappings");
        let state = self.inner.state.lock_interruptible()?;
        let mut handles = Vec::new();
        if state.closed || state.committer.is_none() {
            return Ok(handles);
        }
        handles.try_reserve(state.nodes.len()).map_err(|_| Errno(ENOMEM))?;
        for entry in state.nodes.values() {
            preemption_point();
            #[cfg(test)] publication_phases::visit(1);
            if let Some(node) = entry.node.upgrade() {
                if entry.mapped.is_some() || node.map.has_dirty() || node.map.eviction_pending() || node.map.writeback_pending() {
                    handles.push(capture(node));
                }
            }
        }
        Ok(handles)
    }
    fn sync_mappings(&self, deadline: Option<u64>) -> Result<(), Errno> {
        if self.inner.state.lock_interruptible()?.failed { return Err(Errno(EIO)); }
        let nodes = self.mapped_nodes(core::convert::identity)?;
        // Shutdown stages snapshots without an unbounded foreground generation
        // wait; its final sync_generation below retains the supplied deadline.
        let mut buffer = if deadline.is_some() && nodes.iter().any(|n| n.map.has_dirty()) {
            crate::memory::file_map::writeback_buffer().map_err(|_| Errno(ENOMEM))?
        } else { Vec::new() };
        let _sync = crate::memory::file_map::SyncMaps::new(&nodes);
        for node in &nodes {
            if deadline.is_some_and(|limit| monotonic_ns() >= limit) {
                return Err(Errno(ETIMEDOUT));
            }
            if node.map.has_dirty() {
                if deadline.is_some() {
                    crate::memory::file_map::stage_range_with_buffer(node, 0, u64::MAX, &mut buffer)?;
                } else {
                    crate::memory::file_map::sync_range(node, 0, u64::MAX)?;
                }
            }
        }
        Ok(())
    }
    fn complete_mapping(&self, node: &VNode, first: u64, last: u64) -> bool {
        let state = self.inner.state.lock();
        if state.failed || !node.map.complete_writeback_through(first, last, state.durable.generation) {
            return false;
        }
        self.inner.signal.mappings.store(true, Ordering::Release);
        // Install, abandonment and leaving the sync retention window wake
        // the worker. Observe the blocker under the same lock as completion.
        if !state.publication_blocked() {
            self.inner.signal.worker.wake();
        }
        true
    }
    fn commit_generation(&self, now_ns: u64) -> Result<bool, Errno> {
        self.commit_generation_up_to(now_ns, u64::MAX, false)
    }
    fn commit_generation_up_to(&self, now_ns: u64, last: u64, force: bool) -> Result<bool, Errno> {
        preemption_point();
        #[cfg(not(test))]
        crate::block::cache_kernel::assert_io_context();
        let _publication;
        #[cfg(test)] let _cache_cleanup;
        // FREEZE (E24 coordinator 597, 601): the first of the publication's two
        // mount-exclusion sections. Pointer, map and queue moves only: no tree
        // or cache traversal, no allocation of file data and no yield.
        let frozen = loop {
            let mut guard = {
                let _wait = self.inner.costs.span(CostOp::Commit, CostPhase::Wait);
                self.inner.state.lock_interruptible()?
            };
            // A clean mapping pass can reach here during another publication.
            // It cannot start a freeze section until that publication installs.
            if guard.retaining_for_sync != 0 || (guard.pending.is_some() && !guard.failed) {
                return Ok(false);
            }
            if guard.durable.generation >= last && !guard.failed {
                return Ok(false);
            }
            // 583(x10): start after acquisition; stop before releasing this
            // guard. Excludes acquisition and everything after the guard.
            let _excluded = self.inner.costs.span(CostOp::Commit, CostPhase::MountExcluded);
            if let Some(stale) = guard.stale_views.take() {
                // The views a completed replay replaced: drop them outside the
                // exclusion and before this publication starts.
                drop(_excluded);
                drop(guard);
                drop(stale);
                continue;
            }
            let state = &mut *guard;
            if state.failed {
                return Err(Errno(EIO));
            }
            if state.committer.is_none() {
                return Ok(false);
            }
            if state.next_files.is_some() || !state.refresh.is_empty() {
                // Finish one share per hold before freezing. Account for it
                // separately from the two publication sections.
                drop(_excluded);
                let _replay = self.inner.costs.span(CostOp::Foreground, CostPhase::MountExclusion);
                drop(self.materialized(guard));
                continue;
            }
            let fsync = self.inner.signal.requested.swap(false, Ordering::AcqRel);
            let pressure = self.inner.signal.pressure.swap(false, Ordering::AcqRel);
            let reservation_pressure = core::mem::take(&mut state.reservation_pressure);
            if !state.policy.due(now_ns, state.dirty_bytes, fsync || force || reservation_pressure, pressure) {
                return Ok(false);
            }
            _publication = self.inner.costs.span(CostOp::Commit, CostPhase::Publication);
            #[cfg(test)] { _cache_cleanup = publication_phases::CacheCleanup::new(); }
            #[cfg(test)]
            measurement::prepared(force || reservation_pressure, pressure);
            match self.freeze(state, now_ns) {
                Ok(frozen) => break frozen,
                Err(error) => {
                    state.fail("freeze", error);
                    return Err(Errno(EIO));
                }
            }
        };
        // PREPARE frozen N without mount exclusion; its steps may yield (488).
        // Foreground callers keep running on N+1 and read frozen N's pages.
        preemption_point();
        let mut frozen = frozen;
        frozen.files.begin_preparation();
        let touches_span = self.inner.costs.span(CostOp::Commit, CostPhase::ApplyTouches);
        #[cfg(test)] let touches_phase = publication_phases::Phase::new("apply_touches");
        // N's deferred times, from N's slot (601(2)).
        let mut times = core::mem::take(&mut frozen.touches);
        let touched: Vec<u64> = times.keys().copied().collect();
        for ino in touched {
            preemption_point();
            #[cfg(test)] publication_phases::visit(0);
            let Some(stamps) = times.remove(&ino) else { continue; };
            if let Err(error) = frozen.apply_touch(ino, &stamps) {
                return self.abandon(frozen, "apply-touches", errno(error));
            }
        }
        #[cfg(test)] drop(touches_phase);
        drop(touches_span);
        let drained = {
            let _drain = self.inner.costs.span(CostOp::Commit, CostPhase::DrainDirty);
            frozen.drain_dirty(&self.data_reader())
        };
        if let Err(error) = drained {
            return self.abandon(frozen, "drain-dirty", error);
        }
        // ROOT removal is its own durable operation. The next policy-due
        // generation starts reaping, with at most REAP_PER_COMMIT nodes.
        let reaped = (|| -> Result<u64, ops::Error> {
            let _prepare = self.inner.costs.span(CostOp::Commit, CostPhase::PrepareReap);
            #[cfg(test)] let _phase = publication_phases::Phase::new("prepare_reap");
            let FrozenGeneration { files, grants, reap, free_blocks, ledger, .. } = &mut frozen;
            let txn = files.prepare_commit()?;
            if *reap {
                // Reaping is optional: admission refusal changes no trees
                // and must not discard already acknowledged dirty writes.
                let grant = if grants.try_reserve(1).is_ok() {
                    let blocks = txn.reap_reservation_blocks()?;
                    match ledger.reserve(*free_blocks, blocks, Operation::DeleteSnapshot) {
                        Ok(grant) => Some(grant),
                        Err(_) => None,
                    }
                } else { None };
                *reap = grant.is_some();
                if let Some(grant) = grant {
                    grants.push(grant);
                    for _ in 0..super::txn::REAP_PER_COMMIT {
                        preemption_point();
                        if txn.reap_step()?.is_none() { break; }
                    }
                }
            }
            Ok(txn.dead_count())
        })();
        let dead_remaining = match reaped {
            Ok(remaining) => remaining,
            Err(error) => return self.abandon(frozen, "prepare-reap", errno(error)),
        };
        let prepared = {
            let _prepare = self.inner.costs.span(CostOp::Commit, CostPhase::PrepareFiles);
            frozen.commit.prepare_settled_files(self.inner.device.as_ref(), &mut frozen.files)
        };
        let prepared = match prepared {
            Ok(prepared) => Arc::new(prepared),
            Err(_) => return self.abandon(frozen, "prepare-files", Errno(EIO)),
        };
        let durable_pending = frozen.files.pending_summary();
        // N's next view, built here on frozen N; install only swaps it in.
        let sb = prepared.superblock();
        let store = self
            .inner
            .cache
            .store(self.inner.device.clone(), sb)
            .with_pending(&prepared);
        let next = {
            let _next = self.inner.costs.span(CostOp::Commit, CostPhase::NextFilesView);
            frozen.files.next_generation(store, sb)
        };
        if let Err(error) = next {
            return self.abandon(frozen, "open-next-generation", errno(error));
        }
        {
            // Retire stale clean data-cache entries for the blocks N allocated,
            // still before any view can name them (no view does until install).
            // Afterwards a reader of N would fail N's CRC on such stale bytes.
            let _cleanup = self.inner.costs.span(CostOp::Commit, CostPhase::Cleanup);
            for (start, count) in prepared.data_ranges() {
                preemption_point();
                self.inner.cache.buffers().discard_clean_range(self.inner.data.as_ref(), start, count);
            }
        }
        preemption_point();
        let signal = self.inner.signal.clone();
        let group = WriteGroup::ready_hold(
            self.inner.device.clone(),
            Arc::new(move || {
                signal.pressure.store(true, Ordering::Release);
                signal.worker.wake();
            }),
        );
        let mut publication = {
            let _wait = self.inner.costs.span(CostOp::Commit, CostPhase::Wait);
            self.inner.mounted_root.device.publication.lock()
        };
        #[cfg(any(feature = "breenfs_commit_window", feature = "breenfs_corrupt_inject"))]
        let armed = self.inner.commit_faults.as_ref().and_then(|faults| {
            faults.armed_inode(|ino| frozen.pages.nodes.get(&ino)
                .is_some_and(|node| node.pages.get(1).is_some())).map(|ino| (faults, ino))
        });
        #[cfg(feature = "breenfs_corrupt_inject")]
        let injection = armed.and_then(|(faults, ino)| {
            let injection = faults.injection()?;
            // The probe appends record 2 as a separate block after receipt 1.
            let block = frozen.files.extents(ino).ok()?.into_iter().find_map(|(key, extent)| {
                let index = (BLOCK_SIZE as u64).checked_sub(key.offset)? / BLOCK_SIZE as u64;
                (index < u64::from(extent.num_blocks))
                    .then_some(extent.phys_start + u64::from(extent.extent_offset) + index)
            })?;
            Some((injection, block))
        });
        let result = {
            let _write = self.inner.costs.span(CostOp::Commit, CostPhase::WriteCached);
            prepared.write_cached(
                self.inner.cache.buffers(),
                &self.inner.costs.device(self.inner.device.as_ref()),
                &group,
                #[cfg(feature = "breenfs_commit_window")]
                armed.and_then(|(faults, _)| faults.selected_window()),
                #[cfg(feature = "breenfs_corrupt_inject")]
                injection,
            )
        };
        let FrozenGeneration { files: next, commit, pages, dirty_nodes, touched, grants, reap, mut ledger, .. } = frozen;
        let mut refresh = dirty_nodes;
        refresh.extend(touched);
        // INSTALL (601(4)): the second exclusion section. Swap the published
        // view to N and drop the frozen layer's reference: constant work.
        let (retired, abandoned_replay) = {
            let mut guard = self.mutation_lock(CostOp::Commit);
            // 583(x10): start after acquisition; stop before releasing this
            // guard. Excludes acquisition and everything after the guard.
            let _excluded = self.inner.costs.span(CostOp::Commit, CostPhase::MountExcluded);
            let _completion = self.inner.costs.span(CostOp::Commit, CostPhase::Completion);
            #[cfg(test)] let _phase = publication_phases::Phase::new("completion");
            let state = &mut *guard;
            if result.is_ok() {
                #[cfg(test)]
                measurement::published();
                let sb = prepared.superblock();
                if let Some(committer) = state.committer.as_mut() {
                    committer.install(sb);
                }
                state.durable = sb.clone();
                state.durable_pending = Some(durable_pending);
                // With the free count that excludes N's allocations, stop
                // charging N's grants for them; each is released below (M3).
                ledger.retire(grants.blocks);
                *publication = Some(sb.clone());
                // The publication keeps its own reference: no page is freed here.
                let installed_pages = state.pending.take();
                state.dead_remaining = dead_remaining;
                // Empty: the freeze ran after the previous materialization.
                state.refresh = refresh;
                let replay = state.files.log.as_ref().is_some_and(|log| !log.is_empty());
                let retired = if replay {
                    // Holders replay N+1's record a bounded share at a time
                    // (x11c M2); until then the scratch view reads N's pages.
                    state.installed = installed_pages;
                    state.next_files = Some(next);
                    None
                } else {
                    // N+1 made no change while N was frozen: N's next view is
                    // N+1's view as it stands.
                    state.files.log = None;
                    state.files.replay_bounds = None;
                    state.inodes.entries.fill(None);
                    Some(core::mem::replace(&mut state.files.files, next))
                };
                self.inner.materialize.store(true, Ordering::Release);
                if reap && state.dead_remaining != 0 && !state.reap_shutdown {
                    self.inner.signal.requested.store(true, Ordering::Release);
                }
                (retired, None)
            } else {
                state.fail("publish", result.unwrap_err());
                if let Some(committer) = state.committer.as_mut() {
                    committer.abandon();
                }
                // N is not installed, so N+1's record has no view to replay
                // into (764 k2). The scratch view still serves reads.
                (None, state.files.detach_replay())
            }
        };
        drop(publication);
        // After the exclusion, on the worker, in this order (601(4)): wake N's
        // receipts, then release N's grants.
        commit.wake(result.map_err(|_| super::commit::Error::EIO));
        {
            let _cleanup = self.inner.costs.span(CostOp::Commit, CostPhase::Cleanup);
            let installed = result.is_ok();
            for grant in grants {
                preemption_point();
                #[cfg(test)] publication_phases::visit(3);
                if installed { ledger.release_retired(grant); } else { ledger.release(grant); }
            }
        }
        drop(retired);
        drop(abandoned_replay);
        drop(pages);
        preemption_point();
        self.inner.signal.wake_all();
        result.map(|_| true).map_err(|_| Errno(EIO))
    }
    /// Freeze the open generation N into its publication (E24 coordinator
    /// 597/601). Called inside the freeze section: every step is a constant
    /// move, swap or small allocation, independent of what N changed.
    fn freeze(&self, state: &mut MountState<W>, now_ns: u64) -> Result<FrozenGeneration<W>, Errno> {
        // The refusable steps precede the receipt transfer, so a refusal
        // leaves each fsync waiter with the committer, whose failure wakes it.
        let mut pages = Arc::try_new(FrozenPages { nodes: BTreeMap::new(), bytes: 0 })
            .map_err(|_| Errno(ENOMEM))?;
        // 601(1): N's transaction moves out whole; N+1 gets a scratch fork.
        let store = self.inner.cache.store(self.inner.device.clone(), &state.durable);
        let scratch = state.files.files.fork(store).map_err(errno)?;
        // 601(3): the receipt boundary moves here.
        let commit = state.committer.as_mut().ok_or(Errno(EIO))?.freeze().map_err(|_| Errno(EIO))?;
        let files = core::mem::replace(&mut state.files.files, scratch);
        state.files.log = Some(ReplayLog::new());
        // 601(2): N's pages and touches move whole; N+1 starts with empty maps.
        let frozen = Arc::get_mut(&mut pages).ok_or(Errno(EIO))?;
        frozen.nodes = core::mem::take(&mut state.dirty);
        frozen.bytes = core::mem::take(&mut state.dirty_bytes);
        let dirty_blocks = frozen.bytes / BLOCK_SIZE as u64;
        state.pending = Some(pages.clone());
        let reap = !core::mem::take(&mut state.skip_reap)
            && !state.closed && !state.reap_shutdown && state.dead_remaining != 0;
        let growing_edits = core::mem::take(&mut state.drain_growth);
        state.files.replay_bounds = state.durable_pending.and_then(PendingSummary::census)
            .filter(|_| !reap && !files.has_orphans() && !files.includes_default_tree())
            .map(|(pending, _)| {
                // N can allocate at most durable FREE plus eligible PENDING.
                // A four-block node's REF costs 21 slot + 16 value bytes.
                // A data REF costs at most that plus 4 bytes per page, with
                // at most one REF per dirty page. Allocation replaces or
                // consumes a FREE record. Release and promotion replace a
                // REF or a PENDING record with an item of no larger size.
                // Snapshot creation can add one ROOT (21 + 65 + 255).
                let allocation_budget = state.durable.free_blocks.saturating_add(pending)
                    .min(state.durable.total_blocks);
                let node_bytes = allocation_budget / 4 * 37;
                ReplayBounds { frozen_root: files.root_pointer(), root_growth: None, growing_edits,
                    space_growth_bytes: node_bytes + dirty_blocks * 41 + 341 }
            });
        state.metadata_reserved = 0;
        state.first_write_space = 0;
        state.replay_space = 0;
        state.fresh = true;
        state.policy = CommitPolicy::new(now_ns);
        // SeqCst: touch() pairs this store with its slot record (x11c M4).
        self.inner.generation.store(commit.generation() + 1, Ordering::SeqCst);
        Ok(FrozenGeneration {
            files,
            commit,
            pages,
            dirty_nodes: core::mem::take(&mut state.dirty_nodes),
            touches: core::mem::take(&mut state.touches),
            touched: BTreeSet::new(),
            // 601(1): admission counts N's grants with N+1's until install.
            grants: core::mem::take(&mut state.grants),
            ledger: state.reservations.clone(),
            reap,
            free_blocks: state.durable.free_blocks,
        })
    }
    /// Frozen N failed before install: fail the mount, N+1's receipts with
    /// it, then N's receipts and grants after the exclusion.
    fn abandon(&self, mut frozen: FrozenGeneration<W>, branch: &'static str, error: Errno) -> Result<bool, Errno> {
        let abandoned_replay = {
            let mut state = self.mutation_lock(CostOp::Commit);
            let _excluded = self.inner.costs.span(CostOp::Commit, CostPhase::MountExcluded);
            state.fail(branch, error);
            if let Some(committer) = state.committer.as_mut() {
                committer.abandon();
            }
            // N will not be installed, so N+1's record has no view to replay
            // into (764 k2). Left attached, each later open and close on the
            // latched mount grew it (ledger 763). The scratch view and N's
            // pages still serve reads. The pages are freed after the exclusion.
            state.files.detach_replay()
        };
        drop(abandoned_replay);
        frozen.commit.wake(Err(super::commit::Error::EIO));
        for grant in frozen.grants {
            preemption_point();
            frozen.ledger.release(grant);
        }
        self.inner.signal.wake_all();
        Err(if branch == "prepare-reap" { error } else { Errno(EIO) })
    }
    fn sync_generation(&self, deadline: Option<u64>, closing: bool) -> Result<(), Errno> {
        let waiter = {
            let mut state = self.mutation_admission_lock(CostOp::Fsync)?;
            if state.failed {
                return Err(Errno(EIO));
            }
            if state.closed && !closing {
                return if self.inner.signal.stop.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    Err(Errno(EBUSY))
                };
            }
            if state.committer.is_none() {
                return Ok(());
            }
            // Finish any detached batch and one snapshot of the producer
            // queue. Later arrivals can wait for another caller. Keep the
            // generation open until these touches and closes join its receipt.
            let target = state.retained_batches + if state.retained_events.is_empty() { 1 } else { 2 };
            state.retaining_for_sync += 1;
            let error = loop {
                {
                    let _retain = self.inner.costs.span(CostOp::Fsync, CostPhase::RetainOpen);
                    if let Err(error) = state.retain_open() { break Some(error); }
                }
                if state.retained_batches >= target {
                    break None;
                }
                drop(state);
                preemption_point();
                state = self.mutation_lock(CostOp::Fsync);
                if state.failed {
                    break Some(Errno(EIO));
                }
                if deadline.is_some_and(|limit| monotonic_ns() >= limit) {
                    break Some(Errno(ETIMEDOUT));
                }
                #[cfg(not(test))]
                if crate::task::thread::current_wait_must_unwind() {
                    break Some(Errno(EINTR));
                }
                if state.retained_batches >= target {
                    break None;
                }
            };
            state.retaining_for_sync -= 1;
            if let Some(error) = error {
                drop(state);
                // Pressure callers can retry a publication when retention ends.
                self.inner.signal.wake_all();
                return Err(error);
            }
            let waiter = state.committer.as_mut().ok_or(Errno(EIO))?.waiter();
            self.inner.signal.requested.store(true, Ordering::Release);
            waiter
        };
        self.inner.signal.worker.wake();
        loop {
            let observed = self.inner.signal.epoch();
            if let Some(result) = waiter.result() {
                return result.map_err(|_| Errno(EIO));
            }
            if deadline.is_some_and(|limit| monotonic_ns() >= limit) {
                return Err(Errno(ETIMEDOUT));
            }
            // Host conformance drives the same worker pass explicitly. Kernel
            // callers only sleep; the mount's commit kthread performs the I/O.
            #[cfg(test)]
            self.commit_due(monotonic_ns())?;
            if waiter.result().is_none() {
                self.mutation_wait(CostOp::Fsync, observed, deadline)?;
            }
        }
    }
    // A batch holds publication/reuse exclusion only while reading its durable
    // view. An in-flight next generation cannot overwrite that view under 2.5.
    fn scrub_pass(&self, budget: usize) -> Result<(bool, usize), Errno> {
        let signal = &self.inner.scrub_signal;
        let mut state = self.inner.state.lock();
        if state.scrub_state != SCRUB_RUNNING {
            return Ok((false, 0));
        }
        if state.closed || signal.stop.load(Ordering::Acquire) {
            state.scrub_state = SCRUB_CANCELLED;
            #[cfg(test)]
            signal.started.store(false, Ordering::Release);
            return Ok((false, 0));
        }
        let root = state.durable.space_root;
        let generation = state.durable.generation;
        let walk = state.scrub.as_mut().ok_or(Errno(EIO))?;
        let result = walk.step(&super::commit::DeviceStore(self.inner.device.as_ref()),
            root, generation, budget);
        let completion = walk.completion();
        match result {
            Err(error) => {
                state.scrub_state = SCRUB_FAILED;
                #[cfg(test)]
                signal.started.store(false, Ordering::Release);
                Err(errno(error.into()))
            }
            Ok(bytes) => {
                if let Some(done) = completion {
                    state.scrub_state = SCRUB_DONE;
                    if !state.read_only() {
                        if let Some(committer) = state.committer.as_mut() {
                            committer.record_scrub(done.generation, done.errors);
                        }
                    }
                    #[cfg(test)]
                    signal.started.store(false, Ordering::Release);
                    Ok((false, bytes))
                } else { Ok((true, bytes)) }
            }
        }
    }
    fn start_scrub(&self) -> Result<(), Errno> {
        let signal = &self.inner.scrub_signal;
        {
            let mut state = self.inner.state.lock();
            if state.closed || signal.started.swap(true, Ordering::AcqRel) {
                return Err(Errno(EBUSY));
            }
            state.scrub = Some(Scrub::new());
            state.scrub_state = SCRUB_RUNNING;
            signal.stop.store(false, Ordering::Release);
        }
        #[cfg(not(test))]
        {
            let mount = Arc::downgrade(&self.inner);
            let signal = signal.clone();
            if crate::task::kthread::kthread_run(move || {
                'service: loop {
                    let Some(inner) = mount.upgrade() else { break; };
                    let fs = Self { inner };
                    // Match the commit worker's I/O context on both arches.
                    #[cfg(target_arch = "x86_64")]
                    crate::per_cpu::preempt_disable();
                    #[cfg(target_arch = "aarch64")]
                    crate::per_cpu_aarch64::preempt_disable();
                    let pass = fs.scrub_pass(super::format::NODE_SIZE);
                    drop(fs);
                    #[cfg(target_arch = "x86_64")]
                    crate::per_cpu::preempt_enable();
                    #[cfg(target_arch = "aarch64")]
                    crate::per_cpu_aarch64::preempt_enable();
                    let Ok((true, bytes)) = pass else { break; };
                    // Charge raw I/O at decimal 50 MB/s, with at least one
                    // scheduling interval after releasing exclusion, even on a
                    // slow device. Spurious wakes cannot consume the next batch.
                    let pause = (bytes as u64 * 1_000_000_000 / 50_000_000).max(1_000_000);
                    let deadline = monotonic_ns().saturating_add(pause);
                    while !signal.stop.load(Ordering::Acquire) && monotonic_ns() < deadline {
                        let observed = signal.epoch();
                        if !signal.stop.load(Ordering::Acquire)
                            && signal.wait(observed, Some(deadline)).is_err()
                        {
                            break 'service;
                        }
                    }
                }
                signal.started.store(false, Ordering::Release);
                signal.wake_waiters();
            }, "breenfs-scrub").is_err() {
                let mut state = self.inner.state.lock();
                state.scrub_state = SCRUB_FAILED;
                self.inner.scrub_signal.started.store(false, Ordering::Release);
                self.inner.scrub_signal.wake_waiters();
                return Err(Errno(ENOMEM));
            }
        }
        Ok(())
    }
    fn stop_scrub(&self, deadline: u64) -> Result<(), Errno> {
        let signal = &self.inner.scrub_signal;
        signal.stop.store(true, Ordering::Release);
        signal.wake_waiters();
        #[cfg(test)]
        self.scrub_pass(super::format::NODE_SIZE)?;
        while signal.started.load(Ordering::Acquire) {
            let observed = signal.epoch();
            if monotonic_ns() >= deadline { return Err(Errno(ETIMEDOUT)); }
            if signal.started.load(Ordering::Acquire) {
                signal.wait(observed, Some(deadline))?;
            }
        }
        Ok(())
    }
    fn scrub_status(&self) -> ScrubStatus {
        let state = self.inner.state.lock();
        let mut status = ScrubStatus {
            state: state.scrub_state,
            last_scrub_generation: state.durable.last_scrub_generation,
            last_scrub_errors: state.durable.last_scrub_errors,
            ..ScrubStatus::default()
        };
        if let Some(walk) = &state.scrub {
            status.bytes_scanned = walk.blocks().saturating_mul(BLOCK_SIZE as u64);
            status.errors = walk.errors();
            for (slot, bad) in status.bad.iter_mut().zip(walk.bad()) {
                *slot = BadBlock { block: bad.address,
                    kind: match bad.kind { scrub::Kind::Node => BAD_NODE, scrub::Kind::Data => BAD_DATA },
                    reserved: 0 };
                status.bad_count += 1;
            }
        }
        status
    }
    fn data_reader(&self) -> DataReader<'_, W> {
        DataReader {
            cache: self.inner.cache.buffers(),
            blocks: &self.inner.data,
        }
    }
    /// Change the open generation's pages of `ino` (E24 coordinator 601(2)).
    /// A page the change only partly covers starts from the frozen
    /// generation's copy when one is visible, else from the open generation's
    /// Files view; a frozen page is copied, not changed.
    fn edit_pages(
        &self,
        state: &mut MountState<W>,
        ino: u64,
        branch: &'static str,
        edit: impl FnOnce(&mut GenPages, &mut dyn FnMut(u64, &mut [u8]) -> Result<(), Errno>) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        state.nodes.entry(ino).or_default();
        let mut bad_metadata = None;
        let result = {
            let MountState { files, dirty, dirty_bytes, pending, installed, .. } = state;
            let open = dirty.entry(ino).or_default();
            let before = open.pages.bytes();
            let hidden = open.hide_frozen_from;
            let frozen = pending.as_ref().or(installed.as_ref()).and_then(|pages| pages.nodes.get(&ino));
            let mut read = |off: u64, page: &mut [u8]| -> Result<(), Errno> {
                let index = off / BLOCK_SIZE as u64;
                if let Some(old) = frozen.filter(|_| hidden.is_none_or(|from| index < from))
                    .and_then(|frozen| frozen.pages.get(index))
                {
                    page.copy_from_slice(old);
                    return Ok(());
                }
                files.read_verified(&self.data_reader(), ino, off, page).map_err(|error| {
                    match error {
                        ReadError::Metadata(error) => {
                            bad_metadata = Some(error);
                            errno(error)
                        }
                        ReadError::Device(error) => read_cause_errno(error.into()),
                        ReadError::Data => Errno(EIO),
                    }
                })?;
                Ok(())
            };
            let result = edit(open, &mut read);
            *dirty_bytes = *dirty_bytes - before + open.pages.bytes();
            result
        };
        if let Some(error) = bad_metadata {
            state.record_metadata(error);
            if latches_on_read(error) { state.fail(branch, Errno(EIO)); }
        }
        result
    }
    fn snapshot_request(&self, request: &SnapshotRequest, create: bool) -> Result<(), Errno> {
        let name = snapshot_name(request)?;
        let empty = request.flags == SNAPSHOT_EMPTY;
        if (!create && request.flags != 0)
            || (create && !matches!(request.flags, 0 | SNAPSHOT_READ_ONLY | SNAPSHOT_EMPTY))
        {
            return Err(Errno(EINVAL));
        }
        // Mutation permission precedes any fsync-class work on a read mount.
        self.foreground_lock()?.writable()?;
        if create {
            loop {
                self.sync()?;
                let mut state = self.foreground_lock()?;
                state.writable()?;
                // A writer may have won admission after the receipt. Close
                // that generation too: sharing a tree already private in G
                // would let subsequent writes change the snapshot's contents.
                // Snapshot the materialized view, not a scratch one (x11c M2).
                if !state.fresh || state.pending.is_some() || state.next_files.is_some() {
                    drop(state);
                    continue;
                }
                state.protect_edit()?;
                let grant = state.reserve_metadata(&[2], if empty { 2 } else { 1 }, false, false, Operation::Allocate)?;
                let result = if empty {
                    state.files.create_empty_root(name, now())
                } else {
                    state.files.snapshot(name, request.flags & SNAPSHOT_READ_ONLY != 0)
                };
                let id = state.finish_metadata(&[], grant, result)?;
                state.roots.insert(id, (name.to_vec(), request.flags & SNAPSHOT_READ_ONLY));
                drop(state);
                return self.sync_generation(None, false);
            }
        }
        let _selection = self.inner.mounted_root.device.selection.lock();
        {
            let mut state = self.foreground_lock()?;
            state.writable()?;
            let id = state.roots.iter().find(|(_, (root, _))| root == name)
                .map(|(&id, _)| id).ok_or(Errno(ENOENT))?;
            self.check_root_unmounted(id)?;
            state.protect_edit()?;
            let grant = state.reserve_metadata(&[2], 1, false, false, Operation::DeleteSnapshot)?;
            let result = state.files.delete_root(name);
            state.finish_metadata(&[], grant, result)?;
            state.roots.remove(&id);
            state.skip_reap = true;
        }
        self.sync_generation(None, false)
    }
    fn snapshot_rename(&self, request: &SnapshotRename) -> Result<(), Errno> {
        if request.from.flags != 0 || request.to.flags != 0 {
            return Err(Errno(EINVAL));
        }
        let from = snapshot_name(&request.from)?;
        let to = snapshot_name(&request.to)?;
        let _selection = self.inner.mounted_root.device.selection.lock();
        {
            let mut state = self.foreground_lock()?;
            state.writable()?;
            let (&id, (_, flags)) = state.roots.iter().find(|(_, (name, _))| name == from)
                .ok_or(Errno(ENOENT))?;
            let flags = *flags;
            self.check_root_unmounted(id)?;
            state.protect_edit()?;
            let grant = state.reserve_metadata(&[2], 1, false, false, Operation::Allocate)?;
            let result = state.files.rename_root(from, to);
            state.finish_metadata(&[], grant, result)?;
            state.roots.insert(id, (to.to_vec(), flags));
        }
        self.sync_generation(None, false)
    }

    fn snapshot_next(&self, cursor: u64) -> Result<SnapshotNext, Errno> {
        let state = self.foreground_lock()?;
        let mut next = SnapshotNext {
            id: 0,
            root: SnapshotRequest { flags: 0, name_len: 0, name: [0; 256] },
        };
        if let Some((&id, (name, flags))) = state.roots.range(
            (core::ops::Bound::Excluded(cursor), core::ops::Bound::Unbounded),
        ).next() {
            next.id = id;
            next.root.flags = *flags;
            next.root.name_len = name.len() as u32;
            next.root.name[..name.len()].copy_from_slice(name);
        }
        Ok(next)
    }

    fn snapshot_status(&self) -> Result<SnapshotStatus, Errno> {
        // Read the current durable SPACE tree even on a frozen reader mounted
        // before the writer's latest snapshot or reaper generation.
        let publication = self.inner.mounted_root.device.publication.lock();
        let state = self.foreground_lock()?;
        let sb = publication.as_ref().unwrap_or(&state.durable);
        let store = self.inner.cache.store(self.inner.device.clone(), sb);
        let items = recover::space_items(&store, sb).map_err(|_| Errno(EIO))?;
        Ok(SnapshotStatus {
            root_id: self.inner.root_id,
            flags: if self.inner.frozen { SNAPSHOT_READ_ONLY } else { 0 },
            roots: items.iter().filter(|(key, _)| key.item_type == format::ROOT).count()
                .try_into().map_err(|_| Errno(EOVERFLOW))?,
            dead_remaining: items.iter().filter(|(key, _)| key.item_type == format::DEAD).count() as u64,
        })
    }
}
fn snapshot_name(request: &SnapshotRequest) -> Result<&[u8], Errno> {
    let length = request.name_len as usize;
    if !(1..=255).contains(&length) {
        return Err(Errno(EINVAL));
    }
    let name = &request.name[..length];
    if name == b"." || name == b".." || name.contains(&0) || name.contains(&b'/') || name.contains(&b',') {
        return Err(Errno(EINVAL));
    }
    Ok(name)
}

impl<W: Wait + 'static> FileSystem for BreenFs<W> {
    fn ioctl(&self, node: &VNode, request: u64, arg: u64) -> Result<u64, Errno> {
        self.check_node(node)?;
        match request {
            #[cfg(any(feature = "breenfs_commit_window", feature = "breenfs_corrupt_inject"))]
            super::commit::faults::abi::ARM_PUBLICATION => {
                let faults = self.inner.commit_faults.as_ref().ok_or(Errno(ENOTTY))?;
                return Ok(faults.arm(node.ino, arg));
            }
            SNAPSHOT_CREATE | SNAPSHOT_DELETE => {
                let value = crate::syscall::userptr::copy_from_user(arg as *const SnapshotRequest)
                    .map_err(|error| Errno(error as i32))?;
                self.snapshot_request(&value, request == SNAPSHOT_CREATE)?;
            }
            SNAPSHOT_RENAME => {
                let value = crate::syscall::userptr::copy_from_user(arg as *const SnapshotRename)
                    .map_err(|error| Errno(error as i32))?;
                self.snapshot_rename(&value)?;
            }
            SNAPSHOT_NEXT => {
                let value = crate::syscall::userptr::copy_from_user(arg as *const SnapshotNext)
                    .map_err(|error| Errno(error as i32))?;
                crate::syscall::userptr::copy_to_user(arg as *mut SnapshotNext, &self.snapshot_next(value.id)?)
                    .map_err(|error| Errno(error as i32))?;
            }
            SNAPSHOT_STATUS => {
                crate::syscall::userptr::copy_to_user(arg as *mut SnapshotStatus, &self.snapshot_status()?)
                    .map_err(|error| Errno(error as i32))?;
            }
            SCRUB_START => self.start_scrub()?,
            SCRUB_CANCEL => {
                self.inner.scrub_signal.stop.store(true, Ordering::Release);
                self.inner.scrub_signal.wake_waiters();
            }
            SCRUB_STATUS => {
                crate::syscall::userptr::copy_to_user(arg as *mut ScrubStatus, &self.scrub_status())
                    .map_err(|error| Errno(error as i32))?;
            }
            _ => return Err(Errno(ENOTTY)),
        }
        Ok(0)
    }
    fn kind(&self) -> Result<&'static str, Errno> {
        Ok("breenfs")
    }
    fn root(&self) -> Result<u64, Errno> {
        Ok(2)
    }
    fn statfs(&self) -> Result<StatFs, Errno> {
        let state = self.foreground_lock()?;
        let sb = &state.durable;
        Ok(StatFs {
            magic: u64::from_le_bytes(*b"BREENFSS"),
            block_size: BLOCK_SIZE as u64,
            blocks: sb.total_blocks,
            free_blocks: sb.free_blocks,
            available_blocks: state.reservations.available(sb.free_blocks),
            inodes: 0,
            free_inodes: 0,
            max_name_len: 255,
            flags: MountFlags {
                read_only: state.read_only(),
                ..MountFlags::default()
            },
        })
    }
    fn sync(&self) -> Result<(), Errno> {
        self.sync_mappings(None)?;
        self.sync_generation(None, false)
    }
    fn check_live(&self) -> Result<(), Errno> {
        self.check_durable_roots(None)
    }
    fn init_service(&self) -> Result<(), Errno> {
        {
            let mut state = self.inner.state.lock();
            state.closed = false;
            state.reap_shutdown = false;
            if state.failed || state.committer.as_ref().is_none_or(Committer::read_only) {
                return Ok(());
            }
            if self.inner.signal.started.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            self.inner.signal.stop.store(false, Ordering::Release);
            state.closed = false;
            // A refused unmount can restart a stopped service with bindings.
            for entry in state.nodes.values_mut() {
                if let Some(node) = entry.node.upgrade().filter(|node| entry.links != 0 && node.map.nonempty()) {
                    entry.mapped = Some(node);
                }
            }
        }
        #[cfg(not(test))]
        {
            let mount = Arc::downgrade(&self.inner);
            let signal = self.inner.signal.clone();
            if crate::task::kthread::kthread_run(
                move || {
                    loop {
                        let observed = signal.worker.epoch();
                        if signal.stop.load(Ordering::Acquire) {
                            break;
                        }
                        let Some(inner) = mount.upgrade() else {
                            break;
                        };
                        let fs = Self { inner };
                        #[cfg(target_arch = "x86_64")]
                        crate::per_cpu::preempt_disable();
                        #[cfg(target_arch = "aarch64")]
                        crate::per_cpu_aarch64::preempt_disable();
                        let result = fs.commit_due(monotonic_ns());
                        let (failed, deadline) = {
                            let state = fs.inner.state.lock();
                            // A past transaction deadline cannot make progress
                            // while publication is blocked. Wait for its wake,
                            // with the ordinary interval as a retry bound.
                            (state.failed, if state.failed || state.publication_blocked() {
                                monotonic_ns().saturating_add(COMMIT_INTERVAL_NS)
                            } else { state.policy.deadline() })
                        };
                        drop(fs);
                        #[cfg(target_arch = "x86_64")]
                        crate::per_cpu::preempt_enable();
                        #[cfg(target_arch = "aarch64")]
                        crate::per_cpu_aarch64::preempt_enable();
                        if result.is_err() && !failed {
                            break;
                        }
                        if signal.worker.wait(observed, Some(deadline)).is_err() {
                            break;
                        }
                    }
                    signal.started.store(false, Ordering::Release);
                    signal.wake_waiters();
                },
                "breenfs-commit",
            )
            .is_err()
            {
                self.inner.signal.started.store(false, Ordering::Release);
                return Err(Errno(ENOMEM));
            }
        }
        Ok(())
    }
    fn shutdown(&self, name: &str, deadline: u64) -> Result<(), Errno> {
        // Once shutdown changes admission it must finish its barrier even
        // if SIGKILL arrives, as with the other lock-owning kernel sections.
        #[cfg(not(test))]
        let _custody = crate::task::thread::KillCustody::try_enter().ok_or(Errno(EINTR))?;
        // Mapping writeback can request commits before admission closes below.
        // Those shutdown generations must not perform background reclamation.
        self.inner.state.lock().reap_shutdown = true;
        // Writeback uses mutation admission, so stage before closing it.
        // Failed mounts still take M1's explicit-loss teardown path below.
        let mapped = self.sync_mappings(Some(deadline));
        if mapped == Err(Errno(ENOMEM)) || mapped == Err(Errno(ENOSPC)) {
            self.inner.state.lock().reap_shutdown = false;
            // No storage failure occurred. Preserve admission, service and
            // dirty pins so the caller can retry teardown without data loss.
            return mapped;
        }
        // Close mutation admission before requesting the final generation.
        // The worker remains live until that generation's receipt completes.
        let signal = &self.inner.signal;
        {
            let mut state = self.inner.state.lock();
            if state.closed {
                if signal.stop.load(Ordering::Acquire) && !signal.started.load(Ordering::Acquire)
                    && !self.inner.scrub_signal.started.load(Ordering::Acquire) {
                    let result = if state.failed {
                        Err(Errno(EIO))
                    } else {
                        Ok(())
                    };
                    for entry in state.nodes.values_mut() {
                        entry.mapped = None;
                    }
                    drop(state);
                    return teardown_result(name, result);
                }
                return Err(Errno(EBUSY));
            }
            state.closed = true;
        }
        self.stop_scrub(deadline)?;
        let result = mapped.and_then(|()| self.sync_generation(Some(deadline), true));
        if let Err(error) = result {
            self.inner.state.lock().fail("shutdown-sync", error);
        }
        signal.stop.store(true, Ordering::Release);
        signal.worker.wake();
        #[cfg(test)]
        signal.started.store(false, Ordering::Release);
        while signal.started.load(Ordering::Acquire) {
            let observed = signal.epoch();
            if monotonic_ns() >= deadline {
                return Err(Errno(ETIMEDOUT));
            }
            if signal.started.load(Ordering::Acquire) {
                signal.wait(observed, Some(deadline))?;
            }
        }
        // Drop service-owned VNodes before VFS checks external mount custody.
        // A live binding still owns its VNode; init_service re-pins it if the
        // caller refuses unmount and resumes this mount.
        for entry in self.inner.state.lock().nodes.values_mut() {
            entry.mapped = None;
        }
        teardown_result(name, result)
    }
    fn report(
        &self,
        report: Report,
        name: &str,
        text: &mut alloc::string::String,
    ) -> Result<(), Errno> {
        if matches!(report, Report::State) {
            use core::fmt::Write;
            let state = self.inner.state.lock();
            let pending_bytes = state.frozen_pages().map_or(0, |p| p.bytes);
            // These counts describe the installed SPACE tree (E24 coordinator
            // 567). Reporting must also work if metadata reads are failing.
            let pending = state.durable_pending.and_then(PendingSummary::census);
            let pending_blocks: &dyn core::fmt::Display = match &pending {
                Some((blocks, _)) => blocks,
                None => &"unavailable",
            };
            writeln!(
                text,
                "{} breenfs durable_generation={} dirty_bytes={} read_only={} last_scrub_generation={} last_scrub_errors={} free_blocks={} pending_blocks={}",
                name,
                state.durable.generation,
                state.dirty_bytes + pending_bytes,
                u8::from(state.read_only()),
                state.durable.last_scrub_generation,
                state.durable.last_scrub_errors,
                state.durable.free_blocks,
                pending_blocks
            )
            .map_err(|_| Errno(ENOMEM))?;
            if let Some(failure) = state.metadata_error {
                writeln!(text, "metadata_cause={} metadata_node={}", failure.cause, failure.addr)
                    .map_err(|_| Errno(ENOMEM))?;
            }
            drop(state);
            self.inner.costs.report(self.inner.id, self.inner.root_id, text).map_err(|_| Errno(ENOMEM))?;
        }
        Ok(())
    }
    #[cfg(not(test))]
    fn report_mapping_cache(&self, text: &mut alloc::string::String) -> Result<(), Errno> {
        use core::fmt::Write;

        const LIMIT: usize = 16;
        struct Row {
            ino: u64,
            pages: usize,
            cache: crate::memory::file_map::CacheRetention,
            pins: usize,
            links: u32,
            service_pin: bool,
            frozen: u64,
            installed: u64,
            stale: u64,
            open: u64,
        }
        let mut largest: [Option<(usize, Arc<VNode>)>; LIMIT] = [const { None }; LIMIT];
        let mut rows = [const { None }; LIMIT];
        let (durable, open, worker, queued) = {
            let state = self.inner.state.lock();
            for entry in state.nodes.values() {
                preemption_point();
                let Some(node) = entry.node.upgrade() else { continue };
                let pages = node.map.cache_pages();
                if pages == 0 { continue; }
                let slot = largest.partition_point(|row| row.as_ref().is_some_and(|(held, _)| *held >= pages));
                if slot < LIMIT {
                    largest[slot..].rotate_right(1);
                    largest[slot] = Some((pages, node));
                }
            }
            // The layer counts below describe the private copies of mapped data.
            let layer_pages = |layer: Option<&Arc<FrozenPages>>, ino| {
                layer.and_then(|layer| layer.nodes.get(&ino))
                    .map_or(0, |node| node.pages.bytes() / BLOCK_SIZE as u64)
            };
            for (row, slot) in rows.iter_mut().zip(largest.iter()) {
                preemption_point();
                let Some((pages, node)) = slot else { break };
                let ino = node.ino;
                let entry = &state.nodes[&ino];
                *row = Some(Row {
                    ino,
                    pages: *pages,
                    cache: node.map.cache_retention(),
                    pins: Arc::strong_count(node) - 1, // Exclude this report.
                    links: entry.links,
                    service_pin: entry.mapped.is_some(),
                    frozen: layer_pages(state.pending.as_ref(), ino),
                    installed: layer_pages(state.installed.as_ref(), ino),
                    stale: layer_pages(state.stale_views.as_ref().and_then(|(_, pages)| pages.as_ref()), ino),
                    open: state.dirty.get(&ino).map_or(0, |node| node.pages.bytes() / BLOCK_SIZE as u64),
                });
            }
            (
                state.durable.generation, state.open_generation(),
                u8::from(self.inner.signal.started.load(Ordering::Acquire)),
                u8::from(self.inner.signal.mappings.load(Ordering::Acquire)),
            )
        };
        drop(largest);
        let inodes: [_; LIMIT] = core::array::from_fn(|slot| rows[slot].as_ref().map(|row| row.ino));
        let descriptors = self.mapping_descriptor_counts(&inodes);
        let passes = self.inner.mapping_passes.load(Ordering::Relaxed);
        let evicted = self.inner.mapping_pages_evicted.load(Ordering::Relaxed);
        let dropped = self.inner.mapping_requests_dropped.load(Ordering::Relaxed);
        writeln!(text, "durable={durable} open={open} worker={worker} queued={queued} view_frame_refs=0 worker_passes={passes} pages_evicted={evicted} requests_dropped_with_pages_left={dropped}")
            .map_err(|_| Errno(ENOMEM))?;
        for (slot, row) in rows.into_iter().enumerate() {
            let Some(Row { ino, pages, cache, pins, links, service_pin, frozen, installed, stale, open }) = row else { break };
            let service_pin = usize::from(service_pin);
            writeln!(text, "ino={ino} pages={pages} bindings={} dirty={} staged={} staged_first={} staged_last={} pins={pins} service_pin={service_pin} frozen_pages={frozen} installed_pages={installed} stale_pages={stale} open_pages={open} nlink={links} open_fds={} binding_pins={} descriptor_pins={} other_pins={}",
                cache.bindings, cache.dirty, cache.staged, cache.first_generation, cache.last_generation,
                ReportCount(descriptors.map(|counts| counts[slot].0)), cache.bindings,
                ReportCount(descriptors.map(|counts| counts[slot].1)),
                ReportCount(descriptors.map(|counts| pins.saturating_sub(service_pin + cache.bindings + counts[slot].1)))).map_err(|_| Errno(ENOMEM))?;
        }
        Ok(())
    }
    fn mutation_costs(&self, name: &str, text: &mut alloc::string::String) -> Result<(), Errno> {
        use core::fmt::Write;
        writeln!(text, "{name} breenfs mutation_costs=1 write_bytes={}", self.inner.costs.write_bytes()).map_err(|_| Errno(ENOMEM))?;
        self.inner.costs.report(self.inner.id, self.inner.root_id, text).map_err(|_| Errno(ENOMEM))
    }
    fn failure(&self) -> Option<(&'static str, i32)> {
        self.inner.state.lock().failure
    }
    fn read_only(&self) -> Result<bool, Errno> {
        // Mount status describes the current latch without advancing replay.
        Ok(self.inner.state.lock_interruptible()?.read_only())
    }
    fn max_file_size(&self) -> Result<u64, Errno> {
        Ok((1 << 63) - 4096)
    }
    fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno> {
        self.foreground_lock()?.attrs(ino)
    }
    fn open(&self, ino: u64) -> Result<Arc<VNode>, Errno> {
        let mut state = self.foreground_lock()?;
        self.open_locked(&mut state, ino)
    }
    fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Errno> {
        let mut state = self.foreground_lock()?;
        let result = state.files.lookup(dir, name);
        state.observed(result)
    }
    fn lookup_open(&self, dir: &VNode, name: &[u8]) -> Result<Arc<VNode>, Errno> {
        self.check_node(dir)?;
        let mut state = self.foreground_lock()?;
        let result = state.files.lookup(dir.ino, name);
        let ino = state.observed(result)?;
        self.open_locked(&mut state, ino)
    }
    fn touch(&self, node: &VNode, read: bool) {
        if !self.inner.writable || self.check_node(node).is_err() {
            return;
        }
        let Some(pin) = node.pin::<NodePin<W>>() else {
            return;
        };
        // FIFO callers can hold PM and stream locks: record only atomics here.
        #[cfg(not(test))]
        let seconds = crate::time::current_unix_time_coarse() as u64;
        #[cfg(test)]
        let seconds = crate::time::current_unix_time();
        let mut generation = self.inner.generation.load(Ordering::SeqCst);
        #[cfg(test)]
        super::preempt::context::observe(super::preempt::context::Event::Touch);
        // A freeze between that load and the record can let N's preparation
        // take N's slot before this stamp lands (x11c M4). Re-read after each
        // record: if a freeze moved the generation, record into the open
        // generation's slot too. The freeze's store, these loads and the slot
        // store and take are SeqCst, so either the take saw this stamp or the
        // re-read sees the new generation. A stamp recorded in two slots
        // carries the same time each time it is applied.
        loop {
            pin.touched.record(generation, read, seconds.saturating_add(1));
            let current = self.inner.generation.load(Ordering::SeqCst);
            if current == generation { break; }
            generation = current;
        }
        self.inner.events.push(&pin.touched);
    }
    fn parent(&self, dir: u64) -> Result<u64, Errno> {
        let mut state = self.foreground_lock()?;
        state.directory(dir)?;
        Ok(state.inode(dir)?.parent)
    }
    fn readdir(
        &self,
        dir: u64,
        cookie: u64,
        sink: &mut dyn FnMut(DirEnt, u64) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        let mut state = self.foreground_lock()?;
        let result = read_directory(&state.files, dir, cookie, sink);
        state.observed(result)?
    }

    fn read(&self, node: &VNode, off: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        self.check_node(node)?;
        let publication = (!self.inner.writable)
            .then(|| self.inner.mounted_root.device.publication.lock());
        let mut state = self.foreground_lock()?;
        if let Some(sb) = publication.as_deref().and_then(Option::as_ref)
            .filter(|sb| sb.generation > state.durable.generation)
        {
            let store = self.inner.cache.store(self.inner.device.clone(), sb);
            let result = Files::new(store, sb, self.inner.root_id, false);
            let files = state.observed(result)?;
            state.files.files = state.edited(&[], Ok(files))?;
            state.durable = sb.clone();
        }
        buf.fill(0);
        let size = state.regular(node.ino)?.size;
        if off >= size || buf.is_empty() {
            return Ok(0);
        }
        let length = (size - off).min(buf.len() as u64) as usize;
        let mut done = 0;
        while done < length {
            let pos = off + done as u64;
            let index = pos / BLOCK_SIZE as u64;
            let within = (pos % BLOCK_SIZE as u64) as usize;
            let take = (length - done).min(BLOCK_SIZE - within);
            // Newest first: the open generation's page, then the frozen
            // generation's, then the open Files view (E24 coordinator 597).
            if let Some(page) = state.page(node.ino, index) {
                buf[done..done + take].copy_from_slice(&page[within..within + take]);
                done += take;
            } else {
                // Keep a clean run together so Files resolves each extent's
                // REF once, even when a read spans several blocks.
                let mut end = done + take;
                while end < length {
                    let next = off + end as u64;
                    if state.page(node.ino, next / BLOCK_SIZE as u64).is_some() { break; }
                    end += (length - end).min(BLOCK_SIZE);
                }
                let result = state.files.read_verified(
                    &self.data_reader(), node.ino, pos,
                    &mut buf[done..end],
                );
                state.read_result(result)?;
                done = end;
            }
        }
        node.map.overlay(off, &mut buf[..length]).map_err(|_| Errno(EIO))?;
        self.touch(node, true);
        Ok(length)
    }
    fn readlink(&self, ino: u64) -> Result<Vec<u8>, Errno> {
        let mut state = self.foreground_lock()?;
        state.inode(ino)?;
        let result = state.files.readlink(ino);
        state.observed(result)
    }
    fn fsync(&self, node: &VNode, _: bool) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Fsync, CostPhase::Operation);
        self.check_node(node)?;
        if self.mutation_admission_lock(CostOp::Fsync)?.failed {
            return Err(Errno(EIO));
        }
        if node.map.has_dirty() {
            return crate::memory::file_map::sync_range(node, 0, u64::MAX);
        }
        self.sync_generation(None, false)
    }
    fn request_eviction(&self) {
        self.inner.signal.mappings.store(true, Ordering::Release);
        self.inner.signal.worker.wake();
    }
    fn mapping_io(&self, node: &VNode) -> Result<Box<dyn MappingIo>, Errno> {
        self.check_node(node)?;
        let mut state = self.foreground_lock()?;
        let size = state.regular(node.ino)?.size;
        let transfer = Mapping {
            fs: self.clone(),
            node: self.open_locked(&mut state, node.ino)?,
            size,
            // A failed writable mount must still flush through sync's EIO path.
            writable: state.committer.is_some(),
            staged_generation: Cell::new(None),
        };
        Ok(Box::try_new(transfer).map_err(|_| Errno(ENOMEM))?)
    }
    fn setattr(&self, ino: u64, attrs: SetAttr) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Setattr, CostPhase::Operation);
        let state = self.mutation_admission_lock(CostOp::Setattr)?;
        state.writable()?;
        let (mut state, grant) = self.reserve_with_pressure(CostOp::Setattr, state, || self.mutation_admission_lock(CostOp::Setattr), |state| {
            let current = state.attrs(ino)?;
            if attrs
                .expected
                .is_some_and(|expected| current.attrs != expected)
            {
                return Err(Errno(EAGAIN).into());
            }
            let grant = state.reserve_metadata(&[ino], 1, false, false, Operation::Allocate)?;

            Ok(grant)
        })?;
        let _edit = self.inner.costs.span(CostOp::Setattr, CostPhase::Edit);
        let result = state.files.setattr(
            ino,
            &ops::SetAttr {
                mode: attrs.mode.map(u32::from),
                uid: attrs.uid,
                gid: attrs.gid,
                atime: attrs.atime.map(time),
                mtime: attrs.mtime.map(time),
            },
            now(),
        );
        state.finish_metadata(&[ino], grant, result)
    }
    fn create(&self, dir: u64, name: &[u8], node: NewNode<'_>) -> Result<u64, Errno> {
        let _operation = self.inner.costs.span(CostOp::Create, CostPhase::Operation);
        let state = self.mutation_admission_lock(CostOp::Create)?;
        state.writable()?;
        let (attrs, bits, rdev, target) = match node {
            NewNode::Regular { attrs } => (attrs, 0o100000, 0, None),
            NewNode::Directory { attrs } => (attrs, 0o040000, 0, None),
            NewNode::Symlink { attrs, target } => (attrs, 0o120000, 0, Some(target)),
            NewNode::CharDevice { attrs, rdev } => (attrs, 0o020000, rdev, None),
            NewNode::BlockDevice { attrs, rdev } => (attrs, 0o060000, rdev, None),
            NewNode::Fifo { attrs } => (attrs, 0o010000, 0, None),
            NewNode::Socket { attrs } => (attrs, 0o140000, 0, None),
        };
        let (mut state, grant) = self.reserve_with_pressure(CostOp::Create, state, || self.mutation_admission_lock(CostOp::Create), |state| {
            state.reserve_create(dir, target.is_some())
        })?;
        let _edit = self.inner.costs.span(CostOp::Create, CostPhase::Edit);
        let result = state.files.create_node_prepared(
            dir,
            name,
            Inode {
                mode: bits | (attrs.mode & 0o7777),
                uid: attrs.uid,
                gid: attrs.gid,
                rdev,
                ..Inode::default()
            },
            target,
            now(),
            |_, _| Ok(()),
        );
        state.finish_metadata(&[dir], grant, result)
    }
    fn create_open(
        &self,
        dir: u64,
        name: &[u8],
        kind: NodeKind,
        check: &dyn Fn(NodeAttrs) -> Result<InodeAttrs, Errno>,
        prepare: &mut dyn FnMut(Arc<VNode>) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Create, CostPhase::Operation);
        let state = self.mutation_admission_lock(CostOp::Create)?;
        state.writable()?;
        let (mut state, (attrs, bits, grant)) = self.reserve_with_pressure(CostOp::Create, state, || self.mutation_admission_lock(CostOp::Create), |state| {
            match state.files.lookup(dir, name) {
                Ok(_) => return Err(Errno(EEXIST).into()),
                Err(ops::Error::ENOENT) => (),
                Err(e) => return state.observed(Err(e)).map_err(Into::into),
            }
            let attrs = check(state.directory(dir)?)?;
            let bits = match kind {
                NodeKind::Regular => 0o100000,
                NodeKind::Socket => 0o140000,
                _ => return Err(Errno(EOPNOTSUPP).into()),
            };
            // The create and its open are two replay records while N is frozen.
            state.files.reserve(2).map_err(errno)?;
            let grant = state.reserve_create(dir, false)?;
            Ok((attrs, bits, grant))
        })?;
        let mut prepared = None;
        let mut preparation_error = None;
        let epoch = state.durable.generation;
        let _edit = self.inner.costs.span(CostOp::Create, CostPhase::Edit);
        let result = state.files.create_node_prepared(
            dir,
            name,
            Inode {
                mode: bits | (attrs.mode & 0o7777),
                uid: attrs.uid,
                gid: attrs.gid,
                ..Inode::default()
            },
            None,
            now(),
            |ino, inode| {
                let result = self.new_node(ino, inode, epoch).and_then(|node| {
                    prepare(node.clone())?;
                    prepared = Some((node, inode.nlink));
                    Ok(())
                });
                result.map_err(|e| {
                    preparation_error = Some(e);
                    ops::Error::EIO
                })
            },
        );
        if let Some(e) = preparation_error {
            state.reservations.release(grant.grant);
            return Err(e);
        }
        let ino = state.finish_metadata(&[dir], grant, result)?;
        let (node, links) = prepared.ok_or(Errno(EIO))?;
        let entry = state.nodes.entry(ino).or_default();
        entry.node = Arc::downgrade(&node);
        entry.links = links;
        entry.touched = Some(node.pin::<NodePin<W>>().ok_or(Errno(EIO))?.touched.clone());
        state.files.set_inode_open(ino, true).map_err(errno)?;
        // A refused share stays queued for the next holder.
        let _ = state.retain_open();
        Ok(())
    }
    fn link(&self, ino: u64, dir: u64, name: &[u8], check: &LinkCheck<'_>) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Link, CostPhase::Operation);
        let state = self.mutation_admission_lock(CostOp::Link)?;
        state.writable()?;
        if state.files.tag(ino) != state.files.tag(dir) {
            return Err(Errno(EXDEV));
        }
        let (mut state, grant) = self.reserve_with_pressure(CostOp::Link, state, || self.mutation_admission_lock(CostOp::Link), |state| {
            let parent = state.directory(dir)?;
            let source = state.attrs(ino)?;
            if source.kind == NodeKind::Directory {
                return Err(Errno(EPERM).into());
            }
            check(parent, source)?;
            let grant = state.reserve_metadata(&[ino, dir], 4, false, false, Operation::Allocate)?;

            Ok(grant)
        })?;
        let _edit = self.inner.costs.span(CostOp::Link, CostPhase::Edit);
        let result = state.files.link(ino, dir, name, now());
        state.finish_metadata(&[ino, dir], grant, result)
    }
    fn remove(&self, dir: u64, name: &[u8], which: Remove) -> Result<(), Errno> {
        let cost = if which.directory { CostOp::Rmdir } else { CostOp::Unlink };
        let _operation = self.inner.costs.span(cost, CostPhase::Operation);
        let state = self.mutation_admission_lock(cost)?;
        state.writable()?;
        let (mut state, (grant, victim)) = self.reserve_with_pressure(cost, state, || self.mutation_admission_lock(cost), |state| {
            let parent = state.directory(dir)?;
            let result = state.files.lookup(dir, name);
            let victim = state.observed(result)?;
            let attrs = state.attrs(victim)?;
            if !which.directory && attrs.kind == NodeKind::Directory {
                return Err(Errno(EISDIR).into());
            }
            if which.expected != victim
                || which.owner != attrs.attrs.uid
                || which.parent != parent.attrs
            {
                return Err(Errno(EAGAIN).into());
            }
            // Match ext2's held-directory policy before testing emptiness.
            if which.directory
                && attrs.kind == NodeKind::Directory
                && state.nodes.get(&victim).is_some_and(|entry| entry.node.strong_count() != 0)
            {
                return Err(Errno(EBUSY).into());
            }
            let grant = state.reserve_metadata(
                &[dir, victim], 5, false, !which.directory,
                if which.directory { Operation::Rmdir } else { Operation::Unlink },
            )?;
            Ok((grant, victim))
        })?;
        let _edit = self.inner.costs.span(cost, CostPhase::Edit);
        let result = if which.directory {
            state.files.rmdir(dir, name, now())
        } else {
            state.files.unlink(dir, name, now())
        };
        state.finish_metadata(&[dir, victim], grant, result)?;
        let orphan = Self::take_orphan_cache_pin(&mut state, victim);
        drop(state);
        drop(orphan);
        Ok(())
    }
    fn rename(
        &self,
        from_dir: u64,
        from: &[u8],
        to_dir: u64,
        to: &[u8],
        mode: RenameMode,
        check: &RenameCheck<'_>,
    ) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Rename, CostPhase::Operation);
        let state = self.mutation_admission_lock(CostOp::Rename)?;
        state.writable()?;
        if state.files.tag(from_dir) != state.files.tag(to_dir) {
            return Err(Errno(EXDEV));
        }
        let (mut state, (grant, objects, replaced)) = self.reserve_with_pressure(CostOp::Rename, state, || self.mutation_admission_lock(CostOp::Rename), |state| {
            if mode == RenameMode::Exchange {
                return Err(Errno(EINVAL).into());
            }
            let parent = state.directory(from_dir)?;
            let dest_parent = state.directory(to_dir)?;
            let result = state.files.lookup(from_dir, from);
            let source = state.observed(result)?;
            let source = state.attrs(source)?;
            let dest = match state.files.lookup(to_dir, to) {
                Ok(ino) => Some(state.attrs(ino)?),
                Err(ops::Error::ENOENT) => None,
                Err(e) => return state.observed(Err(e)).map_err(Into::into),
            };
            check(parent, dest_parent, source, dest)?;
            if mode == RenameMode::NoReplace && dest.is_some() {
                return Err(Errno(EEXIST).into());
            }
            let mut objects = alloc::vec![from_dir, to_dir, source.ino];
            if let Some(dest) = dest { objects.push(dest.ino); }
            let grant = state.reserve_metadata(&objects, 12, false, dest.is_some(), Operation::Allocate)?;

            Ok((grant, objects, dest.map(|dest| dest.ino)))
        })?;
        let _edit = self.inner.costs.span(CostOp::Rename, CostPhase::Edit);
        let result = state.files.rename(from_dir, from, to_dir, to, now());
        state.finish_metadata(&objects, grant, result)?;
        let orphan = replaced.and_then(|ino| Self::take_orphan_cache_pin(&mut state, ino));
        drop(state);
        drop(orphan);
        Ok(())
    }
    fn write(&self, node: &VNode, req: WriteReq, bytes: &[u8]) -> Result<WriteResult, WriteError> {
        let _operation = self.inner.costs.span(CostOp::Write, CostPhase::Operation);
        self.check_node(node)?;
        if bytes.len() <= WRITE_PART {
            let state = self.admit_data(node.ino, touched_bytes(bytes.len()), Split::No, CostOp::Write)?;
            return self.write_part(CostOp::Write, state, node, req, bytes);
        }
        // N2: admit a larger call part by part so its dirty pages stay inside
        // the budget, holding the file's mark from the first part to the last.
        let first = self.admit_data(node.ino, touched_bytes(WRITE_PART), Split::Take, CostOp::Write)?;
        let result = self.write_parts(first, node, req, bytes);
        let mut state = self.mutation_lock(CostOp::Write);
        if let Some(entry) = state.nodes.get_mut(&node.ino) {
            entry.splitting = false;
        }
        self.inner.signal.wake_waiters();
        drop(state);
        result
    }
    fn resize(
        &self,
        node: &VNode,
        size: u64,
        clear_setid: bool,
        check: &dyn Fn(NodeAttrs) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Resize, CostPhase::Operation);
        self.check_node(node)?;
        // Shrinking can keep one zeroed partial tail page.
        let state = self.admit_data(node.ino, BLOCK_SIZE as u64, Split::No, CostOp::Resize)?;
        let (mut state, (mut inode, mapped_tail, grant)) = self.reserve_with_pressure(
            CostOp::Resize, state, || self.admit_data(node.ino, BLOCK_SIZE as u64, Split::No, CostOp::Resize), |state| {
                let inode = state.regular(node.ino)?;
                check(state.attrs(node.ino)?)?;
                if size > (1 << 63) - 4096 {
                    return Err(Errno(EFBIG).into());
                }
                if size == inode.size
                    && (!clear_setid || inode.mode & content_privilege(inode.mode) == 0)
                {
                    return Ok((inode, false, None));
                }
                let floor = size / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
                let mapped_tail = if size < inode.size && size % BLOCK_SIZE as u64 != 0 {
                    // A frozen generation's tail page is copied into this one.
                    let index = floor / BLOCK_SIZE as u64;
                    let frozen = state.dirty.get(&node.ino).is_none_or(|open| open.pages.get(index).is_none())
                        && state.page(node.ino, index).is_some();
                    let result = state.files.extents(node.ino);
                    frozen || state.observed(result)?.iter().any(|(k, e)| {
                        k.offset <= floor && floor < k.offset + u64::from(e.num_blocks) * BLOCK_SIZE as u64
                    })
                } else { false };
                // Shrinking records the trim and the inode; growing, the inode.
                state.files.reserve(if size < inode.size { 2 } else { 1 }).map_err(errno)?;
                let grant = state.reserve_data(
                    node.ino,
                    floor,
                    usize::from(mapped_tail),
                    if size < inode.size { Operation::Truncate } else { Operation::Allocate },
                    false,
                )?;
                Ok((inode, mapped_tail, Some(grant)))
            },
        )?;
        let _edit = self.inner.costs.span(CostOp::Resize, CostPhase::Edit);
        let Some(grant) = grant else { return Ok(()); };
        if node.map.prepare_resize(inode.size, size).is_err() {
            state.reservations.release(grant.grant);
            return Err(Errno(ENOMEM));
        }
        if size < inode.size {
            let result = self.edit_pages(&mut state, node.ino, "resize-read-metadata", |open, read| {
                open.pages.shrink(size, mapped_tail, read)?;
                // A frozen generation's pages past the new EOF are not this
                // generation's bytes, even if the file grows again (601(2)).
                // Hide them only once the shrink succeeded: a failed one
                // changes no visible page (x11c C1).
                let end = size.div_ceil(BLOCK_SIZE as u64);
                open.hide_frozen_from = Some(open.hide_frozen_from.map_or(end, |from| from.min(end)));
                Ok(())
            });
            state.finish_data(node.ino, grant, result)?;
            let result = state.files.trim_mappings(node.ino, size);
            inode.nblocks = state.edited(&[node.ino], result)?;
        } else {
            state.finish_data(node.ino, grant, Ok(()))?;
        }
        inode.size = size;
        inode.mtime = now();
        inode.ctime = inode.mtime;
        if clear_setid {
            inode.mode &= !content_privilege(inode.mode);
        }
        let result = state.files.put_inode(node.ino, inode);
        let result = state.edited(&[node.ino], result);
        node.map.transition(node.size.load(Ordering::Acquire));
        result
    }
    fn writeback_pages(
        &self,
        node: &VNode,
        page: u64,
        bytes: &[u8],
        io: &dyn MappingIo,
    ) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Writeback, CostPhase::Operation);
        self.check_node(node)?;
        let mapping = io.as_any().downcast_ref::<Mapping<W>>().ok_or(Errno(EIO))?;
        if !Arc::ptr_eq(&mapping.fs.inner, &self.inner)
            || mapping.node.ino != node.ino
            || mapping.node.incarnation != node.incarnation
        {
            return Err(Errno(EXDEV));
        }
        mapping.staged_generation.set(None);
        let offset = page.checked_mul(BLOCK_SIZE as u64).ok_or(Errno(EFBIG))?;
        // The caller can reuse its mapped-page snapshot after this call. Only
        // this private copy enters the dirty map and later CRC calculation.
        let mut bounce = Vec::new();
        bounce
            .try_reserve_exact(bytes.len())
            .map_err(|_| Errno(ENOMEM))?;
        bounce.extend_from_slice(bytes);
        let state = self.admit(touched_bytes(bounce.len()), CostOp::Writeback)?;
        let (mut state, (mut inode, length, grant)) = self.reserve_with_pressure(
            CostOp::Writeback, state, || self.admit(touched_bytes(bounce.len()), CostOp::Writeback), |state| {
                if !mapping.writable {
                    return Err(Errno(EROFS).into());
                }
                let inode = state.regular(node.ino)?;
                let length = inode.size.saturating_sub(offset).min(bounce.len() as u64) as usize;
                if length == 0 {
                    return Ok((inode, length, None));
                }
                state.files.reserve(1).map_err(errno)?;
                let grant = state.reserve_data(node.ino, offset, length, Operation::Allocate, false)?;
                Ok((inode, length, Some(grant)))
            },
        )?;
        let _edit = self.inner.costs.span(CostOp::Writeback, CostPhase::Edit);
        let Some(grant) = grant else { return Ok(()); };
        let result = self.edit_pages(&mut state, node.ino, "writeback-read-metadata",
            |open, read| open.pages.write(offset, &bounce[..length], read));
        state.finish_data(node.ino, grant, result)?;
        inode.mtime = now();
        inode.ctime = inode.mtime;
        let result = state.files.put_inode(node.ino, inode);
        state.edited(&[node.ino], result)?;
        mapping.staged_generation.set(Some(state.open_generation()));
        Ok(())
    }
    fn clear_mapping_privilege(&self, node: &VNode) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Setattr, CostPhase::Operation);
        self.check_node(node)?;
        let mut state = self.mutation_admission_lock(CostOp::Setattr)?;
        state.writable()?;
        let mut inode = state.regular(node.ino)?;
        if inode.mode & content_privilege(inode.mode) == 0 {
            return Ok(());
        }
        state.protect_edit()?;
        let _edit = self.inner.costs.span(CostOp::Setattr, CostPhase::Edit);
        inode.mode &= !content_privilege(inode.mode);
        inode.ctime = now();
        let result = state.files.put_inode(node.ino, inode);
        state.edited(&[node.ino], result)
    }
    fn xattr(&self, ino: u64, op: Xattr<'_>) -> Result<usize, Errno> {
        let mutation = matches!(op, Xattr::Set { .. } | Xattr::Remove { .. });
        let _operation = mutation.then(|| self.inner.costs.span(CostOp::Xattr, CostPhase::Operation));
        let mut state = if mutation { self.mutation_admission_lock(CostOp::Xattr)? } else { self.foreground_lock()? };
        if state.committer.is_none() && matches!(op, Xattr::Set { .. } | Xattr::Remove { .. }) {
            return Err(Errno(EROFS));
        }
        state.inode(ino)?;
        let result = state.files.list_xattrs(ino);
        let records = state.observed(result)?;
        match op {
            Xattr::Get { name, value } => {
                let record = records
                    .iter()
                    .find(|x| x.name == name)
                    .ok_or(Errno(ENODATA))?;
                copy_xattr(value, &record.value)
            }
            Xattr::List { names } => {
                let mut bytes = Vec::new();
                for record in records {
                    bytes.extend_from_slice(&record.name);
                    bytes.push(0);
                }
                copy_xattr(names, &bytes)
            }
            Xattr::Set { name, value, mode } => {
                let (mut state, grant) = self.reserve_with_pressure(CostOp::Xattr, state, || self.mutation_admission_lock(CostOp::Xattr), |state| {
                    state.writable()?;
                    let result = state.files.list_xattrs(ino);
                    let records = state.observed(result)?;
                    let exists = records.iter().any(|x| x.name == name);
                    match mode {
                        XattrSet::Create if exists => return Err(Errno(EEXIST).into()),
                        XattrSet::Replace if !exists => return Err(Errno(ENODATA).into()),
                        _ => (),
                    }
                    let grant = state.reserve_metadata(&[ino], 2, false, false, Operation::Allocate)?;
                    Ok(grant)
                })?;
                let _edit = self.inner.costs.span(CostOp::Xattr, CostPhase::Edit);
                let result = state.files.xattr(ino, name, Some(value), now());
                state.finish_metadata(&[ino], grant, result).map(|_| 0)
            }
            Xattr::Remove { name } => {
                let (mut state, grant) = self.reserve_with_pressure(CostOp::Xattr, state, || self.mutation_admission_lock(CostOp::Xattr), |state| {
                    state.writable()?;
                    let result = state.files.list_xattrs(ino);
                    let records = state.observed(result)?;
                    if !records.iter().any(|x| x.name == name) {
                        return Err(Errno(ENODATA).into());
                    }
                    let grant = state.reserve_metadata(&[ino], 2, false, false, Operation::RemoveXattr)?;
                    Ok(grant)
                })?;
                let _edit = self.inner.costs.span(CostOp::Xattr, CostPhase::Edit);
                let result = state.files.xattr(ino, name, None, now());
                state.finish_metadata(&[ino], grant, result).map(|_| 0)
            }
        }
    }
}
struct GenerationRoot {
    mount: usize,
    mounted: MountedRoot,
    closed_inodes: Arc<BTreeSet<u64>>,
}

/// Different root and VNode identity, with the same on-disk owner and Files
/// transaction as the writable default root.
struct GenerationView<W: Wait + 'static> {
    writer: BreenFs<W>,
    root: Arc<GenerationRoot>,
}

impl<W: Wait + 'static> GenerationView<W> {
    fn read_files<T>(
        &self,
        read: impl FnOnce(&Files<'static, CachedNodeStore<'static, W>>) -> Result<T, ReadError>,
    ) -> Result<T, Errno> {
        let mut state = self.writer.foreground_lock()?;
        let result = state.files.files.with_read_root(self.root.mounted.id, &self.root.closed_inodes, read);
        let result = state.observed(result)?;
        state.read_result(result)
    }

    fn check_node(&self, node: &VNode) -> Result<(), Errno> {
        if node.pin::<GenerationRoot>().is_some_and(|pin| core::ptr::eq(pin, self.root.as_ref())) {
            Ok(())
        } else {
            Err(Errno(EXDEV))
        }
    }
}

impl<W: Wait + 'static> FileSystem for GenerationView<W> {
    fn kind(&self) -> Result<&'static str, Errno> {
        self.writer.kind()
    }
    fn root(&self) -> Result<u64, Errno> {
        self.writer.root()
    }
    fn statfs(&self) -> Result<StatFs, Errno> {
        let mut stats = self.writer.statfs()?;
        stats.flags.read_only = true;
        Ok(stats)
    }
    fn sync(&self) -> Result<(), Errno> {
        self.writer.sync()
    }
    fn check_live(&self) -> Result<(), Errno> {
        self.writer.check_durable_roots(Some(self.root.mounted.id))
    }
    fn init_service(&self) -> Result<(), Errno> {
        self.writer.init_service()
    }
    fn shutdown(&self, name: &str, deadline: u64) -> Result<(), Errno> {
        self.writer.shutdown(name, deadline)
    }
    fn report(&self, report: Report, name: &str, text: &mut alloc::string::String) -> Result<(), Errno> {
        self.writer.report(report, name, text)
    }
    fn report_mapping_cache(&self, text: &mut alloc::string::String) -> Result<(), Errno> {
        self.writer.report_mapping_cache(text)
    }
    fn mutation_costs(&self, name: &str, text: &mut alloc::string::String) -> Result<(), Errno> {
        self.writer.mutation_costs(name, text)
    }
    fn failure(&self) -> Option<(&'static str, i32)> {
        self.writer.failure()
    }
    fn read_only(&self) -> Result<bool, Errno> {
        Ok(true)
    }
    fn max_file_size(&self) -> Result<u64, Errno> {
        self.writer.max_file_size()
    }
    fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno> {
        let inode = self.read_files(|files| Ok(files.inode(ino)?))?;
        attributes(ino, inode)
    }
    fn open(&self, ino: u64) -> Result<Arc<VNode>, Errno> {
        let inode = self.read_files(|files| Ok(files.inode(ino)?))?;
        let fs = Arc::try_new(Self { writer: self.writer.clone(), root: self.root.clone() })
            .map_err(|_| Errno(ENOMEM))?;
        Arc::try_new(VNode::new(
            self.root.mount, ino, inode.created_generation, inode.size,
            attributes(ino, inode)?.times, fs, inode.created_generation, self.root.clone(),
        )?).map_err(|_| Errno(ENOMEM))
    }
    fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Errno> {
        self.read_files(|files| Ok(files.lookup(dir, name)?))
    }
    fn lookup_open(&self, dir: &VNode, name: &[u8]) -> Result<Arc<VNode>, Errno> {
        self.check_node(dir)?;
        self.open(self.lookup(dir.ino, name)?)
    }
    fn touch(&self, _: &VNode, _: bool) {}
    fn parent(&self, dir: u64) -> Result<u64, Errno> {
        let inode = self.read_files(|files| Ok(files.inode(dir)?))?;
        if kind(inode.mode)? != NodeKind::Directory {
            return Err(Errno(ENOTDIR));
        }
        Ok(inode.parent)
    }
    fn readdir(
        &self,
        dir: u64,
        cookie: u64,
        sink: &mut dyn FnMut(DirEnt, u64) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        self.read_files(|files| Ok(read_directory(files, dir, cookie, sink)?))?
    }
    fn readlink(&self, ino: u64) -> Result<Vec<u8>, Errno> {
        self.read_files(|files| Ok(files.readlink(ino)?))
    }
    fn read(&self, node: &VNode, off: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        self.check_node(node)?;
        buf.fill(0);
        self.read_files(|files| files.read_verified(&self.writer.data_reader(), node.ino, off, buf))
    }
    fn fsync(&self, node: &VNode, _: bool) -> Result<(), Errno> {
        self.check_node(node)
    }
    fn mapping_io(&self, node: &VNode) -> Result<Box<dyn MappingIo>, Errno> {
        self.check_node(node)?;
        let attr = self.getattr(node.ino)?;
        match attr.kind {
            NodeKind::Regular => {}
            NodeKind::Directory => return Err(Errno(EISDIR)),
            _ => return Err(Errno(EINVAL)),
        }
        Ok(Box::try_new(Mapping {
            fs: self.writer.clone(),
            node: self.open(node.ino)?,
            size: attr.size,
            writable: false,
            staged_generation: Cell::new(None),
        }).map_err(|_| Errno(ENOMEM))?)
    }
    fn setattr(&self, _: u64, _: SetAttr) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn create(&self, _: u64, _: &[u8], _: NewNode<'_>) -> Result<u64, Errno> {
        Err(Errno(EROFS))
    }
    fn create_open(
        &self, _: u64, _: &[u8], _: NodeKind,
        _: &dyn Fn(NodeAttrs) -> Result<InodeAttrs, Errno>,
        _: &mut dyn FnMut(Arc<VNode>) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn link(&self, _: u64, _: u64, _: &[u8], _: &LinkCheck<'_>) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn remove(&self, _: u64, _: &[u8], _: Remove) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn rename(&self, _: u64, _: &[u8], _: u64, _: &[u8], _: RenameMode, _: &RenameCheck<'_>) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn write(&self, _: &VNode, _: WriteReq, _: &[u8]) -> Result<WriteResult, WriteError> {
        Err(Errno(EROFS).into())
    }
    fn resize(&self, _: &VNode, _: u64, _: bool, _: &dyn Fn(NodeAttrs) -> Result<(), Errno>) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn writeback_pages(&self, _: &VNode, _: u64, _: &[u8], _: &dyn MappingIo) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn clear_mapping_privilege(&self, _: &VNode) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn xattr(&self, ino: u64, op: Xattr<'_>) -> Result<usize, Errno> {
        match op {
            Xattr::Get { name, value } => {
                let records = self.read_files(|files| Ok(files.list_xattrs(ino)?))?;
                let record = records.iter().find(|record| record.name == name).ok_or(Errno(ENODATA))?;
                copy_xattr(value, &record.value)
            }
            Xattr::List { names } => {
                let records = self.read_files(|files| Ok(files.list_xattrs(ino)?))?;
                let mut bytes = Vec::new();
                for record in records {
                    bytes.extend_from_slice(&record.name);
                    bytes.push(0);
                }
                copy_xattr(names, &bytes)
            }
            Xattr::Set { .. } | Xattr::Remove { .. } => Err(Errno(EROFS)),
        }
    }
}

fn read_directory<S: super::btree::NodeStore>(
    files: &Files<'_, S>,
    dir: u64,
    mut cookie: u64,
    sink: &mut dyn FnMut(DirEnt, u64) -> Result<bool, Errno>,
) -> Result<Result<u64, Errno>, ops::Error> {
    let mut outcome = Ok(());
    files.visit_directory_entries(dir, cookie, &mut |next, entry| {
        let accepted = record_kind(entry.kind).and_then(|kind| sink(
            DirEnt { ino: entry.ino, kind, name: entry.name }, next,
        ));
        match accepted {
            Ok(true) => { cookie = next; true }
            Ok(false) => false,
            Err(error) => { outcome = Err(error); false }
        }
    })?;
    Ok(outcome.map(|()| cookie))
}

fn copy_xattr(out: &mut [u8], bytes: &[u8]) -> Result<usize, Errno> {
    if !out.is_empty() {
        if out.len() < bytes.len() {
            return Err(Errno(ERANGE));
        }
        out[..bytes.len()].copy_from_slice(bytes);
    }
    Ok(bytes.len())
}
// Match the content-change policy in fs/ext2/mod.rs::content_privilege.
fn content_privilege(mode: u32) -> u32 {
    0o4000 | if mode & 0o0010 != 0 { 0o2000 } else { 0 }
}
// Same relatime predicate as fs/ext2/inode.rs::needs_atime_update, using
// BreenFS's full timestamp precision. A read changes neither mtime nor ctime.
fn needs_atime_update(inode: &Inode, now: Time) -> bool {
    let atime = (inode.atime.seconds, inode.atime.nanos);
    let now = (now.seconds, now.nanos);
    now > atime
        && (atime <= (inode.mtime.seconds, inode.mtime.nanos)
            || atime <= (inode.ctime.seconds, inode.ctime.nanos)
            || now >= (inode.atime.seconds.saturating_add(86400), inode.atime.nanos))
}
fn now() -> Time {
    Time {
        seconds: crate::time::current_unix_time() as i64,
        nanos: 0,
    }
}
fn time(t: Timestamp) -> Time {
    Time {
        seconds: t.sec,
        nanos: t.nsec,
    }
}

// Stable mount-private cache identity in filesystem block units. Commit writes
// use the raw device; discard changed data ranges before reusing this view.
struct DataBlocks {
    device: Arc<dyn BlockDevice>,
    blocks: u64,
}
impl BlockDevice for DataBlocks {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        if block >= self.blocks || bytes.len() != BLOCK_SIZE {
            return Err(BlockError::OutOfBounds);
        }
        let sector = self.device.block_size();
        if sector == 0 || BLOCK_SIZE % sector != 0 {
            return Err(BlockError::IoError);
        }
        let start = block
            .checked_mul((BLOCK_SIZE / sector) as u64)
            .ok_or(BlockError::OutOfBounds)?;
        self.device.read_blocks(start, BLOCK_SIZE / sector, bytes)
    }
    fn write_block(&self, _: u64, _: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::IoError)
    }
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        self.blocks
    }
    fn flush(&self) -> Result<(), BlockError> {
        Err(BlockError::IoError)
    }
}
// Foreground views do not name a frozen generation's private data: N+1's
// scratch view predates N's drain, and N's view is installed only after N's
// data is on the device (E24 coordinator 597).
struct DataReader<'a, W: Wait> {
    cache: &'a BufferCache<W>,
    blocks: &'a DataBlocks,
}
impl<W: Wait> BlockDevice for DataReader<'_, W> {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        // Files verifies the returned block against its extent REF before use.
        self.cache
            .read_verified(self.blocks, block, bytes, |_| true)
    }
    fn write_block(&self, _: u64, _: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::IoError)
    }
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        self.blocks.blocks
    }
    fn flush(&self) -> Result<(), BlockError> {
        Err(BlockError::IoError)
    }
}
impl<W: Wait> Drop for Mount<W> {
    fn drop(&mut self) {
        self.signal.wake_all();
        self.cache.buffers().discard_clean(self.data.as_ref());
        drop(self.owner.take());
    }
}

// The transfer retains a VNode, keeping unlinked extents alive. Publication
// holds the mount guard before entering the caller's mapping critical section.
struct Mapping<W: Wait + 'static> {
    fs: BreenFs<W>,
    node: Arc<VNode>,
    size: u64,
    writable: bool,
    staged_generation: Cell<Option<u64>>,
}
impl<W: Wait + 'static> MappingIo for Mapping<W> {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn size(&self) -> u64 {
        self.size
    }
    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>, Errno> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(|_| Errno(ENOMEM))?;
        bytes.resize(length, 0);
        self.node.filesystem.read(&self.node, offset, &mut bytes)?;
        Ok(bytes)
    }
    fn needs_flush(&self) -> bool {
        self.writable
    }
    fn staged_writeback_generation(&self) -> Option<u64> {
        self.staged_generation.get()
    }
    fn sync(&self, first: u64, last: u64) -> Result<(), Errno> {
        if !self.writable {
            return Ok(());
        }
        // The open generation contains this range's staged writeback (or a
        // successor if it already committed). sync joins its receipt and waits
        // for the final flush, outside the mount lock.
        self.fs.sync_generation(None, false)?;
        self.fs.complete_mapping(&self.node, first, last);
        Ok(())
    }
    fn publish(&self, bind: &mut dyn FnMut(bool) -> Result<(), Errno>) -> Result<(), Errno> {
        let mut state = self.fs.foreground_lock()?;
        if self.writable {
            state.writable()?;
        }
        bind(false)?;
        if self.writable {
            let entry = state.nodes.get_mut(&self.node.ino).ok_or(Errno(EIO))?;
            if entry.links != 0 {
                entry.mapped = Some(self.node.clone());
            }
            self.fs.request_eviction();
        }
        Ok(())
    }
}
// The most bytes one write admits at a time (N2). Half the budget lets one
// part fill while the generation holding the previous part is published.
const WRITE_PART: usize = (DIRTY_BUDGET / 2) as usize;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Split {
    No,
    Take,
    Held,
}
// The most dirty bytes a call over `len` bytes can add: the pages it spans at
// its worst alignment.
fn touched_bytes(len: usize) -> u64 {
    if len == 0 {
        return 0;
    }
    ((len as u64).div_ceil(BLOCK_SIZE as u64) + 1) * BLOCK_SIZE as u64
}
fn attributes(ino: u64, i: Inode) -> Result<NodeAttrs, Errno> {
    Ok(NodeAttrs {
        ino,
        kind: kind(i.mode)?,
        attrs: InodeAttrs::new(i.mode, i.uid, i.gid),
        nlink: u64::from(i.nlink),
        size: i.size,
        blocks: i
            .nblocks
            .checked_mul((BLOCK_SIZE / 512) as u64)
            .ok_or(Errno(EIO))?,
        rdev: i.rdev,
        times: NodeTimes {
            atime: stamp(i.atime),
            mtime: stamp(i.mtime),
            ctime: stamp(i.ctime),
            btime: Some(stamp(i.btime)),
        },
    })
}
fn stamp(t: Time) -> Timestamp {
    Timestamp {
        sec: t.seconds,
        nsec: t.nanos,
    }
}
fn kind(mode: u32) -> Result<NodeKind, Errno> {
    match mode & 0o170000 {
        0o100000 => Ok(NodeKind::Regular),
        0o040000 => Ok(NodeKind::Directory),
        0o120000 => Ok(NodeKind::Symlink),
        0o020000 => Ok(NodeKind::CharDevice),
        0o060000 => Ok(NodeKind::BlockDevice),
        0o010000 => Ok(NodeKind::Fifo),
        0o140000 => Ok(NodeKind::Socket),
        _ => Err(Errno(EIO)),
    }
}
fn record_kind(value: u8) -> Result<NodeKind, Errno> {
    match value {
        1 => Ok(NodeKind::Regular),
        2 => Ok(NodeKind::Directory),
        3 => Ok(NodeKind::CharDevice),
        4 => Ok(NodeKind::BlockDevice),
        5 => Ok(NodeKind::Fifo),
        6 => Ok(NodeKind::Socket),
        7 => Ok(NodeKind::Symlink),
        _ => Err(Errno(EIO)),
    }
}
// Called only after mutation admission closes and the commit worker stops.
// A failed transaction cannot be repaired by retrying sync. Report its unknown
// durability, then let mount removal or reboot proceed; fsync still returns EIO.
fn teardown_result(name: &str, result: Result<(), Errno>) -> Result<(), Errno> {
    match result {
        Err(Errno(EIO)) => {
            log::error!(
                "[shutdown] {} breenfs failed read-only; uncommitted changes may be lost",
                name
            );
            Ok(())
        }
        other => other,
    }
}

fn latches_on_read(error: ops::Error) -> bool {
    errno(error) == Errno(EIO) && !matches!(error, ops::Error::Metadata(super::btree::ReadFailure {
        cause, ..
    }) if cause.pre_issue())
}

fn read_cause_errno(cause: super::btree::ReadCause) -> Errno {
    use crate::block::read_failure::AhciReadError;
    use super::btree::ReadCause;
    Errno(match cause {
        ReadCause::Allocation | ReadCause::Ahci(AhciReadError::WaiterAllocation) => ENOMEM,
        ReadCause::Ahci(AhciReadError::Interrupted) => EINTR,
        ReadCause::Ahci(AhciReadError::CannotPark | AhciReadError::WaitPublication) => EAGAIN,
        ReadCause::Ahci(AhciReadError::AdmissionTimeout) => ETIMEDOUT,
        ReadCause::Driver(failure) if failure.context_refused() => EAGAIN,
        _ => EIO,
    })
}

fn recovery_errno(error: super::commit::Error) -> Errno {
    match error {
        super::commit::Error::BlockRead(error) => read_cause_errno(error.into()),
        _ => errno(error.into()),
    }
}

fn errno(error: ops::Error) -> Errno {
    Errno(match error {
        ops::Error::Metadata(failure) => return read_cause_errno(failure.cause),
        ops::Error::DataRead(error) => return read_cause_errno(error.into()),
        ops::Error::EIO => EIO,
        ops::Error::EROFS => EROFS,
        ops::Error::EBUSY => EBUSY,
        ops::Error::EPERM => EPERM,
        ops::Error::ENOTEMPTY => ENOTEMPTY,
        ops::Error::ENOENT => ENOENT,
        ops::Error::ENODATA => ENODATA,
        ops::Error::EEXIST => EEXIST,
        ops::Error::ENOTDIR => ENOTDIR,
        ops::Error::EISDIR => EISDIR,
        ops::Error::EINVAL => EINVAL,
        ops::Error::EFBIG => EFBIG,
        ops::Error::EMLINK => EMLINK,
        ops::Error::ENAMETOOLONG => ENAMETOOLONG,
        ops::Error::ENOSPC => ENOSPC,
        ops::Error::ENOMEM => ENOMEM,
    })
}
