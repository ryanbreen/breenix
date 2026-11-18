//! In-memory copy-on-write transactions (design 2.5, commit step 1).
//!
//! Old node bytes stay in the backing store. Edits and the space-tree fixpoint
//! accumulate private writes; only a successful `settle` supplies a space root
//! to the later commit engine. An error poisons the transaction: discard it and
//! its unpublished FS roots. Reservation policy and device I/O are higher layers.
use super::preempt::preemption_point;
use super::{
    btree::{self, NodeStore},
    format::{self, BlockPtr, Extent, InternalEntry, Key, LeafItemHeader, NodeHeader, Root},
    reserve::{Estimate, Grant, Ledger, Operation},
    space::{self, Kind},
};
use alloc::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    vec,
    vec::Vec,
};
use core::cell::Cell;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Invalid,
    Tree(btree::Error),
    Space(space::Error),
}
impl From<btree::Error> for Error {
    fn from(value: btree::Error) -> Self {
        Self::Tree(value)
    }
}
impl From<space::Error> for Error {
    fn from(value: space::Error) -> Self {
        Self::Space(value)
    }
}
impl From<format::Error> for Error {
    fn from(_: format::Error) -> Self {
        Self::Invalid
    }
}

/// Fixed transaction budget from the snapshot deletion contract.
pub const REAP_PER_COMMIT: usize = 16;

/// Mount-owned DEAD address index. Carry it into the next transaction after
/// publication instead of walking SPACE again. Dropping an aborted transaction
/// also discards its unpublished index changes; recovery rebuilds it from disk.
#[derive(Clone, Default)]
pub struct DeadNodes {
    addresses: BTreeSet<u64>,
}

impl DeadNodes {
    pub fn count(&self) -> u64 {
        self.addresses.len() as u64
    }
}

enum Change {
    PromotePending,
    Allocate {
        start: u64,
        length: u64,
        kind: Kind,
        crcs: Vec<u32>,
    },
    Increment {
        start: u64,
        length: u64,
    },
    Release {
        start: u64,
        length: u64,
        children: bool,
    },
    DeleteRoot {
        id: u64,
    },
    Root {
        id: u64,
        value: Root,
    },
}

/// Read-only node view accepted by the B-tree and space readers.
pub struct View<'a, S> {
    base: &'a S,
    nodes: &'a BTreeMap<u64, Vec<u8>>,
}
impl<S: NodeStore> NodeStore for View<'_, S> {
    fn read_node(&self, ptr: BlockPtr) -> Result<alloc::sync::Arc<btree::Node>, btree::Error> {
        match self.nodes.get(&ptr.addr) {
            Some(bytes) => btree::decode_node(bytes, ptr),
            None => self.base.read_node(ptr),
        }
    }
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.nodes
            .get(&addr)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| self.base.get(addr))
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
        Err(btree::Error::Store)
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        Err(btree::Error::Store)
    }
}

// Reservation-only SPACE view. Use space.rs's allocation/reuse policy without
// recursively editing the real space tree while it is allocating its own path.
// Scratch split addresses index reserved_nodes rather than queued allocations.
struct Reservations<'a, S> {
    view: View<'a, S>,
    changes: &'a mut BTreeMap<u64, Vec<u8>>,
    next: &'a mut u64,
}
impl<S: NodeStore> NodeStore for Reservations<'_, S> {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.changes
            .get(&addr)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| self.view.get(addr))
    }
    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), btree::Error> {
        self.changes.insert(addr, bytes);
        Ok(())
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        *self.next = self.next.checked_sub(4).ok_or(btree::Error::Store)?;
        Ok(*self.next)
    }
}

enum Backing<'a, S> {
    Borrowed(&'a S),
    Owned(S),
}
impl<S> Backing<'_, S> {
    fn get(&self) -> &S {
        match self {
            Self::Borrowed(base) => base,
            Self::Owned(base) => base,
        }
    }
}

pub struct Transaction<'a, S> {
    base: Backing<'a, S>,
    generation: u64,
    original_space: BlockPtr,
    space_root: BlockPtr,
    space_count: Cell<Option<(BlockPtr, u64)>>,
    nodes: BTreeMap<u64, Vec<u8>>,
    reserved_nodes: BTreeMap<u64, Vec<u8>>,
    reservation_root: BlockPtr,
    scratch_next: u64,
    allocated: BTreeSet<u64>,
    allocated_blocks: u64,
    counts: BTreeMap<(u64, u64), u32>,
    deferred: VecDeque<Change>,
    failure: Option<Error>,
    next_root_id: u64,
    dead: DeadNodes,
    reaped: usize,
}
/// Tree geometry for the caller's reservation policy.
pub struct ReservationPaths {
    pub uncopied_fs_nodes: u64,
    pub fs_height: u64,
    pub space_height: u64,
    pub space_nodes: u64,
}
impl<'a, S: NodeStore> Transaction<'a, S> {
    /// Start the first transaction without inventing a durable generation 0.
    /// Block 20 is already private to generation 1; its REF and the remaining
    /// FREE range seed both the real SPACE tree and its reservation view.
    pub fn format(base: S, uuid: [u8; 16], total: u64) -> Result<Self, Error> {
        if !(16384..=1 << 48).contains(&total) {
            return Err(Error::Invalid);
        }
        let mut bytes = vec![0; format::NODE_SIZE];
        let entries = [
            (
                Key {
                    object: 20,
                    item_type: format::REF,
                    offset: 4,
                },
                format::Ref {
                    refcount: 1,
                    kind: 2,
                    alloc_generation: 1,
                    crcs: Vec::new(),
                }
                .encode(),
            ),
            (
                Key {
                    object: 0xffff_ffff_ffff_ff20,
                    item_type: format::FREE,
                    offset: 24,
                },
                format::Free { length: total - 28 }.encode().to_vec(),
            ),
        ];
        let mut end = bytes.len();
        for (i, (key, value)) in entries.iter().enumerate() {
            end -= value.len();
            bytes[end..end + value.len()].copy_from_slice(value);
            let at = 48 + i * 21;
            bytes[at..at + 21].copy_from_slice(
                &LeafItemHeader {
                    key: *key,
                    data_off: end as u16,
                    data_len: value.len() as u16,
                }
                .encode(),
            );
        }
        NodeHeader {
            tree: 2,
            level: 0,
            nitems: 2,
            fs_uuid: uuid,
            addr: 20,
            generation: 1,
        }
        .encode(&mut bytes)?;
        let root = BlockPtr {
            addr: 20,
            generation: 1,
            level: 0,
            csum: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        };
        Ok(Self {
            base: Backing::Owned(base),
            generation: 1,
            original_space: root,
            space_root: root,
            space_count: Cell::new(None),
            reservation_root: root,
            nodes: BTreeMap::from([(20, bytes.clone())]),
            reserved_nodes: BTreeMap::from([(20, bytes)]),
            scratch_next: u64::MAX - 3,
            allocated: BTreeSet::from([20]),
            allocated_blocks: 4,
            counts: BTreeMap::from([((20, 4), 1)]),
            deferred: VecDeque::new(),
            failure: None,
            next_root_id: 0,
            dead: DeadNodes::default(),
            reaped: 0,
        })
    }
    pub fn new(base: &'a S, space_root: BlockPtr, generation: u64) -> Result<Self, Error> {
        Self::with_backing(Backing::Borrowed(base), space_root, generation, None)
    }
    /// Own a read-only store so an adapter can retain a transaction between calls.
    pub fn new_owned(base: S, space_root: BlockPtr, generation: u64) -> Result<Self, Error> {
        Self::with_backing(Backing::Owned(base), space_root, generation, None)
    }
    /// Continue a mount using the index from its preceding transaction at this
    /// space root. This constructor performs no DEAD range walk.
    pub fn new_owned_with_dead(
        base: S,
        space_root: BlockPtr,
        generation: u64,
        dead: DeadNodes,
    ) -> Result<Self, Error> {
        Self::with_backing(Backing::Owned(base), space_root, generation, Some(dead))
    }
    fn with_backing(
        base: Backing<'a, S>,
        space_root: BlockPtr,
        generation: u64,
        dead: Option<DeadNodes>,
    ) -> Result<Self, Error> {
        let h = NodeHeader::decode(&base.get().get(space_root.addr)?)?;
        if h.tree != 2
            || h.addr != space_root.addr
            || h.generation != space_root.generation
            || h.level != space_root.level
            || generation <= h.generation
        {
            return Err(Error::Invalid);
        }
        let dead = match dead {
            Some(dead) => dead,
            None => DeadNodes {
                addresses: space::dead_nodes(base.get(), space_root)?,
            },
        };
        Ok(Self {
            base,
            generation,
            original_space: space_root,
            space_root,
            space_count: Cell::new(None),
            nodes: BTreeMap::new(),
            reserved_nodes: BTreeMap::new(),
            reservation_root: space_root,
            scratch_next: u64::MAX - 3,
            allocated: BTreeSet::new(),
            allocated_blocks: 0,
            counts: BTreeMap::new(),
            deferred: VecDeque::new(),
            failure: None,
            next_root_id: 0,
            dead,
            reaped: 0,
        })
    }
    pub fn view(&self) -> View<'_, S> {
        View {
            base: self.base.get(),
            nodes: &self.nodes,
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub(super) fn reservation_paths(
        &self,
        root: BlockPtr,
        ino: u64,
    ) -> Result<ReservationPaths, Error> {
        let view = self.view();
        let space_nodes = self.space_node_count()?;
        Ok(ReservationPaths {
            uncopied_fs_nodes: if self.private_leaf_header(root)?.is_some() {
                0
            } else {
                btree::reservation_nodes(&view, root, Some(ino), self.generation)?
            },
            fs_height: u64::from(root.level),
            space_height: u64::from(self.space_root.level),
            space_nodes,
        })
    }
    fn space_node_count(&self) -> Result<u64, Error> {
        match self.space_count.get() {
            Some((root, count)) if root == self.space_root => Ok(count),
            _ => {
                let count = btree::reservation_nodes(&self.view(), self.space_root, None, 0)?;
                self.space_count.set(Some((self.space_root, count)));
                Ok(count)
            }
        }
    }
    /// Blocks already allocated plus the unchanged design-2.8 bound for work
    /// still deferred to publication. Include the mounted ROOT's final update
    /// and FS edits for orphan cleanup; no unused split paths are retained.
    pub fn metadata_debt(&self, root: BlockPtr, orphan_count: u64) -> Result<u64, Error> {
        let fs_levels = orphan_count.checked_mul(3 * 4 * (u64::from(root.level) + 2))
            .ok_or(space::Error::ENOSPC)?;
        let changes = (self.deferred.len() as u64).checked_add(1)
            .and_then(|n| n.checked_add(fs_levels))
            .and_then(|n| n.checked_mul(3)).ok_or(space::Error::ENOSPC)?;
        Ok(Estimate {
            data_blocks: self.allocated_blocks,
            fs_levels,
            space_changes: changes,
            space_height: u64::from(self.space_root.level),
            space_nodes: self.space_node_count()?,
            ..Estimate::default()
        }.blocks()?)
    }
    /// Private encoded nodes; a commit engine must call `settle` before use.
    pub fn writes(&self) -> &BTreeMap<u64, Vec<u8>> {
        &self.nodes
    }
    fn healthy(&self) -> Result<(), Error> {
        self.failure.map_or(Ok(()), Err)
    }
    fn result<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                let error = *self.failure.get_or_insert(error);
                Err(error)
            }
        }
    }
    fn count(&mut self, start: u64, length: u64) -> Result<u32, Error> {
        if let Some(count) = self.counts.get(&(start, length)) {
            return Ok(*count);
        }
        let r = space::reference(self.base.get(), self.original_space, start, length)?
            .ok_or(Error::Invalid)?;
        self.counts.insert((start, length), r.refcount);
        Ok(r.refcount)
    }
    fn reserve(&mut self, length: u64, kind: Kind, crcs: &[u32]) -> Result<u64, Error> {
        self.reserve_extent(length, kind, crcs, false)
            .map(|(start, _)| start)
    }
    fn reserve_extent(
        &mut self,
        length: u64,
        kind: Kind,
        crcs: &[u32],
        short_data: bool,
    ) -> Result<(u64, u64), Error> {
        let mut reservations = Reservations {
            view: View {
                base: self.base.get(),
                nodes: &BTreeMap::new(),
            },
            changes: &mut self.reserved_nodes,
            next: &mut self.scratch_next,
        };
        let requested = length;
        let (start, length) = if short_data {
            space::allocate_data_run(
                &mut reservations,
                &mut self.reservation_root,
                crcs,
                self.generation,
            )?
        } else {
            (
                space::allocate(
                    &mut reservations,
                    &mut self.reservation_root,
                    length,
                    kind,
                    crcs,
                    self.generation,
                )?,
                length,
            )
        };
        let crcs = &crcs[..crcs.len().min(length as usize)];
        if length < requested {
            // A short result promoted eligible PENDING ranges before choosing
            // its largest run. Exact-length replay could fit FREE without
            // promotion, so mirror that conversion before replaying the REF.
            self.deferred.push_back(Change::PromotePending);
        }
        self.counts.insert((start, length), 1);
        self.allocated_blocks = self.allocated_blocks.checked_add(length).ok_or(Error::Invalid)?;
        self.allocated.insert(start);
        self.deferred.push_back(Change::Allocate {
            start,
            length,
            kind,
            crcs: crcs.to_vec(),
        });
        Ok((start, length))
    }
    /// The initial reference belongs to the caller and is transferred to its
    /// new EXTENT item. Additional items naming that REF need `increment` first.
    pub fn allocate_data(&mut self, crcs: &[u32]) -> Result<u64, Error> {
        self.healthy()?;
        let result = self.reserve(crcs.len() as u64, Kind::Data, crcs);
        self.result(result)
    }
    /// Reserve one physical data run, which may cover only a prefix. Record
    /// its actual length so deferred replay and EXTENT ownership agree.
    pub fn allocate_data_run(&mut self, crcs: &[u32]) -> Result<(u64, u64), Error> {
        self.healthy()?;
        let result = self.reserve_extent(crcs.len() as u64, Kind::Data, crcs, true);
        self.result(result)
    }
    pub fn increment(&mut self, start: u64, length: u64) -> Result<(), Error> {
        self.healthy()?;
        let result = (|| {
            let count = self
                .count(start, length)?
                .checked_add(1)
                .ok_or(space::Error::EMLINK)?;
            self.counts.insert((start, length), count);
            self.deferred.push_back(Change::Increment { start, length });
            Ok(())
        })();
        self.result(result)
    }
    /// `children` retains outgoing FS references as DEAD for the later reaper.
    /// A COW copy/structural transfer passes false: its replacement owns them.
    pub fn release(&mut self, start: u64, length: u64, children: bool) -> Result<(), Error> {
        self.healthy()?;
        let result = (|| {
            let count = self
                .count(start, length)?
                .checked_sub(1)
                .ok_or(Error::Invalid)?;
            self.counts.insert((start, length), count);
            if count == 0 && children {
                self.dead.addresses.insert(start);
            }
            self.deferred.push_back(Change::Release {
                start,
                length,
                children,
            });
            Ok(())
        })();
        self.result(result)
    }
    pub fn dead_nodes(&self) -> &DeadNodes {
        &self.dead
    }
    pub fn dead_count(&self) -> u64 {
        self.dead.count()
    }
    /// Reserve SPACE COW/split work for the remaining node budget. Reaping is
    /// snapshot deletion work and may draw on the global reserve (design 2.8).
    /// The mount retains this grant through publication, like its other grants.
    pub fn reserve_reap(&self, ledger: &mut Ledger, free_blocks: u64) -> Result<Grant, Error> {
        self.healthy()?;
        let nodes = btree::reservation_nodes(&self.view(), self.space_root, None, 0)?;
        // An internal node has at most 441 references; removing one can change
        // REF plus DEAD/PENDING. Include removal of the reaped DEAD itself.
        let changes = ((REAP_PER_COMMIT - self.reaped) as u64) * (2 * 441 + 2);
        let blocks = Estimate {
            space_changes: changes,
            space_height: u64::from(self.space_root.level),
            space_nodes: nodes,
            ..Estimate::default()
        }
        .blocks()?;
        Ok(ledger.reserve(free_blocks, blocks, Operation::DeleteSnapshot)?)
    }
    /// Reap the lowest-address DEAD node. Stop on an empty index or exhausted
    /// transaction budget; settle preserves that budget.
    pub fn reap_step(&mut self) -> Result<Option<u64>, Error> {
        self.healthy()?;
        if self.reaped == REAP_PER_COMMIT {
            return Ok(None);
        }
        let result = (|| {
            self.settle()?;
            let Some(&addr) = self.dead.addresses.first() else {
                return Ok(None);
            };
            let bytes = self.view().get(addr)?;
            let h = NodeHeader::decode(&bytes)?;
            let space = NodeHeader::decode(&self.view().get(self.space_root.addr)?)?;
            if h.tree != 1
                || h.addr != addr
                || h.fs_uuid != space.fs_uuid
                || h.generation > self.generation
                || h.level >= 8
            {
                return Err(Error::Invalid);
            }
            // Validate the full node layout before changing its references.
            btree::decode_node(
                &bytes,
                BlockPtr {
                    addr,
                    generation: h.generation,
                    level: h.level,
                    csum: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
                },
            )?;
            for (start, length) in outgoing(&bytes)? {
                preemption_point();
                self.release(start, length, h.level != 0)?;
            }
            let mut root = self.space_root;
            let generation = self.generation;
            space::reap(
                &mut EditStore { txn: self, tree: 2 },
                &mut root,
                addr,
                generation,
            )?;
            self.space_root = root;
            self.dead.addresses.remove(&addr);
            self.reaped += 1;
            Ok(Some(addr))
        })();
        self.result(result)
    }
    fn prepare(&mut self, header: &mut NodeHeader) -> Result<(), Error> {
        let private = header.generation == self.generation && self.allocated.contains(&header.addr);
        if !private && header.generation >= self.generation {
            return Err(Error::Invalid);
        }
        let count = self.count(header.addr, 4)?;
        if count == 0 {
            return Err(Error::Invalid);
        }
        if private && count == 1 {
            return Ok(());
        }
        // Ancestors call this before descending. Their increments must be in
        // the effective counts before a child decides whether it is shared.
        if count > 1 {
            if header.tree != 1 {
                return Err(Error::Invalid);
            }
            let bytes = self.view().get(header.addr)?;
            for (start, length) in outgoing(&bytes)? {
                preemption_point();
                self.increment(start, length)?;
            }
        }
        let kind = node_kind(header.tree)?;
        let addr = self.reserve(4, kind, &[])?;
        self.release(header.addr, 4, false)?;
        header.addr = addr;
        header.generation = self.generation;
        Ok(())
    }
    pub fn create_tree(&mut self, uuid: [u8; 16]) -> Result<BlockPtr, Error> {
        self.healthy()?;
        let generation = self.generation;
        let result = btree::create(&mut EditStore { txn: self, tree: 1 }, 1, uuid, generation)
            .map_err(Error::from);
        self.result(result)
    }
    /// Insert into an FS tree. EXTENT insertion transfers a caller-owned data
    /// reference; replacement of a different REF releases the old reference.
    pub fn insert(
        &mut self,
        root: &mut BlockPtr,
        key: Key,
        value: &[u8],
    ) -> Result<Option<Vec<u8>>, Error> {
        self.change(root, key, Some(value))
    }
    pub fn delete(&mut self, root: &mut BlockPtr, key: Key) -> Result<Option<Vec<u8>>, Error> {
        self.change(root, key, None)
    }
    // Only writer-produced private root leaves qualify. Disk-backed nodes still
    // use the full B-tree decoder; check the complete pointer and CRC here too.
    fn private_leaf_header(&self, root: BlockPtr) -> Result<Option<NodeHeader>, Error> {
        if root.level != 0
            || root.generation != self.generation
            || self.counts.get(&(root.addr, 4)) != Some(&1)
        {
            return Ok(None);
        }
        let Some(bytes) = self.nodes.get(&root.addr) else {
            return Ok(None);
        };
        let h = NodeHeader::decode(bytes)?;
        if h.tree != 1
            || h.addr != root.addr
            || h.generation != root.generation
            || h.level != root.level
            || root.csum != u32::from_le_bytes(bytes[8..12].try_into().unwrap())
            || 48 + usize::from(h.nitems) * 21 > bytes.len()
        {
            return Err(Error::Invalid);
        }
        Ok(Some(h))
    }
    fn change(
        &mut self,
        root: &mut BlockPtr,
        key: Key,
        value: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.healthy()?;
        let result = (|| {
            // A same-size INODE replacement changes neither leaf layout nor
            // outgoing extent references. Find its slot without materializing
            // unrelated sibling payloads, then checksum the modified private bytes.
            if let Some(value) = value.filter(|_| key.item_type == format::INODE) {
                if let Some(h) = self.private_leaf_header(*root)? {
                    let bytes = self.nodes.get_mut(&root.addr).ok_or(Error::Invalid)?;
                    let (mut lo, mut hi) = (0, usize::from(h.nitems));
                    while lo < hi {
                        let mid = lo + (hi - lo) / 2;
                        let at = 48 + mid * 21;
                        let item = LeafItemHeader::decode(&bytes[at..at + 21])?;
                        if item.key < key {
                            lo = mid + 1;
                        } else {
                            hi = mid;
                        }
                    }
                    if lo < usize::from(h.nitems) {
                        let at = 48 + lo * 21;
                        let item = LeafItemHeader::decode(&bytes[at..at + 21])?;
                        if item.key == key && usize::from(item.data_len) == value.len() {
                            let start = usize::from(item.data_off);
                            if start < 48 + usize::from(h.nitems) * 21 {
                                return Err(Error::Invalid);
                            }
                            let payload = bytes
                                .get_mut(start..start + value.len())
                                .ok_or(Error::Invalid)?;
                            let old = payload.to_vec();
                            if old != value {
                                payload.copy_from_slice(value);
                                h.encode(bytes)?;
                                root.csum = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
                            }
                            return Ok(Some(old));
                        }
                    }
                }
            }
            let h = NodeHeader::decode(&self.view().get(root.addr)?)?;
            if h.tree != 1 {
                return Err(Error::Invalid);
            }
            let old = btree::search(&self.view(), *root, key)?;
            if old.as_deref() == value {
                return Ok(old);
            }
            let previous_extent = if key.item_type == format::EXTENT {
                old.as_deref().map(extent_ref).transpose()?
            } else {
                None
            };
            let new_extent = if key.item_type == format::EXTENT {
                value.map(extent_ref).transpose()?
            } else {
                None
            };
            if let Some((start, length)) = new_extent {
                if self.count(start, length)? == 0 {
                    return Err(Error::Invalid);
                }
            }
            let mut store = EditStore { txn: self, tree: 1 };
            let old = match value {
                Some(value) => btree::insert(&mut store, root, key, value)?,
                None => btree::delete(&mut store, root, key)?,
            };
            if previous_extent != new_extent {
                if let Some((start, length)) = previous_extent {
                    self.release(start, length, false)?;
                }
            }
            Ok(old)
        })();
        self.result(result)
    }
    /// Queue a ROOT record after editing its FS tree. Its pointer's incoming
    /// reference has already been transferred by COW (or by create_tree).
    pub fn update_root(&mut self, id: u64, value: Root) -> Result<(), Error> {
        self.healthy()?;
        let result = Root::decode(&value.encode()).map_err(Error::from);
        self.result(result)?;
        let next = id.checked_add(1).ok_or(Error::Invalid);
        self.next_root_id = self.next_root_id.max(self.result(next)?);
        self.deferred.push_back(Change::Root { id, value });
        Ok(())
    }
    pub fn delete_root(&mut self, id: u64) -> Result<(), Error> {
        self.healthy()?;
        self.deferred.push_back(Change::DeleteRoot { id });
        Ok(())
    }
    pub fn next_root_id(&self) -> u64 {
        self.next_root_id
    }
    /// Stage the same PENDING conversions in the reservation and real trees.
    pub fn promote_pending(&mut self) -> Result<(), Error> {
        self.healthy()?;
        let mut reservations = Reservations {
            view: View {
                base: self.base.get(),
                nodes: &BTreeMap::new(),
            },
            changes: &mut self.reserved_nodes,
            next: &mut self.scratch_next,
        };
        let result = space::promote_pending(
            &mut reservations,
            &mut self.reservation_root,
            self.generation,
        )
        .map_err(Error::from);
        self.result(result)?;
        self.deferred.push_back(Change::PromotePending);
        Ok(())
    }
    /// Apply deferred changes to the real COW SPACE tree. Its copies/splits add
    /// more allocation/release work to this queue. Edited paths remain in T,
    /// so revisiting them does not recursively copy them again (design 2.5).
    pub fn settle(&mut self) -> Result<BlockPtr, Error> {
        self.healthy()?;
        while let Some(change) = self.deferred.pop_front() {
            preemption_point();
            let mut root = self.space_root;
            let generation = self.generation;
            let mut store = EditStore { txn: self, tree: 2 };
            let result = match change {
                Change::PromotePending => {
                    space::promote_pending(&mut store, &mut root, generation).map_err(Error::from)
                }
                Change::Allocate {
                    start,
                    length,
                    kind,
                    crcs,
                } => space::allocate(&mut store, &mut root, length, kind, &crcs, generation)
                    .map_err(Error::from)
                    .and_then(|actual| {
                        if actual == start {
                            Ok(())
                        } else {
                            Err(Error::Invalid)
                        }
                    }),
                Change::Increment { start, length } => {
                    space::increment(&mut store, &mut root, start, length).map_err(Error::from)
                }
                Change::Release {
                    start,
                    length,
                    children,
                } => space::release(&mut store, &mut root, start, length, generation, children)
                    .map_err(Error::from),
                Change::DeleteRoot { id } => btree::delete(
                    &mut store,
                    &mut root,
                    Key {
                        object: 0xffff_ffff_ffff_ff10,
                        item_type: format::ROOT,
                        offset: id,
                    },
                )
                .map(|_| ())
                .map_err(Error::from),
                Change::Root { id, value } => btree::insert(
                    &mut store,
                    &mut root,
                    Key {
                        object: 0xffff_ffff_ffff_ff10,
                        item_type: format::ROOT,
                        offset: id,
                    },
                    &value.encode(),
                )
                .map(|_| ())
                .map_err(Error::from),
            };
            self.result(result)?;
            self.space_root = root;
        }
        Ok(self.space_root)
    }
}
fn node_kind(tree: u8) -> Result<Kind, Error> {
    match tree {
        1 => Ok(Kind::FsNode),
        2 => Ok(Kind::SpaceNode),
        _ => Err(Error::Invalid),
    }
}
fn extent_ref(bytes: &[u8]) -> Result<(u64, u64), Error> {
    if bytes.len() != 24 {
        return Err(Error::Invalid);
    }
    let e = Extent::decode(bytes)?;
    Ok((e.phys_start, u64::from(e.phys_len)))
}
fn outgoing(bytes: &[u8]) -> Result<Vec<(u64, u64)>, Error> {
    let h = NodeHeader::decode(bytes)?;
    let mut refs = Vec::new();
    for i in 0..usize::from(h.nitems) {
        let width = if h.level == 0 { 21 } else { 37 };
        let at = 48 + i * width;
        let entry = bytes.get(at..at + width).ok_or(Error::Invalid)?;
        if h.level > 0 {
            refs.push((InternalEntry::decode(entry)?.child_addr, 4));
        } else {
            let item = LeafItemHeader::decode(entry)?;
            if item.key.item_type == format::EXTENT {
                let lo = usize::from(item.data_off);
                refs.push(extent_ref(
                    bytes
                        .get(lo..lo + usize::from(item.data_len))
                        .ok_or(Error::Invalid)?,
                )?);
            }
        }
    }
    Ok(refs)
}
struct EditStore<'t, 'a, S> {
    txn: &'t mut Transaction<'a, S>,
    tree: u8,
}
impl<S: NodeStore> EditStore<'_, '_, S> {
    fn bridge<T>(&mut self, result: Result<T, Error>) -> Result<T, btree::Error> {
        self.txn.result(result).map_err(|error| match error {
            Error::Tree(error) | Error::Space(space::Error::Tree(error)) => error,
            _ => btree::Error::Store,
        })
    }
}
impl<S: NodeStore> NodeStore for EditStore<'_, '_, S> {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.txn.view().get(addr)
    }
    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), btree::Error> {
        let result = (|| {
            let h = NodeHeader::decode(&bytes)?;
            if h.addr != addr
                || h.generation != self.txn.generation
                || !self.txn.allocated.contains(&addr)
            {
                return Err(Error::Invalid);
            }
            self.txn.nodes.insert(addr, bytes);
            Ok(())
        })();
        self.bridge(result)
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        let result = node_kind(self.tree).and_then(|kind| self.txn.reserve(4, kind, &[]));
        self.bridge(result)
    }
    fn prepare_edit(&mut self, header: &mut NodeHeader) -> Result<(), btree::Error> {
        let result = self.txn.prepare(header);
        self.bridge(result)
    }
    fn retire(&mut self, header: NodeHeader) -> Result<(), btree::Error> {
        let result = self.txn.release(header.addr, 4, false);
        self.bridge(result)
    }
}
