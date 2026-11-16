//! Host adapter for the kernel's file operations. The oracle is not used here.
use crate::{block::BlockDevice, btree::NodeStore, commit::Committer, ops::Files, *};
use alloc::sync::Arc;

pub struct Store(Arc<dyn BlockDevice>);
impl NodeStore for Store {
    fn get(&self, addr: u64) -> core::result::Result<Vec<u8>, btree::Error> {
        commit::DeviceStore(self.0.as_ref()).get(addr)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> core::result::Result<(), btree::Error> {
        Err(btree::Error::Store)
    }
    fn alloc(&mut self) -> core::result::Result<u64, btree::Error> {
        Err(btree::Error::Store)
    }
}
pub struct WriterAdapter {
    device: Arc<dyn BlockDevice>,
    files: Option<Files<'static, Store>>,
    committer: Committer,
    failed: bool,
    dirty: bool,
}
fn error(e: ops::Error) -> Error {
    use ops::Error as E;
    let errno = match e {
        E::EIO => Errno::EIO,
        E::EPERM => Errno::EPERM,
        E::ENOTEMPTY => Errno::ENOTEMPTY,
        E::ENOENT => Errno::ENOENT,
        E::EEXIST => Errno::EEXIST,
        E::ENOTDIR => Errno::ENOTDIR,
        E::EISDIR => Errno::EISDIR,
        E::EINVAL => Errno::EINVAL,
        E::EFBIG => Errno::EFBIG,
        E::EMLINK => Errno::EMLINK,
        E::ENAMETOOLONG => Errno::ENAMETOOLONG,
        E::ENOSPC => Errno::ENOSPC,
    };
    errno.into()
}
fn time(t: Time) -> format::Time {
    format::Time {
        seconds: t.seconds,
        nanos: t.nanos,
    }
}
fn logical_time(t: format::Time) -> Time {
    Time {
        seconds: t.seconds,
        nanos: t.nanos,
    }
}
impl WriterAdapter {
    pub fn new(device: Arc<dyn BlockDevice>) -> Result<Self> {
        let sb = recover::recover(device.as_ref(), true).map_err(|_| Errno::EIO)?;
        let files = Files::new(Store(device.clone()), &sb, 1).map_err(error)?;
        Ok(Self {
            device,
            files: Some(files),
            committer: Committer::new(sb),
            failed: false,
            dirty: false,
        })
    }
    fn files(&self) -> Result<&Files<'static, Store>> {
        self.files.as_ref().ok_or(Errno::EIO.into())
    }
    pub fn read(&self, ino: Ino, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.files()?
            .read(self.device.as_ref(), ino, offset, buf)
            .map_err(error)
    }
    fn sync(&mut self) -> Result<()> {
        if self.failed {
            return Err(Errno::EIO.into());
        }
        if !self.dirty {
            return Ok(());
        }
        let sb = self
            .files
            .as_mut()
            .ok_or(Errno::EIO)?
            .fsync(self.device.as_ref(), &mut self.committer)
            .map_err(error)?;
        // Publication may fill child pointer checksums. Discard the old overlay.
        self.files = None;
        self.files = Some(Files::new(Store(self.device.clone()), &sb, 1).map_err(error)?);
        self.dirty = false;
        Ok(())
    }
}
impl Fs for WriterAdapter {
    fn execute(&mut self, op: &Operation) -> Result<Reply> {
        if self.failed {
            return Err(Errno::EIO.into());
        }
        if op.at.nanos >= 1_000_000_000 {
            return Err(Errno::EINVAL.into());
        }
        let at = time(op.at);
        let result = (|| {
            match &op.action {
                Action::Fsync { ino } => {
                    self.files()?.inode(*ino).map_err(error)?;
                    self.sync()?;
                    return Ok(Reply::Done);
                }
                Action::Remount => {
                    self.sync()?;
                    self.files = None; // No cached tree or model state survives remount.
                    let sb =
                        recover::recover(self.device.as_ref(), true).map_err(|_| Errno::EIO)?;
                    self.files =
                        Some(Files::new(Store(self.device.clone()), &sb, 1).map_err(error)?);
                    self.committer = Committer::new(sb);
                    return Ok(Reply::Done);
                }
                _ => (),
            }
            let files = self.files.as_mut().ok_or(Errno::EIO)?;
            let reply = match &op.action {
                Action::Create { dir, name, mode } => {
                    Reply::Inode(files.create(*dir, name, *mode, at).map_err(error)?)
                }
                Action::Write { ino, offset, bytes } => Reply::Written(
                    files
                        .write(self.device.as_ref(), *ino, *offset, bytes, at)
                        .map_err(error)?,
                ),
                Action::Truncate { ino, size } => {
                    files
                        .truncate(self.device.as_ref(), *ino, *size, at)
                        .map_err(error)?;
                    Reply::Done
                }
                Action::Setattr { ino, attrs } => {
                    files
                        .setattr(
                            *ino,
                            &ops::SetAttr {
                                mode: attrs.mode,
                                uid: attrs.uid,
                                gid: attrs.gid,
                                atime: attrs.atime.map(time),
                                mtime: attrs.mtime.map(time),
                            },
                            at,
                        )
                        .map_err(error)?;
                    Reply::Done
                }
                Action::Setxattr { ino, name, value } => {
                    files.xattr(*ino, name, Some(value), at).map_err(error)?;
                    Reply::Done
                }
                Action::Removexattr { ino, name } => {
                    files.xattr(*ino, name, None, at).map_err(error)?;
                    Reply::Done
                }
                Action::Mkdir { dir, name, mode } => {
                    Reply::Inode(files.mkdir(*dir, name, *mode, at).map_err(error)?)
                }
                Action::Symlink { dir, name, target } => {
                    Reply::Inode(files.symlink(*dir, name, target, at).map_err(error)?)
                }
                Action::Link { ino, dir, name } => {
                    files.link(*ino, *dir, name, at).map_err(error)?;
                    Reply::Done
                }
                Action::Unlink { dir, name } => {
                    files.unlink(*dir, name, at).map_err(error)?;
                    Reply::Done
                }
                Action::Rmdir { dir, name } => {
                    files.rmdir(*dir, name, at).map_err(error)?;
                    Reply::Done
                }
                Action::Rename {
                    from_dir,
                    from,
                    to_dir,
                    to,
                } => {
                    files
                        .rename(*from_dir, from, *to_dir, to, at)
                        .map_err(error)?;
                    Reply::Done
                }
                Action::Fsync { .. } | Action::Remount => unreachable!("handled before mutation"),
            };
            self.dirty = true;
            Ok(reply)
        })();
        if matches!(result, Err(Error::Errno(Errno::EIO | Errno::ENOSPC))) {
            // txn.rs poisons a failed edit; no later operation may acknowledge it.
            self.failed = true;
            self.files = None;
        }
        result
    }
    fn state(&self) -> Result<State> {
        let files = self.files()?;
        let items = files.items().map_err(error)?;
        let mut state = State::default();
        for (k, v) in items.iter().filter(|(k, _)| k.item_type == format::INODE) {
            let i = format::Inode::decode(v).map_err(|_| Errno::EIO)?;
            // Unlinked objects await commit-time reclamation.
            if i.nlink == 0 {
                continue;
            }
            let contents = match i.mode & 0o170000 {
                0o040000 => {
                    let mut entries = BTreeMap::new();
                    for (_, v) in items
                        .iter()
                        .filter(|(d, _)| d.object == k.object && d.item_type == format::DIR_INDEX)
                    {
                        for r in ops::dir_records(v).map_err(error)? {
                            entries.insert(r.name, r.ino);
                        }
                    }
                    Contents::Directory {
                        parent: i.parent,
                        entries,
                    }
                }
                0o100000 => {
                    let mut bytes = BTreeMap::new();
                    for (e, v) in items
                        .iter()
                        .filter(|(e, _)| e.object == k.object && e.item_type == format::EXTENT)
                    {
                        let extent = format::Extent::decode(v).map_err(|_| Errno::EIO)?;
                        for n in 0..extent.num_blocks {
                            let off = e.offset + u64::from(n) * format::BLOCK_SIZE as u64;
                            let mut block = alloc::vec![0;format::BLOCK_SIZE];
                            let count = files
                                .read(self.device.as_ref(), k.object, off, &mut block)
                                .map_err(error)?;
                            for (index, b) in block[..count]
                                .iter()
                                .copied()
                                .enumerate()
                                .filter(|(_, b)| *b != 0)
                            {
                                bytes.insert(off + index as u64, b);
                            }
                        }
                    }
                    Contents::File {
                        size: i.size,
                        bytes,
                    }
                }
                0o120000 => Contents::Symlink(files.readlink(k.object).map_err(error)?),
                _ => return Err(Error::Unimplemented),
            };
            let mut xattrs = BTreeMap::new();
            for (_, v) in items
                .iter()
                .filter(|(x, _)| x.object == k.object && x.item_type == format::XATTR)
            {
                for x in ops::xattrs(v).map_err(error)? {
                    xattrs.insert(x.name, x.value);
                }
            }
            state.inodes.insert(
                k.object,
                Inode {
                    metadata: Metadata {
                        mode: i.mode,
                        uid: i.uid,
                        gid: i.gid,
                        nlink: i.nlink,
                        atime: logical_time(i.atime),
                        mtime: logical_time(i.mtime),
                        ctime: logical_time(i.ctime),
                        btime: logical_time(i.btime),
                    },
                    contents,
                    xattrs,
                },
            );
        }
        Ok(state)
    }
}
