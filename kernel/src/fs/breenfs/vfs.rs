//! BreenFS operations at the VFS boundary (design 1.1, 2.3 and 2.5).
use super::{
    commit::{Committer, PreparedCommit},
    commit_policy::CommitPolicy,
    commit_signal::{monotonic_ns, CommitSignal},
    dirty::DirtyPages,
    format::{Inode, Superblock, Time, BLOCK_SIZE},
    node_cache::CachedNodeStore,
    ops::{self, Files},
    recover,
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
use core::sync::atomic::Ordering;
use spin::Mutex;

// Extended attributes use ABI errno 61 for an absent attribute.
const ENODATA: i32 = 61;

/// Clones retain the same mount and transaction exclusion.
pub struct BreenFs<W: Wait + 'static> {
    inner: Arc<Mount<W>>,
}
struct Mount<W: Wait + 'static> {
    state: Mutex<MountState<W>>,
    device: Arc<dyn BlockDevice>,
    data: Box<DataBlocks>,
    cache: Cache<W>,
    id: usize,
    root_id: u64,
    signal: Arc<CommitSignal>,
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
    closed: bool,
    nodes: BTreeMap<u64, NodeState>,
    dirty_bytes: u64,
    inodes: InodeCache,
    policy: CommitPolicy,
    pending: Option<Arc<PreparedCommit>>,
    signal: Arc<CommitSignal>,
}
// Backing state for the canonical VNode. Dirty pages outlive its last open
// until commit, without retaining a VNode -> mount ownership cycle.
#[derive(Default)]
struct NodeState {
    node: Weak<VNode>,
    dirty: DirtyPages,
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
    fn inode(&mut self, ino: u64) -> Result<Inode, Errno> {
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
        let inode = self.files.inode(ino).map_err(errno)?;
        let slot = self.inodes.next;
        self.inodes.entries[slot] = Some((ino, inode));
        self.inodes.next = (slot + 1) % self.inodes.entries.len();
        Ok(inode)
    }
    fn attrs(&mut self, ino: u64) -> Result<NodeAttrs, Errno> {
        let mut inode = self.inode(ino)?;
        if let Some(node) = self.nodes.get(&ino).filter(|n| n.dirty.bytes() != 0) {
            let extents = self.files.extents(ino).map_err(errno)?;
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
        self.nodes
            .retain(|_, node| node.node.strong_count() != 0 || node.dirty.bytes() != 0);
        self.files.retain_open_inodes(
            self.nodes
                .iter()
                .filter(|(_, node)| node.node.strong_count() != 0)
                .map(|(&ino, _)| ino)
                .collect(),
        );
    }
    fn edited<T>(&mut self, result: Result<T, ops::Error>) -> Result<T, Errno> {
        self.inodes.entries.fill(None);
        // A storage/allocation failure may have interrupted a multi-item edit.
        // Do not publish that transaction; reservation is a later milestone.
        if matches!(result, Err(ops::Error::EIO | ops::Error::ENOSPC)) {
            self.failed = true;
            if let Some(committer) = self.committer.as_mut() {
                committer.fail();
            }
            self.signal.wake();
        }
        for (&ino, entry) in &self.nodes {
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
    fn drain_dirty(&mut self, device: &dyn BlockDevice) -> Result<(), Errno> {
        for (&ino, node) in &self.nodes {
            if node.dirty.bytes() == 0 {
                continue;
            }
            let mut inode = match self.files.inode(ino) {
                Ok(inode) => inode,
                // An orphan whose last open closed is reclaimed by fsync.
                Err(ops::Error::ENOENT) => continue,
                Err(e) => return Err(errno(e)),
            };
            node.dirty.write_runs(inode.size, |offset, bytes| {
                self.files
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
        Self::mount_with(device, Cache::Shared(cache), mount, root_id, false)
    }
    /// Writable recovery repairs the rings before opening the first transaction.
    /// The caller starts the per-mount worker through init_service().
    pub fn mount_writable(
        device: Arc<dyn BlockDevice>,
        cache: Arc<BufferCache<W>>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Shared(cache), mount, root_id, true)
    }
    /// Kernel read mounts borrow the system cache.
    pub fn mount_cached(
        device: Arc<dyn BlockDevice>,
        cache: &'static BufferCache<W>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Kernel(cache), mount, root_id, false)
    }
    /// Writable kernel mounts borrow the system cache, as read mounts do.
    pub fn mount_writable_cached(
        device: Arc<dyn BlockDevice>,
        cache: &'static BufferCache<W>,
        mount: usize,
        root_id: u64,
    ) -> Result<Arc<Self>, Errno> {
        Self::mount_with(device, Cache::Kernel(cache), mount, root_id, true)
    }
    fn mount_with(
        device: Arc<dyn BlockDevice>,
        cache: Cache<W>,
        mount: usize,
        root_id: u64,
        writable: bool,
    ) -> Result<Arc<Self>, Errno> {
        let durable = recover::recover(device.as_ref(), writable).map_err(|_| Errno(EIO))?;
        let files =
            Files::new(cache.store(device.clone(), &durable), &durable, root_id).map_err(errno)?;
        let committer = writable.then(|| Committer::new(durable.clone()));
        let data = Box::try_new(DataBlocks {
            device: device.clone(),
            blocks: durable.total_blocks,
        })
        .map_err(|_| Errno(ENOMEM))?;
        let signal = Arc::new(CommitSignal::new());
        let inner = Arc::try_new(Mount {
            state: Mutex::new(MountState {
                files,
                committer,
                durable,
                failed: false,
                closed: false,
                nodes: BTreeMap::new(),
                dirty_bytes: 0,
                policy: CommitPolicy::new(monotonic_ns()),
                pending: None,
                signal: signal.clone(),
                inodes: InodeCache {
                    entries: [None; 32],
                    next: 0,
                },
            }),
            device,
            data,
            cache,
            id: mount,
            root_id,
            signal,
        })
        .map_err(|_| Errno(ENOMEM))?;
        Arc::try_new(Self { inner }).map_err(|_| Errno(ENOMEM))
    }
    fn check_node(&self, node: &VNode) -> Result<(), Errno> {
        if node.mount != self.inner.id
            || node
                .pin::<Mount<W>>()
                .is_none_or(|pin| !core::ptr::eq(pin, self.inner.as_ref()))
        {
            return Err(Errno(EXDEV));
        }
        Ok(())
    }
    fn new_node(&self, ino: u64, inode: Inode, epoch: u64) -> Result<Arc<VNode>, Errno> {
        let fs = Arc::try_new(self.clone()).map_err(|_| Errno(ENOMEM))?;
        Arc::try_new(VNode::new(
            self.inner.id,
            ino,
            inode.created_generation,
            inode.size,
            attributes(ino, inode)?.times,
            fs,
            epoch,
            self.inner.clone(),
        )?)
        .map_err(|_| Errno(ENOMEM))
    }
    fn open_locked(&self, state: &mut MountState<W>, ino: u64) -> Result<Arc<VNode>, Errno> {
        let inode = state.inode(ino)?;
        if let Some(node) = state.nodes.get(&ino).and_then(|n| n.node.upgrade()) {
            return Ok(node);
        }
        let node = self.new_node(ino, inode, state.durable.generation)?;
        state.nodes.entry(ino).or_default().node = Arc::downgrade(&node);
        state.retain_open();
        Ok(node)
    }
    /// Resident dirty-page bytes charged to this mount, including closed files
    /// awaiting commit. Available to the mount's commit-pressure policy.
    pub fn dirty_bytes(&self) -> u64 {
        let state = self.inner.state.lock();
        state.dirty_bytes + state.pending.as_ref().map_or(0, |p| p.dirty_bytes())
    }
    /// One commit-worker pass. Preparation and opening G+1 are serialized;
    /// device publication and completion waits hold no mount lock.
    fn commit_due(&self, now_ns: u64) -> Result<bool, Errno> {
        #[cfg(not(test))]
        crate::block::cache_kernel::assert_io_context();
        let prepared = {
            let mut state = self.inner.state.lock();
            if state.failed {
                return Err(Errno(EIO));
            }
            if state.committer.is_none() || state.pending.is_some() {
                return Ok(false);
            }
            let fsync = self.inner.signal.requested.swap(false, Ordering::AcqRel);
            let pressure = self.inner.signal.pressure.swap(false, Ordering::AcqRel);
            if !state.policy.due(now_ns, state.dirty_bytes, fsync, pressure) {
                return Ok(false);
            }
            state.retain_open();
            if state.drain_dirty(&self.data_reader(None)).is_err() {
                state.failed = true;
                if let Some(committer) = state.committer.as_mut() {
                    committer.fail();
                }
                self.inner.signal.wake();
                return Err(Errno(EIO));
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
                    state.failed = true;
                    self.inner.signal.wake();
                    return Err(Errno(EIO));
                }
            };
            let sb = prepared.superblock();
            let store = self
                .inner
                .cache
                .store(self.inner.device.clone(), sb)
                .with_pending(prepared.clone());
            match Files::new(store, sb, self.inner.root_id) {
                Ok(files) => state.files = files,
                Err(_) => {
                    state
                        .committer
                        .as_mut()
                        .ok_or(Errno(EIO))?
                        .complete(&prepared, Err(super::commit::Error::EIO));
                    state.pending = Some(prepared);
                    state.failed = true;
                    self.inner.signal.wake();
                    return Err(Errno(EIO));
                }
            }
            state.inodes.entries.fill(None);
            for node in state.nodes.values_mut() {
                node.dirty.clear();
            }
            state.dirty_bytes = 0;
            state.retain_open();
            state.policy = CommitPolicy::new(now_ns);
            state.pending = Some(prepared.clone());
            prepared
        };
        let signal = self.inner.signal.clone();
        let group = WriteGroup::hold(
            self.inner.device.as_ref(),
            Arc::new(move || {
                signal.pressure.store(true, Ordering::Release);
                signal.wake();
            }),
        );
        let result = prepared.write_cached(
            self.inner.cache.buffers(),
            self.inner.device.as_ref(),
            &group,
        );
        {
            let mut state = self.inner.state.lock();
            state
                .committer
                .as_mut()
                .ok_or(Errno(EIO))?
                .complete(&prepared, result);
            if result.is_ok() {
                state.durable = prepared.superblock().clone();
                self.inner
                    .cache
                    .buffers()
                    .discard_clean(self.inner.data.as_ref());
                state.pending = None;
            } else {
                state.failed = true;
            }
        }
        self.inner.signal.wake();
        result.map(|_| true).map_err(|_| Errno(EIO))
    }
    fn sync_generation(&self, deadline: Option<u64>, closing: bool) -> Result<(), Errno> {
        let waiter = {
            let mut state = self.inner.state.lock();
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
                self.inner.signal.wait(observed, deadline);
            }
        }
    }
    fn data_reader(&self, pending: Option<Arc<PreparedCommit>>) -> DataReader<'_, W> {
        DataReader {
            cache: self.inner.cache.buffers(),
            blocks: &self.inner.data,
            pending,
        }
    }
}
impl<W: Wait + 'static> FileSystem for BreenFs<W> {
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
            available_blocks: sb.free_blocks.saturating_sub(sb.global_reserve),
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
        self.sync_generation(None, false)
    }
    fn init_service(&self) -> Result<(), Errno> {
        {
            let mut state = self.inner.state.lock();
            if state.failed || state.committer.as_ref().is_none_or(Committer::read_only) {
                return Ok(());
            }
            if self.inner.signal.started.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            self.inner.signal.stop.store(false, Ordering::Release);
            state.closed = false;
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
                        let deadline = fs.inner.state.lock().policy.deadline();
                        drop(fs);
                        #[cfg(target_arch = "x86_64")]
                        crate::per_cpu::preempt_enable();
                        #[cfg(target_arch = "aarch64")]
                        crate::per_cpu_aarch64::preempt_enable();
                        if result.is_err() {
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
    fn shutdown(&self, _name: &str, deadline: u64) -> Result<(), Errno> {
        // Close mutation admission before requesting the final generation.
        // The worker remains live until that generation's receipt completes.
        let signal = &self.inner.signal;
        {
            let mut state = self.inner.state.lock();
            if state.closed {
                if signal.stop.load(Ordering::Acquire) && !signal.started.load(Ordering::Acquire) {
                    return if state.failed {
                        Err(Errno(EIO))
                    } else {
                        Ok(())
                    };
                }
                return Err(Errno(EBUSY));
            }
            state.closed = true;
        }
        let result = self.sync_generation(Some(deadline), true);
        if result.is_err() {
            let mut state = self.inner.state.lock();
            state.failed = true;
            if let Some(committer) = state.committer.as_mut() {
                committer.fail();
            }
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
        result
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
                "{} breenfs durable_generation={} dirty_bytes={} read_only={}",
                name,
                state.durable.generation,
                state.dirty_bytes + pending_bytes,
                u8::from(state.read_only())
            )
            .map_err(|_| Errno(ENOMEM))?;
        }
        Ok(())
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
        self.inner
            .state
            .lock()
            .files
            .lookup(dir, name)
            .map_err(errno)
    }
    fn lookup_open(&self, dir: &VNode, name: &[u8]) -> Result<Arc<VNode>, Errno> {
        self.check_node(dir)?;
        let mut state = self.inner.state.lock();
        let ino = state.files.lookup(dir.ino, name).map_err(errno)?;
        self.open_locked(&mut state, ino)
    }
    fn touch(&self, _: &VNode, _: bool) {}
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
        let state = self.inner.state.lock();
        loop {
            let entries = state
                .files
                .directory_entries(dir, cookie, 64)
                .map_err(errno)?;
            if entries.is_empty() {
                return Ok(cookie);
            }
            for (next, entry) in entries {
                if !sink(
                    DirEnt {
                        ino: entry.ino,
                        kind: record_kind(entry.kind)?,
                        name: entry.name,
                    },
                    next,
                )? {
                    return Ok(cookie);
                }
                cookie = next;
            }
        }
    }
    fn read(&self, node: &VNode, off: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        self.check_node(node)?;
        let mut state = self.inner.state.lock();
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
            } else {
                state
                    .files
                    .read(
                        &self.data_reader(state.pending.clone()),
                        node.ino,
                        pos,
                        &mut buf[done..done + take],
                    )
                    .map_err(errno)?;
            }
            done += take;
        }
        Ok(length)
    }
    fn readlink(&self, ino: u64) -> Result<Vec<u8>, Errno> {
        let mut state = self.inner.state.lock();
        state.inode(ino)?;
        state.files.readlink(ino).map_err(errno)
    }
    fn fsync(&self, node: &VNode, _: bool) -> Result<(), Errno> {
        self.check_node(node)?;
        self.sync()
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
        let mut state = self.inner.state.lock();
        state.writable()?;
        let current = state.attrs(ino)?;
        if attrs
            .expected
            .is_some_and(|expected| current.attrs != expected)
        {
            return Err(Errno(EAGAIN));
        }
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
        state.edited(result)
    }
    fn create(&self, dir: u64, name: &[u8], node: NewNode<'_>) -> Result<u64, Errno> {
        let mut state = self.inner.state.lock();
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
        state.edited(result)
    }
    fn create_open(
        &self,
        dir: u64,
        name: &[u8],
        kind: NodeKind,
        check: &dyn Fn(NodeAttrs) -> Result<InodeAttrs, Errno>,
        prepare: &mut dyn FnMut(Arc<VNode>) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        let mut state = self.inner.state.lock();
        state.writable()?;
        match state.files.lookup(dir, name) {
            Ok(_) => return Err(Errno(EEXIST)),
            Err(ops::Error::ENOENT) => (),
            Err(e) => return Err(errno(e)),
        }
        let attrs = check(state.directory(dir)?)?;
        let bits = match kind {
            NodeKind::Regular => 0o100000,
            NodeKind::Socket => 0o140000,
            _ => return Err(Errno(EOPNOTSUPP)),
        };
        let mut prepared = None;
        let mut preparation_error = None;
        let epoch = state.durable.generation;
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
            return Err(e);
        }
        let ino = state.edited(result)?;
        let node = prepared.ok_or(Errno(EIO))?;
        state.nodes.entry(ino).or_default().node = Arc::downgrade(&node);
        state.retain_open();
        Ok(())
    }
    fn link(&self, ino: u64, dir: u64, name: &[u8], check: &LinkCheck<'_>) -> Result<(), Errno> {
        let mut state = self.inner.state.lock();
        state.writable()?;
        let parent = state.directory(dir)?;
        let source = state.attrs(ino)?;
        if source.kind == NodeKind::Directory {
            return Err(Errno(EPERM));
        }
        check(parent, source)?;
        let result = state.files.link(ino, dir, name, now());
        state.edited(result)
    }
    fn remove(&self, dir: u64, name: &[u8], which: Remove) -> Result<(), Errno> {
        let mut state = self.inner.state.lock();
        state.writable()?;
        let parent = state.directory(dir)?;
        let victim = state.files.lookup(dir, name).map_err(errno)?;
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
        let result = if which.directory {
            state.files.rmdir(dir, name, now())
        } else {
            state.files.unlink(dir, name, now())
        };
        state.edited(result)
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
        let mut state = self.inner.state.lock();
        state.writable()?;
        if mode == RenameMode::Exchange {
            return Err(Errno(EINVAL));
        }
        let parent = state.directory(from_dir)?;
        let dest_parent = state.directory(to_dir)?;
        let source = state.files.lookup(from_dir, from).map_err(errno)?;
        let source = state.attrs(source)?;
        let dest = match state.files.lookup(to_dir, to) {
            Ok(ino) => Some(state.attrs(ino)?),
            Err(ops::Error::ENOENT) => None,
            Err(e) => return Err(errno(e)),
        };
        check(parent, dest_parent, source, dest)?;
        if mode == RenameMode::NoReplace && dest.is_some() {
            return Err(Errno(EEXIST));
        }
        let result = state.files.rename(from_dir, from, to_dir, to, now());
        state.edited(result)
    }
    fn write(&self, node: &VNode, req: WriteReq, bytes: &[u8]) -> Result<WriteResult, WriteError> {
        self.check_node(node)?;
        let mut state = self.inner.state.lock();
        state.writable()?;
        let mut inode = state.regular(node.ino)?;
        let offset = match req.at {
            WriteAt::Offset(offset) => offset,
            WriteAt::Append => inode.size,
        };
        if bytes.is_empty() {
            return Ok(WriteResult {
                offset,
                bytes: 0,
                limited: false,
            });
        }
        if req.limit != u64::MAX && offset >= req.limit {
            return Err(WriteError::FileSizeLimit);
        }
        let length = (req.limit - offset).min(bytes.len() as u64) as usize;
        let end = offset
            .checked_add(length as u64)
            .filter(|end| *end <= (1 << 63) - 4096)
            .ok_or(Errno(EFBIG))?;
        let pending = state.pending.clone();
        let MountState {
            files,
            nodes,
            dirty_bytes,
            ..
        } = &mut *state;
        let dirty = &mut nodes.entry(node.ino).or_default().dirty;
        let before = dirty.bytes();
        dirty.write(offset, &bytes[..length], |off, page| {
            files
                .read(&self.data_reader(pending.clone()), node.ino, off, page)
                .map_err(errno)?;
            Ok(())
        })?;
        *dirty_bytes += dirty.bytes() - before;
        inode.size = inode.size.max(end);
        inode.mtime = now();
        inode.ctime = inode.mtime;
        if req.clear_setid {
            inode.mode &= !content_privilege(inode.mode);
        }
        let result = state.files.put_inode(node.ino, inode);
        state.edited(result)?;
        Ok(WriteResult {
            offset,
            bytes: length,
            limited: length < bytes.len(),
        })
    }
    fn resize(
        &self,
        node: &VNode,
        size: u64,
        clear_setid: bool,
        check: &dyn Fn(NodeAttrs) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        self.check_node(node)?;
        let mut state = self.inner.state.lock();
        state.writable()?;
        let mut inode = state.regular(node.ino)?;
        check(state.attrs(node.ino)?)?;
        if size > (1 << 63) - 4096 {
            return Err(Errno(EFBIG));
        }
        if size == inode.size {
            return Ok(());
        }
        if size < inode.size {
            let floor = size / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
            let mapped_tail = state
                .files
                .extents(node.ino)
                .map_err(errno)?
                .iter()
                .any(|(k, e)| {
                    k.offset <= floor
                        && floor < k.offset + u64::from(e.num_blocks) * BLOCK_SIZE as u64
                });
            let pending = state.pending.clone();
            let MountState {
                files,
                nodes,
                dirty_bytes,
                ..
            } = &mut *state;
            let dirty = &mut nodes.entry(node.ino).or_default().dirty;
            let before = dirty.bytes();
            dirty.shrink(size, mapped_tail, |off, page| {
                files
                    .read(&self.data_reader(pending.clone()), node.ino, off, page)
                    .map_err(errno)?;
                Ok(())
            })?;
            *dirty_bytes = *dirty_bytes - before + dirty.bytes();
            let result = state.files.trim_mappings(node.ino, size);
            inode.nblocks = state.edited(result)?;
        }
        inode.size = size;
        inode.mtime = now();
        inode.ctime = inode.mtime;
        if clear_setid {
            inode.mode &= !content_privilege(inode.mode);
        }
        let result = state.files.put_inode(node.ino, inode);
        state.edited(result)
    }
    fn writeback_pages(
        &self,
        node: &VNode,
        page: u64,
        bytes: &[u8],
        io: &dyn MappingIo,
    ) -> Result<(), Errno> {
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
        let mut state = self.inner.state.lock();
        state.writable()?;
        if !mapping.writable {
            return Err(Errno(EROFS));
        }
        let mut inode = state.regular(node.ino)?;
        let length = inode.size.saturating_sub(offset).min(bounce.len() as u64) as usize;
        if length == 0 {
            return Ok(());
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
        dirty.write(offset, &bounce[..length], |off, page| {
            files
                .read(&self.data_reader(pending.clone()), node.ino, off, page)
                .map_err(errno)?;
            Ok(())
        })?;
        *dirty_bytes += dirty.bytes() - before;
        inode.mtime = now();
        inode.ctime = inode.mtime;
        let result = state.files.put_inode(node.ino, inode);
        state.edited(result)
    }
    fn clear_mapping_privilege(&self, node: &VNode) -> Result<(), Errno> {
        self.check_node(node)?;
        let mut state = self.inner.state.lock();
        state.writable()?;
        let mut inode = state.regular(node.ino)?;
        if inode.mode & content_privilege(inode.mode) == 0 {
            return Ok(());
        }
        inode.mode &= !content_privilege(inode.mode);
        inode.ctime = now();
        let result = state.files.put_inode(node.ino, inode);
        state.edited(result)
    }
    fn xattr(&self, ino: u64, op: Xattr<'_>) -> Result<usize, Errno> {
        let mut state = self.inner.state.lock();
        if state.committer.is_none() {
            return match op {
                Xattr::Set { .. } | Xattr::Remove { .. } => Err(Errno(EROFS)),
                _ => Err(Errno(EOPNOTSUPP)),
            };
        }
        state.inode(ino)?;
        let records = state.files.list_xattrs(ino).map_err(errno)?;
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
                state.writable()?;
                let exists = records.iter().any(|x| x.name == name);
                match mode {
                    XattrSet::Create if exists => return Err(Errno(EEXIST)),
                    XattrSet::Replace if !exists => return Err(Errno(ENODATA)),
                    _ => (),
                }
                let result = state.files.xattr(ino, name, Some(value), now());
                state.edited(result).map(|_| 0)
            }
            Xattr::Remove { name } => {
                state.writable()?;
                if !records.iter().any(|x| x.name == name) {
                    return Err(Errno(ENODATA));
                }
                let result = state.files.xattr(ino, name, None, now());
                state.edited(result).map(|_| 0)
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
        // Data checksum enforcement joins this path in the checksum milestone.
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
        self.fs.sync()?;
        #[cfg(not(test))]
        {
            let state = self.fs.inner.state.lock();
            // A later writeback may have joined G+1 during the wait. Leave its
            // candidates dirty unless the inode has no unpublished data.
            if state.pending.is_none()
                && state
                    .nodes
                    .get(&self.node.ino)
                    .is_none_or(|n| n.dirty.bytes() == 0)
            {
                self.node.map.complete_writeback(_first, _last);
            }
        }
        Ok(())
    }
    fn publish(&self, bind: &mut dyn FnMut(bool) -> Result<(), Errno>) -> Result<(), Errno> {
        let state = self.fs.inner.state.lock();
        if self.writable {
            state.writable()?;
        }
        bind(false)
    }
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
fn errno(error: ops::Error) -> Errno {
    Errno(match error {
        ops::Error::EIO => EIO,
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
