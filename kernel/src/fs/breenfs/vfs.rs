//! Read-only breenfs operations at the VFS boundary (design 1.1 and 2.3).
//! The mounted device must remain unchanged for the lifetime of this view.
use super::{
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
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
use spin::Mutex;

/// Immutable durable tree; clones retain one mount and its verified cache view.
pub struct BreenFs<W: Wait + 'static> {
    inner: Arc<Mount<W>>,
}
struct Mount<W: Wait + 'static> {
    files: Files<'static, CachedNodeStore<'static, W>>,
    device: Arc<dyn BlockDevice>,
    durable: Superblock,
    id: usize,
    nodes: Mutex<BTreeMap<u64, Weak<VNode>>>,
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
        let durable = recover::recover(device.as_ref(), false).map_err(|_| Errno(EIO))?;
        let store = CachedNodeStore::new_owned(cache, device.clone(), &durable);
        let files = Files::new(store, &durable, root_id).map_err(errno)?;
        let inner = Arc::try_new(Mount {
            files,
            device,
            durable,
            id: mount,
            nodes: Mutex::new(BTreeMap::new()),
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
}
impl<W: Wait + 'static> FileSystem for BreenFs<W> {
    fn kind(&self) -> Result<&'static str, Errno> {
        Ok("breenfs")
    }
    fn root(&self) -> Result<u64, Errno> {
        Ok(2)
    }
    fn statfs(&self) -> Result<StatFs, Errno> {
        let sb = &self.inner.durable;
        Ok(StatFs {
            magic: u64::from_le_bytes(*b"BREENFSS"),
            block_size: BLOCK_SIZE as u64,
            blocks: sb.total_blocks,
            free_blocks: sb.free_blocks,
            available_blocks: sb.free_blocks.saturating_sub(sb.global_reserve),
            // There is no fixed inode table; numbers are allocated monotonically.
            inodes: 0,
            free_inodes: 0,
            max_name_len: 255,
            flags: MountFlags {
                read_only: true,
                ..MountFlags::default()
            },
        })
    }
    fn sync(&self) -> Result<(), Errno> {
        Ok(())
    }
    fn read_only(&self) -> Result<bool, Errno> {
        Ok(true)
    }
    fn max_file_size(&self) -> Result<u64, Errno> {
        Ok((1 << 63) - 4096)
    }
    fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno> {
        attributes(ino, self.inner.files.inode(ino).map_err(errno)?)
    }
    fn open(&self, ino: u64) -> Result<Arc<VNode>, Errno> {
        let inode = self.inner.files.inode(ino).map_err(errno)?;
        let attrs = attributes(ino, inode)?;
        let mut nodes = self.inner.nodes.lock();
        if let Some(node) = nodes.get(&ino).and_then(Weak::upgrade) {
            return Ok(node);
        }
        let fs = Arc::try_new(self.clone()).map_err(|_| Errno(ENOMEM))?;
        let node = Arc::try_new(VNode::new(
            self.inner.id,
            ino,
            inode.created_generation,
            inode.size,
            attrs.times,
            fs,
            self.inner.durable.generation,
            self.inner.clone(),
        )?)
        .map_err(|_| Errno(ENOMEM))?;
        // A dead weak observer retains no inode, but prune it before adding one
        // so a directory walk/open workload does not grow the table indefinitely.
        nodes.retain(|_, node| node.strong_count() != 0);
        nodes.insert(ino, Arc::downgrade(&node));
        Ok(node)
    }
    fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Errno> {
        self.inner.files.lookup(dir, name).map_err(errno)
    }
    fn lookup_open(&self, dir: &VNode, name: &[u8]) -> Result<Arc<VNode>, Errno> {
        self.check_node(dir)?;
        self.open(self.lookup(dir.ino, name)?)
    }
    fn touch(&self, _: &VNode, _: bool) {}
    fn parent(&self, dir: u64) -> Result<u64, Errno> {
        let inode = self.inner.files.inode(dir).map_err(errno)?;
        if kind(inode.mode)? != NodeKind::Directory {
            return Err(Errno(ENOTDIR));
        }
        Ok(inode.parent)
    }
    fn readdir(
        &self,
        dir: u64,
        mut cookie: u64,
        sink: &mut dyn FnMut(DirEnt, u64) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        loop {
            let entries = self
                .inner
                .files
                .directory_entries(dir, cookie, 64)
                .map_err(errno)?;
            if entries.is_empty() {
                return Ok(cookie);
            }
            for (next, entry) in entries {
                let entry = DirEnt {
                    ino: entry.ino,
                    kind: record_kind(entry.kind)?,
                    name: entry.name,
                };
                if !sink(entry, next)? {
                    return Ok(cookie);
                }
                cookie = next;
            }
        }
    }
    fn read(&self, node: &VNode, off: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        self.check_node(node)?;
        // Initialize the page buffer before reading up to EOF, including its
        // partial mapped block tail (read_conformance::reads_equal).
        buf.fill(0);
        self.inner
            .files
            .read(self.inner.device.as_ref(), node.ino, off, buf)
            .map_err(errno)
    }
    fn readlink(&self, ino: u64) -> Result<Vec<u8>, Errno> {
        self.inner.files.readlink(ino).map_err(errno)
    }
    fn fsync(&self, node: &VNode, _: bool) -> Result<(), Errno> {
        self.check_node(node)
    }
    fn setattr(&self, _: u64, _: SetAttr) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn create(&self, _: u64, _: &[u8], _: NewNode<'_>) -> Result<u64, Errno> {
        Err(Errno(EROFS))
    }
    fn create_open(
        &self,
        _: u64,
        _: &[u8],
        _: NodeKind,
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
    fn rename(
        &self,
        _: u64,
        _: &[u8],
        _: u64,
        _: &[u8],
        _: RenameMode,
        _: &RenameCheck<'_>,
    ) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn write(&self, _: &VNode, _: WriteReq, _: &[u8]) -> Result<WriteResult, WriteError> {
        Err(Errno(EROFS).into())
    }
    fn resize(
        &self,
        _: &VNode,
        _: u64,
        _: bool,
        _: &dyn Fn(NodeAttrs) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn writeback_pages(&self, _: &VNode, _: u64, _: &[u8], _: &dyn MappingIo) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn clear_mapping_privilege(&self, _: &VNode) -> Result<(), Errno> {
        Err(Errno(EROFS))
    }
    fn xattr(&self, _: u64, op: Xattr<'_>) -> Result<usize, Errno> {
        match op {
            Xattr::Set { .. } | Xattr::Remove { .. } => Err(Errno(EROFS)),
            Xattr::Get { .. } | Xattr::List { .. } => Err(Errno(EOPNOTSUPP)),
        }
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
