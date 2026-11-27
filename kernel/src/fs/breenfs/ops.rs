//! File operations over the private transaction tree (design 2.3 and 2.5).
//! Callers serialize operations and discard the transaction after a storage error.
//! Credentials and pathname traversal belong to VFS; reservation policy is separate.
use super::preempt::preemption_point;
use super::{
    btree::{self, NodeStore},
    commit::{self, Committer},
    crc32c,
    format::{
        self, BlockPtr, DirRecord, Extent, Inode, Key, Root, Superblock, Time, Xattr, BLOCK_SIZE,
    },
    recover, siphash, space,
    txn::{self, Transaction},
};
use crate::block::BlockDevice;
use core::cell::RefCell;
use alloc::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec,
    vec::Vec,
};

const MAX_FILE_SIZE: u64 = (1u64 << 63) - 4096;
type Inodes = Arc<BTreeSet<u64>>;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    EIO,
    Metadata(btree::ReadFailure),
    DataRead(crate::block::BlockError),
    EROFS,
    EBUSY,
    EPERM,
    ENOTEMPTY,
    ENOENT,
    ENODATA,
    EEXIST,
    ENOTDIR,
    EISDIR,
    EINVAL,
    EFBIG,
    EMLINK,
    ENAMETOOLONG,
    ENOSPC,
    /// The heap refused memory the change needs, before the change was made.
    ENOMEM,
}
impl From<format::Error> for Error {
    fn from(_: format::Error) -> Self {
        Self::EIO
    }
}
impl From<btree::Error> for Error {
    fn from(error: btree::Error) -> Self {
        match error { btree::Error::Read(failure) => Self::Metadata(failure), _ => Self::EIO }
    }
}
impl From<commit::Error> for Error {
    fn from(error: commit::Error) -> Self {
        match error {
            commit::Error::NodeRead(failure) => Self::Metadata(failure),
            _ => Self::EIO,
        }
    }
}
impl From<txn::Error> for Error {
    fn from(e: txn::Error) -> Self {
        match e {
            txn::Error::Space(super::space::Error::ENOSPC) => Self::ENOSPC,
            txn::Error::Space(super::space::Error::EMLINK) => Self::EMLINK,
            txn::Error::Tree(error) | txn::Error::Space(super::space::Error::Tree(error)) => error.into(),
            _ => Self::EIO,
        }
    }
}
// Data corruption fails the read without poisoning unrelated files. Metadata
// failures also close writable admission at the adapter (design 2.7).
pub(super) enum ReadError {
    Metadata(Error),
    Device(crate::block::BlockError),
    Data,
}
impl From<Error> for ReadError {
    fn from(error: Error) -> Self { Self::Metadata(error) }
}
impl ReadError {
    pub(super) fn error(self) -> Error {
        match self {
            Self::Metadata(error) => error,
            Self::Device(error) => Error::DataRead(error),
            Self::Data => Error::EIO,
        }
    }
}
impl From<btree::Error> for ReadError {
    fn from(error: btree::Error) -> Self { Self::Metadata(error.into()) }
}

// Clean file reads (Phase 1.1, audit R7). A read seeks to the first EXTENT
// item that intersects it and issues one device request per run of blocks
// that are adjacent both in the file and on the device, up to the block
// cache's per-request limit, straight into the caller's buffer. The format
// keeps one CRC per 4 KiB block in the extent's REF, so each block is still
// checked on its own; what is shared per extent is the REF lookup.
const RUN_BLOCKS: usize = crate::block::cache::RUN_BYTES / BLOCK_SIZE;

/// One EXTENT item: file bytes `[offset, end)` map `extent`.
#[derive(Clone, Copy)]
struct Mapped {
    offset: u64,
    end: u64,
    extent: Extent,
}
impl Mapped {
    fn new(key: Key, extent: Extent) -> Result<Self, Error> {
        let end = u64::from(extent.num_blocks)
            .checked_mul(BLOCK_SIZE as u64)
            .and_then(|length| key.offset.checked_add(length))
            .ok_or(Error::EIO)?;
        Ok(Self { offset: key.offset, end, extent })
    }
}

// In-core extent maps (audit R7). A Files view keeps the decoded EXTENT
// items of recently read inodes, sorted by file offset, and the REF
// checksums of the extents they name. EXTENT inserts and deletes for an
// inode go through Files::insert and Files::delete, which drop that inode's
// map, so a map does not outlive the items it decodes. Maps and checksums share
// one byte budget, a sixty-fourth of the smallest kernel heap; a file whose
// items alone exceed it is read through a B-tree seek on each call instead.
// claim-lint:ok: in ops.rs the only FS-tree edits are Files::insert, Files::delete and create_empty_root's INODE insert.
const EXTENT_MAP_BYTES: usize = 1 << 20;
const EXTENT_MAPS: usize = 64;

// Host tests refuse the extent-map cache's own allocations one at a time
// (tools/breenfs-host-tests/src/extent_runs.rs); a scope marks them.
#[cfg(test)]
pub mod cache_allocations {
    extern crate std;
    std::thread_local! {
        static DEPTH: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
    }
    pub struct Scope;
    impl Drop for Scope {
        fn drop(&mut self) {
            let _ = DEPTH.try_with(|depth| depth.set(depth.get() - 1));
        }
    }
    pub fn scope() -> Scope {
        let _ = DEPTH.try_with(|depth| depth.set(depth.get() + 1));
        Scope
    }
    /// Whether this thread is inside a cache allocation.
    pub fn active() -> bool {
        DEPTH.try_with(|depth| depth.get() > 0).unwrap_or(false)
    }
}
const MAP_OVERHEAD: usize = 128;
const CRCS_OVERHEAD: usize = 64;

#[derive(Default)]
struct ExtentMaps {
    // Projected roots (with_read_root) are not cached.
    disabled: bool,
    maps: BTreeMap<u64, ExtentMap>,
    bytes: usize,
    clock: u64,
}
struct ExtentMap {
    // Sorted by offset and non-overlapping; empty when `oversized`.
    extents: Vec<Mapped>,
    oversized: bool,
    crcs: BTreeMap<u64, Vec<u32>>,
    bytes: usize,
    used: u64,
}
impl ExtentMaps {
    fn disabled() -> Self {
        Self { disabled: true, ..Self::default() }
    }
    fn invalidate(&mut self, ino: u64) {
        if let Some(map) = self.maps.remove(&ino) {
            self.bytes -= map.bytes;
        }
    }
    /// Evict least recently used maps until a new map of `bytes` fits.
    fn make_room(&mut self, bytes: usize) -> bool {
        while self.bytes + bytes > EXTENT_MAP_BYTES || self.maps.len() >= EXTENT_MAPS {
            let Some(victim) = self.maps.iter().min_by_key(|(_, map)| map.used).map(|(ino, _)| *ino)
            else {
                return false;
            };
            self.invalidate(victim);
        }
        true
    }
}

/// The destination of one clean read: `buf` holds file bytes from `base`.
/// Bytes before `done` are final; a pending run of `blocks` device blocks
/// from `phys` covers file bytes `[lo, hi)` of blocks starting at `first`.
struct Runs<'r> {
    device: &'r dyn BlockDevice,
    base: u64,
    buf: &'r mut [u8],
    done: usize,
    first: u64,
    lo: u64,
    hi: u64,
    phys: u64,
    blocks: usize,
    crcs: [u32; RUN_BLOCKS],
    // Whole blocks for a run that starts or ends inside a block.
    staging: Vec<u8>,
}
impl<'r> Runs<'r> {
    fn new(device: &'r dyn BlockDevice, base: u64, buf: &'r mut [u8]) -> Self {
        Self {
            device, base, buf, done: 0, first: 0, lo: 0, hi: 0, phys: 0, blocks: 0,
            crcs: [0; RUN_BLOCKS], staging: Vec::new(),
        }
    }
    fn end(&self) -> u64 {
        self.base + self.buf.len() as u64
    }
    /// The part of `m` this read wants, as file offsets.
    fn wanted(&self, m: &Mapped) -> Option<(u64, u64)> {
        let (lo, hi) = (self.base.max(m.offset), self.end().min(m.end));
        (lo < hi).then_some((lo, hi))
    }
    /// Fill the hole with 00 bytes up to file offset `to`, which must not precede `done`.
    fn zero_to(&mut self, to: u64) -> Result<usize, ReadError> {
        let at = (to - self.base) as usize;
        if at < self.done {
            return Err(ReadError::Metadata(Error::EIO));
        }
        self.buf[self.done..at].fill(0);
        Ok(at)
    }
    /// Add the wanted blocks of a device extent, checked against `crcs`
    /// (the REF's per-block checksums), to the pending run or new ones.
    fn push(&mut self, m: Mapped, crcs: &[u32]) -> Result<(), ReadError> {
        let Some((lo, hi)) = self.wanted(&m) else { return Ok(()) };
        let size = BLOCK_SIZE as u64;
        let mut index = (lo - m.offset) / size;
        let last = (hi - m.offset).div_ceil(size);
        while index < last {
            preemption_point();
            let logical = m.offset + index * size;
            let within = u64::from(m.extent.extent_offset) + index;
            let phys = m.extent.phys_start.checked_add(within).ok_or(ReadError::Metadata(Error::EIO))?;
            let crc = *usize::try_from(within).ok().and_then(|i| crcs.get(i))
                .ok_or(ReadError::Metadata(Error::EIO))?;
            let joins = self.blocks != 0
                && self.blocks < RUN_BLOCKS
                && self.phys.checked_add(self.blocks as u64) == Some(phys)
                && self.first + self.blocks as u64 * size == logical
                && self.hi == logical;
            if !joins {
                self.flush()?;
                self.first = logical;
                self.phys = phys;
                self.lo = logical.max(lo);
            }
            self.crcs[self.blocks] = crc;
            self.blocks += 1;
            self.hi = (logical + size).min(hi);
            index += 1;
        }
        Ok(())
    }
    /// Copy the wanted part of an extent whose data has not reached the device.
    fn private(&mut self, m: Mapped, bytes: &[u8]) -> Result<(), ReadError> {
        let Some((lo, hi)) = self.wanted(&m) else { return Ok(()) };
        self.flush()?;
        let at = self.zero_to(lo)?;
        let from = u64::from(m.extent.extent_offset) * BLOCK_SIZE as u64 + (lo - m.offset);
        let length = (hi - lo) as usize;
        let source = usize::try_from(from).ok()
            .and_then(|from| bytes.get(from..from.checked_add(length)?))
            .ok_or(ReadError::Data)?;
        self.buf[at..at + length].copy_from_slice(source);
        self.done = at + length;
        Ok(())
    }
    /// Issue the pending run as one device request and check its blocks.
    fn flush(&mut self) -> Result<(), ReadError> {
        if self.blocks == 0 {
            return Ok(());
        }
        let blocks = core::mem::take(&mut self.blocks);
        let length = blocks * BLOCK_SIZE;
        let at = self.zero_to(self.lo)?;
        let skip = (self.lo - self.first) as usize;
        let take = (self.hi - self.lo) as usize;
        let direct = skip == 0 && take == length;
        if !direct && self.staging.len() < length {
            self.staging
                .try_reserve_exact(length - self.staging.len())
                .map_err(|_| ReadError::Device(crate::block::BlockError::OutOfMemory))?;
            self.staging.resize(length, 0);
        }
        let bytes = if direct { &mut self.buf[at..at + length] } else { &mut self.staging[..length] };
        commit::read_into(self.device, self.phys, bytes).map_err(|error| match error {
            commit::Error::BlockRead(error) => ReadError::Device(error),
            _ => ReadError::Data,
        })?;
        let corrupt = bytes
            .chunks_exact(BLOCK_SIZE)
            .zip(&self.crcs[..blocks])
            .position(|(block, expected)| crc32c::checksum(block) != *expected);
        #[cfg(feature = "breenfs_corrupt_inject")]
        if let Some(n) = corrupt {
            crate::serial_println!(
                "\n[breenfs] corrupt-read block={} result=ReadError::Data", self.phys + n as u64);
        }
        if corrupt.is_some() {
            return Err(ReadError::Data);
        }
        if !direct {
            self.buf[at..at + take].copy_from_slice(&self.staging[skip..skip + take]);
        }
        self.done = at + take;
        Ok(())
    }
    /// Issue the last run and fill the hole after it with 00 bytes.
    fn finish(mut self) -> Result<(), ReadError> {
        self.flush()?;
        self.buf[self.done..].fill(0);
        Ok(())
    }
}
#[derive(Clone, Default)]
pub struct SetAttr {
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub atime: Option<Time>,
    pub mtime: Option<Time>,
}

// DIR_ENTRY is ordered by name hash, DIR_INDEX by cookie. Keep the most
// recently searched directory per Files view, bounded to 512 v1 names.
// A larger directory or an allocation failure uses a streaming lookup.
const COOKIE_CACHE_ENTRIES: usize = 512;
struct DirectoryCookies {
    dir: u64,
    names: Vec<(Vec<u8>, u64)>,
}

impl DirectoryCookies {
    fn new(dir: u64) -> Option<Self> {
        let mut names = Vec::new();
        names.try_reserve_exact(COOKIE_CACHE_ENTRIES).ok()?;
        Some(Self { dir, names })
    }

    fn position(&self, name: &[u8]) -> Result<usize, usize> {
        self.names.binary_search_by(|(cached, _)| cached.as_slice().cmp(name))
    }

    fn insert(&mut self, position: usize, name: &[u8], cookie: u64) -> bool {
        let mut owned = Vec::new();
        if self.names.len() == COOKIE_CACHE_ENTRIES || owned.try_reserve_exact(name.len()).is_err() {
            return false;
        }
        owned.extend_from_slice(name);
        self.names.insert(position, (owned, cookie));
        true
    }
}

// In a persistent mount, default-root inodes are tagged only in memory.
// Both trees retain their on-disk inode numbers and share one SPACE transaction.
pub(super) const DEFAULT_TREE: u64 = 1 << 63;

pub struct Files<'a, S: NodeStore + 'a> {
    txn: Transaction<'a, S>,
    root: Root,
    root_id: u64,
    default: Option<Root>,
    // Shared copy-on-write with a scratch fork, so freeze copies no set.
    roots: Arc<BTreeMap<u64, Root>>,
    next_root_id: u64,
    changed_roots: BTreeSet<u64>,
    writable: bool,
    hash_key: [u8; 16],
    cookies: Option<DirectoryCookies>,
    // Non-private extents retain their generation's immutable REF tree. New
    // extents are served from data until the next Files view is opened.
    space_root: BlockPtr,
    data: BTreeMap<u64, Vec<u8>>,
    open_inodes: Inodes,
    orphans: Inodes,
    orphan_work: Inodes,
    unvalidated_orphans: Inodes,
    // Cleanup newly deferred by this transaction, funded by its namespace grants.
    orphan_count: u64,
    // Decoded EXTENT items of recently read inodes; see ExtentMaps.
    extent_maps: RefCell<ExtentMaps>,
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
            default: None,
            roots: Arc::default(),
            next_root_id: sb.next_root_id,
            changed_roots: BTreeSet::new(),
            writable: true,
            hash_key: sb.dirhash_key,
            cookies: None,
            space_root: sb.space_root,
            data: BTreeMap::new(),
            open_inodes: Inodes::default(),
            orphans: Inodes::default(),
            orphan_work: Inodes::default(),
            unvalidated_orphans: Inodes::default(),
            orphan_count: 0,
            extent_maps: RefCell::default(),
        };
        files.put_inode(
            2,
            root_inode(1, sb.mkfs_time),
        )?;
        Ok(files)
    }
    /// Import source timestamps after data and children, before mkfs publication.
    /// Later transactions must use setattr's normal ctime/btime semantics.
    pub fn set_initial_times(&mut self, ino: u64, times: [Time; 4]) -> Result<(), Error> {
        self.require_writable()?;
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
    pub fn new(store: S, sb: &Superblock, root_id: u64, writable: bool) -> Result<Self, Error> {
        let entries = recover::space_items(&store, sb)?;
        #[cfg(test)]
        super::commit::publication_measurement::files_view(entries.len());
        let roots: BTreeMap<_, _> = entries
            .into_iter()
            .filter(|(k, _)| { preemption_point(); k.item_type == format::ROOT })
            .map(|(k, bytes)| Ok((k.offset, Root::decode(&bytes)?)))
            .collect::<Result<_, Error>>()?;
        let root = roots.get(&root_id).ok_or(Error::ENOENT)?.clone();
        if writable && root.flags & 1 != 0 {
            return Err(Error::EROFS);
        }
        // Initial recovery reads the ORPHAN range once. Continuations retain
        // this index; later publications visit only newly eligible work.
        let orphans: BTreeSet<_> = btree::range(&store, root.root,
            key(ORPHAN_OBJECT, format::ORPHAN, 0),
            key(ORPHAN_OBJECT, format::ORPHAN, u64::MAX))?
            .into_iter().map(|(key, _)| key.offset).collect();
        Ok(Self {
            txn: Transaction::new_owned(
                store,
                sb.space_root,
                sb.generation.checked_add(1).ok_or(Error::EIO)?,
            )?,
            root,
            root_id,
            default: None,
            roots: Arc::new(roots),
            next_root_id: sb.next_root_id,
            changed_roots: BTreeSet::new(),
            writable,
            hash_key: sb.dirhash_key,
            cookies: None,
            space_root: sb.space_root,
            data: BTreeMap::new(),
            open_inodes: Inodes::default(),
            orphan_work: Arc::new(orphans.clone()),
            unvalidated_orphans: Arc::new(orphans.clone()),
            orphans: Arc::new(orphans),
            orphan_count: 0,
            extent_maps: RefCell::default(),
        })
    }
    pub fn include_default_tree(&mut self) -> Result<(), Error> {
        if self.root_id == 1 || !self.writable {
            return Err(Error::EINVAL);
        }
        let default = self.roots.get(&1).ok_or(Error::ENOENT)?.clone();
        if default.flags & 1 != 0 || default.next_inode >= DEFAULT_TREE
            || self.root.next_inode >= DEFAULT_TREE
        {
            return Err(Error::EIO);
        }
        let orphans = btree::range(&self.txn.view(), default.root,
            key(ORPHAN_OBJECT, format::ORPHAN, 0),
            key(ORPHAN_OBJECT, format::ORPHAN, u64::MAX))?;
        for (record, _) in orphans {
            if record.offset >= DEFAULT_TREE { return Err(Error::EIO); }
            let ino = record.offset | DEFAULT_TREE;
            Arc::make_mut(&mut self.orphans).insert(ino);
            Arc::make_mut(&mut self.orphan_work).insert(ino);
            Arc::make_mut(&mut self.unvalidated_orphans).insert(ino);
        }
        self.default = Some(default);
        Ok(())
    }

    pub fn includes_default_tree(&self) -> bool { self.default.is_some() }

    pub(super) fn tag(&self, ino: u64) -> u64 {
        if self.default.is_some() { ino & DEFAULT_TREE } else { 0 }
    }

    fn tree(&self, ino: u64) -> &Root {
        if self.tag(ino) == 0 { &self.root } else { self.default.as_ref().unwrap() }
    }

    fn tree_mut(&mut self, ino: u64) -> (&mut Transaction<'a, S>, &mut Root) {
        if self.tag(ino) == 0 {
            (&mut self.txn, &mut self.root)
        } else {
            (&mut self.txn, self.default.as_mut().unwrap())
        }
    }

    fn inode_key(&self, ino: u64, kind: u8, offset: u64) -> Key {
        key(ino ^ self.tag(ino), kind, offset)
    }

    // An EXTENT change is the inode's write generation for its extent map.
    fn insert(&mut self, ino: u64, key: Key, bytes: &[u8]) -> Result<(), Error> {
        if key.item_type == format::EXTENT {
            self.extent_maps.get_mut().invalidate(ino);
        }
        let (txn, root) = self.tree_mut(ino);
        txn.insert(&mut root.root, key, bytes)?;
        Ok(())
    }

    fn delete(&mut self, ino: u64, key: Key) -> Result<(), Error> {
        if key.item_type == format::EXTENT {
            self.extent_maps.get_mut().invalidate(ino);
        }
        let (txn, root) = self.tree_mut(ino);
        txn.delete(&mut root.root, key)?;
        Ok(())
    }

    fn namespace_ino(&self, context: u64, local: u64) -> Result<u64, Error> {
        if self.default.is_some() && local >= DEFAULT_TREE {
            return Err(Error::EIO);
        }
        Ok(local | self.tag(context))
    }

    fn decode_inode(&self, ino: u64, value: &[u8]) -> Result<Inode, Error> {
        let mut inode = Inode::decode(value)?;
        if inode.parent != 0 {
            inode.parent = self.namespace_ino(ino, inode.parent)?;
        }
        Ok(inode)
    }

    /// Freeze generation N (E24 coordinator 597/601(1)) and return N+1's
    /// scratch Files view over frozen N and `store`, the published N-1 store.
    /// Constant work: the transaction's private maps move into one shared
    /// immutable layer, and the open/orphan/ROOT indexes are shared
    /// copy-on-write. This view keeps preparing N; the fork only describes N+1
    /// for foreground callers until N+1's changes are replayed onto N's
    /// published view. Its data map starts empty: N's private data is written
    /// only by preparation.
    pub fn fork(&mut self, store: S) -> Result<Self, Error> {
        Ok(Self {
            txn: self.txn.fork(store)?,
            root: self.root.clone(),
            root_id: self.root_id,
            default: self.default.clone(),
            roots: self.roots.clone(),
            next_root_id: self.next_root_id,
            changed_roots: BTreeSet::new(),
            writable: self.writable,
            hash_key: self.hash_key,
            // Resolve cookies against the scratch tree when it first needs
            // them. The publisher keeps its own cache through continuation.
            cookies: None,
            space_root: self.space_root,
            data: BTreeMap::new(),
            open_inodes: self.open_inodes.clone(),
            orphans: self.orphans.clone(),
            orphan_work: self.orphan_work.clone(),
            unvalidated_orphans: self.unvalidated_orphans.clone(),
            orphan_count: 0,
            // Built on first read of the scratch tree, as cookies are.
            extent_maps: RefCell::default(),
        })
    }
    /// The stored INODE item, whether or not the inode is still linked.
    pub fn stored_inode(&self, ino: u64) -> Result<Inode, Error> {
        let value = btree::search(&self.txn.view(), self.tree(ino).root, self.inode_key(ino, format::INODE, 0))?
            .ok_or(Error::ENOENT)?;
        self.decode_inode(ino, &value)
    }
    /// Continue from this generation's COW state. The immutable node store
    /// supplies filled pointers; ROOT and DEAD indexes retain their custody.
    /// No mount/recovery scan belongs on this publication path.
    pub fn next_generation(&mut self, store: S, sb: &Superblock) -> Result<(), Error> {
        let generation = sb.generation.checked_add(1).ok_or(Error::EIO)?;
        #[cfg(test)]
        super::commit::publication_measurement::files_view(self.changed_roots.len());
        for &id in &self.changed_roots {
            preemption_point();
            let root = persisted_root(&store, sb, id)?;
            if id == self.root_id { self.root = root.clone(); }
            if id == 1 && self.default.is_some() { self.default = Some(root.clone()); }
            Arc::make_mut(&mut self.roots).insert(id, root);
        }
        let space_nodes = self.txn.space_node_count();
        self.txn = Transaction::new_owned_with_dead(store, sb.space_root, generation,
            self.txn.take_dead_nodes(), space_nodes)?;
        self.space_root = sb.space_root;
        self.next_root_id = sb.next_root_id;
        self.changed_roots.clear();
        self.orphan_count = 0;
        Ok(())
    }
    fn require_writable(&self) -> Result<(), Error> {
        if self.writable { Ok(()) } else { Err(Error::EROFS) }
    }
    /// The caller closes dirty-page admission before taking this tree view.
    /// Share only the root; descendants acquire extra references on later COW.
    pub fn snapshot(&mut self, name: &[u8], read_only: bool) -> Result<u64, Error> {
        self.require_writable()?;
        valid_root_name(name)?;
        if self.root.name == name || self.roots.values().any(|root| root.name == name) {
            return Err(Error::EEXIST);
        }
        let id = self.next_root_id;
        if id <= 1 || self.roots.contains_key(&id) {
            return Err(Error::EIO);
        }
        let next = id.checked_add(1).ok_or(Error::ENOSPC)?;
        let mut root = self.root.clone();
        root.source_root = self.root_id;
        root.created_generation = self.txn.generation();
        root.flags = u32::from(read_only);
        root.name = name.to_vec();
        self.txn.increment(root.root.addr, 4)?;
        self.txn.update_root(id, root.clone())?;
        Arc::make_mut(&mut self.roots).insert(id, root);
        self.changed_roots.insert(id);
        self.next_root_id = next;
        Ok(id)
    }
    /// Allocate an independent filesystem tree for a staged installation.
    pub fn create_empty_root(&mut self, name: &[u8], at: Time) -> Result<u64, Error> {
        self.require_writable()?;
        valid_root_name(name)?;
        valid_time(at)?;
        if self.root.name == name || self.roots.values().any(|root| root.name == name) {
            return Err(Error::EEXIST);
        }
        let id = self.next_root_id;
        if id <= 1 || self.roots.contains_key(&id) {
            return Err(Error::EIO);
        }
        let next = id.checked_add(1).ok_or(Error::ENOSPC)?;
        let generation = self.txn.generation();
        let uuid = self.txn.view().read_node(self.root.root)?.header.fs_uuid;
        let mut root = self.txn.create_tree(uuid)?;
        self.txn.insert(&mut root, key(2, format::INODE, 0), &root_inode(generation, at).encode())?;
        let root = Root {
            root, next_inode: 16, source_root: 0, created_generation: generation,
            ctime: at, flags: 0, name: name.to_vec(),
        };
        self.txn.update_root(id, root.clone())?;
        Arc::make_mut(&mut self.roots).insert(id, root);
        self.changed_roots.insert(id);
        self.next_root_id = next;
        Ok(id)
    }
    /// Publish a staged tree by changing its ROOT name in one transaction.
    pub fn rename_root(&mut self, from: &[u8], to: &[u8]) -> Result<(), Error> {
        self.require_writable()?;
        valid_root_name(from)?;
        valid_root_name(to)?;
        let (&id, root) = self.roots.iter().find(|(_, root)| root.name == from)
            .ok_or(Error::ENOENT)?;
        if id == 1 || id == self.root_id {
            return Err(Error::EBUSY);
        }
        if self.roots.values().any(|root| root.name == to) {
            return Err(Error::EEXIST);
        }
        let mut root = root.clone();
        root.name = to.to_vec();
        self.txn.update_root(id, root.clone())?;
        Arc::make_mut(&mut self.roots).insert(id, root);
        self.changed_roots.insert(id);
        Ok(())
    }
    /// Remove the incoming ROOT reference; a zero-count node keeps its children
    /// under DEAD ownership until a later reaper step (design 2.6).
    pub fn delete_root(&mut self, name: &[u8]) -> Result<(), Error> {
        self.require_writable()?;
        valid_root_name(name)?;
        if self.root.name == name {
            return Err(Error::EBUSY);
        }
        let (&id, root) = self.roots.iter().find(|(_, root)| root.name == name)
            .ok_or(Error::ENOENT)?;
        if id == 1 {
            return Err(Error::EBUSY);
        }
        self.txn.release(root.root.addr, 4, true)?;
        self.txn.delete_root(id)?;
        Arc::make_mut(&mut self.roots).remove(&id);
        self.changed_roots.remove(&id);
        Ok(())
    }
    /// The mount snapshots live VNodes under its mutex; drops perform no I/O.
    pub fn retain_open_inodes(&mut self, inodes: BTreeSet<u64>) {
        let previous = core::mem::replace(&mut self.open_inodes, Arc::new(inodes));
        for &ino in previous.symmetric_difference(&self.open_inodes) {
            if self.orphans.contains(&ino) {
                if self.open_inodes.contains(&ino) && !self.unvalidated_orphans.contains(&ino) {
                    Arc::make_mut(&mut self.orphan_work).remove(&ino);
                } else { Arc::make_mut(&mut self.orphan_work).insert(ino); }
            }
        }
    }
    /// Update one VNode lifetime without rebuilding the open-inode snapshot.
    pub fn set_inode_open(&mut self, ino: u64, open: bool) {
        if open { Arc::make_mut(&mut self.open_inodes).insert(ino); }
        else { Arc::make_mut(&mut self.open_inodes).remove(&ino); }
        if self.orphans.contains(&ino) {
            if open && !self.unvalidated_orphans.contains(&ino) {
                Arc::make_mut(&mut self.orphan_work).remove(&ino);
            } else { Arc::make_mut(&mut self.orphan_work).insert(ino); }
        }
    }
    pub fn items(&self) -> Result<Vec<(Key, Vec<u8>)>, Error> {
        items(&self.txn.view(), self.root.root)
    }
    /// Describe COW paths without changing the private transaction.
    pub fn reservation_paths(&self, ino: u64) -> Result<txn::ReservationPaths, Error> {
        Ok(self.txn.reservation_paths(self.tree(ino).root, ino ^ self.tag(ino))?)
    }
    pub fn root_pointer(&self) -> BlockPtr { self.root.root }
    /// Project a pinned root through the same transaction's read paths. Open
    /// orphans in the writer's inode number space do not belong to this root.
    pub fn with_read_root<T>(
        &mut self,
        id: u64,
        closed: &Inodes,
        read: impl FnOnce(&Self) -> T,
    ) -> Result<T, Error> {
        let root = self.roots.get(&id).ok_or(Error::ENOENT)?.root;
        let original = core::mem::replace(&mut self.root.root, root);
        let opened = core::mem::replace(&mut self.open_inodes, closed.clone());
        // The writer's maps describe its own root, not the projected one.
        let maps = core::mem::replace(self.extent_maps.get_mut(), ExtentMaps::disabled());
        let value = read(self);
        *self.extent_maps.get_mut() = maps;
        self.open_inodes = opened;
        self.root.root = original;
        Ok(value)
    }
    pub fn root_growth(&self, root: BlockPtr) -> Result<btree::RootGrowth, Error> {
        Ok(btree::RootGrowth::read(&self.txn.view(), root)?)
    }
    pub fn has_orphans(&self) -> bool { !self.orphans.is_empty() }
    /// Namespace edits can reach parents, victims, the next inode and ORPHAN.
    /// Count their COW paths before publishing any item changes. Shared ancestors
    /// may be counted more than once, making this an upper bound.
    pub fn metadata_reservation_paths(
        &self,
        objects: &[u64],
        new_inode: bool,
        orphans: bool,
    ) -> Result<txn::ReservationPaths, Error> {
        let mut objects: BTreeSet<_> = objects.iter().copied().collect();
        let dir = objects.first().copied().or((new_inode || orphans).then_some(2))
            .ok_or(Error::EINVAL)?;
        let tag = self.tag(dir);
        if objects.iter().any(|ino| self.tag(*ino) != tag) {
            return Err(Error::EINVAL);
        }
        if new_inode { objects.insert(self.tree(dir).next_inode | tag); }
        let mut paths = self.reservation_paths(dir)?;
        paths.uncopied_fs_nodes = 0;
        for object in objects {
            paths.uncopied_fs_nodes = paths.uncopied_fs_nodes.checked_add(
                self.reservation_paths(object)?.uncopied_fs_nodes,
            ).ok_or(Error::ENOSPC)?;
        }
        if orphans {
            paths.uncopied_fs_nodes = paths.uncopied_fs_nodes.checked_add(
                self.txn.reservation_paths(self.tree(dir).root, ORPHAN_OBJECT)?.uncopied_fs_nodes,
            ).ok_or(Error::ENOSPC)?;
        }
        Ok(paths)
    }
    /// Newly deferred orphan cleanup belongs to this transaction's grants;
    /// existing ORPHAN items are already part of the durable filesystem.
    pub fn metadata_debt(&self) -> Result<u64, Error> {
        Ok(self.metadata_estimate()?.blocks().map_err(txn::Error::from)?)
    }
    pub(super) fn metadata_estimate(&self) -> Result<super::reserve::Estimate, Error> {
        let root = self.default.as_ref().filter(|root| root.root.level > self.root.root.level)
            .unwrap_or(&self.root);
        let mut estimate = self.txn.metadata_estimate(root.root, self.orphan_count)?;
        if self.default.is_some() {
            estimate.space_changes = estimate.space_changes.checked_add(3).ok_or(Error::ENOSPC)?;
        }
        Ok(estimate)
    }
    /// Quarantine of the prepared generation, before opening its successor.
    pub fn pending_summary(&self) -> space::PendingSummary {
        self.txn.pending_summary()
    }
    /// Additional deferred orphan paths if a growing edit can add a root level.
    pub fn orphan_growth_paths(&self, dir: u64, growing_items: usize) -> Result<u64, Error> {
        if self.orphan_count == 0 || !btree::may_grow_root(&self.txn.view(), self.tree(dir).root, growing_items)? {
            return Ok(0);
        }
        self.orphan_count.checked_mul(3 * 4).ok_or(Error::ENOSPC)
    }
    pub fn inode(&self, ino: u64) -> Result<Inode, Error> {
        let inode = self.stored_inode(ino)?;
        if inode.nlink == 0 && !self.open_inodes.contains(&ino) {
            return Err(Error::ENOENT);
        }
        Ok(inode)
    }
    pub(super) fn put_inode(&mut self, ino: u64, mut inode: Inode) -> Result<(), Error> {
        #[cfg(test)]
        btree::measurement::mutation(4, 1);
        self.require_writable()?;
        inode.parent ^= self.tag(inode.parent);
        self.insert(ino, self.inode_key(ino, format::INODE, 0), &inode.encode())
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
        let k = self.inode_key(dir, format::DIR_ENTRY, siphash::hash(&self.hash_key, name));
        let bytes = btree::search(&self.txn.view(), self.tree(dir).root, k)?.unwrap_or_default();
        Ok((k, dir_records(&bytes)?))
    }
    pub fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Error> {
        valid_name(name)?;
        self.directory(dir)?;
        self.bucket(dir, name)?
            .1
            .into_iter()
            .find(|r| r.name == name)
            .ok_or(Error::ENOENT)
            .and_then(|record| self.namespace_ino(dir, record.ino))
    }
    /// DIR_INDEX order is the persistent directory-cookie order. A read-only
    /// caller can bound each batch without materializing the whole directory.
    pub fn directory_entries(
        &self,
        dir: u64,
        cookie: u64,
        limit: usize,
    ) -> Result<Vec<(u64, DirRecord)>, Error> {
        let mut out = Vec::new();
        if limit != 0 {
            self.visit_directory_entries(dir, cookie, &mut |next, entry| {
                out.push((next, entry));
                out.len() < limit
            })?;
        } else {
            self.directory(dir)?;
        }
        Ok(out)
    }
    /// Stream DIR_INDEX records without restarting at internal batch boundaries.
    /// The caller keeps the tree stable for the duration of the sink call.
    pub fn visit_directory_entries(
        &self,
        dir: u64,
        cookie: u64,
        emit: &mut dyn FnMut(u64, DirRecord) -> bool,
    ) -> Result<(), Error> {
        let inode = self.directory(dir)?;
        btree::visit_range(
            &self.txn.view(),
            self.tree(dir).root,
            self.inode_key(dir, format::DIR_INDEX, cookie.max(2)),
            self.inode_key(dir, format::DIR_INDEX, u64::MAX),
            &mut |k, bytes| {
                if k.offset >= inode.next_index {
                    return Err(Error::EIO);
                }
                let mut records = dir_records(bytes)?;
                if records.len() != 1 {
                    return Err(Error::EIO);
                }
                let mut record = records.remove(0);
                record.ino = self.namespace_ino(dir, record.ino).map_err(|_| btree::Error::InvalidNode)?;
                Ok(emit(k.offset + 1, record))
            },
        )
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
            ino: ino ^ self.tag(ino),
            kind,
            name: name.to_vec(),
        }
        .encode();
        bytes.extend_from_slice(&record);
        self.insert(dir, k, &bytes)?;
        self.insert(dir, self.inode_key(dir, format::DIR_INDEX, parent.next_index), &record)?;
        let cookie = parent.next_index;
        parent.next_index += 1;
        parent.size += 1;
        if kind == 2 {
            parent.nlink += 1;
        }
        parent.mtime = at;
        parent.ctime = at;
        self.put_inode(dir, parent)?;
        if let Some(cache) = self.cookies.as_mut().filter(|cache| cache.dir == dir) {
            let inserted = cache.position(name).err()
                .is_some_and(|position| cache.insert(position, name, cookie));
            if !inserted {
                self.cookies = None;
            }
        }
        Ok(())
    }

    fn directory_cookie(&mut self, dir: u64, parent: &Inode, name: &[u8]) -> Result<u64, Error> {
        if let Some(cache) = self.cookies.as_ref().filter(|cache| cache.dir == dir) {
            let position = cache.position(name).map_err(|_| Error::EIO)?;
            return Ok(cache.names[position].1);
        }
        self.cookies = None;
        let mut cache = if parent.size <= COOKIE_CACHE_ENTRIES as u64 {
            DirectoryCookies::new(dir)
        } else {
            None
        };
        let mut cookie = None;
        btree::visit_range(
            &self.txn.view(),
            self.tree(dir).root,
            self.inode_key(dir, format::DIR_INDEX, 0),
            self.inode_key(dir, format::DIR_INDEX, u64::MAX),
            &mut |key, bytes| {
                let entry = DirRecord::decode_name(bytes)?;
                if bytes.len() != 10 + entry.len() || !(2..parent.next_index).contains(&key.offset) {
                    return Err(Error::EIO);
                }
                if entry == name && cookie.replace(key.offset).is_some() {
                    return Err(Error::EIO);
                }
                if let Some(building) = &mut cache {
                    if !building.insert(building.names.len(), entry, key.offset) {
                        cache = None;
                    }
                }
                Ok(cache.is_some() || cookie.is_none())
            },
        )?;
        if let Some(cache) = &mut cache {
            cache.names.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            if cache.names.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err(Error::EIO);
            }
        }
        self.cookies = cache;
        cookie.ok_or(Error::EIO)
    }

    fn remove_entry(&mut self, dir: u64, name: &[u8], at: Time) -> Result<DirRecord, Error> {
        let mut parent = self.directory(dir)?;
        let (k, mut records) = self.bucket(dir, name)?;
        let pos = records
            .iter()
            .position(|r| r.name == name)
            .ok_or(Error::ENOENT)?;
        let record = records.remove(pos);
        let cookie = self.directory_cookie(dir, &parent, name)?;
        let index = self.inode_key(dir, format::DIR_INDEX, cookie);
        let indexed = btree::search(&self.txn.view(), self.tree(dir).root, index)?.ok_or(Error::EIO)?;
        if indexed != record.encode() { return Err(Error::EIO); }
        if records.is_empty() {
            self.delete(dir, k)?;
        } else {
            let bytes: Vec<_> = records.iter().flat_map(DirRecord::encode).collect();
            self.insert(dir, k, &bytes)?;
        }
        self.delete(dir, index)?;
        parent.size = parent.size.checked_sub(1).ok_or(Error::EIO)?;
        if record.kind == 2 {
            parent.nlink = parent.nlink.checked_sub(1).ok_or(Error::EIO)?;
        }
        parent.mtime = at;
        parent.ctime = at;
        self.put_inode(dir, parent)?;
        if let Some(cache) = &mut self.cookies {
            if let Ok(position) = cache.position(name) {
                cache.names.remove(position);
            }
        }
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
        self.create_node_prepared(
            dir,
            name,
            Inode {
                mode,
                ..Inode::default()
            },
            target,
            at,
            |_, _| Ok(()),
        )
    }
    /// Prepare an open before publishing its name. Policy and preparation errors
    /// leave the transaction tree unchanged.
    pub fn create_node_prepared(
        &mut self,
        dir: u64,
        name: &[u8],
        mut inode: Inode,
        target: Option<&[u8]>,
        at: Time,
        prepare: impl FnOnce(u64, Inode) -> Result<(), Error>,
    ) -> Result<u64, Error> {
        self.require_writable()?;
        valid_time(at)?;
        let kind = inode_kind(inode.mode)?;
        match (kind, target) {
            (7, Some(t)) if !t.is_empty() && !t.contains(&0) => {
                if t.len() > 4095 {
                    return Err(Error::ENAMETOOLONG);
                }
            }
            (7, _) | (_, Some(_)) => return Err(Error::EINVAL),
            _ => (),
        }
        self.vacant(dir, name, kind)?;
        let local = self.tree(dir).next_inode;
        let ino = local | self.tag(dir);
        let next = local
            .checked_add(1)
            .filter(|&next| local >= 16 && (self.default.is_none() || next < DEFAULT_TREE))
            .ok_or(Error::EMLINK)?;
        inode.nlink = if kind == 2 { 2 } else { 1 };
        inode.parent = if kind == 2 { dir } else { 0 };
        inode.next_index = if kind == 2 { 2 } else { 0 };
        inode.size = target.map_or(0, |t| t.len() as u64);
        inode.created_generation = self.txn.generation();
        inode.atime = at;
        inode.mtime = at;
        inode.ctime = at;
        inode.btime = at;
        prepare(ino, inode)?;
        self.put_inode(ino, inode)?;
        if let Some(target) = target {
            self.insert(ino, self.inode_key(ino, format::INLINE, 0), target)?;
        }
        self.add_entry(dir, name, ino, kind, at)?;
        self.tree_mut(dir).1.next_inode = next;
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
            self.tree(ino).root,
            self.inode_key(ino, format::INLINE, 0),
        )?
        .ok_or(Error::EIO)?;
        if target.len() as u64 != inode.size {
            return Err(Error::EIO);
        }
        Ok(target)
    }
    pub fn link(&mut self, ino: u64, dir: u64, name: &[u8], at: Time) -> Result<(), Error> {
        self.require_writable()?;
        if self.tag(ino) != self.tag(dir) { return Err(Error::EINVAL); }
        let mut inode = self.inode(ino)?;
        // A pathname lookup that raced the last unlink cannot resurrect it.
        if inode.nlink == 0 {
            return Err(Error::ENOENT);
        }
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
        for (k, _) in self.range(ino, self.inode_key(ino, 0, 0), self.inode_key(ino, u8::MAX, u64::MAX))? {
            preemption_point();
            self.delete(ino, k)?;
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
            self.insert(ino, key(ORPHAN_OBJECT, format::ORPHAN, ino ^ self.tag(ino)), &[])?;
            self.orphan_count = self.orphan_count.checked_add(1).ok_or(Error::EIO)?;
            Arc::make_mut(&mut self.orphans).insert(ino);
            if !self.open_inodes.contains(&ino) { Arc::make_mut(&mut self.orphan_work).insert(ino); }
        }
        self.put_inode(ino, inode)
    }
    pub fn unlink(&mut self, dir: u64, name: &[u8], at: Time) -> Result<(), Error> {
        self.require_writable()?;
        let ino = self.lookup(dir, name)?;
        if inode_kind(self.inode(ino)?.mode)? == 2 {
            return Err(Error::EISDIR);
        }
        valid_time(at)?;
        self.remove_entry(dir, name, at)?;
        self.drop_link(ino, at)
    }
    pub fn rmdir(&mut self, dir: u64, name: &[u8], at: Time) -> Result<(), Error> {
        self.require_writable()?;
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
        self.require_writable()?;
        if self.tag(from_dir) != self.tag(to_dir) { return Err(Error::EINVAL); }
        let ino = self.lookup(from_dir, from)?;
        valid_name(to)?;
        let dest_parent = self.directory(to_dir)?;
        let mut inode = self.inode(ino)?;
        let kind = inode_kind(inode.mode)?;
        let (_, bucket) = self.bucket(to_dir, to)?;
        let dest = bucket.iter().find(|r| r.name == to)
            .map(|r| self.namespace_ino(to_dir, r.ino)).transpose()?;
        if kind == 2 {
            let mut ancestor = to_dir;
            let mut seen = BTreeSet::new();
            loop {
                if ancestor == ino {
                    return Err(Error::EINVAL);
                }
                if ancestor == (2 | self.tag(to_dir)) {
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
        let candidates: Vec<_> = self.orphan_work.iter().copied().collect();
        for ino in candidates {
            preemption_point();
            #[cfg(test)] btree::measurement::orphan_visit();
            let k = key(ORPHAN_OBJECT, format::ORPHAN, ino ^ self.tag(ino));
            let value = btree::search(&self.txn.view(), self.tree(ino).root, k)?.ok_or(Error::EIO)?;
            if !value.is_empty() { return Err(Error::EIO); }
            let inode = self.stored_inode(ino).map_err(|error| {
                if error == Error::ENOENT { Error::EIO } else { error }
            })?;
            if inode.nlink != 0 || inode_kind(inode.mode)? == 2 {
                return Err(Error::EIO);
            }
            if self.open_inodes.contains(&ino) {
                Arc::make_mut(&mut self.unvalidated_orphans).remove(&ino);
                Arc::make_mut(&mut self.orphan_work).remove(&ino);
                continue;
            }
            self.delete_inode(ino)?;
            self.delete(ino, k)?;
            Arc::make_mut(&mut self.orphans).remove(&ino);
            Arc::make_mut(&mut self.orphan_work).remove(&ino);
            Arc::make_mut(&mut self.unvalidated_orphans).remove(&ino);
        }
        Ok(())
    }
    fn range(&self, ino: u64, lower: Key, upper: Key) -> Result<Vec<(Key, Vec<u8>)>, Error> {
        Ok(btree::range(
            &self.txn.view(),
            self.tree(ino).root,
            lower,
            upper,
        )?)
    }
    pub(super) fn extents(&self, ino: u64) -> Result<Vec<(Key, Extent)>, Error> {
        self.range(
            ino,
            self.inode_key(ino, format::EXTENT, 0),
            self.inode_key(ino, format::EXTENT, u64::MAX),
        )?
        .into_iter()
        .map(|(k, v)| { preemption_point(); Ok((k, Extent::decode(&v)?)) })
        .collect()
    }
    /// The REF checksums of a data extent on the device.
    fn data_crcs(&self, e: Extent) -> Result<Vec<u32>, ReadError> {
        let r = space::reference(
            &self.txn.view(), self.space_root, e.phys_start, u64::from(e.phys_len),
        ).map_err(|error| Error::from(txn::Error::from(error)))?
            .ok_or(ReadError::Metadata(Error::EIO))?;
        if r.kind != space::Kind::Data as u8 {
            return Err(ReadError::Metadata(Error::EIO));
        }
        Ok(r.crcs)
    }
    /// Decode the inode's EXTENT items in file order, at most `limit` of
    /// them. Gives up, returning no vector, when there are more or the heap refuses it.
    fn decode_extents(&self, ino: u64, limit: usize) -> Result<Option<Vec<Mapped>>, ReadError> {
        let mut extents: Vec<Mapped> = Vec::new();
        let mut complete = true;
        let mut last = 0;
        btree::visit_range(
            &self.txn.view(), self.tree(ino).root,
            self.inode_key(ino, format::EXTENT, 0),
            self.inode_key(ino, format::EXTENT, u64::MAX),
            &mut |key, bytes| {
                preemption_point();
                let m = Mapped::new(key, Extent::decode(bytes).map_err(Error::from)?)?;
                if m.offset < last {
                    return Err(ReadError::Metadata(Error::EIO));
                }
                last = m.end;
                // Grow geometrically, capped at `limit` items.
                let grow = extents.len().clamp(16, limit).min(limit - extents.len());
                #[cfg(test)]
                let cache = cache_allocations::scope();
                let refused = extents.len() == limit
                    || (extents.len() == extents.capacity() && extents.try_reserve_exact(grow).is_err());
                #[cfg(test)]
                drop(cache);
                if refused {
                    complete = false;
                    return Ok(false);
                }
                extents.push(m);
                Ok(true)
            },
        )?;
        Ok(complete.then_some(extents))
    }
    /// Whether `maps` holds a usable map for `ino`, building it on a miss.
    fn load_extent_map(&self, maps: &mut ExtentMaps, ino: u64) -> Result<bool, ReadError> {
        if maps.disabled {
            return Ok(false);
        }
        maps.clock += 1;
        if let Some(map) = maps.maps.get_mut(&ino) {
            map.used = maps.clock;
            return Ok(!map.oversized);
        }
        let limit = (EXTENT_MAP_BYTES - MAP_OVERHEAD) / core::mem::size_of::<Mapped>();
        let (extents, oversized) = match self.decode_extents(ino, limit)? {
            Some(extents) => (extents, false),
            None => (Vec::new(), true),
        };
        let bytes = extents.capacity() * core::mem::size_of::<Mapped>() + MAP_OVERHEAD;
        if !maps.make_room(bytes) {
            return Ok(false);
        }
        #[cfg(test)]
        let cache = cache_allocations::scope();
        maps.maps.insert(ino, ExtentMap {
            extents, oversized, crcs: BTreeMap::new(), bytes, used: maps.clock,
        });
        #[cfg(test)]
        drop(cache);
        maps.bytes += bytes;
        Ok(!oversized)
    }
    /// Read through the inode's map: a binary search finds the first
    /// intersecting item, and REF checksums are kept while the budget allows.
    fn read_mapped(&self, maps: &mut ExtentMaps, ino: u64, runs: &mut Runs<'_>) -> Result<(), ReadError> {
        let ExtentMaps { maps: table, bytes: used, .. } = maps;
        let Some(map) = table.get_mut(&ino) else { return Ok(()) };
        let end = runs.end();
        let mut i = map.extents.partition_point(|m| m.end <= runs.base);
        while let Some(&m) = map.extents.get(i) {
            if m.offset >= end {
                break;
            }
            preemption_point();
            let phys = m.extent.phys_start;
            if let Some(bytes) = self.data.get(&phys) {
                runs.private(m, bytes)?;
            } else if let Some(crcs) = map.crcs.get(&phys) {
                runs.push(m, crcs)?;
            } else {
                let crcs = self.data_crcs(m.extent)?;
                let cost = crcs.len() * 4 + CRCS_OVERHEAD;
                if *used + cost <= EXTENT_MAP_BYTES {
                    *used += cost;
                    map.bytes += cost;
                    #[cfg(test)]
                    let cache = cache_allocations::scope();
                    let kept = map.crcs.entry(phys).or_insert(crcs);
                    #[cfg(test)]
                    drop(cache);
                    runs.push(m, kept)?;
                } else {
                    runs.push(m, &crcs)?;
                }
            }
            i += 1;
        }
        Ok(())
    }
    /// Read without a map: seek to the last item at or before the read's
    /// start, then visit the items up to its end.
    fn read_seek(&self, ino: u64, runs: &mut Runs<'_>) -> Result<(), ReadError> {
        let view = self.txn.view();
        let root = self.tree(ino).root;
        let mut start = self.inode_key(ino, format::EXTENT, runs.base);
        btree::visit_range_reverse(
            &view, root, self.inode_key(ino, format::EXTENT, 0), start,
            &mut |key, _| {
                start = key;
                Ok::<_, ReadError>(false)
            },
        )?;
        let upper = self.inode_key(ino, format::EXTENT, runs.end() - 1);
        let mut last = 0;
        btree::visit_range(&view, root, start, upper, &mut |key, bytes| {
            preemption_point();
            let m = Mapped::new(key, Extent::decode(bytes).map_err(Error::from)?)?;
            if m.offset < last {
                return Err(ReadError::Metadata(Error::EIO));
            }
            last = m.end;
            if let Some(bytes) = self.data.get(&m.extent.phys_start) {
                runs.private(m, bytes)?;
            } else if runs.wanted(&m).is_some() {
                runs.push(m, &self.data_crcs(m.extent)?)?;
            }
            Ok(true)
        })
    }
    pub fn read(
        &self,
        device: &dyn BlockDevice,
        ino: u64,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, Error> {
        self.read_verified(device, ino, offset, buf).map_err(ReadError::error)
    }
    /// Read file bytes at `offset`, at most to EOF. Holes read as 00 bytes.
    pub(super) fn read_verified(
        &self,
        device: &dyn BlockDevice,
        ino: u64,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ReadError> {
        let inode = self.regular(ino)?;
        let n = inode.size.saturating_sub(offset).min(buf.len() as u64) as usize;
        let mut runs = Runs::new(device, offset, &mut buf[..n]);
        if n != 0 {
            // A read holds its view's maps for its duration; a nested borrow
            // would read through the seek instead.
            let mapped = match self.extent_maps.try_borrow_mut() {
                Ok(mut maps) => {
                    let usable = self.load_extent_map(&mut maps, ino)?;
                    if usable {
                        self.read_mapped(&mut maps, ino, &mut runs)?;
                    }
                    usable
                }
                Err(_) => false,
            };
            if !mapped {
                self.read_seek(ino, &mut runs)?;
            }
        }
        runs.finish()?;
        Ok(n)
    }
    // Logical runs are bounded to 1 MiB (design 1.2); fragmented FREE space
    // can require several physical extents for one such run.
    fn replace_extent(&mut self, ino: u64, off: u64, bytes: &[u8]) -> Result<(), Error> {
        let crcs: Vec<_> = bytes
            .chunks_exact(BLOCK_SIZE)
            .map(|bytes| { preemption_point(); crc32c::checksum(bytes) })
            .collect();
        let mut done = 0;
        while done < crcs.len() {
            preemption_point();
            let (addr, blocks) = self.txn.allocate_data_run(&crcs[done..])?;
            let next = done + blocks as usize;
            self.data
                .insert(addr, bytes[done * BLOCK_SIZE..next * BLOCK_SIZE].to_vec());
            self.insert(
                ino,
                self.inode_key(ino, format::EXTENT, off + (done * BLOCK_SIZE) as u64),
                &Extent {
                    phys_start: addr,
                    phys_len: blocks as u32,
                    extent_offset: 0,
                    num_blocks: blocks as u32,
                    flags: 0,
                }
                .encode(),
            )?;
            done = next;
        }
        Ok(())
    }
    // Remove intersecting items. The blocks of an item that start before
    // `start` or at or after `end` stay, as at most two bookend items naming
    // the old extent: its head and its tail (audit R7's bookend items). No
    // format change: an item already carries its own extent_offset and
    // num_blocks. Each bookend needs its own reference; txn::delete releases
    // the old one.
    fn remove_range(&mut self, ino: u64, start: u64, end: u64) -> Result<(), Error> {
        if start >= end {
            return Ok(());
        }
        let size = BLOCK_SIZE as u64;
        for (k, e) in self.extents(ino)? {
            preemption_point();
            let blocks = u64::from(e.num_blocks);
            let extent_end = k.offset + blocks * size;
            if k.offset >= end || extent_end <= start {
                continue;
            }
            // Blocks [0, head) start before `start`; blocks [tail, blocks)
            // start at or after `end`.
            let head = start.saturating_sub(k.offset).div_ceil(size).min(blocks);
            let tail = end.saturating_sub(k.offset).div_ceil(size).min(blocks);
            for (first, last) in [(0, head), (tail, blocks)] {
                if first >= last {
                    continue;
                }
                self.txn.increment(e.phys_start, u64::from(e.phys_len))?;
                let bookend = Extent {
                    extent_offset: e.extent_offset + first as u32,
                    num_blocks: (last - first) as u32,
                    ..e
                };
                // The head replaces the old key (same REF); the old incoming
                // reference is explicitly released below.
                self.insert(
                    ino,
                    self.inode_key(ino, format::EXTENT, k.offset + first * size),
                    &bookend.encode(),
                )?;
            }
            if k.offset < start {
                // Same-key replacement retained its old reference in txn::insert.
                self.txn
                    .release(e.phys_start, u64::from(e.phys_len), false)?;
            } else {
                self.delete(ino, k)?;
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
        self.require_writable()?;
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
        // Boundary blocks retain old bytes. Replaced blocks may be
        // corrupt and must not be read (including dirty-page commit writeback).
        if offset != start {
            self.read(device, ino, start, &mut blocks[..BLOCK_SIZE])?;
        }
        let tail = (rounded - start) as usize - BLOCK_SIZE;
        if end != rounded && (tail != 0 || offset == start) {
            self.read(device, ino, start + tail as u64, &mut blocks[tail..])?;
        }
        let lo = (offset - start) as usize;
        blocks[lo..lo + bytes.len()].copy_from_slice(bytes);
        self.remove_range(ino, start, rounded)?;
        for (n, run) in blocks.chunks(256 * BLOCK_SIZE).enumerate() {
            preemption_point();
            self.replace_extent(ino, start + (n * 256 * BLOCK_SIZE) as u64, run)?;
        }
        inode.size = inode.size.max(end);
        inode.mtime = at;
        inode.ctime = at;
        inode.nblocks = self
            .extents(ino)?
            .iter()
            .map(|(_, e)| { preemption_point(); u64::from(e.num_blocks) })
            .sum();
        self.put_inode(ino, inode)?;
        Ok(bytes.len())
    }
    /// Drop whole mappings beyond a VNode's new EOF without allocating data.
    /// The adapter retains and zeroes a partial mapped tail in its dirty map.
    pub fn trim_mappings(&mut self, ino: u64, size: u64) -> Result<u64, Error> {
        self.require_writable()?;
        self.remove_range(
            ino,
            size.div_ceil(BLOCK_SIZE as u64) * BLOCK_SIZE as u64,
            u64::MAX,
        )?;
        Ok(self
            .extents(ino)?
            .iter()
            .map(|(_, e)| { preemption_point(); u64::from(e.num_blocks) })
            .sum())
    }
    pub fn truncate(
        &mut self,
        device: &dyn BlockDevice,
        ino: u64,
        size: u64,
        at: Time,
    ) -> Result<(), Error> {
        self.require_writable()?;
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
                self.replace_extent(ino, floor, &b)?;
            }
        }
        inode.size = size;
        inode.mtime = at;
        inode.ctime = at;
        inode.nblocks = self
            .extents(ino)?
            .iter()
            .map(|(_, e)| { preemption_point(); u64::from(e.num_blocks) })
            .sum();
        self.put_inode(ino, inode)
    }
    pub fn setattr(&mut self, ino: u64, attrs: &SetAttr, at: Time) -> Result<(), Error> {
        self.require_writable()?;
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
    pub fn list_xattrs(&self, ino: u64) -> Result<Vec<Xattr>, Error> {
        self.inode(ino)?;
        let mut records = Vec::new();
        for (_, bytes) in self.range(
            ino,
            self.inode_key(ino, format::XATTR, 0),
            self.inode_key(ino, format::XATTR, u64::MAX),
        )? {
            records.extend(xattrs(&bytes)?);
        }
        Ok(records)
    }
    pub fn xattr(
        &mut self,
        ino: u64,
        name: &[u8],
        value: Option<&[u8]>,
        at: Time,
    ) -> Result<(), Error> {
        self.require_writable()?;
        let mut i = self.inode(ino)?;
        valid_time(at)?;
        if name.is_empty()
            || name.len() > 255
            || name.contains(&0)
            || value.is_some_and(|v| v.len() > 4096)
        {
            return Err(Error::EINVAL);
        }
        let k = self.inode_key(ino, format::XATTR, siphash::hash(&self.hash_key, name));
        let old = btree::search(&self.txn.view(), self.tree(ino).root, k)?.unwrap_or_default();
        let mut records = xattrs(&old)?;
        let found = records.iter().position(|x| x.name == name);
        if value.is_none() && found.is_none() {
            return Err(Error::ENODATA);
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
            self.delete(ino, k)?;
        } else {
            self.insert(ino, k, &bytes)?;
        }
        i.ctime = at;
        self.put_inode(ino, i)
    }
    pub fn fsync(
        &mut self,
        device: &dyn BlockDevice,
        committer: &mut Committer,
    ) -> Result<Superblock, Error> {
        self.require_writable()?;
        Ok(committer.commit_files(device, self)?)
    }
    pub(super) fn begin_preparation(&self) {
        self.txn.begin_preparation();
    }
    pub fn prepare_commit(&mut self) -> Result<&mut Transaction<'a, S>, Error> {
        preemption_point();
        self.require_writable()?;
        self.begin_preparation();
        self.reclaim_orphans()?;
        self.txn.promote_pending()?;
        self.txn.update_root(self.root_id, self.root.clone())?;
        self.changed_roots.insert(self.root_id);
        if let Some(default) = &self.default {
            self.txn.update_root(1, default.clone())?;
            self.changed_roots.insert(1);
        }
        self.txn.settle()?;
        Ok(&mut self.txn)
    }
    pub(crate) fn commit_buffers(
        &mut self,
    ) -> (&mut Transaction<'a, S>, &mut BTreeMap<u64, Vec<u8>>) {
        // Reap can also release a private extent, so filter just before
        // serialization after the last settling pass.
        self.data.retain(|addr, _| {
            preemption_point();
            self.txn.data_is_live(*addr)
        });
        (&mut self.txn, &mut self.data)
    }
}
pub(super) const ORPHAN_OBJECT: u64 = 0xffff_ffff_ffff_ff00;
fn root_inode(generation: u64, at: Time) -> Inode {
    Inode {
        mode: 0o40755, nlink: 2, parent: 2, next_index: 2, created_generation: generation,
        atime: at, mtime: at, ctime: at, btime: at, ..Inode::default()
    }
}
fn valid_root_name(name: &[u8]) -> Result<(), Error> {
    if name.contains(&b',') { return Err(Error::EINVAL); }
    valid_name(name).map_err(|_| Error::EINVAL)
}
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
pub(super) fn persisted_root<S: NodeStore>(store: &S, sb: &Superblock, id: u64) -> Result<Root, Error> {
    let bytes = btree::search(store, sb.space_root, key(0xffff_ffff_ffff_ff10, format::ROOT, id))?
        .ok_or(Error::EIO)?;
    Ok(Root::decode(&bytes)?)
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
