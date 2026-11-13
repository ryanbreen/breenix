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
        StatFs, Timestamp, VNode, WriteAt, Xattr,
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
            available_blocks: stats.free_blocks.saturating_sub(stats.reserved_blocks),
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
        fs.flush_inode_times(ino).map_err(errno)?;
        let mut inode = fs.read_inode(ino).map_err(errno)?;
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
        let handle = fs.pin_inode(ext2::ino32(ino)?).map_err(errno)?;
        let attrs = attributes(ino, &fs.read_inode(ext2::ino32(ino)?).map_err(errno)?)?;
        Ok(Arc::new(VNode::new(
            self.mount.mount_id,
            ino,
            handle.object.key.incarnation,
            attrs.size,
            attrs.times,
            Arc::new(handle),
        )))
    }

    fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Errno> {
        component(name)?;
        let guard = read_mount(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        let (_, inode) = directory(fs, dir)?;
        fs.lookup_in_dir_bytes(&inode, name)
            .map_err(errno)?
            .map(u64::from)
            .ok_or(Errno(ENOENT))
    }

    fn readdir(
        &self,
        dir: u64,
        cookie: u64,
        sink: &mut dyn FnMut(DirEnt) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        let guard = read_mount(self.mount).map_err(errno)?;
        let fs = guard.as_ref().ok_or(Errno(EIO))?;
        let (_, inode) = directory(fs, dir)?;
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
            let child = fs.read_inode(record.entry.inode).map_err(errno)?;
            if !sink(DirEnt {
                ino: record.entry.inode.into(),
                kind: kind(&child)?,
                name,
            })? {
                break;
            }
            next = record.next as u64;
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

    fn link(&self, ino: u64, dir: u64, name: &[u8]) -> Result<(), Errno> {
        component(name)?;
        let mut guard = write_mount(self.mount).map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        let (dir, _) = directory(fs, dir)?;
        fs.link_in_dir(ext2::ino32(ino)?, dir, name).map_err(errno)
    }

    fn remove(&self, dir: u64, name: &[u8], which: Remove) -> Result<(), Errno> {
        component(name)?;
        let mut guard = write_mount(self.mount).map_err(errno)?;
        let parent = {
            let fs = guard.as_ref().ok_or(Errno(EIO))?;
            let (ino, _) = directory(fs, dir)?;
            fs.pin_inode(ino).map_err(errno)?
        };
        guard = guard.wait_for_entry_data(&parent, name).map_err(errno)?;
        let fs = guard.as_mut().ok_or(Errno(EIO))?;
        let dir = parent.verify(fs).map_err(errno)?;
        match which {
            Remove::Dir => fs.remove_directory_in(dir, name),
            Remove::NonDir => fs.unlink_in_dir(dir, name),
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
        buf[..bytes.len()].copy_from_slice(&bytes);
        refresh(node, &inode)?;
        Ok(bytes.len())
    }

    fn write(&self, node: &VNode, at: WriteAt, buf: &[u8]) -> Result<usize, Errno> {
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
        let offset = match at {
            WriteAt::Offset(off) => off,
            WriteAt::Append => inode.size(),
        };
        if !buf.is_empty()
            && offset
                .checked_add(buf.len() as u64)
                .is_none_or(|end| end > fs.max_file_size())
        {
            return Err(Errno(EFBIG));
        }
        let result = fs.write_file_range(ino, offset, buf).map_err(errno);
        if let Ok(inode) = fs.read_inode(ino) {
            refresh(node, &inode)?;
        }
        result
    }

    fn resize(&self, node: &VNode, size: u64) -> Result<(), Errno> {
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
        // Credential decisions and any set-ID clearing belong to the caller.
        let ticket = fs.begin_resize_file_as(ino, size, false).map_err(errno)?;
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
