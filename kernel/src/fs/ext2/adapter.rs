//! Ext2 operations at the filesystem boundary. Credential policy stays in VFS.
use crate::fs::{
    attrs::MountFlags,
    errno::Errno,
    ext2::{
        self,
        errno::errno,
        live_inode::{FileHandle, MountPin},
        read_mount, sync_mount_guard, write_mount, Ext2Fs, Ext2Inode, RenameError,
    },
    vfs::{
        DirEnt, FileSystem, NewNode, NodeAttrs, NodeKind, NodeTimes, Remove, RenameMode, SetAttr,
        StatFs, Timestamp, VNode, WriteAt, WriteError, WriteReq, WriteResult, Xattr,
    },
};
use crate::syscall::errno::*;
use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::Ordering;

pub struct Ext2Adapter {
    mount: MountPin,
}
impl Ext2Adapter {
    pub fn new(mount: MountPin) -> Self {
        Self { mount }
    }

    /// Register the selected root and its generation binds without taking a
    /// backend lock under the mount table. The mounting caller already owns fs.
    pub fn register_root<B>(
        &self,
        fs: &Ext2Fs,
        flags: MountFlags,
        table: &mut crate::fs::vfs::mount_table::MountTable<B>,
        backend: impl Fn() -> B,
    ) -> Result<(), Errno> {
        use crate::fs::vfs::mount_table::Entry;
        let ns = fs.namespace();
        let native = self.mount.mount_id;
        table.insert(Entry::new(
            native,
            b"/".to_vec(),
            "ext2",
            flags,
            backend(),
            ns.root.into(),
            None,
            None,
        ))?;
        for (name, target) in generation_binds(ns) {
            let id = table.allocate_id()?;
            let mut path = Vec::new();
            path.try_reserve_exact(name.len() + 1)
                .map_err(|_| Errno(ENOMEM))?;
            path.push(b'/');
            path.extend_from_slice(name);
            let mut entry = Entry::new(
                id,
                path,
                "ext2",
                flags,
                backend(),
                target.into(),
                Some((native, target.into())),
                Some((native, ns.root.into())),
            );
            entry.bind = Some((native, target.into()));
            table.insert(entry)?;
        }
        Ok(())
    }

    /// Lookup while a legacy namei caller already holds this storage guard.
    /// Keep the namespace translation here until those callers use the trait.
    pub fn lookup_in(&self, fs: &Ext2Fs, dir: u64, name: &[u8]) -> Result<Option<u32>, Errno> {
        component(name)?;
        let (dir, inode) = directory(fs, dir)?;
        let ns = fs.namespace();
        if ns.hidden(dir, name) {
            return Err(Errno(ENOENT));
        }
        match ns.shared(dir, name) {
            Some(ino) => Ok(Some(ino)),
            None => fs.lookup_in_dir_bytes(&inode, name).map_err(errno),
        }
    }

    /// Construct an open from the inode already loaded or being created under
    /// this guard. No second inode lookup may fail after a name is published.
    fn open_loaded(&self, fs: &Ext2Fs, ino: u32, inode: &Ext2Inode) -> Result<Arc<VNode>, Errno> {
        let handle = fs.pin_loaded_inode(ino, inode.size()).map_err(errno)?;
        let attrs = attributes(ino.into(), inode)?;
        let incarnation = handle.object.key.incarnation;
        let pin = Arc::try_new(handle).map_err(|_| Errno(ENOMEM))?;
        Arc::try_new(VNode::new(
            self.mount.mount_id, ino.into(), incarnation, attrs.size, attrs.times, pin,
        )).map_err(|_| Errno(ENOMEM))
    }

    fn pin_node(&self, fs: &Ext2Fs, node: &VNode) -> Result<FileHandle, Errno> {
        if node.mount != self.mount.mount_id {
            return Err(Errno(EXDEV));
        }
        let handle = fs.pin_inode(ext2::ino32(node.ino)?).map_err(errno)?;
        if handle.object.key.incarnation != node.incarnation {
            return Err(Errno(EIO));
        }
        handle.verify(fs).map_err(errno)?;
        Ok(handle)
    }
}

// Temporary guard boundary for callers migrated in 24.4-24.6. The adapter's
// instance pin, rather than a pathname or a root/home choice, selects storage.
#[cfg(not(test))]
impl Ext2Adapter {
    pub fn read(&self) -> Result<ext2::Ext2ReadGuard, Errno> {
        read_mount(self.mount).map_err(errno)
    }
    pub fn write(&self) -> Result<ext2::Ext2WriteGuard, Errno> {
        write_mount(self.mount).map_err(errno)
    }
    pub fn exclusive(&self) -> Result<ext2::Ext2WriteGuard, Errno> {
        sync_mount_guard(self.mount).map_err(errno)
    }
}

/// Bind targets exposed directly below the selected root. Raw partitions
/// retain their own directory tree and need no generation binds.
fn generation_binds(ns: ext2::generation::Namespace) -> impl Iterator<Item = (&'static [u8], u32)> {
    ext2::generation::SHARED_DIRS
        .into_iter()
        .enumerate()
        .map(move |(i, name)| {
            (
                name.as_bytes(),
                if ns.mutable[i] != 0 {
                    ns.mutable[i]
                } else if i == 0 {
                    ns.etc
                } else {
                    0
                },
            )
        })
        .chain([
            (b"root".as_slice(), ns.data),
            (ext2::generation::SHARED.as_bytes(), ns.shared),
        ])
        .filter(move |(_, ino)| ns.root != 2 && *ino != 0)
}

fn component(name: &[u8]) -> Result<(), Errno> {
    if name.len() > 255 {
        return Err(Errno(ENAMETOOLONG));
    }
    if name.is_empty() || name.contains(&b'/') || name.contains(&0) || name == b"." || name == b".."
    {
        return Err(Errno(EINVAL));
    }
    Ok(())
}

fn directory(fs: &Ext2Fs, ino: u64) -> Result<(u32, Ext2Inode), Errno> {
    let ino = ext2::ino32(ino)?;
    let inode = fs.read_inode(ino).map_err(errno)?;
    if !inode.is_dir() {
        return Err(Errno(ENOTDIR));
    }
    if inode.i_links_count == 0 {
        return Err(Errno(ENOENT));
    }
    Ok((ino, inode))
}

fn kind(inode: &Ext2Inode) -> Result<NodeKind, Errno> {
    Ok(match inode.file_type() {
        ext2::FileType::Regular => NodeKind::Regular,
        ext2::FileType::Directory => NodeKind::Directory,
        ext2::FileType::SymLink => NodeKind::Symlink,
        ext2::FileType::CharDevice => NodeKind::CharDevice,
        ext2::FileType::BlockDevice => NodeKind::BlockDevice,
        ext2::FileType::Fifo => NodeKind::Fifo,
        ext2::FileType::Socket => NodeKind::Socket,
        ext2::FileType::Unknown => return Err(Errno(EIO)),
    })
}

fn attributes(ino: u64, inode: &Ext2Inode) -> Result<NodeAttrs, Errno> {
    let time = |sec: u32| Timestamp {
        sec: sec.into(),
        nsec: 0,
    };
    Ok(NodeAttrs {
        ino,
        kind: kind(inode)?,
        attrs: inode.attrs(),
        nlink: inode.i_links_count.into(),
        size: inode.size(),
        blocks: inode.i_blocks.into(),
        rdev: ext2::create_node::node_device(inode),
        times: NodeTimes {
            atime: time(inode.i_atime),
            mtime: time(inode.i_mtime),
            ctime: time(inode.i_ctime),
            btime: None,
        },
    })
}

fn regular_inode(fs: &Ext2Fs, ino: u32) -> Result<Ext2Inode, Errno> {
    let inode = fs.read_inode(ino).map_err(errno)?;
    if inode.is_dir() {
        return Err(Errno(EISDIR));
    }
    if !inode.is_file() {
        return Err(Errno(EINVAL));
    }
    Ok(inode)
}

fn refresh(node: &VNode, inode: &Ext2Inode) -> Result<(), Errno> {
    node.size.store(inode.size(), Ordering::Release);
    *node.times.lock() = attributes(node.ino, inode)?.times;
    Ok(())
}

fn seconds(time: Timestamp) -> Result<u32, Errno> {
    if time.nsec >= 1_000_000_000 {
        return Err(Errno(EINVAL));
    }
    u32::try_from(time.sec).map_err(|_| Errno(EOVERFLOW))
}

fn rename_errno(error: RenameError) -> Errno {
    Errno(match error {
        RenameError::NotFound => ENOENT,
        RenameError::NotDirectory => ENOTDIR,
        RenameError::IsDirectory => EISDIR,
        RenameError::NotEmpty => ENOTEMPTY,
        RenameError::Invalid => EINVAL,
        RenameError::Busy => EBUSY,
        RenameError::NoSpace => ENOSPC,
        RenameError::TooManyLinks => EMLINK,
        RenameError::Io => EIO,
        RenameError::NoMemory => ENOMEM,
    })
}

impl FileSystem for Ext2Adapter {
    fn kind(&self) -> Result<&'static str, Errno> {
        Ok("ext2")
    }

    fn root(&self) -> Result<u64, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        Ok(guard.as_ref().ok_or(Errno(EIO))?.namespace().root.into())
    }

    fn statfs(&self) -> Result<StatFs, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        let stats = fs.statfs();
        Ok(StatFs {
            magic: 0xef53,
            fsid: self.mount.mount_id as u64,
            block_size: stats.block_size,
            blocks: stats.blocks,
            free_blocks: stats.free_blocks,
            // The allocator does not enforce the image's reserved-block count.
            available_blocks: stats.free_blocks,
            inodes: stats.inodes,
            free_inodes: stats.free_inodes,
            max_name_len: 255,
            flags: MountFlags {
                read_only: fs.is_read_only(),
                ..MountFlags::default()
            },
        })
    }

    fn sync(&self) -> Result<(), Errno> {
        let handles = {
            let guard = sync_mount_guard(self.mount).map_err(errno)?;
            guard
                .as_ref()
                .ok_or(Errno(EIO))?
                .dirty_handles()
                .map_err(errno)?
        };
        for handle in &handles {
            crate::memory::file_map::sync_range(handle, 0, u64::MAX).map_err(errno)?;
        }
        let guard = sync_mount_guard(self.mount).map_err(errno)?;
        guard.as_ref().ok_or(Errno(EIO))?.sync().map_err(errno)
    }

    fn read_only(&self) -> Result<bool, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        Ok(guard.as_ref().ok_or(Errno(EIO))?.is_read_only())
    }

    fn max_file_size(&self) -> Result<u64, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        Ok(guard.as_ref().ok_or(Errno(EIO))?.max_file_size())
    }

    fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        attributes(ino, &fs.read_inode(ext2::ino32(ino)?).map_err(errno)?)
    }

    fn setattr(&self, ino: u64, attrs: SetAttr) -> Result<(), Errno> {
        let ino = ext2::ino32(ino)?;
        // Validate before changing pending times or on-disk metadata.
        let atime = attrs.atime.map(seconds).transpose()?;
        let mtime = attrs.mtime.map(seconds).transpose()?;
        let mut guard = write_mount(self.mount)
            .map_err(errno)?
            .wait_for_inode(ino)
            .map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        fs.ensure_writable().map_err(errno)?;
        let mut inode = fs.read_inode(ino).map_err(errno)?;
        if attrs.expected.is_some_and(|expected| expected != inode.attrs()) {
            return Err(Errno(EAGAIN));
        }
        fs.flush_inode_times(ino).map_err(errno)?;
        // Flushing pending times can update the inode's stored timestamps.
        inode = fs.read_inode(ino).map_err(errno)?;
        if let Some(mode) = attrs.mode {
            inode.i_mode = (inode.i_mode & 0o170000) | (mode & 0o7777);
        }
        inode.set_owner(
            attrs.uid.unwrap_or(inode.uid()),
            attrs.gid.unwrap_or(inode.gid()),
        );
        if let Some(time) = atime {
            inode.i_atime = time;
        }
        if let Some(time) = mtime {
            inode.i_mtime = time;
        }
        inode.update_timestamps(false, false, true);
        fs.write_inode(ino, &inode).map_err(errno)
    }

    fn open(&self, ino: u64) -> Result<Arc<VNode>, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        let ino = ext2::ino32(ino)?;
        self.open_loaded(fs, ino, &fs.read_inode(ino).map_err(errno)?)
    }

    fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Errno> {
        component(name)?;
        let guard = read_mount(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        self.lookup_in(fs, dir, name)?
            .map(u64::from)
            .ok_or(Errno(ENOENT))
    }

    fn parent(&self, dir: u64) -> Result<u64, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        let (dir, inode) = directory(fs, dir)?;
        let record = fs.lookup_in_dir_bytes(&inode, b"..").map_err(errno)?
            .ok_or(Errno(EIO))?;
        fs.namespace().entry(dir, b"..", record)
            .map(u64::from).ok_or(Errno(EIO))
    }

    fn readdir(
        &self,
        dir: u64,
        cookie: u64,
        sink: &mut dyn FnMut(DirEnt, u64) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        let (dir, inode) = directory(fs, dir)?;
        let ns = fs.namespace();
        let data = fs.read_directory(&inode).map_err(errno)?;
        let mut next = cookie;
        for record in ext2::DirReader::new(&data).positioned() {
            if (record.offset as u64) < cookie {
                continue;
            }
            if record.entry.is_dot() || record.entry.is_dotdot() {
                next = record.next as u64;
                continue;
            }
            // DirReader's display name may be lossy; recover the record bytes.
            let start = record.offset + 8;
            let len = data[record.offset + 6] as usize;
            let mut name = Vec::new();
            name.try_reserve_exact(len).map_err(|_| Errno(ENOMEM))?;
            name.extend_from_slice(&data[start..start + len]);
            let Some(ino) = ns.entry(dir, &name, record.entry.inode) else {
                next = record.next as u64;
                continue;
            };
            let child = fs.read_inode(ino).map_err(errno)?;
            if !sink(DirEnt {
                ino: ino.into(),
                kind: kind(&child)?,
                name,
            }, record.next as u64)? {
                return Ok(next);
            }
            next = record.next as u64;
        }
        // A shared store has no placeholder in older generation directories.
        // Synthetic cookies occupy a separate range so physical growth cannot
        // repeat a previously accepted bind entry.
        if dir == ns.root {
            for (index, (name, ino)) in generation_binds(ns).enumerate() {
                let position = (1u64 << 62) + index as u64;
                if position < cookie {
                    continue;
                }
                if fs
                    .lookup_in_dir_bytes(&inode, name)
                    .map_err(errno)?
                    .is_some()
                {
                    continue;
                }
                let child = fs.read_inode(ino).map_err(errno)?;
                if !sink(DirEnt {
                    ino: ino.into(),
                    kind: kind(&child)?,
                    name: name.to_vec(),
                }, position + 1)? {
                    return Ok(next);
                }
                next = position + 1;
            }
        }
        Ok(next)
    }

    fn create(&self, dir: u64, name: &[u8], node: NewNode<'_>) -> Result<u64, Errno> {
        component(name)?;
        let mut guard = write_mount(self.mount).map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        fs.ensure_writable().map_err(errno)?;
        let (dir, _) = directory(fs, dir)?;
        let (attrs, kind, dev) = match node {
            NewNode::Directory { attrs } => {
                return fs
                    .create_directory_in(
                        dir,
                        name,
                        (attrs.mode & 0o7777) as u16,
                        attrs.uid,
                        attrs.gid,
                    )
                    .map(u64::from)
                    .map_err(errno)
            }
            NewNode::Symlink { attrs, target } => {
                if target.contains(&0) {
                    return Err(Errno(EINVAL));
                }
                return fs
                    .create_symlink_node_in(
                        target,
                        dir,
                        name,
                        (attrs.mode & 0o7777) as u16,
                        attrs.uid,
                        attrs.gid,
                    )
                    .map(u64::from)
                    .map_err(errno);
            }
            NewNode::Regular { attrs } => (attrs, ext2::EXT2_S_IFREG, 0),
            NewNode::CharDevice { attrs, rdev } => (attrs, ext2::EXT2_S_IFCHR, rdev),
            NewNode::BlockDevice { attrs, rdev } => (attrs, ext2::EXT2_S_IFBLK, rdev),
            NewNode::Fifo { attrs } => (attrs, ext2::EXT2_S_IFIFO, 0),
            NewNode::Socket { attrs } => (attrs, ext2::EXT2_S_IFSOCK, 0),
        };
        let dev = u32::try_from(dev).map_err(|_| Errno(EOVERFLOW))?;
        fs.create_node(
            dir,
            name,
            kind | (attrs.mode & 0o7777) as u16,
            attrs.uid,
            attrs.gid,
            dev,
        )
        .map(u64::from)
        .map_err(errno)
    }

    fn create_open(
        &self,
        dir: u64,
        name: &[u8],
        check: &dyn Fn(NodeAttrs) -> Result<crate::fs::attrs::InodeAttrs, Errno>,
        prepare: &mut dyn FnMut(Arc<VNode>) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        component(name)?;
        let mut guard = write_mount(self.mount).map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        fs.ensure_writable().map_err(errno)?;
        let (dir, parent) = directory(fs, dir)?;
        if self.lookup_in(fs, dir.into(), name)?.is_some() {
            return Err(Errno(EEXIST));
        }
        let attrs = check(attributes(dir.into(), &parent)?)?;
        let mode = ext2::EXT2_S_IFREG | (attrs.mode & 0o7777) as u16;
        let inode = ext2::create_node::node_inode(mode, attrs.uid, attrs.gid, 0);
        let mut preparation_error = None;
        fs.create_node_prepared(dir, name, mode, attrs.uid, attrs.gid, 0, |fs, ino| {
            self.open_loaded(fs, ino, &inode).and_then(prepare).map_err(|error| {
                preparation_error = Some(error);
                // Transport the original typed callback error through ext2's
                // internal string API; it is restored at this boundary below.
                "No memory for ext2 inode object"
            })
        }).map_err(|error| preparation_error.unwrap_or_else(|| errno(error)))
    }

    fn link(&self, ino: u64, dir: u64, name: &[u8], check: &crate::fs::vfs::LinkCheck<'_>) -> Result<(), Errno> {
        component(name)?;
        let mut guard = write_mount(self.mount).map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        let (dir, parent) = directory(fs, dir)?;
        let source = fs.read_inode(ext2::ino32(ino)?).map_err(errno)?;
        check(attributes(dir.into(), &parent)?, attributes(ino, &source)?)?;
        fs.link_in_dir(ext2::ino32(ino)?, dir, name).map_err(errno)
    }

    fn remove(&self, dir: u64, name: &[u8], which: Remove) -> Result<(), Errno> {
        component(name)?;
        // Removal must remain possible without waiting for free allocations.
        let mut guard = sync_mount_guard(self.mount).map_err(errno)?;
        let parent = {
            let fs = guard.as_ref().ok_or(Errno(EIO))?;
            let (ino, _) = directory(fs, dir)?;
            fs.pin_inode(ino).map_err(errno)?
        };
        guard = guard.wait_for_entry_data(&parent, name).map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        let dir = parent.verify(fs).map_err(errno)?;
        let (_, parent_inode) = directory(fs, dir.into())?;
        let victim = self.lookup_in(fs, dir.into(), name)?.ok_or(Errno(ENOENT))?;
        // The visible entry may be a generation bind rather than its raw
        // placeholder. Reject directory unlink before comparing retry inputs.
        if !which.directory && fs.read_inode(victim).map_err(errno)?.is_dir() {
            return Err(Errno(EISDIR));
        }
        if u64::from(victim) != which.expected
            || parent_inode.attrs() != which.parent
            || fs.read_inode(victim).map_err(errno)?.uid() != which.owner
        {
            return Err(Errno(EAGAIN));
        }
        // No guard release between the precondition and the namespace writes.
        if which.directory {
            fs.remove_directory_in(dir, name)
        } else {
            fs.unlink_in_dir(dir, name)
        }
        .map_err(errno)
    }

    fn rename(
        &self,
        from_dir: u64,
        from: &[u8],
        to_dir: u64,
        to: &[u8],
        mode: RenameMode,
        check: &crate::fs::vfs::RenameCheck<'_>,
    ) -> Result<(), Errno> {
        if mode == RenameMode::Exchange {
            return Err(Errno(EINVAL));
        }
        component(from)?;
        component(to)?;
        let mut guard = write_mount(self.mount).map_err(errno)?;
        let (old, new) = {
            let fs = guard.as_ref().ok_or(Errno(EIO))?;
            let (from_dir, _) = directory(fs, from_dir)?;
            let (to_dir, _) = directory(fs, to_dir)?;
            (
                fs.pin_inode(from_dir).map_err(errno)?,
                fs.pin_inode(to_dir).map_err(errno)?,
            )
        };
        guard = guard.wait_for_entry_data(&old, from).map_err(errno)?;
        guard = guard.wait_for_entry_data(&new, to).map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        fs.ensure_writable().map_err(errno)?;
        let (from_dir, parent) = directory(fs, old.verify(fs).map_err(errno)?.into())?;
        let (to_dir, dest_parent) = directory(fs, new.verify(fs).map_err(errno)?.into())?;
        let source = fs.lookup_in_dir_bytes(&parent, from).map_err(errno)?.ok_or(Errno(ENOENT))?;
        let source_attrs = attributes(source.into(), &fs.read_inode(source).map_err(errno)?)?;
        let victim = fs.lookup_in_dir_bytes(&dest_parent, to).map_err(errno)?
            .map(|ino| attributes(ino.into(), &fs.read_inode(ino).map_err(errno)?)).transpose()?;
        check(attributes(from_dir.into(), &parent)?, attributes(to_dir.into(), &dest_parent)?, source_attrs, victim)?;
        if mode == RenameMode::NoReplace {
            let dest = fs
                .read_inode(new.verify(fs).map_err(errno)?)
                .map_err(errno)?;
            if fs.lookup_in_dir_bytes(&dest, to).map_err(errno)?.is_some() {
                return Err(Errno(EEXIST));
            }
        }
        // No guard release between the absence check and the namespace writes.
        fs.rename(&old, from, &new, to, false).map_err(rename_errno)
    }

    fn readlink(&self, ino: u64) -> Result<Vec<u8>, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        guard
            .as_ref()
            .ok_or(Errno(EIO))?
            .read_symlink_bytes(ext2::ino32(ino)?)
            .map_err(errno)
    }

    fn read(&self, node: &VNode, off: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        let handle = self.pin_node(fs, node)?;
        let ino = handle.verify(fs).map_err(errno)?;
        let inode = regular_inode(fs, ino)?;
        let bytes = fs
            .read_file_range_coherent(ino, &inode, off, buf.len())
            .map_err(errno)?;
        refresh(node, &inode)?;
        let update_atime = !bytes.is_empty() && !fs.is_read_only() && inode.needs_atime_update();
        buf[..bytes.len()].copy_from_slice(&bytes);
        drop(guard);
        if update_atime {
            // Reload under the raw write guard:
            // a timestamp update need not finish another inode's suffix work.
            // A completed read keeps its result if metadata I/O fails.
            let _ = (|| -> Result<(), Errno> {
                #[cfg(not(test))]
                let mut guard = ext2::fs_write_raw(ext2::pin_is_home(self.mount).map_err(errno)?)
                    .map_err(errno)?;
                #[cfg(test)]
                let mut guard = write_mount(self.mount).map_err(errno)?;
                let fs = guard.as_mut().ok_or(Errno(EIO))?;
                let ino = handle.verify(fs).map_err(errno)?;
                fs.update_atime(ino).map_err(errno)?;
                refresh(node, &fs.read_inode(ino).map_err(errno)?)
            })();
        }
        Ok(bytes.len())
    }

    fn write(&self, node: &VNode, req: WriteReq, buf: &[u8]) -> Result<WriteResult, WriteError> {
        let ino = ext2::ino32(node.ino)?;
        let mut guard = write_mount(self.mount)
            .map_err(errno)?
            .wait_for_inode(ino)
            .map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        let handle = self.pin_node(fs, node)?;
        fs.ensure_writable().map_err(errno)?;
        let inode = regular_inode(fs, handle.verify(fs).map_err(errno)?)?;
        // Append uses the on-disk size under the same write guard as the write,
        // including when another open VNode extended the file.
        let offset = match req.at {
            WriteAt::Offset(off) => off,
            WriteAt::Append => inode.size(),
        };
        // Match resource::write_length, with the append EOF protected by this
        // guard too. Empty writes leave the inode's privilege bits alone.
        if !buf.is_empty() && req.limit != u64::MAX && offset >= req.limit {
            return Err(WriteError::FileSizeLimit);
        }
        let length = if req.limit == u64::MAX {
            buf.len()
        } else {
            (buf.len() as u64).min(req.limit.saturating_sub(offset)) as usize
        };
        if length != 0
            && offset
                .checked_add(length as u64)
                .is_none_or(|end| end > fs.max_file_size())
        {
            return Err(Errno(EFBIG).into());
        }
        let result = fs
            .write_file_range_as(ino, offset, &buf[..length], req.clear_setid)
            .map(|bytes| WriteResult {
                offset,
                bytes,
                limited: length < buf.len(),
            })
            .map_err(errno).map_err(WriteError::from);
        if let Ok(inode) = fs.read_inode(ino) {
            refresh(node, &inode)?;
        }
        result
    }

    fn resize(&self, node: &VNode, size: u64, clear_setid: bool) -> Result<(), Errno> {
        let ino = ext2::ino32(node.ino)?;
        let mut guard = write_mount(self.mount)
            .map_err(errno)?
            .wait_for_inode(ino)
            .map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        let handle = self.pin_node(fs, node)?;
        fs.ensure_writable().map_err(errno)?;
        regular_inode(fs, handle.verify(fs).map_err(errno)?)?;
        if size > fs.max_file_size() {
            return Err(Errno(EFBIG));
        }
        let ticket = fs.begin_resize_file_as(ino, size, clear_setid).map_err(errno)?;
        if let Some(ticket) = ticket {
            guard = guard.finish_shrink(ticket).map_err(errno)?;
        }
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        refresh(
            node,
            &fs.read_inode(handle.verify(fs).map_err(errno)?)
                .map_err(errno)?,
        )
    }

    fn fsync(&self, node: &VNode, _data_only: bool) -> Result<(), Errno> {
        let guard = sync_mount_guard(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        let handle = self.pin_node(fs, node)?;
        if handle.object.map.has_dirty() {
            // Keep ext2's existing fsync/fdatasync behavior: write dirty shared
            // pages, then flush data and metadata through the pinned transfer.
            drop(guard);
            return crate::memory::file_map::sync_range(&handle, 0, u64::MAX).map_err(errno);
        }
        fs.check_shrink(handle.verify(fs).map_err(errno)?)
            .map_err(errno)?;
        fs.sync().map_err(errno)
    }

    fn writeback_pages(&self, node: &VNode, page: u64, bytes: &[u8]) -> Result<(), Errno> {
        let offset = page.checked_mul(4096).ok_or(Errno(EFBIG))?;
        let handle = {
            let guard = read_mount(self.mount).map_err(errno)?;
            let fs = guard.as_ref().ok_or(Errno(EIO))?;
            self.pin_node(fs, node)?
        };
        let written = ext2::mapped_io::write_mapped_range(&handle, offset, bytes).map_err(errno)?;
        if written != bytes.len() {
            return Err(Errno(EIO));
        }
        Ok(())
    }

    fn xattr(&self, _ino: u64, _op: Xattr<'_>) -> Result<usize, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
}
