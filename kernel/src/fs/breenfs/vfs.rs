//! BreenFS operations at the VFS boundary (design 1.1, 2.3 and 2.5).
use super::preempt::preemption_point;
#[path = "mutation_cost.rs"]
mod mutation_cost;
use mutation_cost::{Costs, Op as CostOp, Phase as CostPhase};
use super::{
    abi::*,
    scrub::{self, Scrub},
    commit::{Committer, PreparedCommit},
    commit_policy::{CommitPolicy, COMMIT_INTERVAL_NS, DIRTY_BUDGET},
    commit_signal::{monotonic_ns, CommitSignal},
    dirty::DirtyPages,
    format::{self, Inode, Root, Superblock, Time, BLOCK_SIZE},
    mount_lock::{MountGuard, MountLock},
    node_cache::CachedNodeStore,
    ops::{self, Files, ReadError},
    recover,
    reserve::{Estimate, Grant, Ledger, Operation},
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
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

#[cfg(test)]
pub mod measurement {
    std::thread_local! {
        static COMMITS: core::cell::Cell<[u64; 3]> = const { core::cell::Cell::new([0; 3]) };
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
        let observed = fs.inner.signal.epoch();
        fs.commit_due(now_ns).map(|worked| (worked, fs.inner.signal.epoch() != observed))
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
#[path = "../../../../tools/breenfs-host-tests/src/metadata_failure.rs"]
mod metadata_failure;

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
    costs: Costs,
    state: MountLock<MountState<W>>,
    device: Arc<dyn BlockDevice>,
    owner: Option<WritableOwner>,
    data: Box<DataBlocks>,
    cache: Cache<W>,
    id: usize,
    root_id: u64,
    mounted_root: MountedRoot,
    frozen: bool,
    writable: bool,
    signal: Arc<CommitSignal>,
    scrub_signal: Arc<CommitSignal>,
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
struct MountState<W: Wait + 'static> {
    files: Files<'static, CachedNodeStore<'static, W>>,
    committer: Option<Committer>,
    durable: Superblock,
    failed: bool,
    failure: Option<(&'static str, i32)>,
    metadata_error: Option<super::btree::ReadFailure>,
    read_refusal_reported: bool,
    scrub: Option<Scrub>,
    scrub_state: u32,
    #[cfg(not(test))]
    mount: usize,
    closed: bool,
    nodes: BTreeMap<u64, NodeState>,
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
    pending: Option<Arc<PreparedCommit>>,
    reservations: Ledger,
    grants: Vec<Grant>,
    metadata_reserved: u64,
    pending_grants: Vec<Grant>,
    signal: Arc<CommitSignal>,
}
// Transaction dirty pages outlive the last open until commit. Mapping pins
// are held separately until cache retirement or stopped-service teardown.
#[derive(Default)]
struct NodeState {
    node: Weak<VNode>,
    // Published mappings retain their VNode until service retires the cache.
    mapped: Option<Arc<VNode>>,
    dirty: DirtyPages,
    reserved_metadata: u64,
    touched: Option<Arc<TouchedTimes>>,
    // A write admitted in parts (N2) holds other writes and resizes of this
    // file off between its parts.
    splitting: bool,
}
// Retained by NodeState after close, without retaining the mount itself.
#[derive(Default)]
struct TouchedTimes {
    access: AtomicU64,
    modify: AtomicU64,
}
impl TouchedTimes {
    fn pending(&self) -> bool {
        self.access.load(Ordering::Acquire) != 0 || self.modify.load(Ordering::Acquire) != 0
    }
}
struct NodePin<W: Wait + 'static> {
    mount: Arc<Mount<W>>,
    touched: Arc<TouchedTimes>,
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
    fn apply_touches(&mut self, ino: u64) -> Result<(), Errno> {
        preemption_point();
        let Some(touched) = self.nodes.get(&ino).and_then(|entry| entry.touched.as_ref())
        else {
            return Ok(());
        };
        let access = touched.access.swap(0, Ordering::AcqRel);
        let modify = touched.modify.swap(0, Ordering::AcqRel);
        if (access == 0 && modify == 0)
            || self.failed
            || self.committer.as_ref().is_none_or(Committer::read_only)
        {
            return Ok(());
        }
        let mut inode = match self.files.inode(ino) {
            Ok(inode) => inode,
            // A final touch can race commit's reclaim of a closed orphan.
            Err(ops::Error::ENOENT) => return Ok(()),
            Err(error) => return self.edited(Err(error)),
        };
        let before = inode;
        if modify != 0 {
            inode.mtime = Time {
                seconds: (modify - 1) as i64,
                nanos: 0,
            };
            inode.ctime = inode.mtime;
        }
        if access != 0 {
            let at = Time {
                seconds: (access - 1) as i64,
                nanos: 0,
            };
            if needs_atime_update(&inode, at) {
                inode.atime = at;
            }
        }
        if inode != before {
            let result = self.files.put_inode(ino, inode);
            self.edited(result)?;
        }
        Ok(())
    }
    fn inode(&mut self, ino: u64) -> Result<Inode, Errno> {
        self.apply_touches(ino)?;
        // VNode drops do no I/O or mount locking. Refresh the writer's open
        // snapshot here so a closed orphan cannot be opened again by inode.
        self.retain_open();
        if let Some((_, inode)) = self
            .inodes
            .entries
            .iter()
            .flatten()
            .find(|(cached, _)| *cached == ino)
        {
            if inode.nlink == 0
                && self
                    .nodes
                    .get(&ino)
                    .is_none_or(|node| node.node.strong_count() == 0)
            {
                return Err(Errno(ENOENT));
            }
            return Ok(*inode);
        }
        let result = self.files.inode(ino);
        let inode = self.observed(result)?;
        let slot = self.inodes.next;
        self.inodes.entries[slot] = Some((ino, inode));
        self.inodes.next = (slot + 1) % self.inodes.entries.len();
        Ok(inode)
    }
    fn attrs(&mut self, ino: u64) -> Result<NodeAttrs, Errno> {
        let mut inode = self.inode(ino)?;
        if self.nodes.get(&ino).is_some_and(|n| n.dirty.bytes() != 0) {
            let result = self.files.extents(ino);
            let extents = self.observed(result)?;
            let node = self.nodes.get(&ino).ok_or(Errno(EIO))?;
            inode.nblocks += node
                .dirty
                .indexes()
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
    fn retain_open(&mut self) {
        self.nodes.retain(|_, node| {
            preemption_point();
            node.node.strong_count() != 0
                || node.splitting
                || node.dirty.bytes() != 0
                || node.touched.as_ref().is_some_and(|times| times.pending())
        });
        self.files.retain_open_inodes(
            self.nodes
                .iter()
                .filter(|(_, node)| { preemption_point(); node.node.strong_count() != 0 })
                .map(|(&ino, _)| ino)
                .collect(),
        );
    }
    fn fail(&mut self, branch: &'static str, error: Errno) {
        if self.failure.is_some() { return; }
        self.failure = Some((branch, error.0));
        self.failed = true;
        if let Some(failure) = self.metadata_error {
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
        self.signal.wake();
    }
    fn observed<T>(&mut self, result: Result<T, ops::Error>) -> Result<T, Errno> {
        if let Err(error) = result.as_ref() { self.record_metadata(*error); }
        if result.as_ref().is_err_and(|error| errno(*error) == Errno(EIO)) {
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
            Err(ReadError::Data) => Err(Errno(EIO)),
        }
    }
    fn edited<T>(&mut self, result: Result<T, ops::Error>) -> Result<T, Errno> {
        if result.is_ok() { self.fresh = false; }
        self.inodes.entries.fill(None);
        // A storage/allocation failure may have interrupted a multi-item edit.
        // Do not publish that transaction. Reservation refusals return before
        // mutation via admission; an ENOSPC here is still a bug.
        if let Err(error) = result.as_ref() { self.record_metadata(*error); }
        if result.as_ref().is_err_and(|error| matches!(errno(*error).0, EIO | ENOSPC)) {
            self.fail("edited", errno(result.as_ref().err().copied().expect("edit error")));
        }
        for (&ino, entry) in &self.nodes {
            #[cfg(test)]
            super::btree::measurement::mutation(7, 1);
            preemption_point();
            if let Some(node) = entry.node.upgrade() {
                if let Ok(inode) = self.files.inode(ino) {
                    node.size.store(inode.size, Ordering::Release);
                    *node.times.lock() = attributes(ino, inode)?.times;
                }
            }
        }
        if result.is_ok() {
            self.signal.wake();
        }
        result.map_err(errno)
    }
    /// Dirty pages plus the private data of the generation being published:
    /// the bytes mutation admission charges against DIRTY_BUDGET.
    fn charged(&self) -> u64 {
        self.dirty_bytes + self.pending.as_ref().map_or(0, |p| p.dirty_bytes())
    }
    /// Called with mutation admission held, before publishing dirty pages or
    /// inode changes. Clean mapped blocks also need fresh COW allocations.
    fn reserve_data(
        &mut self,
        ino: u64,
        offset: u64,
        length: usize,
        operation: Operation,
    ) -> Result<(Grant, u64), Errno> {
        let node = self.nodes.get(&ino).ok_or(Errno(EIO))?;
        let (new_pages, pages, runs) = node.dirty.reservation_runs(offset, length);
        let reserved_metadata = node.reserved_metadata;
        let result = self.files.reservation_paths(ino);
        let paths = self.observed(result)?;
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
            .ok_or(Errno(ENOSPC))?;
        if blocks != 0 {
            self.grants.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
        }
        // durable.free_blocks includes the allocations covered by both the
        // publishing generation and the open generation. Do not debit them
        // a second time before asking the ledger for a grant.
        self.reservations.reserve(self.durable.free_blocks, blocks, operation)
            .map(|grant| (grant, metadata))
            .map_err(|_| Errno(ENOSPC))
    }
    fn finish_data(&mut self, ino: u64, (grant, metadata): (Grant, u64), result: Result<(), Errno>) -> Result<(), Errno> {
        if result.is_ok() {
            let node = self.nodes.get_mut(&ino).ok_or(Errno(EIO))?;
            node.reserved_metadata = node.reserved_metadata.max(metadata);
            if grant.blocks() != 0 { self.grants.push(grant); }
            else { self.reservations.release(grant); }
            // The data grant already funds the inode/COW work just performed.
            // A later namespace grant must not charge that debt a second time.
            let debt = self.files.metadata_debt();
            let debt = self.observed(debt)?;
            self.metadata_reserved = self.metadata_reserved.max(debt);
        } else {
            // DirtyPages prepares its pages before changing the dirty map.
            self.reservations.release(grant);
        }
        result
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
    ) -> Result<Grant, Errno> {
        let result = self.files.metadata_reservation_paths(objects, new_inode, orphans);
        let paths = self.observed(result)?;
        let fs_levels = edits.max(3).checked_mul(4 * (paths.fs_height + 2))
            .ok_or(Errno(ENOSPC))?;
        let space_changes = paths.uncopied_fs_nodes.checked_add(fs_levels)
            .and_then(|n| n.checked_mul(3)).ok_or(Errno(ENOSPC))?;
        let blocks = Estimate {
            data_blocks: 0,
            uncopied_fs_nodes: paths.uncopied_fs_nodes,
            fs_levels,
            space_changes,
            space_height: paths.space_height,
            space_nodes: paths.space_nodes,
        }.blocks().map_err(|_| Errno(ENOSPC))?;
        self.grants.try_reserve(1).map_err(|_| Errno(ENOMEM))?;
        self.reservations.reserve(self.durable.free_blocks, blocks, operation)
            .map_err(|_| Errno(ENOSPC))
    }
    fn finish_metadata<T>(&mut self, mut grant: Grant, result: Result<T, ops::Error>) -> Result<T, Errno> {
        if result.is_ok() {
            // durable.free_blocks still includes private allocations. Keep
            // those and the work publication still owes, once per transaction;
            // return unused worst-case paths (E24 ledger 509(n)).
            let debt = self.files.metadata_debt();
            let debt = match debt {
                Ok(debt) => debt,
                Err(error) => {
                    self.reservations.release(grant);
                    return self.edited(Err(error));
                }
            };
            let retained = debt.saturating_sub(self.metadata_reserved);
            if self.reservations.settle(&mut grant, retained).is_err() {
                self.reservations.release(grant);
                return self.edited(Err(ops::Error::ENOSPC));
            }
            self.metadata_reserved += retained;
            if retained != 0 { self.grants.push(grant); }
            else { self.reservations.release(grant); }
        } else { self.reservations.release(grant); }
        self.edited(result)
    }
    fn drain_dirty(&mut self, device: &dyn BlockDevice) -> Result<(), Errno> {
        let result = self.drain_pages(device);
        // Draining frees pages as it copies them; keep the charge truthful
        // even when a failed drain leaves the mount failed.
        self.dirty_bytes = self.nodes.values().map(|node| { preemption_point(); node.dirty.bytes() }).sum();
        result
    }
    fn drain_pages(&mut self, device: &dyn BlockDevice) -> Result<(), Errno> {
        for (&ino, node) in self.nodes.iter_mut() {
            preemption_point();
            if node.dirty.bytes() == 0 {
                continue;
            }
            let mut inode = match self.files.inode(ino) {
                Ok(inode) => inode,
                // An orphan whose last open closed is reclaimed by fsync.
                Err(ops::Error::ENOENT) => continue,
                Err(e) => return Err(errno(e)),
            };
            let files = &mut self.files;
            node.dirty.write_runs(inode.size, |offset, bytes| {
                files
                    .write(device, ino, offset, bytes, inode.mtime)
                    .map_err(errno)?;
                Ok(())
            })?;
            // Allocation changes nblocks; write time and subsequent setattr
            // metadata belong to the syscall, rather than the later commit.
            inode.nblocks = self.files.inode(ino).map_err(errno)?.nblocks;
            self.files.put_inode(ino, inode).map_err(errno)?;
        }
        Ok(())
    }
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
        Self::mount_with(device, Cache::Shared(cache), mount, root_id, None, false)
    }
    /// Writable recovery repairs the rings before opening the first transaction.
    /// The caller starts the per-mount worker through init_service().
    pub fn mount_writable(
        device: Arc<dyn BlockDevice>,
        cache: Arc<BufferCache<W>>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Shared(cache), mount, root_id, None, true)
    }
    /// Kernel read mounts borrow the system cache.
    pub fn mount_cached(
        device: Arc<dyn BlockDevice>,
        cache: &'static BufferCache<W>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Kernel(cache), mount, root_id, None, false)
    }
    /// Writable kernel mounts borrow the system cache, as read mounts do.
    pub fn mount_writable_cached(
        device: Arc<dyn BlockDevice>,
        cache: &'static BufferCache<W>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Kernel(cache), mount, root_id, None, true)
    }
    /// Explicit mounts use the same root-data parser with either cache owner.
    pub fn mount_named(
        device: Arc<dyn BlockDevice>, cache: Arc<BufferCache<W>>, mount: usize,
        data: Option<&[u8]>, writable: bool,
    ) -> Result<Arc<Self>, Errno> {
        let name = super::mount_args::parse_root(data).map_err(|e| Errno(e as i32))?;
        Self::mount_with(device, Cache::Shared(cache), mount, 1, name, writable)
    }
    pub fn mount_named_cached(
        device: Arc<dyn BlockDevice>, cache: &'static BufferCache<W>, mount: usize,
        data: Option<&[u8]>, writable: bool,
    ) -> Result<Arc<Self>, Errno> {
        let name = super::mount_args::parse_root(data).map_err(|e| Errno(e as i32))?;
        Self::mount_with(device, Cache::Kernel(cache), mount, 1, name, writable)
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
    ) -> Result<Arc<Self>, Errno> {
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
        let durable = recover::recover(device.as_ref(), writable).map_err(|_| Errno(EIO))?;
        // Recovery can publish orphan reclamation even if root selection fails.
        *publication = Some(durable.clone());
        let store = cache.store(device.clone(), &durable);
        let roots = recover::space_items(&store, &durable).map_err(|_| Errno(EIO))?;
        let mut selected = None;
        for (key, bytes) in roots.iter().filter(|(key, _)| key.item_type == format::ROOT) {
            let root = Root::decode(bytes).map_err(|_| Errno(EIO))?;
            if root_name.map_or(key.offset == root_id, |name| root.name == name) {
                selected = Some((key.offset, root.flags & 1 != 0));
                break;
            }
        }
        let (root_id, frozen) = selected.ok_or(Errno(ENOENT))?;
        let files =
            Files::new(store, &durable, root_id, writable).map_err(errno)?;
        let mounted_root = MountedRoot::claim(mounts.clone(), root_id)?;
        drop(publication);
        drop(selection);
        let dead_remaining = roots.iter().filter(|(key, _)| key.item_type == format::DEAD).count() as u64;
        let roots = roots.into_iter().filter(|(key, _)| key.item_type == format::ROOT)
            .map(|(key, bytes)| Root::decode(&bytes)
                .map(|root| (key.offset, (root.name, root.flags))))
            .collect::<Result<_, _>>().map_err(|_| Errno(EIO))?;
        let committer = writable.then(|| Committer::new(durable.clone()));
        let data = Box::try_new(DataBlocks {
            device: device.clone(),
            blocks: durable.total_blocks,
        })
        .map_err(|_| Errno(ENOMEM))?;
        let signal = Arc::new(CommitSignal::new());
        let inner = Arc::try_new(Mount {
            costs: Costs::new(),
            state: MountLock::new(MountState {
                files,
                committer,
                reservations: Ledger::new(durable.global_reserve),
                grants: Vec::new(),
                metadata_reserved: 0,
                pending_grants: Vec::new(),
                durable,
                failed: false,
                failure: None,
                metadata_error: None,
                read_refusal_reported: false,
                scrub: None,
                scrub_state: SCRUB_IDLE,
                #[cfg(not(test))]
                mount,
                closed: false,
                nodes: BTreeMap::new(),
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
            frozen,
            writable,
            signal,
            scrub_signal: Arc::new(CommitSignal::new()),
        })
        .map_err(|_| Errno(ENOMEM))?;
        Arc::try_new(Self { inner }).map_err(|_| Errno(ENOMEM))
    }
    fn mutation_lock(&self, op: CostOp) -> MountGuard<'_, MountState<W>> {
        let _wait = self.inner.costs.span(op, CostPhase::Wait);
        self.inner.state.lock()
    }
    fn mutation_wait(&self, op: CostOp, observed: u64, deadline: Option<u64>) {
        let _wait = self.inner.costs.span(op, CostPhase::Wait);
        self.inner.signal.wait(observed, deadline);
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
            touched: Arc::try_new(TouchedTimes::default()).map_err(|_| Errno(ENOMEM))?,
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
        let node = self.new_node(ino, inode, state.durable.generation)?;
        let entry = state.nodes.entry(ino).or_default();
        entry.node = Arc::downgrade(&node);
        entry.touched = Some(node.pin::<NodePin<W>>().ok_or(Errno(EIO))?.touched.clone());
        state.retain_open();
        Ok(node)
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
            let state = self.mutation_lock(cost);
            state.writable()?;
            if state.dirty_bytes.saturating_add(incoming) > DIRTY_BUDGET / 2 {
                // edited() also wakes the worker under the mount lock.
                self.inner.signal.pressure.store(true, Ordering::Release);
                self.inner.signal.wake();
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
            self.mutation_wait(cost, observed, None);
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
            self.mutation_wait(cost, observed, None);
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
        mut reserve: impl FnMut(&mut MountState<W>) -> Result<T, Errno>,
    ) -> Result<(MountGuard<'a, MountState<W>>, T), Errno> {
        let mut limit = None;
        loop {
            state.writable()?;
            let result = {
                let _reservation = self.inner.costs.span(cost, CostPhase::Reservation);
                reserve(&mut state)
            };
            match result {
                Ok(grant) => return Ok((state, grant)),
                Err(error) if error == Errno(ENOSPC) => (),
                Err(error) => return Err(error),
            }
            let last = {
                let _reservation = self.inner.costs.span(cost, CostPhase::Reservation);
                match limit {
                    Some(last) => last,
                    None => {
                        // The detached generation can already contain releases
                        // absent from durable. Its private nodes are read through
                        // the same view used by the open transaction.
                        let sb = state.pending.as_ref().map_or(&state.durable, |p| p.superblock());
                        let mut store = self.inner.cache.store(self.inner.device.clone(), sb);
                        if let Some(prepared) = &state.pending { store = store.with_pending(prepared); }
                        let promotion = super::space::pending_promotion_generation(&store, sb.space_root)
                            .map_err(|_| Errno(EIO))?;
                        // Capture a finite publication bound on the first refusal.
                        // The open generation can hold admission capacity without
                        // any durable PENDING range (#3064); release its grants too.
                        let retained = (!state.grants.is_empty() || !state.pending_grants.is_empty())
                            .then(|| sb.generation.checked_add(1).ok_or(Errno(ENOSPC)))
                            .transpose()?;
                        let generation = promotion.into_iter().chain(retained).max()
                            .ok_or(Errno(ENOSPC))?;
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
            if state.durable.generation >= last { return Err(Errno(ENOSPC)); }
            let observed = self.inner.signal.epoch();
            drop(state);
            // Do not service mappings recursively: a mapping writer can be the
            // caller. Detached publication and its wait hold no mount lock.
            let published = {
                let _publication = self.inner.costs.span(cost, CostPhase::Publication);
                self.commit_generation_up_to(monotonic_ns(), last, true)?
            };
            if !published {
                self.mutation_wait(cost, observed, None);
            }
            state = admit()?;
        }
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
                    Some(state.reserve_data(node.ino, offset, length, Operation::Allocate)?)
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
            state.reservations.release(grant.0);
            return Err(Errno(ENOMEM).into());
        }
        let pending = state.pending.clone();
        let MountState {
            files,
            nodes,
            dirty_bytes,
            ..
        } = &mut *state;
        let dirty = &mut nodes.entry(node.ino).or_default().dirty;
        let before = dirty.bytes();
        let mut bad_metadata = None;
        let result = dirty.write(offset, &bytes[..length], |off, page| {
            files.read_verified(&self.data_reader(pending.clone()), node.ino, off, page)
                .map_err(|error| {
                    if let ReadError::Metadata(error) = error { bad_metadata = Some(error); }
                    errno(error.error())
                })?;
            Ok(())
        });
        *dirty_bytes += dirty.bytes() - before;
        if let Some(error) = bad_metadata {
            state.record_metadata(error);
            if errno(error) == Errno(EIO) { state.fail("write-read-metadata", Errno(EIO)); }
        }
        state.finish_data(node.ino, grant, result)?;
        inode.size = inode.size.max(end);
        inode.mtime = now();
        inode.ctime = inode.mtime;
        if req.clear_setid {
            inode.mode &= !content_privilege(inode.mode);
        }
        match state.files.put_inode(node.ino, inode) {
            Err(error) => {
                let result = state.edited::<()>(Err(error));
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
                state.signal.wake();
            }
        }
        // Growth clears the old EOF tail before the new bytes are copied.
        node.map.transition(inode.size);
        node.map.commit_write(offset, &bytes[..length]);
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
        let retry_ns = self.inner.state.lock().mapping_retry_ns;
        if now_ns >= retry_ns && self.inner.signal.mappings.swap(false, Ordering::AcqRel) {
            if let Err(error) = self.service_mappings(now_ns) {
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
        self.commit_generation(now_ns)
    }
    fn service_mappings(&self, now_ns: u64) -> Result<(), Errno> {
        let handles = self.mapped_nodes()?;
        let mut budget = crate::memory::file_map::EVICT_BUDGET;
        let mut more = false;
        for node in handles {
            preemption_point();
            if !self.inner.state.lock().failed {
                more |= node.map.service_writeback_with(&node, &mut budget, |first, last| {
                    self.inner.signal.requested.store(true, Ordering::Release);
                    self.commit_generation(now_ns)?;
                    self.complete_mapping(&node, first, last);
                    Ok(())
                })?;
            }
            // Population and mutation take the mount lock before MapState.
            let mut state = self.inner.state.lock();
            if state.failed {
                if let Some(entry) = state.nodes.get_mut(&node.ino) {
                    let internal = 1 + usize::from(entry.mapped.is_some());
                    if Arc::strong_count(&node) == internal {
                        // No descriptor or binding can observe these pages.
                        // Failed storage cannot write them back; retire them
                        // under the ordinary page budget, reporting EIO below.
                        more |= node.map.discard_unobserved(&mut budget);
                    } else if node.map.eviction_pending() {
                        // External handles now own the cache lifetime. Dropping
                        // the final descriptor must not leave a service cycle.
                        entry.mapped = None;
                    }
                }
            }
            if node.map.eviction_pending() { more |= node.map.evict(&mut budget); }
            if !node.map.nonempty() {
                if let Some(entry) = state.nodes.get_mut(&node.ino) { entry.mapped = None; }
            }
        }
        if more { self.request_eviction(); }
        Ok(())
    }
    fn mapped_nodes(&self) -> Result<Vec<Arc<VNode>>, Errno> {
        let state = self.inner.state.lock();
        let mut handles = Vec::new();
        if state.closed || state.committer.is_none() {
            return Ok(handles);
        }
        handles.try_reserve(state.nodes.len()).map_err(|_| Errno(ENOMEM))?;
        for entry in state.nodes.values() {
            preemption_point();
            if let Some(node) = entry.node.upgrade() {
                if entry.mapped.is_some() || node.map.has_dirty() || node.map.eviction_pending() || node.map.writeback_pending() {
                    handles.push(node);
                }
            }
        }
        Ok(handles)
    }
    fn sync_mappings(&self, deadline: Option<u64>) -> Result<(), Errno> {
        if self.inner.state.lock().failed { return Err(Errno(EIO)); }
        let nodes = self.mapped_nodes()?;
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
    fn complete_mapping(&self, node: &VNode, first: u64, last: u64) {
        let state = self.inner.state.lock();
        // A writeback that joined G+1 while G flushed keeps dirty custody.
        if !state.failed && state.pending.is_none()
            && state.nodes.get(&node.ino).is_none_or(|n| n.dirty.bytes() == 0)
        {
            node.map.complete_writeback(first, last);
        }
    }
    fn commit_generation(&self, now_ns: u64) -> Result<bool, Errno> {
        self.commit_generation_up_to(now_ns, u64::MAX, false)
    }
    fn commit_generation_up_to(&self, now_ns: u64, last: u64, force: bool) -> Result<bool, Errno> {
        preemption_point();
        #[cfg(not(test))]
        crate::block::cache_kernel::assert_io_context();
        let _publication;
        let (prepared, reap) = {
            let mut state = self.mutation_lock(CostOp::Commit);
            if state.failed {
                return Err(Errno(EIO));
            }
            if state.committer.is_none() || state.pending.is_some() || state.durable.generation >= last {
                return Ok(false);
            }
            let fsync = self.inner.signal.requested.swap(false, Ordering::AcqRel);
            let pressure = self.inner.signal.pressure.swap(false, Ordering::AcqRel);
            if !state.policy.due(now_ns, state.dirty_bytes, fsync || force, pressure) {
                return Ok(false);
            }
            _publication = self.inner.costs.span(CostOp::Commit, CostPhase::Publication);
            #[cfg(test)]
            measurement::prepared(force, pressure);
            let touched: Vec<_> = state.nodes.keys().map(|&ino| { preemption_point(); ino }).collect();
            for ino in touched {
                preemption_point();
                state.apply_touches(ino)?;
            }
            state.retain_open();
            if let Err(error) = state.drain_dirty(&self.data_reader(None)) {
                state.fail("drain-dirty", error);
                return Err(Errno(EIO));
            }
            // ROOT removal is its own durable operation. The next policy-due
            // generation starts reaping, with at most REAP_PER_COMMIT nodes.
            let mut reap = !core::mem::take(&mut state.skip_reap)
                && !state.closed && !state.reap_shutdown && state.dead_remaining != 0;
            let reaped = (|| -> Result<(), ops::Error> {
                let MountState { files, reservations, grants, durable, .. } = &mut *state;
                let (txn, _) = files.prepare_commit()?;
                if reap {
                    // Reaping is optional: admission refusal changes no trees
                    // and must not discard already acknowledged dirty writes.
                    let grant = if grants.try_reserve(1).is_ok() {
                        match txn.reserve_reap(reservations, durable.free_blocks)
                            .map_err(ops::Error::from)
                        {
                            Ok(grant) => Some(grant),
                            Err(ops::Error::ENOSPC) => None,
                            Err(error) => return Err(error),
                        }
                    } else { None };
                    reap = grant.is_some();
                    if let Some(grant) = grant {
                        grants.push(grant);
                        for _ in 0..super::txn::REAP_PER_COMMIT {
                            preemption_point();
                            if txn.reap_step()?.is_none() { break; }
                        }
                    }
                }
                state.dead_remaining = txn.dead_count();
                Ok(())
            })();
            if let Err(error) = reaped {
                state.fail("prepare-reap", errno(error));
                return Err(errno(error));
            }
            let MountState {
                files, committer, ..
            } = &mut *state;
            let result = committer
                .as_mut()
                .ok_or(Errno(EIO))?
                .prepare_files(self.inner.device.as_ref(), files);
            let prepared = match result {
                Ok(prepared) => Arc::new(prepared),
                Err(_) => {
                    state.fail("prepare-files", Errno(EIO));
                    return Err(Errno(EIO));
                }
            };
            let sb = prepared.superblock();
            let store = self
                .inner
                .cache
                .store(self.inner.device.clone(), sb)
                .with_pending(&prepared);
            match Files::new(store, sb, self.inner.root_id, true) {
                Ok(files) => {
                    state.files = files;
                    state.fresh = true;
                }
                Err(error) => {
                    state
                        .committer
                        .as_mut()
                        .ok_or(Errno(EIO))?
                        .complete(&prepared, Err(super::commit::Error::EIO));
                    state.pending = Some(prepared);
                    state.fail("open-next-generation", errno(error));
                    return Err(Errno(EIO));
                }
            }
            state.inodes.entries.fill(None);
            for node in state.nodes.values_mut() {
                preemption_point();
                node.dirty.clear();
                node.reserved_metadata = 0;
            }
            state.dirty_bytes = 0;
            state.retain_open();
            state.policy = CommitPolicy::new(now_ns);
            state.pending_grants = core::mem::take(&mut state.grants);
            state.metadata_reserved = 0;
            state.pending = Some(prepared.clone());
            (prepared, reap)
        };
        preemption_point();
        let signal = self.inner.signal.clone();
        let group = WriteGroup::hold(
            self.inner.device.as_ref(),
            Arc::new(move || {
                signal.pressure.store(true, Ordering::Release);
                signal.wake();
            }),
        );
        let mut publication = {
            let _wait = self.inner.costs.span(CostOp::Commit, CostPhase::Wait);
            self.inner.mounted_root.device.publication.lock()
        };
        let result = prepared.write_cached(
            self.inner.cache.buffers(),
            self.inner.device.as_ref(),
            &group,
        );
        {
            let mut state = self.mutation_lock(CostOp::Commit);
            state
                .committer
                .as_mut()
                .ok_or(Errno(EIO))?
                .complete(&prepared, result);
            if result.is_ok() {
                #[cfg(test)]
                measurement::published();
                state.durable = prepared.superblock().clone();
                *publication = Some(state.durable.clone());
                for grant in core::mem::take(&mut state.pending_grants) {
                    preemption_point();
                    state.reservations.release(grant);
                }
                self.inner
                    .cache
                    .buffers()
                    .discard_clean(self.inner.data.as_ref());
                state.pending = None;
                if reap && state.dead_remaining != 0 && !state.reap_shutdown {
                    self.inner.signal.requested.store(true, Ordering::Release);
                }
            } else {
                state.fail("publish", Errno(EIO));
            }
        }
        drop(publication);
        preemption_point();
        self.inner.signal.wake();
        result.map(|_| true).map_err(|_| Errno(EIO))
    }
    fn sync_generation(&self, deadline: Option<u64>, closing: bool) -> Result<(), Errno> {
        let waiter = {
            let mut state = self.mutation_lock(CostOp::Fsync);
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
            let Some(committer) = state.committer.as_mut() else {
                return Ok(());
            };
            let waiter = committer.waiter();
            self.inner.signal.requested.store(true, Ordering::Release);
            waiter
        };
        self.inner.signal.wake();
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
                self.mutation_wait(CostOp::Fsync, observed, deadline);
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
            Err(_) => {
                state.scrub_state = SCRUB_FAILED;
                #[cfg(test)]
                signal.started.store(false, Ordering::Release);
                Err(Errno(EIO))
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
                loop {
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
                        if !signal.stop.load(Ordering::Acquire) {
                            signal.wait(observed, Some(deadline));
                        }
                    }
                }
                signal.started.store(false, Ordering::Release);
                signal.wake();
            }, "breenfs-scrub").is_err() {
                let mut state = self.inner.state.lock();
                state.scrub_state = SCRUB_FAILED;
                self.inner.scrub_signal.started.store(false, Ordering::Release);
                self.inner.scrub_signal.wake();
                return Err(Errno(ENOMEM));
            }
        }
        Ok(())
    }
    fn stop_scrub(&self, deadline: u64) -> Result<(), Errno> {
        let signal = &self.inner.scrub_signal;
        signal.stop.store(true, Ordering::Release);
        signal.wake();
        #[cfg(test)]
        self.scrub_pass(super::format::NODE_SIZE)?;
        while signal.started.load(Ordering::Acquire) {
            let observed = signal.epoch();
            if monotonic_ns() >= deadline { return Err(Errno(ETIMEDOUT)); }
            if signal.started.load(Ordering::Acquire) {
                signal.wait(observed, Some(deadline));
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
    fn data_reader(&self, pending: Option<Arc<PreparedCommit>>) -> DataReader<'_, W> {
        DataReader {
            cache: self.inner.cache.buffers(),
            blocks: &self.inner.data,
            pending,
        }
    }
    fn snapshot_request(&self, request: &SnapshotRequest, create: bool) -> Result<(), Errno> {
        let length = request.name_len as usize;
        if !(1..=255).contains(&length)
            || request.flags & !SNAPSHOT_READ_ONLY != 0
            || (!create && request.flags != 0)
        {
            return Err(Errno(EINVAL));
        }
        let name = &request.name[..length];
        if name == b"." || name == b".." || name.contains(&0)
            || name.contains(&b'/') || name.contains(&b',')
        {
            return Err(Errno(EINVAL));
        }
        // Mutation permission precedes any fsync-class work on a read mount.
        self.inner.state.lock().writable()?;
        if create {
            loop {
                self.sync()?;
                let mut state = self.inner.state.lock();
                state.writable()?;
                // A writer may have won admission after the receipt. Close
                // that generation too: sharing a tree already private in G
                // would let subsequent writes change the snapshot's contents.
                if !state.fresh || state.pending.is_some() {
                    drop(state);
                    continue;
                }
                let grant = state.reserve_metadata(&[2], 1, false, false, Operation::Allocate)?;
                let result = state.files.snapshot(name, request.flags & SNAPSHOT_READ_ONLY != 0);
                let id = state.finish_metadata(grant, result)?;
                state.roots.insert(id, (name.to_vec(), request.flags));
                drop(state);
                return self.sync_generation(None, false);
            }
        }
        let _selection = self.inner.mounted_root.device.selection.lock();
        {
            let mut state = self.inner.state.lock();
            state.writable()?;
            let id = state.roots.iter().find(|(_, (root, _))| root == name)
                .map(|(&id, _)| id).ok_or(Errno(ENOENT))?;
            self.check_root_unmounted(id)?;
            let grant = state.reserve_metadata(&[2], 1, false, false, Operation::DeleteSnapshot)?;
            let result = state.files.delete_root(name);
            state.finish_metadata(grant, result)?;
            state.roots.remove(&id);
            state.skip_reap = true;
        }
        self.sync_generation(None, false)
    }
    fn snapshot_status(&self) -> Result<SnapshotStatus, Errno> {
        // Read the current durable SPACE tree even on a frozen reader mounted
        // before the writer's latest snapshot or reaper generation.
        let publication = self.inner.mounted_root.device.publication.lock();
        let state = self.inner.state.lock();
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
impl<W: Wait + 'static> FileSystem for BreenFs<W> {
    fn ioctl(&self, node: &VNode, request: u64, arg: u64) -> Result<u64, Errno> {
        self.check_node(node)?;
        match request {
            SNAPSHOT_CREATE | SNAPSHOT_DELETE => {
                let value = crate::syscall::userptr::copy_from_user(arg as *const SnapshotRequest)
                    .map_err(|error| Errno(error as i32))?;
                self.snapshot_request(&value, request == SNAPSHOT_CREATE)?;
            }
            SNAPSHOT_STATUS => {
                crate::syscall::userptr::copy_to_user(arg as *mut SnapshotStatus, &self.snapshot_status()?)
                    .map_err(|error| Errno(error as i32))?;
            }
            SCRUB_START => self.start_scrub()?,
            SCRUB_CANCEL => {
                self.inner.scrub_signal.stop.store(true, Ordering::Release);
                self.inner.scrub_signal.wake();
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
        let state = self.inner.state.lock();
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
                if let Some(node) = entry.node.upgrade().filter(|node| node.map.nonempty()) {
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
                        let observed = signal.epoch();
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
                            // Retry a deferred eviction allocation at a bounded
                            // interval, even though publication is disabled.
                            (state.failed, if state.failed {
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
                        signal.wait(observed, Some(deadline));
                    }
                    signal.started.store(false, Ordering::Release);
                    signal.wake();
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
        signal.wake();
        #[cfg(test)]
        signal.started.store(false, Ordering::Release);
        while signal.started.load(Ordering::Acquire) {
            let observed = signal.epoch();
            if monotonic_ns() >= deadline {
                return Err(Errno(ETIMEDOUT));
            }
            if signal.started.load(Ordering::Acquire) {
                signal.wait(observed, Some(deadline));
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
            let pending_bytes = state.pending.as_ref().map_or(0, |p| p.dirty_bytes());
            writeln!(
                text,
                "{} breenfs durable_generation={} dirty_bytes={} read_only={} last_scrub_generation={} last_scrub_errors={}",
                name,
                state.durable.generation,
                state.dirty_bytes + pending_bytes,
                u8::from(state.read_only()),
                state.durable.last_scrub_generation,
                state.durable.last_scrub_errors
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
    fn mutation_costs(&self, name: &str, text: &mut alloc::string::String) -> Result<(), Errno> {
        use core::fmt::Write;
        writeln!(text, "{name} breenfs mutation_costs=1").map_err(|_| Errno(ENOMEM))?;
        self.inner.costs.report(self.inner.id, self.inner.root_id, text).map_err(|_| Errno(ENOMEM))
    }
    fn failure(&self) -> Option<(&'static str, i32)> {
        self.inner.state.lock().failure
    }
    fn read_only(&self) -> Result<bool, Errno> {
        Ok(self.inner.state.lock().read_only())
    }
    fn max_file_size(&self) -> Result<u64, Errno> {
        Ok((1 << 63) - 4096)
    }
    fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno> {
        self.inner.state.lock().attrs(ino)
    }
    fn open(&self, ino: u64) -> Result<Arc<VNode>, Errno> {
        self.open_locked(&mut self.inner.state.lock(), ino)
    }
    fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Errno> {
        let mut state = self.inner.state.lock();
        let result = state.files.lookup(dir, name);
        state.observed(result)
    }
    fn lookup_open(&self, dir: &VNode, name: &[u8]) -> Result<Arc<VNode>, Errno> {
        self.check_node(dir)?;
        let mut state = self.inner.state.lock();
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
        let pending = if read {
            &pin.touched.access
        } else {
            &pin.touched.modify
        };
        pending.store(seconds.saturating_add(1), Ordering::Release);
    }
    fn parent(&self, dir: u64) -> Result<u64, Errno> {
        let mut state = self.inner.state.lock();
        state.directory(dir)?;
        Ok(state.inode(dir)?.parent)
    }
    fn readdir(
        &self,
        dir: u64,
        mut cookie: u64,
        sink: &mut dyn FnMut(DirEnt, u64) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        let mut state = self.inner.state.lock();
        let mut outcome = Ok(());
        let result = state.files.visit_directory_entries(dir, cookie, &mut |next, entry| {
            let accepted = record_kind(entry.kind).and_then(|kind| sink(
                DirEnt { ino: entry.ino, kind, name: entry.name }, next,
            ));
            match accepted {
                Ok(true) => { cookie = next; true }
                Ok(false) => false,
                Err(error) => { outcome = Err(error); false }
            }
        });
        state.observed(result)?;
        outcome?;
        Ok(cookie)
    }

    fn read(&self, node: &VNode, off: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        self.check_node(node)?;
        let publication = (!self.inner.writable)
            .then(|| self.inner.mounted_root.device.publication.lock());
        let mut state = self.inner.state.lock();
        if let Some(sb) = publication.as_deref().and_then(Option::as_ref)
            .filter(|sb| sb.generation > state.durable.generation)
        {
            let store = self.inner.cache.store(self.inner.device.clone(), sb);
            let result = Files::new(store, sb, self.inner.root_id, false);
            state.files = state.edited(result)?;
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
            if let Some(page) = state.nodes.get(&node.ino).and_then(|n| n.dirty.get(index)) {
                buf[done..done + take].copy_from_slice(&page[within..within + take]);
                done += take;
            } else {
                // Keep a clean run together so Files resolves each extent's
                // REF once, even when a read spans several blocks.
                let mut end = done + take;
                while end < length {
                    let next = off + end as u64;
                    if state.nodes.get(&node.ino)
                        .is_some_and(|n| n.dirty.get(next / BLOCK_SIZE as u64).is_some())
                    { break; }
                    end += (length - end).min(BLOCK_SIZE);
                }
                let result = state.files.read_verified(
                    &self.data_reader(state.pending.clone()), node.ino, pos,
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
        let mut state = self.inner.state.lock();
        state.inode(ino)?;
        let result = state.files.readlink(ino);
        state.observed(result)
    }
    fn fsync(&self, node: &VNode, _: bool) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Fsync, CostPhase::Operation);
        self.check_node(node)?;
        if self.mutation_lock(CostOp::Fsync).failed {
            return Err(Errno(EIO));
        }
        if node.map.has_dirty() {
            return crate::memory::file_map::sync_range(node, 0, u64::MAX);
        }
        self.sync_generation(None, false)
    }
    fn request_eviction(&self) {
        self.inner.signal.mappings.store(true, Ordering::Release);
        self.inner.signal.wake();
    }
    fn mapping_io(&self, node: &VNode) -> Result<Box<dyn MappingIo>, Errno> {
        self.check_node(node)?;
        let mut state = self.inner.state.lock();
        let size = state.regular(node.ino)?.size;
        let transfer = Mapping {
            fs: self.clone(),
            node: self.open_locked(&mut state, node.ino)?,
            size,
            // A failed writable mount must still flush through sync's EIO path.
            writable: state.committer.is_some(),
        };
        Ok(Box::try_new(transfer).map_err(|_| Errno(ENOMEM))?)
    }
    fn setattr(&self, ino: u64, attrs: SetAttr) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Setattr, CostPhase::Operation);
        let state = self.mutation_lock(CostOp::Setattr);
        state.writable()?;
        let (mut state, grant) = self.reserve_with_pressure(CostOp::Setattr, state, || Ok(self.mutation_lock(CostOp::Setattr)), |state| {
            let current = state.attrs(ino)?;
            if attrs
                .expected
                .is_some_and(|expected| current.attrs != expected)
            {
                return Err(Errno(EAGAIN));
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
        state.finish_metadata(grant, result)
    }
    fn create(&self, dir: u64, name: &[u8], node: NewNode<'_>) -> Result<u64, Errno> {
        let _operation = self.inner.costs.span(CostOp::Create, CostPhase::Operation);
        let state = self.mutation_lock(CostOp::Create);
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
        let (mut state, grant) = self.reserve_with_pressure(CostOp::Create, state, || Ok(self.mutation_lock(CostOp::Create)), |state| {
            state.reserve_metadata(&[dir], 5, true, false, Operation::Allocate)
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
        state.finish_metadata(grant, result)
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
        let state = self.mutation_lock(CostOp::Create);
        state.writable()?;
        let (mut state, (attrs, bits, grant)) = self.reserve_with_pressure(CostOp::Create, state, || Ok(self.mutation_lock(CostOp::Create)), |state| {
            match state.files.lookup(dir, name) {
                Ok(_) => return Err(Errno(EEXIST)),
                Err(ops::Error::ENOENT) => (),
                Err(e) => return state.observed(Err(e)),
            }
            let attrs = check(state.directory(dir)?)?;
            let bits = match kind {
                NodeKind::Regular => 0o100000,
                NodeKind::Socket => 0o140000,
                _ => return Err(Errno(EOPNOTSUPP)),
            };
            let grant = state.reserve_metadata(&[dir], 5, true, false, Operation::Allocate)?;
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
                    prepared = Some(node);
                    Ok(())
                });
                result.map_err(|e| {
                    preparation_error = Some(e);
                    ops::Error::EIO
                })
            },
        );
        if let Some(e) = preparation_error {
            state.reservations.release(grant);
            return Err(e);
        }
        let ino = state.finish_metadata(grant, result)?;
        let node = prepared.ok_or(Errno(EIO))?;
        let entry = state.nodes.entry(ino).or_default();
        entry.node = Arc::downgrade(&node);
        entry.touched = Some(node.pin::<NodePin<W>>().ok_or(Errno(EIO))?.touched.clone());
        state.retain_open();
        Ok(())
    }
    fn link(&self, ino: u64, dir: u64, name: &[u8], check: &LinkCheck<'_>) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Link, CostPhase::Operation);
        let state = self.mutation_lock(CostOp::Link);
        state.writable()?;
        let (mut state, grant) = self.reserve_with_pressure(CostOp::Link, state, || Ok(self.mutation_lock(CostOp::Link)), |state| {
            let parent = state.directory(dir)?;
            let source = state.attrs(ino)?;
            if source.kind == NodeKind::Directory {
                return Err(Errno(EPERM));
            }
            check(parent, source)?;
            let grant = state.reserve_metadata(&[ino, dir], 4, false, false, Operation::Allocate)?;

            Ok(grant)
        })?;
        let _edit = self.inner.costs.span(CostOp::Link, CostPhase::Edit);
        let result = state.files.link(ino, dir, name, now());
        state.finish_metadata(grant, result)
    }
    fn remove(&self, dir: u64, name: &[u8], which: Remove) -> Result<(), Errno> {
        let cost = if which.directory { CostOp::Rmdir } else { CostOp::Unlink };
        let _operation = self.inner.costs.span(cost, CostPhase::Operation);
        let state = self.mutation_lock(cost);
        state.writable()?;
        let (mut state, grant) = self.reserve_with_pressure(cost, state, || Ok(self.mutation_lock(cost)), |state| {
            let parent = state.directory(dir)?;
            let result = state.files.lookup(dir, name);
            let victim = state.observed(result)?;
            let attrs = state.attrs(victim)?;
            if !which.directory && attrs.kind == NodeKind::Directory {
                return Err(Errno(EISDIR));
            }
            if which.expected != victim
                || which.owner != attrs.attrs.uid
                || which.parent != parent.attrs
            {
                return Err(Errno(EAGAIN));
            }
            // Match ext2's held-directory policy before testing emptiness.
            if which.directory
                && attrs.kind == NodeKind::Directory
                && state.nodes.get(&victim).is_some_and(|entry| entry.node.strong_count() != 0)
            {
                return Err(Errno(EBUSY));
            }
            let grant = state.reserve_metadata(
                &[dir, victim], 5, false, !which.directory,
                if which.directory { Operation::Rmdir } else { Operation::Unlink },
            )?;
            Ok(grant)
        })?;
        let _edit = self.inner.costs.span(cost, CostPhase::Edit);
        let result = if which.directory {
            state.files.rmdir(dir, name, now())
        } else {
            state.files.unlink(dir, name, now())
        };
        state.finish_metadata(grant, result)
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
        let state = self.mutation_lock(CostOp::Rename);
        state.writable()?;
        let (mut state, grant) = self.reserve_with_pressure(CostOp::Rename, state, || Ok(self.mutation_lock(CostOp::Rename)), |state| {
            if mode == RenameMode::Exchange {
                return Err(Errno(EINVAL));
            }
            let parent = state.directory(from_dir)?;
            let dest_parent = state.directory(to_dir)?;
            let result = state.files.lookup(from_dir, from);
            let source = state.observed(result)?;
            let source = state.attrs(source)?;
            let dest = match state.files.lookup(to_dir, to) {
                Ok(ino) => Some(state.attrs(ino)?),
                Err(ops::Error::ENOENT) => None,
                Err(e) => return state.observed(Err(e)),
            };
            check(parent, dest_parent, source, dest)?;
            if mode == RenameMode::NoReplace && dest.is_some() {
                return Err(Errno(EEXIST));
            }
            let mut objects = alloc::vec![from_dir, to_dir, source.ino];
            if let Some(dest) = dest { objects.push(dest.ino); }
            let grant = state.reserve_metadata(&objects, 12, false, dest.is_some(), Operation::Allocate)?;

            Ok(grant)
        })?;
        let _edit = self.inner.costs.span(CostOp::Rename, CostPhase::Edit);
        let result = state.files.rename(from_dir, from, to_dir, to, now());
        state.finish_metadata(grant, result)
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
        self.inner.signal.wake();
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
                    return Err(Errno(EFBIG));
                }
                if size == inode.size
                    && (!clear_setid || inode.mode & content_privilege(inode.mode) == 0)
                {
                    return Ok((inode, false, None));
                }
                let floor = size / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
                let mapped_tail = if size < inode.size && size % BLOCK_SIZE as u64 != 0 {
                    let result = state.files.extents(node.ino);
                    state.observed(result)?.iter().any(|(k, e)| {
                        k.offset <= floor && floor < k.offset + u64::from(e.num_blocks) * BLOCK_SIZE as u64
                    })
                } else { false };
                let grant = state.reserve_data(
                    node.ino,
                    floor,
                    usize::from(mapped_tail),
                    if size < inode.size { Operation::Truncate } else { Operation::Allocate },
                )?;
                Ok((inode, mapped_tail, Some(grant)))
            },
        )?;
        let _edit = self.inner.costs.span(CostOp::Resize, CostPhase::Edit);
        let Some(grant) = grant else { return Ok(()); };
        if node.map.prepare_resize(inode.size, size).is_err() {
            state.reservations.release(grant.0);
            return Err(Errno(ENOMEM));
        }
        if size < inode.size {
            let pending = state.pending.clone();
            let MountState {
                files,
                nodes,
                dirty_bytes,
                ..
            } = &mut *state;
            let dirty = &mut nodes.entry(node.ino).or_default().dirty;
            let before = dirty.bytes();
            let mut bad_metadata = None;
            let result = dirty.shrink(size, mapped_tail, |off, page| {
                files.read_verified(&self.data_reader(pending.clone()), node.ino, off, page)
                    .map_err(|error| {
                        if let ReadError::Metadata(error) = error { bad_metadata = Some(error); }
                        errno(error.error())
                    })?;
                Ok(())
            });
            *dirty_bytes = *dirty_bytes - before + dirty.bytes();
            if let Some(error) = bad_metadata {
                state.record_metadata(error);
                if errno(error) == Errno(EIO) { state.fail("resize-read-metadata", Errno(EIO)); }
            }
            state.finish_data(node.ino, grant, result)?;
            let result = state.files.trim_mappings(node.ino, size);
            inode.nblocks = state.edited(result)?;
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
        let result = state.edited(result);
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
                    return Err(Errno(EROFS));
                }
                let inode = state.regular(node.ino)?;
                let length = inode.size.saturating_sub(offset).min(bounce.len() as u64) as usize;
                if length == 0 {
                    return Ok((inode, length, None));
                }
                let grant = state.reserve_data(node.ino, offset, length, Operation::Allocate)?;
                Ok((inode, length, Some(grant)))
            },
        )?;
        let _edit = self.inner.costs.span(CostOp::Writeback, CostPhase::Edit);
        let Some(grant) = grant else { return Ok(()); };
        let pending = state.pending.clone();
        let MountState {
            files,
            nodes,
            dirty_bytes,
            ..
        } = &mut *state;
        let dirty = &mut nodes.entry(node.ino).or_default().dirty;
        let before = dirty.bytes();
        let mut bad_metadata = None;
        let result = dirty.write(offset, &bounce[..length], |off, page| {
            files.read_verified(&self.data_reader(pending.clone()), node.ino, off, page)
                .map_err(|error| {
                    if let ReadError::Metadata(error) = error { bad_metadata = Some(error); }
                    errno(error.error())
                })?;
            Ok(())
        });
        *dirty_bytes += dirty.bytes() - before;
        if let Some(error) = bad_metadata {
            state.record_metadata(error);
            if errno(error) == Errno(EIO) { state.fail("writeback-read-metadata", Errno(EIO)); }
        }
        state.finish_data(node.ino, grant, result)?;
        inode.mtime = now();
        inode.ctime = inode.mtime;
        let result = state.files.put_inode(node.ino, inode);
        state.edited(result)
    }
    fn clear_mapping_privilege(&self, node: &VNode) -> Result<(), Errno> {
        let _operation = self.inner.costs.span(CostOp::Setattr, CostPhase::Operation);
        self.check_node(node)?;
        let mut state = self.mutation_lock(CostOp::Setattr);
        state.writable()?;
        let mut inode = state.regular(node.ino)?;
        if inode.mode & content_privilege(inode.mode) == 0 {
            return Ok(());
        }
        let _edit = self.inner.costs.span(CostOp::Setattr, CostPhase::Edit);
        inode.mode &= !content_privilege(inode.mode);
        inode.ctime = now();
        let result = state.files.put_inode(node.ino, inode);
        state.edited(result)
    }
    fn xattr(&self, ino: u64, op: Xattr<'_>) -> Result<usize, Errno> {
        let mutation = matches!(op, Xattr::Set { .. } | Xattr::Remove { .. });
        let _operation = mutation.then(|| self.inner.costs.span(CostOp::Xattr, CostPhase::Operation));
        let mut state = if mutation { self.mutation_lock(CostOp::Xattr) } else { self.inner.state.lock() };
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
                let (mut state, grant) = self.reserve_with_pressure(CostOp::Xattr, state, || Ok(self.mutation_lock(CostOp::Xattr)), |state| {
                    state.writable()?;
                    let result = state.files.list_xattrs(ino);
                    let records = state.observed(result)?;
                    let exists = records.iter().any(|x| x.name == name);
                    match mode {
                        XattrSet::Create if exists => return Err(Errno(EEXIST)),
                        XattrSet::Replace if !exists => return Err(Errno(ENODATA)),
                        _ => (),
                    }
                    let grant = state.reserve_metadata(&[ino], 2, false, false, Operation::Allocate)?;
                    Ok(grant)
                })?;
                let _edit = self.inner.costs.span(CostOp::Xattr, CostPhase::Edit);
                let result = state.files.xattr(ino, name, Some(value), now());
                state.finish_metadata(grant, result).map(|_| 0)
            }
            Xattr::Remove { name } => {
                let (mut state, grant) = self.reserve_with_pressure(CostOp::Xattr, state, || Ok(self.mutation_lock(CostOp::Xattr)), |state| {
                    state.writable()?;
                    let result = state.files.list_xattrs(ino);
                    let records = state.observed(result)?;
                    if !records.iter().any(|x| x.name == name) {
                        return Err(Errno(ENODATA));
                    }
                    let grant = state.reserve_metadata(&[ino], 2, false, false, Operation::RemoveXattr)?;
                    Ok(grant)
                })?;
                let _edit = self.inner.costs.span(CostOp::Xattr, CostPhase::Edit);
                let result = state.files.xattr(ino, name, None, now());
                state.finish_metadata(grant, result).map(|_| 0)
            }
        }
    }
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
// use the raw device; discard its clean entries before reusing this view.
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
struct DataReader<'a, W: Wait> {
    cache: &'a BufferCache<W>,
    blocks: &'a DataBlocks,
    pending: Option<Arc<PreparedCommit>>,
}
impl<W: Wait> BlockDevice for DataReader<'_, W> {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        // Files verifies the returned block against its extent REF before use.
        if let Some(data) = self.pending.as_ref().and_then(|p| p.data_block(block)) {
            if data.len() != bytes.len() {
                return Err(BlockError::IoError);
            }
            bytes.copy_from_slice(data);
            return Ok(());
        }
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
        self.signal.wake();
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
        self.fs.read(&self.node, offset, &mut bytes)?;
        Ok(bytes)
    }
    fn needs_flush(&self) -> bool {
        self.writable
    }
    fn sync(&self, _first: u64, _last: u64) -> Result<(), Errno> {
        if !self.writable {
            return Ok(());
        }
        // The open generation contains this range's staged writeback (or a
        // successor if it already committed). sync joins its receipt and waits
        // for the final flush, outside the mount lock.
        self.fs.sync_generation(None, false)?;
        self.fs.complete_mapping(&self.node, _first, _last);
        Ok(())
    }
    fn publish(&self, bind: &mut dyn FnMut(bool) -> Result<(), Errno>) -> Result<(), Errno> {
        let mut state = self.fs.inner.state.lock();
        if self.writable {
            state.writable()?;
        }
        bind(false)?;
        if self.writable {
            state.nodes.get_mut(&self.node.ino).ok_or(Errno(EIO))?.mapped = Some(self.node.clone());
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

fn errno(error: ops::Error) -> Errno {
    Errno(match error {
        ops::Error::Metadata(super::btree::ReadFailure {
            cause: super::btree::ReadCause::Allocation, ..
        }) => ENOMEM,
        ops::Error::Metadata(super::btree::ReadFailure {
            cause: super::btree::ReadCause::Driver(failure), ..
        }) if failure.context_refused() => EAGAIN,
        ops::Error::EIO | ops::Error::Metadata(_) => EIO,
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
    })
}
