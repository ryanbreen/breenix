//! Ext2's Mount, Inode and Names groups at the filesystem boundary.
//! Data and xattrs are explicitly unsupported until their migration units.
use crate::fs::{
    attrs::MountFlags,
    errno::Errno,
    ext2::{
        self, errno::errno, live_inode::MountPin, read_mount, write_mount, Ext2Fs, Ext2Inode,
        RenameError,
    },
    vfs::{
        DirEnt, FileSystem, NewNode, NodeAttrs, NodeKind, NodeTimes, Remove, RenameMode, SetAttr,
        StatFs, Timestamp, VNode,
    },
};
use crate::syscall::errno::*;
use alloc::{sync::Arc, vec::Vec};

pub struct Ext2Adapter {
    mount: MountPin,
}
impl Ext2Adapter {
    pub fn new(mount: MountPin) -> Self {
        Self { mount }
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
        let guard = read_mount(self.mount).map_err(errno)?;
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
}
