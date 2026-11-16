//! File operations over the private transaction tree (design 2.3 and 2.5).
//! Callers serialize operations and discard the transaction after a storage error.
//! Credentials and pathname traversal belong to VFS; reservation policy is separate.
use super::{
    btree::{self, NodeStore},
    commit::{self, Committer},
    crc32c,
    format::{
        self, BlockPtr, DirRecord, Extent, Inode, Key, Root, Superblock, Time, Xattr, BLOCK_SIZE,
    },
    recover, siphash,
    txn::{self, Transaction},
};
use crate::block::BlockDevice;
use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec,
    vec::Vec,
};

const MAX_FILE_SIZE: u64 = (1u64 << 63) - 4096;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    EIO,
    EPERM,
    ENOTEMPTY,
    ENOENT,
    EEXIST,
    ENOTDIR,
    EISDIR,
    EINVAL,
    EFBIG,
    EMLINK,
    ENAMETOOLONG,
    ENOSPC,
}
impl From<format::Error> for Error {
    fn from(_: format::Error) -> Self {
        Self::EIO
    }
}
impl From<btree::Error> for Error {
    fn from(_: btree::Error) -> Self {
        Self::EIO
    }
}
impl From<commit::Error> for Error {
    fn from(_: commit::Error) -> Self {
        Self::EIO
    }
}
impl From<txn::Error> for Error {
    fn from(e: txn::Error) -> Self {
        match e {
            txn::Error::Space(super::space::Error::ENOSPC) => Self::ENOSPC,
            txn::Error::Space(super::space::Error::EMLINK) => Self::EMLINK,
            _ => Self::EIO,
        }
    }
}
#[derive(Default)]
pub struct SetAttr {
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub atime: Option<Time>,
    pub mtime: Option<Time>,
}

pub struct Files<'a, S: NodeStore + 'a> {
    txn: Transaction<'a, S>,
    root: Root,
    root_id: u64,
    hash_key: [u8; 16],
    data: BTreeMap<u64, Vec<u8>>,
}
impl<'a, S: NodeStore + 'a> Files<'a, S> {
    /// Create root 1 in the formatter's sole, unpublished generation.
    pub fn format(store: S, sb: &Superblock) -> Result<Self, Error> {
        valid_time(sb.mkfs_time)?;
        let mut txn = Transaction::format(store, sb.fs_uuid, sb.total_blocks)?;
        let root = txn.create_tree(sb.fs_uuid)?;
        let mut files = Self {
            txn,
            root: Root {
                root,
                next_inode: 16,
                source_root: 0,
                created_generation: 1,
                ctime: sb.mkfs_time,
                flags: 0,
                name: b"default".to_vec(),
            },
            root_id: 1,
            hash_key: sb.dirhash_key,
            data: BTreeMap::new(),
        };
        files.put_inode(
            2,
            Inode {
                mode: 0o40755,
                nlink: 2,
                parent: 2,
                next_index: 2,
                created_generation: 1,
                atime: sb.mkfs_time,
                mtime: sb.mkfs_time,
                ctime: sb.mkfs_time,
                btime: sb.mkfs_time,
                ..Inode::default()
            },
        )?;
        Ok(files)
    }
    /// Import source timestamps after data and children, before mkfs publication.
    /// Later transactions must use setattr's normal ctime/btime semantics.
    pub fn set_initial_times(&mut self, ino: u64, times: [Time; 4]) -> Result<(), Error> {
        if self.txn.generation() != 1 {
            return Err(Error::EINVAL);
        }
        for t in times {
            valid_time(t)?;
        }
        let mut inode = self.inode(ino)?;
        [inode.atime, inode.mtime, inode.ctime, inode.btime] = times;
        self.put_inode(ino, inode)
    }
    pub fn new(store: S, sb: &Superblock, root_id: u64) -> Result<Self, Error> {
        let root = recover::space_items(&store, sb)?
            .into_iter()
            .find(|(k, _)| k.item_type == format::ROOT && k.offset == root_id)
            .ok_or(Error::ENOENT)?
            .1;
        let root = Root::decode(&root)?;
        if root.flags & 1 != 0 {
            return Err(Error::EINVAL);
        }
        Ok(Self {
            txn: Transaction::new_owned(
                store,
                sb.space_root,
                sb.generation.checked_add(1).ok_or(Error::EIO)?,
            )?,
            root,
            root_id,
            hash_key: sb.dirhash_key,
            data: BTreeMap::new(),
        })
    }
    pub fn items(&self) -> Result<Vec<(Key, Vec<u8>)>, Error> {
        items(&self.txn.view(), self.root.root)
    }
    pub fn inode(&self, ino: u64) -> Result<Inode, Error> {
        let value = btree::search(&self.txn.view(), self.root.root, key(ino, format::INODE, 0))?
            .ok_or(Error::ENOENT)?;
        let inode = Inode::decode(&value)?;
        if inode.nlink == 0 {
            return Err(Error::ENOENT);
        }
        Ok(inode)
    }
    fn put_inode(&mut self, ino: u64, inode: Inode) -> Result<(), Error> {
        self.txn.insert(
            &mut self.root.root,
            key(ino, format::INODE, 0),
            &inode.encode(),
        )?;
        Ok(())
    }
    fn regular(&self, ino: u64) -> Result<Inode, Error> {
        let inode = self.inode(ino)?;
        match inode.mode & 0o170000 {
            0o100000 => Ok(inode),
            0o040000 => Err(Error::EISDIR),
            _ => Err(Error::EINVAL),
        }
    }
    fn directory(&self, ino: u64) -> Result<Inode, Error> {
        let inode = self.inode(ino)?;
        if inode.mode & 0o170000 != 0o040000 {
            return Err(Error::ENOTDIR);
        }
        Ok(inode)
    }
    fn bucket(&self, dir: u64, name: &[u8]) -> Result<(Key, Vec<DirRecord>), Error> {
        let k = key(dir, format::DIR_ENTRY, siphash::hash(&self.hash_key, name));
        let bytes = btree::search(&self.txn.view(), self.root.root, k)?.unwrap_or_default();
        Ok((k, dir_records(&bytes)?))
    }
    pub fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Error> {
        valid_name(name)?;
        self.directory(dir)?;
        self.bucket(dir, name)?
            .1
            .into_iter()
            .find(|r| r.name == name)
            .map(|r| r.ino)
            .ok_or(Error::ENOENT)
    }
    // Check the next directory cookie and collision payload before edits.
    fn vacant(&self, dir: u64, name: &[u8], kind: u8) -> Result<(), Error> {
        valid_name(name)?;
        let parent = self.directory(dir)?;
        let (_, bucket) = self.bucket(dir, name)?;
        if bucket.iter().any(|r| r.name == name) {
            return Err(Error::EEXIST);
        }
        if bucket.iter().map(|r| 10 + r.name.len()).sum::<usize>() + 10 + name.len() > 6144 {
            return Err(Error::ENOSPC);
        }
        parent.next_index.checked_add(1).ok_or(Error::EMLINK)?;
        parent.size.checked_add(1).ok_or(Error::EIO)?;
        if kind == 2 {
            parent.nlink.checked_add(1).ok_or(Error::EMLINK)?;
        }
        Ok(())
    }
    fn add_entry(
        &mut self,
        dir: u64,
        name: &[u8],
        ino: u64,
        kind: u8,
        at: Time,
    ) -> Result<(), Error> {
        self.vacant(dir, name, kind)?;
        let mut parent = self.directory(dir)?;
        let (k, records) = self.bucket(dir, name)?;
        let mut bytes: Vec<_> = records.iter().flat_map(DirRecord::encode).collect();
        let record = DirRecord {
            ino,
            kind,
            name: name.to_vec(),
        }
        .encode();
        bytes.extend_from_slice(&record);
        self.txn.insert(&mut self.root.root, k, &bytes)?;
        self.txn.insert(
            &mut self.root.root,
            key(dir, format::DIR_INDEX, parent.next_index),
            &record,
        )?;
        parent.next_index += 1;
        parent.size += 1;
        if kind == 2 {
            parent.nlink += 1;
        }
        parent.mtime = at;
        parent.ctime = at;
        self.put_inode(dir, parent)
    }
    fn remove_entry(&mut self, dir: u64, name: &[u8], at: Time) -> Result<DirRecord, Error> {
        let mut parent = self.directory(dir)?;
        let (k, mut records) = self.bucket(dir, name)?;
        let pos = records
            .iter()
            .position(|r| r.name == name)
            .ok_or(Error::ENOENT)?;
        let record = records.remove(pos);
        let index = self
            .range(
                key(dir, format::DIR_INDEX, 0),
                key(dir, format::DIR_INDEX, u64::MAX),
            )?
            .into_iter()
            .find_map(|(k, v)| {
                if k.object != dir || k.item_type != format::DIR_INDEX {
                    return None;
                }
                match dir_records(&v) {
                    Ok(rs) if rs.len() == 1 && rs[0] == record => Some(Ok(k)),
                    Err(e) => Some(Err(e)),
                    _ => None,
                }
            })
            .ok_or(Error::EIO)??;
        if records.is_empty() {
            self.txn.delete(&mut self.root.root, k)?;
        } else {
            let bytes: Vec<_> = records.iter().flat_map(DirRecord::encode).collect();
            self.txn.insert(&mut self.root.root, k, &bytes)?;
        }
        self.txn.delete(&mut self.root.root, index)?;
        parent.size = parent.size.checked_sub(1).ok_or(Error::EIO)?;
        if record.kind == 2 {
            parent.nlink = parent.nlink.checked_sub(1).ok_or(Error::EIO)?;
        }
        parent.mtime = at;
        parent.ctime = at;
        self.put_inode(dir, parent)?;
        Ok(record)
    }
    fn new_inode(
        &mut self,
        dir: u64,
        name: &[u8],
        mode: u32,
        target: Option<&[u8]>,
        at: Time,
    ) -> Result<u64, Error> {
        valid_time(at)?;
        let kind = inode_kind(mode)?;
        self.vacant(dir, name, kind)?;
        let ino = self.root.next_inode;
        let next = ino
            .checked_add(1)
            .filter(|_| ino >= 16)
            .ok_or(Error::EMLINK)?;
        let inode = Inode {
            mode,
            nlink: if kind == 2 { 2 } else { 1 },
            parent: if kind == 2 { dir } else { 0 },
            next_index: if kind == 2 { 2 } else { 0 },
            size: target.map_or(0, |t| t.len() as u64),
            created_generation: self.txn.generation(),
            atime: at,
            mtime: at,
            ctime: at,
            btime: at,
            ..Inode::default()
        };
        self.put_inode(ino, inode)?;
        if let Some(target) = target {
            self.txn
                .insert(&mut self.root.root, key(ino, format::INLINE, 0), target)?;
        }
        self.add_entry(dir, name, ino, kind, at)?;
        self.root.next_inode = next;
        Ok(ino)
    }
    pub fn create(&mut self, dir: u64, name: &[u8], mode: u32, at: Time) -> Result<u64, Error> {
        self.new_inode(dir, name, 0o100000 | (mode & 0o7777), None, at)
    }
    pub fn mkdir(&mut self, dir: u64, name: &[u8], mode: u32, at: Time) -> Result<u64, Error> {
        self.new_inode(dir, name, 0o040000 | (mode & 0o7777), None, at)
    }
    pub fn symlink(
        &mut self,
        dir: u64,
        name: &[u8],
        target: &[u8],
        at: Time,
    ) -> Result<u64, Error> {
        if target.is_empty() || target.contains(&0) {
            return Err(Error::EINVAL);
        }
        if target.len() > 4095 {
            return Err(Error::ENAMETOOLONG);
        }
        self.new_inode(dir, name, 0o120777, Some(target), at)
    }
    pub fn readlink(&self, ino: u64) -> Result<Vec<u8>, Error> {
        let inode = self.inode(ino)?;
        if inode.mode & 0o170000 != 0o120000 {
            return Err(Error::EINVAL);
        }
        let target = btree::search(
            &self.txn.view(),
            self.root.root,
            key(ino, format::INLINE, 0),
        )?
        .ok_or(Error::EIO)?;
        if target.len() as u64 != inode.size {
            return Err(Error::EIO);
        }
        Ok(target)
    }
    pub fn link(&mut self, ino: u64, dir: u64, name: &[u8], at: Time) -> Result<(), Error> {
        let mut inode = self.inode(ino)?;
        let kind = inode_kind(inode.mode)?;
        if kind == 2 {
            return Err(Error::EPERM);
        }
        valid_time(at)?;
        self.vacant(dir, name, kind)?;
        inode.nlink = inode.nlink.checked_add(1).ok_or(Error::EMLINK)?;
        self.add_entry(dir, name, ino, kind, at)?;
        inode.ctime = at;
        self.put_inode(ino, inode)
    }
    fn delete_inode(&mut self, ino: u64) -> Result<(), Error> {
        for (k, _) in self.range(key(ino, 0, 0), key(ino, u8::MAX, u64::MAX))? {
            self.txn.delete(&mut self.root.root, k)?;
        }
        Ok(())
    }
    fn drop_link(&mut self, ino: u64, at: Time) -> Result<(), Error> {
        let mut inode = self.inode(ino)?;
        if inode_kind(inode.mode)? == 2 {
            return self.delete_inode(ino);
        }
        inode.nlink = inode.nlink.checked_sub(1).ok_or(Error::EIO)?;
        inode.ctime = at;
        if inode.nlink == 0 {
            self.txn.insert(
                &mut self.root.root,
                key(ORPHAN_OBJECT, format::ORPHAN, ino),
                &[],
            )?;
        }
        self.put_inode(ino, inode)
    }
    pub fn unlink(&mut self, dir: u64, name: &[u8], at: Time) -> Result<(), Error> {
        let ino = self.lookup(dir, name)?;
        if inode_kind(self.inode(ino)?.mode)? == 2 {
            return Err(Error::EPERM);
        }
        valid_time(at)?;
        self.remove_entry(dir, name, at)?;
        self.drop_link(ino, at)
    }
    pub fn rmdir(&mut self, dir: u64, name: &[u8], at: Time) -> Result<(), Error> {
        let ino = self.lookup(dir, name)?;
        if self.directory(ino)?.size != 0 {
            return Err(Error::ENOTEMPTY);
        }
        valid_time(at)?;
        self.remove_entry(dir, name, at)?;
        self.delete_inode(ino)
    }
    pub fn rename(
        &mut self,
        from_dir: u64,
        from: &[u8],
        to_dir: u64,
        to: &[u8],
        at: Time,
    ) -> Result<(), Error> {
        let ino = self.lookup(from_dir, from)?;
        valid_name(to)?;
        let dest_parent = self.directory(to_dir)?;
        let mut inode = self.inode(ino)?;
        let kind = inode_kind(inode.mode)?;
        let (_, bucket) = self.bucket(to_dir, to)?;
        let dest = bucket.iter().find(|r| r.name == to).map(|r| r.ino);
        if kind == 2 {
            let mut ancestor = to_dir;
            let mut seen = BTreeSet::new();
            loop {
                if ancestor == ino {
                    return Err(Error::EINVAL);
                }
                if ancestor == 2 {
                    break;
                }
                if !seen.insert(ancestor) {
                    return Err(Error::EIO);
                }
                ancestor = self.directory(ancestor)?.parent;
            }
        }
        if dest == Some(ino) {
            return Ok(());
        }
        if let Some(replaced) = dest {
            let other = self.inode(replaced)?;
            match (kind == 2, inode_kind(other.mode)? == 2) {
                (true, false) => return Err(Error::ENOTDIR),
                (false, true) => return Err(Error::EISDIR),
                (true, true) if other.size != 0 => return Err(Error::ENOTEMPTY),
                _ => (),
            }
        }
        valid_time(at)?;
        dest_parent.next_index.checked_add(1).ok_or(Error::EMLINK)?;
        if kind == 2 && from_dir != to_dir && dest.is_none() && dest_parent.nlink == u32::MAX {
            return Err(Error::EMLINK);
        }
        let retained: usize = bucket
            .iter()
            .filter(|r| r.name != to && !(from_dir == to_dir && r.name == from))
            .map(|r| 10 + r.name.len())
            .sum();
        if retained + 10 + to.len() > 6144 {
            return Err(Error::ENOSPC);
        }
        self.remove_entry(from_dir, from, at)?;
        if let Some(replaced) = dest {
            self.remove_entry(to_dir, to, at)?;
            self.drop_link(replaced, at)?;
        }
        self.add_entry(to_dir, to, ino, kind, at)?;
        if kind == 2 {
            inode.parent = to_dir;
        }
        inode.ctime = at;
        self.put_inode(ino, inode)
    }
    fn reclaim_orphans(&mut self) -> Result<(), Error> {
        let orphans = self.range(
            key(ORPHAN_OBJECT, format::ORPHAN, 0),
            key(ORPHAN_OBJECT, format::ORPHAN, u64::MAX),
        )?;
        for (k, value) in orphans {
            if k.object != ORPHAN_OBJECT || !value.is_empty() {
                return Err(Error::EIO);
            }
            let inode = btree::search(
                &self.txn.view(),
                self.root.root,
                key(k.offset, format::INODE, 0),
            )?
            .ok_or(Error::EIO)?;
            let inode = Inode::decode(&inode)?;
            if inode.nlink != 0 || inode_kind(inode.mode)? == 2 {
                return Err(Error::EIO);
            }
            self.delete_inode(k.offset)?;
            self.txn.delete(&mut self.root.root, k)?;
        }
        Ok(())
    }
    fn range(&self, lower: Key, upper: Key) -> Result<Vec<(Key, Vec<u8>)>, Error> {
        Ok(btree::range(
            &self.txn.view(),
            self.root.root,
            lower,
            upper,
        )?)
    }
    fn extents(&self, ino: u64) -> Result<Vec<(Key, Extent)>, Error> {
        self.range(
            key(ino, format::EXTENT, 0),
            key(ino, format::EXTENT, u64::MAX),
        )?
        .into_iter()
        .map(|(k, v)| Ok((k, Extent::decode(&v)?)))
        .collect()
    }
    fn block(&self, device: &dyn BlockDevice, e: Extent, index: u64) -> Result<Vec<u8>, Error> {
        let addr = e.phys_start + u64::from(e.extent_offset) + index;
        if let Some(bytes) = self.data.get(&e.phys_start) {
            let lo = (addr - e.phys_start) as usize * BLOCK_SIZE;
            return bytes
                .get(lo..lo + BLOCK_SIZE)
                .map(|b| b.to_vec())
                .ok_or(Error::EIO);
        }
        Ok(commit::read(device, addr, BLOCK_SIZE)?)
    }
    pub fn read(
        &self,
        device: &dyn BlockDevice,
        ino: u64,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, Error> {
        let inode = self.regular(ino)?;
        let n = inode.size.saturating_sub(offset).min(buf.len() as u64) as usize;
        buf[..n].fill(0);
        for (k, e) in self.extents(ino)? {
            let lo = offset.max(k.offset);
            let hi =
                (offset + n as u64).min(k.offset + u64::from(e.num_blocks) * BLOCK_SIZE as u64);
            let mut pos = lo;
            while pos < hi {
                let index = (pos - k.offset) / BLOCK_SIZE as u64;
                let within = ((pos - k.offset) % BLOCK_SIZE as u64) as usize;
                let take = ((hi - pos) as usize).min(BLOCK_SIZE - within);
                let block = self.block(device, e, index)?;
                buf[(pos - offset) as usize..(pos - offset) as usize + take]
                    .copy_from_slice(&block[within..within + take]);
                pos += take as u64;
            }
        }
        Ok(n)
    }
    // One private extent per contiguous run, bounded to 1 MiB (design 1.2).
    fn replace_extent(&mut self, ino: u64, off: u64, bytes: Vec<u8>) -> Result<(), Error> {
        let crcs: Vec<_> = bytes
            .chunks_exact(BLOCK_SIZE)
            .map(crc32c::checksum)
            .collect();
        let blocks = crcs.len() as u32;
        let addr = self.txn.allocate_data(&crcs)?;
        self.data.insert(addr, bytes);
        self.txn.insert(
            &mut self.root.root,
            key(ino, format::EXTENT, off),
            &Extent {
                phys_start: addr,
                phys_len: blocks,
                extent_offset: 0,
                num_blocks: blocks,
                flags: 0,
            }
            .encode(),
        )?;
        Ok(())
    }
    // Remove intersecting items and retain unaffected block slices. A split
    // needs a reference per replacement item; txn::delete releases the old one.
    fn remove_range(&mut self, ino: u64, start: u64, end: u64) -> Result<(), Error> {
        for (k, e) in self.extents(ino)? {
            let extent_end = k.offset + u64::from(e.num_blocks) * BLOCK_SIZE as u64;
            if k.offset >= end || extent_end <= start {
                continue;
            }
            for off in (k.offset..extent_end).step_by(BLOCK_SIZE) {
                if off < start || off >= end {
                    self.txn.increment(e.phys_start, u64::from(e.phys_len))?;
                    let slice = Extent {
                        extent_offset: e.extent_offset
                            + ((off - k.offset) / BLOCK_SIZE as u64) as u32,
                        num_blocks: 1,
                        ..e
                    };
                    // The first slice can replace the old key (same REF); the
                    // old incoming reference is explicitly released below.
                    self.txn.insert(
                        &mut self.root.root,
                        key(ino, format::EXTENT, off),
                        &slice.encode(),
                    )?;
                }
            }
            if k.offset < start {
                // Same-key replacement retained its old reference in txn::insert.
                self.txn
                    .release(e.phys_start, u64::from(e.phys_len), false)?;
            } else {
                self.txn.delete(&mut self.root.root, k)?;
            }
        }
        Ok(())
    }
    pub fn write(
        &mut self,
        device: &dyn BlockDevice,
        ino: u64,
        offset: u64,
        bytes: &[u8],
        at: Time,
    ) -> Result<usize, Error> {
        let mut inode = self.regular(ino)?;
        valid_time(at)?;
        if bytes.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= MAX_FILE_SIZE)
            .ok_or(Error::EFBIG)?;
        let start = offset / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
        let rounded = end.div_ceil(BLOCK_SIZE as u64) * BLOCK_SIZE as u64;
        let mut blocks = vec![0; (rounded - start) as usize];
        self.read(device, ino, start, &mut blocks)?;
        let lo = (offset - start) as usize;
        blocks[lo..lo + bytes.len()].copy_from_slice(bytes);
        self.remove_range(ino, start, rounded)?;
        for (n, run) in blocks.chunks(256 * BLOCK_SIZE).enumerate() {
            self.replace_extent(ino, start + (n * 256 * BLOCK_SIZE) as u64, run.to_vec())?;
        }
        inode.size = inode.size.max(end);
        inode.mtime = at;
        inode.ctime = at;
        inode.nblocks = self
            .extents(ino)?
            .iter()
            .map(|(_, e)| u64::from(e.num_blocks))
            .sum();
        self.put_inode(ino, inode)?;
        Ok(bytes.len())
    }
    pub fn truncate(
        &mut self,
        device: &dyn BlockDevice,
        ino: u64,
        size: u64,
        at: Time,
    ) -> Result<(), Error> {
        let mut inode = self.regular(ino)?;
        valid_time(at)?;
        if size > MAX_FILE_SIZE {
            return Err(Error::EFBIG);
        }
        if size == inode.size {
            return Ok(());
        }
        if size < inode.size {
            let floor = size / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
            let tail = if size % BLOCK_SIZE as u64 != 0
                && self.extents(ino)?.iter().any(|(k, e)| {
                    k.offset <= floor
                        && floor < k.offset + u64::from(e.num_blocks) * BLOCK_SIZE as u64
                }) {
                let mut b = vec![0; BLOCK_SIZE];
                self.read(device, ino, floor, &mut b)?;
                b[(size - floor) as usize..].fill(0);
                Some(b)
            } else {
                None
            };
            self.remove_range(ino, floor, u64::MAX)?;
            if let Some(b) = tail {
                self.replace_extent(ino, floor, b)?;
            }
        }
        inode.size = size;
        inode.mtime = at;
        inode.ctime = at;
        inode.nblocks = self
            .extents(ino)?
            .iter()
            .map(|(_, e)| u64::from(e.num_blocks))
            .sum();
        self.put_inode(ino, inode)
    }
    pub fn setattr(&mut self, ino: u64, attrs: &SetAttr, at: Time) -> Result<(), Error> {
        let mut i = self.inode(ino)?;
        valid_time(at)?;
        for t in [attrs.atime, attrs.mtime].into_iter().flatten() {
            valid_time(t)?;
        }
        if let Some(v) = attrs.mode {
            i.mode = (i.mode & 0o170000) | (v & 0o7777);
        }
        if let Some(v) = attrs.uid {
            i.uid = v;
        }
        if let Some(v) = attrs.gid {
            i.gid = v;
        }
        if let Some(v) = attrs.atime {
            i.atime = v;
        }
        if let Some(v) = attrs.mtime {
            i.mtime = v;
        }
        if attrs.mode.is_some()
            || attrs.uid.is_some()
            || attrs.gid.is_some()
            || attrs.atime.is_some()
            || attrs.mtime.is_some()
        {
            i.ctime = at;
        }
        self.put_inode(ino, i)
    }
    pub fn xattr(
        &mut self,
        ino: u64,
        name: &[u8],
        value: Option<&[u8]>,
        at: Time,
    ) -> Result<(), Error> {
        let mut i = self.inode(ino)?;
        valid_time(at)?;
        if name.is_empty()
            || name.len() > 255
            || name.contains(&0)
            || value.is_some_and(|v| v.len() > 4096)
        {
            return Err(Error::EINVAL);
        }
        let k = key(ino, format::XATTR, siphash::hash(&self.hash_key, name));
        let old = btree::search(&self.txn.view(), self.root.root, k)?.unwrap_or_default();
        let mut records = xattrs(&old)?;
        let found = records.iter().position(|x| x.name == name);
        if value.is_none() && found.is_none() {
            return Err(Error::ENOENT);
        }
        if let Some(n) = found {
            records.remove(n);
        }
        if let Some(v) = value {
            records.push(Xattr {
                name: name.to_vec(),
                value: v.to_vec(),
            });
        }
        let bytes: Vec<_> = records.iter().flat_map(Xattr::encode).collect();
        if bytes.len() > 6144 {
            return Err(Error::ENOSPC);
        }
        if bytes.is_empty() {
            self.txn.delete(&mut self.root.root, k)?;
        } else {
            self.txn.insert(&mut self.root.root, k, &bytes)?;
        }
        i.ctime = at;
        self.put_inode(ino, i)
    }
    pub fn fsync(
        &mut self,
        device: &dyn BlockDevice,
        committer: &mut Committer,
    ) -> Result<Superblock, Error> {
        Ok(committer.commit_files(device, self)?)
    }
    pub(crate) fn prepare_commit(
        &mut self,
    ) -> Result<(&mut Transaction<'a, S>, &BTreeMap<u64, Vec<u8>>), Error> {
        self.reclaim_orphans()?;
        self.txn.update_root(self.root_id, self.root.clone())?;
        // A private allocation replaced again within this transaction is PENDING
        // and need not be submitted as data in the commit engine's REF set.
        let ptr = self.txn.settle()?;
        let refs = items(&self.txn.view(), ptr)?;
        let live: BTreeSet<_> = refs
            .into_iter()
            .filter(|(k, _)| k.item_type == format::REF)
            .map(|(k, _)| k.object)
            .collect();
        self.data.retain(|addr, _| live.contains(addr));
        Ok((&mut self.txn, &self.data))
    }
}
const ORPHAN_OBJECT: u64 = 0xffff_ffff_ffff_ff00;
fn valid_name(name: &[u8]) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::ENOENT);
    }
    if name.len() > 255 {
        return Err(Error::ENAMETOOLONG);
    }
    if name == b"." || name == b".." || name.contains(&0) || name.contains(&b'/') {
        return Err(Error::EINVAL);
    }
    Ok(())
}
fn inode_kind(mode: u32) -> Result<u8, Error> {
    match mode & 0o170000 {
        0o100000 => Ok(1),
        0o040000 => Ok(2),
        0o020000 => Ok(3),
        0o060000 => Ok(4),
        0o010000 => Ok(5),
        0o140000 => Ok(6),
        0o120000 => Ok(7),
        _ => Err(Error::EIO),
    }
}
fn valid_time(at: Time) -> Result<(), Error> {
    if at.nanos >= 1_000_000_000 {
        Err(Error::EINVAL)
    } else {
        Ok(())
    }
}
pub fn key(object: u64, item_type: u8, offset: u64) -> Key {
    Key {
        object,
        item_type,
        offset,
    }
}
pub fn dir_records(mut bytes: &[u8]) -> Result<Vec<DirRecord>, Error> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let r = DirRecord::decode(bytes)?;
        let len = 10 + r.name.len();
        out.push(r);
        bytes = bytes.get(len..).ok_or(Error::EIO)?;
    }
    Ok(out)
}
pub fn xattrs(mut bytes: &[u8]) -> Result<Vec<Xattr>, Error> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let x = Xattr::decode(bytes)?;
        let len = 3 + x.name.len() + x.value.len();
        out.push(x);
        bytes = bytes.get(len..).ok_or(Error::EIO)?;
    }
    Ok(out)
}
pub fn items(store: &impl NodeStore, root: BlockPtr) -> Result<Vec<(Key, Vec<u8>)>, Error> {
    Ok(btree::range(
        store,
        root,
        Key::default(),
        key(u64::MAX, u8::MAX, u64::MAX),
    )?)
}
