//! BreenFS operations at the VFS boundary (design 1.1, 2.3 and 2.5).
use super::{
    commit::Committer,
    format::{Inode, Superblock, Time, BLOCK_SIZE},
    node_cache::CachedNodeStore,
    ops::{self, Files},
    recover,
};
use crate::{
    block::{
        cache::{BufferCache, Wait},
        BlockDevice,
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
    cache: Cache<W>,
    id: usize,
    root_id: u64,
}
enum Cache<W: Wait + 'static> {
    Shared(Arc<BufferCache<W>>),
    Kernel(&'static BufferCache<W>),
}
impl<W: Wait> Cache<W> {
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
    nodes: BTreeMap<u64, Weak<VNode>>,
    inodes: InodeCache,
}
// Bounded metadata cache; mutation completion invalidates its entries.
struct InodeCache {
    entries: [Option<(u64, Inode)>; 32],
    next: usize,
}
impl<W: Wait> MountState<W> {
    fn read_only(&self) -> bool {
        self.failed || self.committer.as_ref().is_none_or(Committer::read_only)
    }
    fn writable(&self) -> Result<(), Errno> {
        if self.read_only() {
            Err(Errno(EROFS))
        } else {
            Ok(())
        }
    }
    fn inode(&mut self, ino: u64) -> Result<Inode, Errno> {
        if let Some((_, inode)) = self
            .inodes
            .entries
            .iter()
            .flatten()
            .find(|(cached, _)| *cached == ino)
        {
            return Ok(*inode);
        }
        let inode = self.files.inode(ino).map_err(errno)?;
        let slot = self.inodes.next;
        self.inodes.entries[slot] = Some((ino, inode));
        self.inodes.next = (slot + 1) % self.inodes.entries.len();
        Ok(inode)
    }
    fn attrs(&mut self, ino: u64) -> Result<NodeAttrs, Errno> {
        attributes(ino, self.inode(ino)?)
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
        self.nodes.retain(|_, node| node.strong_count() != 0);
        self.files
            .retain_open_inodes(self.nodes.keys().copied().collect());
    }
    fn edited<T>(&mut self, result: Result<T, ops::Error>) -> Result<T, Errno> {
        self.inodes.entries.fill(None);
        // A storage/allocation failure may have interrupted a multi-item edit.
        // Do not publish that transaction; reservation is a later milestone.
        if matches!(result, Err(ops::Error::EIO | ops::Error::ENOSPC)) {
            self.failed = true;
        }
        for (&ino, weak) in &self.nodes {
            if let Some(node) = weak.upgrade() {
                if let Ok(inode) = self.files.inode(ino) {
                    node.size.store(inode.size, Ordering::Release);
                    *node.times.lock() = attributes(ino, inode)?.times;
                }
            }
        }
        result.map_err(errno)
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
    /// Commits run synchronously under the mount mutex in this stage.
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
        let inner = Arc::try_new(Mount {
            state: Mutex::new(MountState {
                files,
                committer,
                durable,
                failed: false,
                nodes: BTreeMap::new(),
                inodes: InodeCache {
                    entries: [None; 32],
                    next: 0,
                },
            }),
            device,
            cache,
            id: mount,
            root_id,
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
        if let Some(node) = state.nodes.get(&ino).and_then(Weak::upgrade) {
            return Ok(node);
        }
        let node = self.new_node(ino, inode, state.durable.generation)?;
        state.nodes.insert(ino, Arc::downgrade(&node));
        state.retain_open();
        Ok(node)
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
        let mut state = self.inner.state.lock();
        if state.failed {
            return Err(Errno(EIO));
        }
        if state.committer.is_none() {
            return Ok(());
        }
        state.retain_open();
        let MountState {
            files, committer, ..
        } = &mut *state;
        let result = files.fsync(
            self.inner.device.as_ref(),
            committer.as_mut().ok_or(Errno(EIO))?,
        );
        let sb = match result {
            Ok(sb) => sb,
            Err(_) => {
                state.failed = true;
                return Err(Errno(EIO));
            }
        };
        state.durable = sb;
        let result = Files::new(
            self.inner
                .cache
                .store(self.inner.device.clone(), &state.durable),
            &state.durable,
            self.inner.root_id,
        );
        match result {
            Ok(files) => state.files = files,
            Err(_) => {
                state.failed = true;
                return Err(Errno(EIO));
            }
        }
        state.inodes.entries.fill(None);
        state.retain_open();
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
        if off >= state.regular(node.ino)?.size || buf.is_empty() {
            return Ok(0);
        }
        state
            .files
            .read(self.inner.device.as_ref(), node.ino, off, buf)
            .map_err(errno)
    }
    fn readlink(&self, ino: u64) -> Result<Vec<u8>, Errno> {
        self.inner.state.lock().files.readlink(ino).map_err(errno)
    }
    fn fsync(&self, node: &VNode, _: bool) -> Result<(), Errno> {
        self.check_node(node)?;
        self.sync()
    }
    fn mapping_io(&self, node: &VNode) -> Result<Box<dyn MappingIo>, Errno> {
        self.check_node(node)?;
        let mut state = self.inner.state.lock();
        let size = state.regular(node.ino)?.size;
        let transfer = ReadMapping {
            fs: self.clone(),
            node: self.open_locked(&mut state, node.ino)?,
            size,
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
        state.nodes.insert(ino, Arc::downgrade(&node));
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
    fn write(&self, _: &VNode, _: WriteReq, _: &[u8]) -> Result<WriteResult, WriteError> {
        self.inner.state.lock().writable()?;
        Err(Errno(EOPNOTSUPP).into())
    }
    fn resize(
        &self,
        _: &VNode,
        _: u64,
        _: bool,
        _: &dyn Fn(NodeAttrs) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        self.inner.state.lock().writable()?;
        Err(Errno(EOPNOTSUPP))
    }
    fn writeback_pages(&self, _: &VNode, _: u64, _: &[u8], _: &dyn MappingIo) -> Result<(), Errno> {
        self.inner.state.lock().writable()?;
        Err(Errno(EOPNOTSUPP))
    }
    fn clear_mapping_privilege(&self, _: &VNode) -> Result<(), Errno> {
        self.inner.state.lock().writable()?;
        Err(Errno(EOPNOTSUPP))
    }
    fn xattr(&self, ino: u64, op: Xattr<'_>) -> Result<usize, Errno> {
        let mut state = self.inner.state.lock();
        if state.committer.is_none() {
            return match op {
                Xattr::Set { .. } | Xattr::Remove { .. } => Err(Errno(EROFS)),
                _ => Err(Errno(EOPNOTSUPP)),
            };
        }
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

// The transfer retains a VNode, keeping unlinked extents alive. Content writes
// and writable mappings are not exposed by this namespace-only stage.
struct ReadMapping<W: Wait + 'static> {
    fs: BreenFs<W>,
    node: Arc<VNode>,
    size: u64,
}
impl<W: Wait + 'static> MappingIo for ReadMapping<W> {
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
        false
    }
    fn sync(&self, _: u64, _: u64) -> Result<(), Errno> {
        Ok(())
    }
    fn publish(&self, bind: &mut dyn FnMut(bool) -> Result<(), Errno>) -> Result<(), Errno> {
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
