//! File operations over the private transaction tree (design 2.3 and 2.5).
//! Callers serialize operations and discard the transaction after a storage error.
//! Credentials and pathname traversal belong to VFS; reservation policy is separate.
use super::{
    btree::{self, NodeStore},
    commit::{self, Committer},
    crc32c,
    format::{
        self, BlockPtr, DirRecord, Extent, Inode, InternalEntry, Key, LeafItemHeader, NodeHeader,
        Root, Superblock, Time, Xattr, BLOCK_SIZE,
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

pub struct Files<S: NodeStore + 'static> {
    txn: Transaction<'static, S>,
    root: Root,
    root_id: u64,
    hash_key: [u8; 16],
    data: BTreeMap<u64, Vec<u8>>,
}
impl<S: NodeStore + 'static> Files<S> {
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
        Ok(Inode::decode(&value)?)
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
    pub fn create(&mut self, dir: u64, name: &[u8], mode: u32, at: Time) -> Result<u64, Error> {
        if name.is_empty() {
            return Err(Error::ENOENT);
        }
        if name.len() > 255 {
            return Err(Error::ENAMETOOLONG);
        }
        if name == b"." || name == b".." || name.contains(&0) || name.contains(&b'/') {
            return Err(Error::EINVAL);
        }
        valid_time(at)?;
        let mut parent = self.inode(dir)?;
        if parent.mode & 0o170000 != 0o040000 {
            return Err(Error::ENOTDIR);
        }
        let hash = siphash::hash(&self.hash_key, name);
        let dk = key(dir, format::DIR_ENTRY, hash);
        let mut bucket = btree::search(&self.txn.view(), self.root.root, dk)?.unwrap_or_default();
        for record in dir_records(&bucket)? {
            if record.name == name {
                return Err(Error::EEXIST);
            }
        }
        let ino = self.root.next_inode;
        let next = ino
            .checked_add(1)
            .filter(|_| ino >= 16)
            .ok_or(Error::EMLINK)?;
        let next_index = parent.next_index.checked_add(1).ok_or(Error::EMLINK)?;
        let record = DirRecord {
            ino,
            kind: 1,
            name: name.to_vec(),
        }
        .encode();
        if bucket.len() + record.len() > 6144 {
            return Err(Error::ENOSPC);
        }
        let size = parent.size.checked_add(1).ok_or(Error::EIO)?;
        let inode = Inode {
            mode: 0o100000 | (mode & 0o7777),
            nlink: 1,
            created_generation: self.txn.generation(),
            atime: at,
            mtime: at,
            ctime: at,
            btime: at,
            ..Inode::default()
        };
        self.put_inode(ino, inode)?;
        bucket.extend_from_slice(&record);
        self.txn.insert(&mut self.root.root, dk, &bucket)?;
        self.txn.insert(
            &mut self.root.root,
            key(dir, format::DIR_INDEX, parent.next_index),
            &record,
        )?;
        parent.next_index = next_index;
        parent.size = size;
        parent.mtime = at;
        parent.ctime = at;
        self.put_inode(dir, parent)?;
        self.root.next_inode = next;
        Ok(ino)
    }
    fn extents(&self, ino: u64) -> Result<Vec<(Key, Extent)>, Error> {
        self.items()?
            .into_iter()
            .filter(|(k, _)| k.object == ino && k.item_type == format::EXTENT)
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
    // Store one private 4 KiB allocation per item. Larger contiguous allocations
    // can be coalesced later without changing the sparse-file representation.
    fn replace_block(&mut self, ino: u64, off: u64, bytes: Vec<u8>) -> Result<(), Error> {
        let addr = self.txn.allocate_data(&[crc32c::checksum(&bytes)])?;
        self.data.insert(addr, bytes);
        self.txn.insert(
            &mut self.root.root,
            key(ino, format::EXTENT, off),
            &Extent {
                phys_start: addr,
                phys_len: 1,
                extent_offset: 0,
                num_blocks: 1,
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
        let mut blocks = Vec::new();
        for off in (start..rounded).step_by(BLOCK_SIZE) {
            let mut block = vec![0; BLOCK_SIZE];
            self.read(device, ino, off, &mut block)?;
            let lo = offset.max(off);
            let hi = end.min(off + BLOCK_SIZE as u64);
            block[(lo - off) as usize..(hi - off) as usize]
                .copy_from_slice(&bytes[(lo - offset) as usize..(hi - offset) as usize]);
            blocks.push((off, block));
        }
        self.remove_range(ino, start, rounded)?;
        for (off, block) in blocks {
            self.replace_block(ino, off, block)?;
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
                self.replace_block(ino, floor, b)?;
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
        Ok(committer.commit(device, &mut self.txn, &self.data)?)
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
    fn walk(
        store: &impl NodeStore,
        ptr: BlockPtr,
        seen: &mut BTreeSet<u64>,
        out: &mut Vec<(Key, Vec<u8>)>,
    ) -> Result<(), Error> {
        if !seen.insert(ptr.addr) {
            return Err(Error::EIO);
        }
        let bytes = store.get(ptr.addr)?;
        let h = NodeHeader::decode(&bytes)?;
        if (h.addr, h.generation, h.level) != (ptr.addr, ptr.generation, ptr.level)
            || bytes[8..12] != ptr.csum.to_le_bytes()
            || h.level >= 8
        {
            return Err(Error::EIO);
        }
        for n in 0..usize::from(h.nitems) {
            if h.level == 0 {
                let at = 48 + n * 21;
                let i = LeafItemHeader::decode(bytes.get(at..at + 21).ok_or(Error::EIO)?)?;
                let value = btree::search(store, ptr, i.key)?.ok_or(Error::EIO)?;
                out.push((i.key, value));
            } else {
                let at = 48 + n * 37;
                let e = InternalEntry::decode(bytes.get(at..at + 37).ok_or(Error::EIO)?)?;
                walk(
                    store,
                    BlockPtr {
                        addr: e.child_addr,
                        generation: e.child_generation,
                        csum: e.child_csum,
                        level: h.level - 1,
                    },
                    seen,
                    out,
                )?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(store, root, &mut BTreeSet::new(), &mut out)?;
    out.sort_by_key(|(k, _)| *k);
    Ok(out)
}
