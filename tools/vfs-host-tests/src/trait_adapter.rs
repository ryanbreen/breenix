//! Translate the 24.8 model interface to production FileSystem calls.
use crate::{
    attrs::InodeAttrs,
    block::{cache::BufferCache, BlockDevice, BlockError},
    breenfs::{commit::DeviceStore, format, ops::Files, recover, vfs::BreenFs},
    errno::Errno as KernelErrno,
    read_conformance::NoWait,
    syscall_errno as errno,
    vfs::{self, FileSystem},
};
use breenfs_host_tests::{recording::RecordingDevice, *};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// The two host crates compile the same kernel block interface independently.
/// This bridge only converts their error types; I/O goes to the 24.8 recorder.
struct Device(Arc<RecordingDevice>);
fn block_error(error: breenfs_host_tests::block::BlockError) -> BlockError {
    use breenfs_host_tests::block::BlockError as E;
    match error {
        E::IoError => BlockError::IoError,
        E::OutOfMemory => BlockError::OutOfMemory,
        E::OutOfBounds => BlockError::OutOfBounds,
    }
}
impl BlockDevice for Device {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> core::result::Result<(), BlockError> {
        breenfs_host_tests::block::BlockDevice::read_block(self.0.as_ref(), block, bytes)
            .map_err(block_error)
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> core::result::Result<(), BlockError> {
        breenfs_host_tests::block::BlockDevice::write_block(self.0.as_ref(), block, bytes)
            .map_err(block_error)
    }
    fn block_size(&self) -> usize {
        512
    }
    fn num_blocks(&self) -> u64 {
        breenfs_host_tests::block::BlockDevice::num_blocks(self.0.as_ref())
    }
    fn flush(&self) -> core::result::Result<(), BlockError> {
        breenfs_host_tests::block::BlockDevice::flush(self.0.as_ref()).map_err(block_error)
    }
}
fn error(error: KernelErrno) -> Error {
    let value = match error.0 {
        errno::EPERM => Errno::EPERM,
        errno::ENOENT => Errno::ENOENT,
        61 => Errno::ENODATA,
        errno::EEXIST => Errno::EEXIST,
        errno::ENOTDIR => Errno::ENOTDIR,
        errno::EISDIR => Errno::EISDIR,
        errno::EINVAL => Errno::EINVAL,
        errno::EFBIG => Errno::EFBIG,
        errno::EMLINK => Errno::EMLINK,
        errno::ENAMETOOLONG => Errno::ENAMETOOLONG,
        errno::ENOTEMPTY => Errno::ENOTEMPTY,
        errno::EIO => Errno::EIO,
        errno::ENOSPC => Errno::ENOSPC,
        other => panic!("unmodeled trait errno {other}"),
    };
    value.into()
}
fn timestamp(t: Time) -> vfs::Timestamp {
    vfs::Timestamp {
        sec: t.seconds,
        nsec: t.nanos,
    }
}
fn time(t: vfs::Timestamp) -> Time {
    Time {
        seconds: t.sec,
        nanos: t.nsec,
    }
}

pub(crate) struct TraitAdapter {
    device: Arc<Device>,
    mounted: Option<Arc<dyn FileSystem>>,
}
impl TraitAdapter {
    pub(crate) fn new(device: Arc<RecordingDevice>) -> Self {
        let device = Arc::new(Device(device));
        let mounted = Some(Self::mount(device.clone()));
        Self { device, mounted }
    }
    fn mount(device: Arc<Device>) -> Arc<dyn FileSystem> {
        BreenFs::mount_writable(device, Arc::new(BufferCache::new(NoWait)), 71, 1).unwrap()
    }
    fn fs(&self) -> &dyn FileSystem {
        self.mounted.as_ref().unwrap().as_ref()
    }
    fn remove(
        &self,
        dir: u64,
        name: &[u8],
        directory: bool,
    ) -> core::result::Result<(), KernelErrno> {
        let fs = self.fs();
        let ino = fs.lookup(dir, name)?;
        fs.remove(
            dir,
            name,
            vfs::Remove {
                directory,
                expected: ino,
                owner: fs.getattr(ino)?.attrs.uid,
                parent: fs.getattr(dir)?.attrs,
            },
        )
    }
    fn apply(&mut self, op: &Operation) -> core::result::Result<Reply, KernelErrno> {
        let fs = self.fs();
        let attrs = |mode| InodeAttrs::new(mode, 0, 0);
        Ok(match &op.action {
            Action::Create { dir, name, mode } => Reply::Inode(fs.create(
                *dir,
                name,
                vfs::NewNode::Regular {
                    attrs: attrs(*mode),
                },
            )?),
            Action::Mkdir { dir, name, mode } => Reply::Inode(fs.create(
                *dir,
                name,
                vfs::NewNode::Directory {
                    attrs: attrs(*mode),
                },
            )?),
            Action::Symlink { dir, name, target } => Reply::Inode(fs.create(
                *dir,
                name,
                vfs::NewNode::Symlink {
                    attrs: attrs(0o777),
                    target,
                },
            )?),
            Action::Write { ino, offset, bytes } => {
                let node = fs.open(*ino)?;
                Reply::Written(
                    fs.write(
                        &node,
                        vfs::WriteReq {
                            at: vfs::WriteAt::Offset(*offset),
                            limit: u64::MAX,
                            clear_setid: false,
                        },
                        bytes,
                    )
                    .map_err(vfs::WriteError::errno)?
                    .bytes,
                )
            }
            Action::Truncate { ino, size } => {
                fs.resize(fs.open(*ino)?.as_ref(), *size, false, &|_| Ok(()))?;
                Reply::Done
            }
            Action::Link { ino, dir, name } => {
                fs.link(*ino, *dir, name, &|_, _| Ok(()))?;
                Reply::Done
            }
            Action::Unlink { dir, name } => {
                self.remove(*dir, name, false)?;
                Reply::Done
            }
            Action::Rmdir { dir, name } => {
                self.remove(*dir, name, true)?;
                Reply::Done
            }
            Action::Rename {
                from_dir,
                from,
                to_dir,
                to,
            } => {
                fs.rename(
                    *from_dir,
                    from,
                    *to_dir,
                    to,
                    vfs::RenameMode::Replace,
                    &|_, _, _, _| Ok(()),
                )?;
                Reply::Done
            }
            Action::Setattr { ino, attrs } => {
                fs.setattr(
                    *ino,
                    vfs::SetAttr {
                        expected: None,
                        mode: attrs.mode.map(|m| m as u16),
                        uid: attrs.uid,
                        gid: attrs.gid,
                        atime: attrs.atime.map(timestamp),
                        mtime: attrs.mtime.map(timestamp),
                    },
                )?;
                Reply::Done
            }
            Action::Setxattr { ino, name, value } => {
                fs.xattr(
                    *ino,
                    vfs::Xattr::Set {
                        name,
                        value,
                        mode: vfs::XattrSet::Upsert,
                    },
                )?;
                Reply::Done
            }
            Action::Removexattr { ino, name } => {
                fs.xattr(*ino, vfs::Xattr::Remove { name })?;
                Reply::Done
            }
            Action::Fsync { ino } => {
                fs.fsync(fs.open(*ino)?.as_ref(), false)?;
                Reply::Done
            }
            Action::Remount => {
                fs.sync()?;
                self.mounted = None; // Drop VNodes, mount state and buffer cache.
                self.mounted = Some(Self::mount(self.device.clone()));
                Reply::Done
            }
        })
    }
}
impl Fs for TraitAdapter {
    fn execute(&mut self, op: &Operation) -> Result<Reply> {
        if op.at.nanos >= 1_000_000_000 {
            return Err(Errno::EINVAL.into());
        }
        crate::time::with_unix_time(op.at.seconds as u64, || self.apply(op)).map_err(error)
    }
    fn state(&self) -> Result<State> {
        let fs = self.fs();
        // Only called after remount. Inspect allocation keys to bound sparse
        // reads (streams include 4-GiB holes); contents and metadata are read through
        // FileSystem, independently of the model and WriterAdapter.
        let sb = recover::recover(self.device.as_ref(), false).map_err(|_| Errno::EIO)?;
        let files =
            Files::new(DeviceStore(self.device.as_ref()), &sb, 1).map_err(|_| Errno::EIO)?;
        let items = files.items().map_err(|_| Errno::EIO)?;
        let mut state = State::default();
        for (key, _) in items
            .iter()
            .filter(|(key, _)| key.item_type == format::INODE)
        {
            let ino = key.object;
            let attrs = fs.getattr(ino).map_err(error)?;
            if attrs.nlink == 0 {
                continue;
            }
            let contents = match attrs.kind {
                vfs::NodeKind::Directory => {
                    let mut entries = BTreeMap::new();
                    fs.readdir(ino, 0, &mut |entry, _| {
                        assert!(entries.insert(entry.name, entry.ino).is_none());
                        Ok(true)
                    })
                    .map_err(error)?;
                    for (name, child) in &entries {
                        assert_eq!(fs.lookup(ino, name).map_err(error)?, *child);
                    }
                    Contents::Directory {
                        parent: fs.parent(ino).map_err(error)?,
                        entries,
                    }
                }
                vfs::NodeKind::Symlink => Contents::Symlink(fs.readlink(ino).map_err(error)?),
                vfs::NodeKind::Regular => {
                    let node = fs.open(ino).map_err(error)?;
                    let mut pages = BTreeSet::new();
                    for (k, value) in items
                        .iter()
                        .filter(|(k, _)| k.object == ino && k.item_type == format::EXTENT)
                    {
                        let extent = format::Extent::decode(value).map_err(|_| Errno::EIO)?;
                        let first = k.offset / 4096;
                        let end = first + u64::from(extent.num_blocks);
                        pages.extend(first..end);
                        if first != 0 {
                            pages.insert(first - 1);
                        }
                        pages.insert(end);
                    }
                    pages.insert(0);
                    pages.insert(attrs.size / 4096);
                    if attrs.size != 0 {
                        pages.insert((attrs.size - 1) / 4096);
                    }
                    let mut bytes = BTreeMap::new();
                    for page in pages {
                        let offset = page * 4096;
                        let mut buf = [0xa5; 4096];
                        let count = fs.read(&node, offset, &mut buf).map_err(error)?;
                        assert_eq!(count as u64, attrs.size.saturating_sub(offset).min(4096));
                        assert!(buf[count..].iter().all(|b| *b == 0), "EOF tail ino={ino}");
                        for (index, value) in buf[..count]
                            .iter()
                            .copied()
                            .enumerate()
                            .filter(|(_, b)| *b != 0)
                        {
                            bytes.insert(offset + index as u64, value);
                        }
                    }
                    Contents::File {
                        size: attrs.size,
                        bytes,
                    }
                }
                _ => return Err(Error::Unimplemented),
            };
            let mut names = vec![
                0;
                fs.xattr(ino, vfs::Xattr::List { names: &mut [] })
                    .map_err(error)?
            ];
            assert_eq!(
                fs.xattr(ino, vfs::Xattr::List { names: &mut names })
                    .map_err(error)?,
                names.len()
            );
            let mut xattrs = BTreeMap::new();
            for name in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
                let mut value = vec![
                    0;
                    fs.xattr(
                        ino,
                        vfs::Xattr::Get {
                            name,
                            value: &mut []
                        }
                    )
                    .map_err(error)?
                ];
                assert_eq!(
                    fs.xattr(
                        ino,
                        vfs::Xattr::Get {
                            name,
                            value: &mut value
                        }
                    )
                    .map_err(error)?,
                    value.len()
                );
                assert!(xattrs.insert(name.to_vec(), value).is_none());
            }
            state.inodes.insert(
                ino,
                Inode {
                    metadata: Metadata {
                        mode: attrs.attrs.mode,
                        uid: attrs.attrs.uid,
                        gid: attrs.attrs.gid,
                        nlink: attrs.nlink.try_into().unwrap(),
                        atime: time(attrs.times.atime),
                        mtime: time(attrs.times.mtime),
                        ctime: time(attrs.times.ctime),
                        btime: time(attrs.times.btime.unwrap()),
                    },
                    contents,
                    xattrs,
                },
            );
        }
        Ok(state)
    }
}
