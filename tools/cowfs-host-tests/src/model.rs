//! Independent inode-relative POSIX.1-2017 oracle, with cowfs durability from
//! docs/fs/cowfs-design.md 2.5. Credentials and path traversal stay in the VFS.
//! Xattrs are the design's extension: missing removal is ENOENT, invalid limits
//! are EINVAL. Remount means a clean unmount/mount, not a simulated crash.
use crate::*;

pub struct Model {
    live: State,
    durable: State,
    next_inode: Ino,
}
impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}
impl Model {
    pub fn new() -> Self {
        let mut live = State::default();
        live.inodes.insert(
            ROOT,
            Inode {
                metadata: Metadata {
                    mode: 0o040755,
                    uid: 0,
                    gid: 0,
                    nlink: 2,
                    atime: Time::default(),
                    mtime: Time::default(),
                    ctime: Time::default(),
                    btime: Time::default(),
                },
                contents: Contents::Directory {
                    parent: ROOT,
                    entries: BTreeMap::new(),
                },
                xattrs: BTreeMap::new(),
            },
        );
        Self {
            durable: live.clone(),
            live,
            next_inode: 16,
        }
    }

    pub fn durable_state(&self) -> &State {
        &self.durable
    }

    fn inode(&self, ino: Ino) -> Result<&Inode> {
        self.live.inodes.get(&ino).ok_or(Errno::ENOENT.into())
    }
    fn inode_mut(&mut self, ino: Ino) -> &mut Inode {
        self.live.inodes.get_mut(&ino).expect("validated inode")
    }
    fn entries(&self, dir: Ino) -> Result<&BTreeMap<Vec<u8>, Ino>> {
        match &self.inode(dir)?.contents {
            Contents::Directory { entries, .. } => Ok(entries),
            _ => Err(Errno::ENOTDIR.into()),
        }
    }
    fn entries_mut(&mut self, dir: Ino) -> &mut BTreeMap<Vec<u8>, Ino> {
        match &mut self.inode_mut(dir).contents {
            Contents::Directory { entries, .. } => entries,
            _ => unreachable!("validated directory"),
        }
    }
    fn name(name: &[u8]) -> Result<()> {
        if name.is_empty() {
            return Err(Errno::ENOENT.into());
        }
        if name.len() > 255 {
            return Err(Errno::ENAMETOOLONG.into());
        }
        if name == b"." || name == b".." || name.contains(&0) || name.contains(&b'/') {
            return Err(Errno::EINVAL.into());
        }
        Ok(())
    }
    fn vacant(&self, dir: Ino, name: &[u8]) -> Result<()> {
        Self::name(name)?;
        if self.entries(dir)?.contains_key(name) {
            return Err(Errno::EEXIST.into());
        }
        Ok(())
    }
    pub fn lookup(&self, dir: Ino, name: &[u8]) -> Result<Ino> {
        Self::name(name)?;
        self.entries(dir)?
            .get(name)
            .copied()
            .ok_or(Errno::ENOENT.into())
    }
    fn changed(&mut self, ino: Ino, at: Time, data: bool) {
        let meta = &mut self.inode_mut(ino).metadata;
        meta.ctime = at;
        if data {
            meta.mtime = at;
        }
    }
    fn new_inode(
        &mut self,
        dir: Ino,
        name: &[u8],
        mode: u32,
        contents: Contents,
        at: Time,
    ) -> Result<Ino> {
        self.vacant(dir, name)?;
        let directory = matches!(contents, Contents::Directory { .. });
        if directory && self.inode(dir)?.metadata.nlink == u32::MAX {
            return Err(Errno::EMLINK.into());
        }
        let ino = self.next_inode;
        let next = ino.checked_add(1).ok_or(Error::Errno(Errno::EMLINK))?;
        self.live.inodes.insert(
            ino,
            Inode {
                metadata: Metadata {
                    mode,
                    uid: 0,
                    gid: 0,
                    nlink: if directory { 2 } else { 1 },
                    atime: at,
                    mtime: at,
                    ctime: at,
                    btime: at,
                },
                contents,
                xattrs: BTreeMap::new(),
            },
        );
        self.next_inode = next;
        self.entries_mut(dir).insert(name.to_vec(), ino);
        if directory {
            self.inode_mut(dir).metadata.nlink += 1;
        }
        self.changed(dir, at, true);
        Ok(ino)
    }
    pub fn create(&mut self, dir: Ino, name: &[u8], mode: u32, at: Time) -> Result<Ino> {
        self.new_inode(
            dir,
            name,
            0o100000 | (mode & 0o7777),
            Contents::File {
                size: 0,
                bytes: BTreeMap::new(),
            },
            at,
        )
    }
    pub fn mkdir(&mut self, dir: Ino, name: &[u8], mode: u32, at: Time) -> Result<Ino> {
        self.new_inode(
            dir,
            name,
            0o040000 | (mode & 0o7777),
            Contents::Directory {
                parent: dir,
                entries: BTreeMap::new(),
            },
            at,
        )
    }
    pub fn symlink(&mut self, dir: Ino, name: &[u8], target: &[u8], at: Time) -> Result<Ino> {
        if target.is_empty() || target.contains(&0) {
            return Err(Errno::EINVAL.into());
        }
        if target.len() > 4095 {
            return Err(Errno::ENAMETOOLONG.into());
        }
        self.new_inode(dir, name, 0o120777, Contents::Symlink(target.to_vec()), at)
    }
    fn regular(&self, ino: Ino) -> Result<()> {
        match self.inode(ino)?.contents {
            Contents::File { .. } => Ok(()),
            Contents::Directory { .. } => Err(Errno::EISDIR.into()),
            Contents::Symlink(_) => Err(Errno::EINVAL.into()),
        }
    }
    pub fn write(&mut self, ino: Ino, offset: u64, data: &[u8], at: Time) -> Result<usize> {
        self.regular(ino)?;
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .filter(|n| *n <= MAX_FILE_SIZE)
            .ok_or(Error::Errno(Errno::EFBIG))?;
        if let Contents::File { size, bytes } = &mut self.inode_mut(ino).contents {
            for (index, value) in data.iter().copied().enumerate() {
                let off = offset + index as u64;
                if value == 0 {
                    bytes.remove(&off);
                } else {
                    bytes.insert(off, value);
                }
            }
            *size = (*size).max(end);
        }
        self.changed(ino, at, true);
        Ok(data.len())
    }
    pub fn read(&self, ino: Ino, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.regular(ino)?;
        if let Contents::File { size, bytes } = &self.inode(ino)?.contents {
            let count = (*size).saturating_sub(offset).min(buf.len() as u64) as usize;
            for (index, slot) in buf[..count].iter_mut().enumerate() {
                *slot = bytes.get(&(offset + index as u64)).copied().unwrap_or(0);
            }
            return Ok(count);
        }
        unreachable!("validated regular file")
    }
    pub fn truncate(&mut self, ino: Ino, length: u64, at: Time) -> Result<()> {
        self.regular(ino)?;
        if length > MAX_FILE_SIZE {
            return Err(Errno::EFBIG.into());
        }
        if let Contents::File { size, bytes } = &mut self.inode_mut(ino).contents {
            if *size == length {
                return Ok(());
            }
            bytes.retain(|offset, _| *offset < length);
            *size = length;
        }
        self.changed(ino, at, true);
        Ok(())
    }
    pub fn link(&mut self, ino: Ino, dir: Ino, name: &[u8], at: Time) -> Result<()> {
        if matches!(self.inode(ino)?.contents, Contents::Directory { .. }) {
            return Err(Errno::EPERM.into());
        }
        self.vacant(dir, name)?;
        if self.inode(ino)?.metadata.nlink == u32::MAX {
            return Err(Errno::EMLINK.into());
        }
        self.entries_mut(dir).insert(name.to_vec(), ino);
        self.inode_mut(ino).metadata.nlink += 1;
        self.changed(ino, at, false);
        self.changed(dir, at, true);
        Ok(())
    }
    fn drop_link(&mut self, ino: Ino, directory: bool, at: Time) {
        if directory || self.inode_mut(ino).metadata.nlink == 1 {
            self.live.inodes.remove(&ino);
        } else {
            self.inode_mut(ino).metadata.nlink -= 1;
            self.changed(ino, at, false);
        }
    }
    pub fn unlink(&mut self, dir: Ino, name: &[u8], at: Time) -> Result<()> {
        let ino = self.lookup(dir, name)?;
        if matches!(self.inode(ino)?.contents, Contents::Directory { .. }) {
            // POSIX permits EPERM for unlink of a directory.
            return Err(Errno::EPERM.into());
        }
        self.entries_mut(dir).remove(name);
        self.drop_link(ino, false, at);
        self.changed(dir, at, true);
        Ok(())
    }
    pub fn rmdir(&mut self, dir: Ino, name: &[u8], at: Time) -> Result<()> {
        let ino = self.lookup(dir, name)?;
        if !self.entries(ino)?.is_empty() {
            return Err(Errno::ENOTEMPTY.into());
        }
        self.entries_mut(dir).remove(name);
        self.live.inodes.remove(&ino);
        self.inode_mut(dir).metadata.nlink -= 1;
        self.changed(dir, at, true);
        Ok(())
    }
    pub fn rename(
        &mut self,
        from_dir: Ino,
        from: &[u8],
        to_dir: Ino,
        to: &[u8],
        at: Time,
    ) -> Result<()> {
        let ino = self.lookup(from_dir, from)?;
        Self::name(to)?;
        let dest = self.entries(to_dir)?.get(to).copied();
        let directory = matches!(self.inode(ino)?.contents, Contents::Directory { .. });
        if directory {
            let mut ancestor = to_dir;
            loop {
                if ancestor == ino {
                    return Err(Errno::EINVAL.into());
                }
                if ancestor == ROOT {
                    break;
                }
                match self.inode(ancestor)?.contents {
                    Contents::Directory { parent, .. } => ancestor = parent,
                    _ => unreachable!("directory parent"),
                }
            }
        }
        // Two names for the same inode are a successful no-op (POSIX rename).
        if dest == Some(ino) {
            return Ok(());
        }
        if let Some(replaced) = dest {
            let dest_directory =
                matches!(self.inode(replaced)?.contents, Contents::Directory { .. });
            match (directory, dest_directory) {
                (true, false) => return Err(Errno::ENOTDIR.into()),
                (false, true) => return Err(Errno::EISDIR.into()),
                (true, true) if !self.entries(replaced)?.is_empty() => {
                    return Err(Errno::ENOTEMPTY.into())
                }
                _ => (),
            }
        }
        if directory
            && from_dir != to_dir
            && dest.is_none()
            && self.inode(to_dir)?.metadata.nlink == u32::MAX
        {
            return Err(Errno::EMLINK.into());
        }
        // Validation finishes before the namespace or link counts change.
        self.entries_mut(from_dir).remove(from);
        if let Some(replaced) = dest {
            self.drop_link(replaced, directory, at);
            if directory {
                self.inode_mut(to_dir).metadata.nlink -= 1;
            }
        }
        self.entries_mut(to_dir).insert(to.to_vec(), ino);
        if directory {
            self.inode_mut(from_dir).metadata.nlink -= 1;
            self.inode_mut(to_dir).metadata.nlink += 1;
            if let Contents::Directory { parent, .. } = &mut self.inode_mut(ino).contents {
                *parent = to_dir;
            }
        }
        self.changed(ino, at, false);
        self.changed(from_dir, at, true);
        self.changed(to_dir, at, true);
        Ok(())
    }
    pub fn setattr(&mut self, ino: Ino, attrs: &SetAttr, at: Time) -> Result<()> {
        self.inode(ino)?;
        for t in [attrs.atime, attrs.mtime].into_iter().flatten() {
            if t.nanos >= 1_000_000_000 {
                return Err(Errno::EINVAL.into());
            }
        }
        let meta = &mut self.inode_mut(ino).metadata;
        if let Some(mode) = attrs.mode {
            meta.mode = (meta.mode & 0o170000) | (mode & 0o7777);
        }
        if let Some(uid) = attrs.uid {
            meta.uid = uid;
        }
        if let Some(gid) = attrs.gid {
            meta.gid = gid;
        }
        if let Some(time) = attrs.atime {
            meta.atime = time;
        }
        if let Some(time) = attrs.mtime {
            meta.mtime = time;
        }
        if attrs != &SetAttr::default() {
            meta.ctime = at;
        }
        Ok(())
    }
    fn xattr_name(name: &[u8]) -> Result<()> {
        if name.is_empty() || name.len() > 255 || name.contains(&0) {
            return Err(Errno::EINVAL.into());
        }
        Ok(())
    }
    pub fn setxattr(&mut self, ino: Ino, name: &[u8], value: &[u8], at: Time) -> Result<()> {
        self.inode(ino)?;
        Self::xattr_name(name)?;
        if value.len() > 4096 {
            return Err(Errno::EINVAL.into());
        }
        self.inode_mut(ino)
            .xattrs
            .insert(name.to_vec(), value.to_vec());
        self.changed(ino, at, false);
        Ok(())
    }
    pub fn removexattr(&mut self, ino: Ino, name: &[u8], at: Time) -> Result<()> {
        self.inode(ino)?;
        Self::xattr_name(name)?;
        if !self.inode(ino)?.xattrs.contains_key(name) {
            return Err(Errno::ENOENT.into());
        }
        self.inode_mut(ino).xattrs.remove(name);
        self.changed(ino, at, false);
        Ok(())
    }
    pub fn fsync(&mut self, ino: Ino) -> Result<()> {
        self.inode(ino)?;
        // Design 2.5 closes the shared transaction, including namespace changes.
        self.durable = self.live.clone();
        Ok(())
    }
    pub fn remount(&mut self) -> Result<()> {
        self.durable = self.live.clone();
        self.live = self.durable.clone();
        Ok(())
    }
}
impl Fs for Model {
    fn execute(&mut self, op: &Operation) -> Result<Reply> {
        if op.at.nanos >= 1_000_000_000 {
            return Err(Errno::EINVAL.into());
        }
        let at = op.at;
        match &op.action {
            Action::Create { dir, name, mode } => {
                return self.create(*dir, name, *mode, at).map(Reply::Inode)
            }
            Action::Write { ino, offset, bytes } => {
                return self.write(*ino, *offset, bytes, at).map(Reply::Written)
            }
            Action::Truncate { ino, size } => self.truncate(*ino, *size, at),
            Action::Link { ino, dir, name } => self.link(*ino, *dir, name, at),
            Action::Unlink { dir, name } => self.unlink(*dir, name, at),
            Action::Rename {
                from_dir,
                from,
                to_dir,
                to,
            } => self.rename(*from_dir, from, *to_dir, to, at),
            Action::Mkdir { dir, name, mode } => {
                return self.mkdir(*dir, name, *mode, at).map(Reply::Inode)
            }
            Action::Rmdir { dir, name } => self.rmdir(*dir, name, at),
            Action::Symlink { dir, name, target } => {
                return self.symlink(*dir, name, target, at).map(Reply::Inode)
            }
            Action::Setattr { ino, attrs } => self.setattr(*ino, attrs, at),
            Action::Setxattr { ino, name, value } => self.setxattr(*ino, name, value, at),
            Action::Removexattr { ino, name } => self.removexattr(*ino, name, at),
            Action::Fsync { ino } => self.fsync(*ino),
            Action::Remount => self.remount(),
        }?;
        Ok(Reply::Done)
    }
    fn state(&self) -> Result<State> {
        Ok(self.live.clone())
    }
}

#[cfg(test)]
include!("model_tests.rs");
