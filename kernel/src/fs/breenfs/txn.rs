//! In-memory copy-on-write transactions (design 2.5, commit step 1).
//!
//! Old node bytes stay in the backing store. Edits and the space-tree fixpoint
//! accumulate private writes; only a successful `settle` supplies a space root
//! to the later commit engine. An error poisons the transaction: discard it and
//! its unpublished FS roots. Reservation policy and device I/O are higher layers.
use super::preempt::preemption_point;
use super::{
    btree::{self, Node, NodeStore},
    format::{self, BlockPtr, Extent, Free, Key, NodeHeader, Root},
    reserve::Estimate,
    space::{self, Kind},
};
use alloc::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
    vec::Vec,
};
use core::cell::RefCell;

/// First address handed out by a scratch transaction. Real block numbers are
/// below 2^48 (format limit) and reservation scratch splits count down from
/// u64::MAX, so scratch addresses name neither (E24 coordinator 601(1)).
const SCRATCH_BASE: u64 = 1 << 62;

type Nodes = BTreeMap<u64, Arc<Node>>;
type PublicationReads = RefCell<Option<BTreeMap<u64, (BlockPtr, Arc<Node>)>>>;

fn read_private(node: &Arc<Node>, ptr: BlockPtr) -> Result<Arc<Node>, btree::Error> {
    if node.private_ptr() != ptr {
        return Err(btree::Error::InvalidNode.at_node(ptr.addr));
    }
    Ok(node.clone())
}

/// Generation N's private node and allocation state at freeze (E24
/// coordinator 597/601(1)). It is immutable once shared: N's preparation and
/// N+1's scratch view each layer their own private maps above it, so a
/// foreground reader never observes N's preparation edits in progress.
#[derive(Default)]
pub struct Frozen {
    nodes: Nodes,
    counts: BTreeMap<(u64, u64), u32>,
    allocated: BTreeSet<u64>,
    data_allocations: BTreeMap<u64, u64>,
}

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
    nodes: &'a Nodes,
    frozen: Option<&'a Nodes>,
    reads: &'a PublicationReads,
}
impl<S> View<'_, S> {
    fn private(&self, addr: u64) -> Option<&Arc<Node>> {
        self.nodes.get(&addr).or_else(|| self.frozen.and_then(|nodes| nodes.get(&addr)))
    }
}
impl<S: NodeStore> NodeStore for View<'_, S> {
    fn read_node(&self, ptr: BlockPtr) -> Result<Arc<Node>, btree::Error> {
        if let Some(node) = self.private(ptr.addr) {
            return read_private(node, ptr);
        }
        if let Some((key, node)) = self.reads.borrow().as_ref().and_then(|reads| reads.get(&ptr.addr)) {
            if *key == ptr { return Ok(node.clone()); }
        }
        let node = self.base.read_node(ptr)?;
        if let Some(reads) = self.reads.borrow_mut().as_mut() {
            reads.insert(ptr.addr, (ptr, node.clone()));
        }
        Ok(node)
    }
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        if let Some(node) = self.private(addr) {
            return node.encode().map(|(_, bytes)| bytes);
        }
        self.base.get(addr)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
        Err(btree::Error::Store)
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        Err(btree::Error::Store)
    }
}

// Reservation-only SPACE view. Use the same first-fit/reuse policy without
// recursively editing the real SPACE tree while allocating its own path.
struct Reservations<'a, S> {
    view: View<'a, S>,
    changes: &'a mut Nodes,
    next: &'a mut u64,
}
impl<S: NodeStore> NodeStore for Reservations<'_, S> {
    fn read_node(&self, ptr: BlockPtr) -> Result<Arc<Node>, btree::Error> {
        match self.changes.get(&ptr.addr) {
            Some(node) => read_private(node, ptr),
            None => self.view.read_node(ptr),
        }
    }
    fn get(&self, _: u64) -> Result<Vec<u8>, btree::Error> {
        Err(btree::Error::Store)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
        Err(btree::Error::Store)
    }
    fn stages_decoded(&self) -> bool { true }
    fn take_decoded(&mut self, ptr: BlockPtr) -> Result<Arc<Node>, btree::Error> {
        if let Some(node) = self.changes.remove(&ptr.addr) {
            read_private(&node, ptr)?;
            return Ok(node);
        }
        self.view.read_node(ptr)
    }
    fn put_decoded(&mut self, node: Node) -> Result<(), btree::Error> {
        self.changes.insert(node.header.addr, Arc::new(node));
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
    space_nodes: u64,
    original_space_nodes: u64,
    // Preparation reads each immutable node at most once even when the
    // working set is larger than the mounted reader's bounded cache.
    reads: PublicationReads,
    free_delta: i64,
    nodes: Nodes,
    reserved_nodes: Nodes,
    reservation_root: BlockPtr,
    scratch_next: u64,
    allocated: BTreeSet<u64>,
    allocated_blocks: u64,
    data_allocations: BTreeMap<u64, u64>,
    counts: BTreeMap<(u64, u64), u32>,
    deferred: VecDeque<Change>,
    failure: Option<Error>,
    next_root_id: u64,
    dead: DeadNodes,
    reaped: usize,
    pending_summary: space::PendingSummary,
    // Freeze (601(1)) moves the four private maps above into this shared,
    // immutable layer; the maps above then hold only later private changes.
    frozen: Option<Arc<Frozen>>,
    // A scratch fork names nodes with fake addresses; settle refuses it:
    // N+1 allocates no real block while N is in flight (601(1)).
    scratch: Option<u64>,
}
/// Tree geometry for the caller's reservation policy.
pub struct ReservationPaths {
    pub uncopied_fs_nodes: u64,
    pub private_fs_root: bool,
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
        let node = Arc::new(Node {
            header: NodeHeader { tree: 2, fs_uuid: uuid, addr: 20, generation: 1,
                ..NodeHeader::default() },
            items: entries.into_iter().map(|(key, value)| (key, Arc::from(value))).collect(),
            children: Vec::new(),
        });
        let root = node.private_ptr();
        Ok(Self {
            base: Backing::Owned(base),
            generation: 1,
            original_space: root,
            space_root: root,
            space_nodes: 1,
            original_space_nodes: 1,
            reads: RefCell::new(None),
            free_delta: 0,
            reservation_root: root,
            nodes: BTreeMap::from([(20, node.clone())]),
            reserved_nodes: BTreeMap::from([(20, node)]),
            scratch_next: u64::MAX - 3,
            allocated: BTreeSet::from([20]),
            allocated_blocks: 4,
            data_allocations: BTreeMap::new(),
            counts: BTreeMap::from([((20, 4), 1)]),
            deferred: VecDeque::new(),
            failure: None,
            next_root_id: 0,
            dead: DeadNodes::default(),
            reaped: 0,
            pending_summary: space::PendingSummary::default(),
            frozen: None,
            scratch: None,
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
        space_nodes: u64,
    ) -> Result<Self, Error> {
        Self::with_backing(Backing::Owned(base), space_root, generation, Some((dead, space_nodes)))
    }
    fn with_backing(
        base: Backing<'a, S>,
        space_root: BlockPtr,
        generation: u64,
        continuation: Option<(DeadNodes, u64)>,
    ) -> Result<Self, Error> {
        let h = base.get().read_node(space_root)?.header;
        if h.tree != 2
            || h.addr != space_root.addr
            || h.generation != space_root.generation
            || h.level != space_root.level
            || generation <= h.generation
        {
            return Err(Error::Invalid);
        }
        let (dead, space_nodes) = match continuation {
            Some(continuation) => continuation,
            None => (
                DeadNodes { addresses: space::dead_nodes(base.get(), space_root)? },
                btree::reservation_nodes(base.get(), space_root, None, 0)?,
            ),
        };
        Ok(Self {
            base,
            generation,
            original_space: space_root,
            space_root,
            space_nodes,
            original_space_nodes: space_nodes,
            reads: RefCell::new(None),
            free_delta: 0,
            nodes: BTreeMap::new(),
            reserved_nodes: BTreeMap::new(),
            reservation_root: space_root,
            scratch_next: u64::MAX - 3,
            allocated: BTreeSet::new(),
            allocated_blocks: 0,
            data_allocations: BTreeMap::new(),
            counts: BTreeMap::new(),
            deferred: VecDeque::new(),
            failure: None,
            next_root_id: 0,
            dead,
            reaped: 0,
            pending_summary: space::PendingSummary::default(),
            frozen: None,
            scratch: None,
        })
    }
    pub fn view(&self) -> View<'_, S> {
        View {
            base: self.base.get(),
            nodes: &self.nodes,
            reads: &self.reads,
            frozen: self.frozen.as_deref().map(|frozen| &frozen.nodes),
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Freeze this generation's private state (E24 coordinator 597/601(1)).
    /// Moves its node, count and allocation maps into one shared immutable
    /// layer, a constant number of pointer moves, and returns N+1's scratch
    /// transaction over that layer and `base`, the published N-1 store. This
    /// transaction keeps preparing N above the same layer; neither side can
    /// change what the other reads. The scratch fork allocates fake addresses
    /// only and refuses publication work.
    pub fn fork(&mut self, base: S) -> Result<Transaction<'a, S>, Error> {
        self.healthy()?;
        if self.frozen.is_some() || self.scratch.is_some() {
            return Err(Error::Invalid);
        }
        // Refuse before any map moves.
        let generation = self.generation.checked_add(1).ok_or(Error::Invalid)?;
        let frozen = Arc::new(Frozen {
            nodes: core::mem::take(&mut self.nodes),
            counts: core::mem::take(&mut self.counts),
            allocated: core::mem::take(&mut self.allocated),
            data_allocations: core::mem::take(&mut self.data_allocations),
        });
        self.frozen = Some(frozen.clone());
        Ok(Transaction {
            base: Backing::Owned(base),
            generation,
            original_space: self.original_space,
            space_root: self.original_space,
            space_nodes: self.original_space_nodes,
            original_space_nodes: self.original_space_nodes,
            reads: RefCell::new(None),
            free_delta: 0,
            nodes: BTreeMap::new(),
            reserved_nodes: BTreeMap::new(),
            reservation_root: self.original_space,
            scratch_next: u64::MAX - 3,
            allocated: BTreeSet::new(),
            allocated_blocks: 0,
            data_allocations: BTreeMap::new(),
            counts: BTreeMap::new(),
            deferred: VecDeque::new(),
            failure: None,
            next_root_id: self.next_root_id,
            dead: DeadNodes::default(),
            reaped: 0,
            pending_summary: space::PendingSummary::default(),
            frozen: Some(frozen),
            scratch: Some(SCRATCH_BASE),
        })
    }
    fn private_node(&self, addr: u64) -> Option<&Arc<Node>> {
        self.nodes.get(&addr)
            .or_else(|| self.frozen.as_ref().and_then(|frozen| frozen.nodes.get(&addr)))
    }
    fn known_count(&self, start: u64, length: u64) -> Option<u32> {
        self.counts.get(&(start, length)).copied()
            .or_else(|| self.frozen.as_ref().and_then(|frozen| frozen.counts.get(&(start, length)).copied()))
    }
    fn is_allocated(&self, addr: u64) -> bool {
        self.allocated.contains(&addr)
            || self.frozen.as_ref().is_some_and(|frozen| frozen.allocated.contains(&addr))
    }
    fn data_length(&self, start: u64) -> Option<u64> {
        self.data_allocations.get(&start).copied()
            .or_else(|| self.frozen.as_ref().and_then(|frozen| frozen.data_allocations.get(&start).copied()))
    }
    fn publishable(&self) -> Result<(), Error> {
        if self.scratch.is_some() { Err(Error::Invalid) } else { self.healthy() }
    }
    pub(super) fn reservation_paths(
        &self,
        root: BlockPtr,
        ino: u64,
    ) -> Result<ReservationPaths, Error> {
        let view = self.view();
        let space_nodes = self.space_nodes;
        Ok(ReservationPaths {
            uncopied_fs_nodes: if self.owns_leaf(root) {
                0
            } else {
                btree::reservation_nodes(&view, root, Some(ino), self.generation)?
            },
            private_fs_root: root.generation == self.generation
                && self.known_count(root.addr, 4) == Some(1),
            fs_height: u64::from(root.level),
            space_height: u64::from(self.space_root.level),
            space_nodes,
        })
    }
    pub fn space_node_count(&self) -> u64 {
        self.space_nodes
    }
    /// Blocks already allocated plus the unchanged design-2.8 bound for work
    /// still deferred to publication. Include the mounted ROOT's final update
    /// and FS edits for orphan cleanup; no unused split paths are retained.
    pub(super) fn metadata_estimate(&self, root: BlockPtr, orphan_count: u64) -> Result<Estimate, Error> {
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
            space_nodes: self.space_nodes,
            ..Estimate::default()
        })
    }
    pub(super) fn begin_preparation(&self) {
        if self.reads.borrow().is_none() {
            self.reads.replace(Some(BTreeMap::new()));
        }
    }
    pub(super) fn free_blocks(&self, base: u64) -> Option<u64> {
        base.checked_add_signed(self.free_delta)
    }
    /// Private decoded nodes after settlement, including the frozen layer.
    pub fn writes(&self) -> impl Iterator<Item = (&u64, &Arc<Node>)> + '_ {
        let frozen = self.frozen.as_deref().map(|frozen| &frozen.nodes);
        frozen.into_iter().flatten()
            .filter(|(addr, _)| !self.nodes.contains_key(addr))
            .chain(self.nodes.iter())
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
        if let Some(count) = self.known_count(start, length) {
            return Ok(count);
        }
        let original = View { base: self.base.get(), nodes: &BTreeMap::new(),
            frozen: None, reads: &self.reads };
        let r = space::reference(&original, self.original_space, start, length)?
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
        if let Some(next) = self.scratch.as_mut() {
            // N+1 while N is in flight: a view-only address. Record the same
            // custody and deferred change as a real allocation, so reservation
            // debt still counts it; replay onto N's published view allocates.
            let start = *next;
            *next = next.checked_add(length).ok_or(Error::Invalid)?;
            self.counts.insert((start, length), 1);
            if kind == Kind::Data { self.data_allocations.insert(start, length); }
            self.allocated_blocks = self.allocated_blocks.checked_add(length).ok_or(Error::Invalid)?;
            self.allocated.insert(start);
            self.deferred.push_back(Change::Allocate { start, length, kind, crcs: crcs.to_vec() });
            return Ok((start, length));
        }
        let mut reservations = Reservations {
            view: View {
                base: self.base.get(),
                nodes: &BTreeMap::new(),
                frozen: None,
                reads: &self.reads,
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
        if kind == Kind::Data { self.data_allocations.insert(start, length); }
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
    pub(crate) fn take_dead_nodes(&mut self) -> DeadNodes {
        core::mem::take(&mut self.dead)
    }
    pub(crate) fn data_is_live(&self, start: u64) -> bool {
        self.data_length(start).is_some_and(|length|
            self.known_count(start, length).is_some_and(|count| count != 0))
    }
    /// Allocations made by this generation that still have an incoming REF.
    pub(crate) fn live_data(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        let frozen = self.frozen.as_deref().map(|frozen| &frozen.data_allocations);
        frozen.into_iter().flatten()
            .filter(|(start, _)| !self.data_allocations.contains_key(start))
            .chain(self.data_allocations.iter())
            .filter_map(|(&start, &length)| {
                preemption_point();
                self.data_is_live(start).then_some((start, length))
            })
    }
    pub fn dead_count(&self) -> u64 {
        self.dead.count()
    }
    /// Reserve SPACE COW/split work for the remaining node budget. Reaping is
    /// snapshot deletion work and may draw on the global reserve (design 2.8).
    /// The mount retains this grant through publication, like its other grants.
    /// The mount reserves these blocks for its grant, after this SPACE read
    /// and outside its shared ledger's lock.
    pub fn reap_reservation_blocks(&self) -> Result<u64, Error> {
        self.publishable()?;
        // An internal node has at most 441 references; removing one can change
        // REF plus DEAD/PENDING. Include removal of the reaped DEAD itself.
        let changes = ((REAP_PER_COMMIT - self.reaped) as u64) * (2 * 441 + 2);
        Ok(Estimate {
            space_changes: changes,
            space_height: u64::from(self.space_root.level),
            space_nodes: self.space_nodes,
            ..Estimate::default()
        }
        .blocks()?)
    }
    /// Reap the lowest-address DEAD node. Stop on an empty index or exhausted
    /// transaction budget; settle preserves that budget.
    pub fn reap_step(&mut self) -> Result<Option<u64>, Error> {
        self.publishable()?;
        if self.reaped == REAP_PER_COMMIT {
            return Ok(None);
        }
        let result = (|| {
            self.settle()?;
            let Some(&addr) = self.dead.addresses.first() else {
                return Ok(None);
            };
            let node = if let Some(node) = self.private_node(addr) {
                node.clone()
            } else {
                let bytes = self.base.get().get(addr)?;
                let h = NodeHeader::decode(&bytes)?;
                btree::decode_node(&bytes, BlockPtr { addr, generation: h.generation,
                    level: h.level, csum: u32::from_le_bytes(bytes[8..12].try_into().unwrap()) })?
            };
            let h = node.header;
            let space = self.view().read_node(self.space_root)?.header;
            if h.tree != 1 || h.addr != addr || h.fs_uuid != space.fs_uuid
                || h.generation > self.generation || h.level >= 8
            {
                return Err(Error::Invalid);
            }
            for (start, length) in outgoing(&node)? {
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
            self.pending_summary.include(4, generation);
            self.dead.addresses.remove(&addr);
            self.reaped += 1;
            Ok(Some(addr))
        })();
        self.result(result)
    }
    fn prepare(&mut self, node: &mut Node) -> Result<(), Error> {
        let header = node.header;
        let private = header.generation == self.generation && self.is_allocated(header.addr);
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
            for (start, length) in outgoing(node)? {
                preemption_point();
                self.increment(start, length)?;
            }
        }
        let kind = node_kind(header.tree)?;
        let addr = self.reserve(4, kind, &[])?;
        self.release(header.addr, 4, false)?;
        node.header.addr = addr;
        node.header.generation = self.generation;
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
    fn owns_leaf(&self, root: BlockPtr) -> bool {
        root.level == 0 && root.generation == self.generation
            && self.known_count(root.addr, 4) == Some(1)
            && self.private_node(root.addr).is_some()
    }
    fn change(
        &mut self,
        root: &mut BlockPtr,
        key: Key,
        value: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.healthy()?;
        let result = (|| {
            if self.view().read_node(*root)?.header.tree != 1 {
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
        self.publishable()?;
        let mut reservations = Reservations {
            view: View {
                base: self.base.get(),
                nodes: &BTreeMap::new(),
                frozen: None,
                reads: &self.reads,
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
    /// Read after prepare_commit has staged PENDING promotion and settled
    /// the remaining releases into the real SPACE tree.
    pub(super) fn pending_summary(&self) -> space::PendingSummary {
        self.pending_summary
    }
    /// Apply deferred changes to the real COW SPACE tree. Its copies/splits add
    /// more allocation/release work to this queue. Edited paths remain in T,
    /// so revisiting them does not recursively copy them again (design 2.5).
    pub fn settle(&mut self) -> Result<BlockPtr, Error> {
        self.publishable()?;
        while let Some(change) = self.deferred.pop_front() {
            preemption_point();
            let mut root = self.space_root;
            let generation = self.generation;
            let mut store = EditStore { txn: self, tree: 2 };
            let result = match change {
                Change::PromotePending => {
                    // This scan already visits the surviving quarantine.
                    // Later allocations cannot promote more in this generation.
                    space::promote_pending(&mut store, &mut root, generation)
                        .map(|summary| self.pending_summary = summary)
                        .map_err(Error::from)
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
                    .map(|quarantined| {
                        if quarantined {
                            self.pending_summary.include(length, generation);
                        }
                    })
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
fn outgoing(node: &Node) -> Result<Vec<(u64, u64)>, Error> {
    if node.header.level != 0 {
        return Ok(node.children.iter().map(|entry| (entry.child_addr, 4)).collect());
    }
    node.items.iter().filter(|(key, _)| key.item_type == format::EXTENT)
        .map(|(_, bytes)| extent_ref(bytes))
        .collect()
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
    fn read_node(&self, ptr: BlockPtr) -> Result<Arc<btree::Node>, btree::Error> {
        self.txn.view().read_node(ptr)
    }
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.txn.view().get(addr)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
        Err(btree::Error::Store)
    }
    fn stages_decoded(&self) -> bool { true }
    fn take_decoded(&mut self, ptr: BlockPtr) -> Result<Arc<Node>, btree::Error> {
        if self.txn.known_count(ptr.addr, 4) == Some(1) {
            if let Some(node) = self.txn.nodes.remove(&ptr.addr) {
                read_private(&node, ptr)?;
                return Ok(node);
            }
        }
        self.read_node(ptr)
    }
    fn put_decoded(&mut self, node: Node) -> Result<(), btree::Error> {
        let h = node.header;
        if h.generation != self.txn.generation || !self.txn.is_allocated(h.addr) {
            return self.bridge(Err(Error::Invalid));
        }
        #[cfg(test)]
        btree::measurement::mutation(5, 1);
        self.txn.nodes.insert(h.addr, Arc::new(node));
        Ok(())
    }
    fn item_changed(&mut self, key: Key, old: Option<&[u8]>, new: Option<&[u8]>) -> Result<(), btree::Error> {
        if self.tree != 2 || key.item_type != format::FREE { return Ok(()); }
        let length = |payload: Option<&[u8]>| {
            payload.map_or(Ok(0), |bytes| {
                let length = Free::decode(bytes)?.length;
                i64::try_from(length).map_err(|_| Error::Invalid)
            })
        };
        let result = (|| {
            let change = length(new)?.checked_sub(length(old)?).ok_or(Error::Invalid)?;
            self.txn.free_delta = self.txn.free_delta.checked_add(change).ok_or(Error::Invalid)?;
            Ok(())
        })();
        self.bridge(result)
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        let result = (|| {
            let addr = self.txn.reserve(4, node_kind(self.tree)?, &[])?;
            // Only a new split node adds to the reachable topology. COW
            // internally calls reserve directly and replaces one node for one.
            if self.tree == 2 { self.txn.space_nodes += 1; }
            Ok(addr)
        })();
        self.bridge(result)
    }
    fn prepare_node(&mut self, node: &mut Node) -> Result<(), btree::Error> {
        let result = self.txn.prepare(node);
        self.bridge(result)
    }
    fn retire(&mut self, header: NodeHeader) -> Result<(), btree::Error> {
        let result = (|| {
            self.txn.release(header.addr, 4, false)?;
            if self.tree == 2 {
                self.txn.space_nodes = self.txn.space_nodes.checked_sub(1).ok_or(Error::Invalid)?;
            }
            Ok(())
        })();
        self.bridge(result)
    }
}
