//! In-memory copy-on-write transactions (design 2.5, commit step 1).
//!
//! Old node bytes stay in the backing store. Edits and the space-tree fixpoint
//! accumulate private writes; only a successful `settle` supplies a space root
//! to the later commit engine. An error poisons the transaction: discard it and
//! its unpublished FS roots. Reservation policy and device I/O are higher layers.
use super::preempt::preemption_point;
use super::{
    btree::{self, Node, NodeStore},
    format::{self, BlockPtr, Extent, Free, Key, NodeHeader, Ref, Root},
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

/// Mount-owned space state carried from a settled transaction into the next
/// one, so no transaction walks FREE or PENDING on disk: the DEAD index, the
/// space-tree node count, the FREE index, the quarantine this mount wrote and
/// the end of the last data allocation (the goal for a file with no extent).
#[derive(Clone, Default)]
pub struct SpaceState {
    dead: DeadNodes,
    space_nodes: u64,
    free: space::FreeIndex,
    quarantine: space::Quarantine,
    data_goal: u64,
    // The device's oldest pinned generation (E2 design 7.1), kept current by
    // the mount's pin table: allocation-time promotion runs inside tree
    // edits, which take no bound argument. Unset outside a mount.
    reuse_bound: Option<alloc::sync::Arc<core::sync::atomic::AtomicU64>>,
}
impl SpaceState {
    pub fn free(&self) -> &space::FreeIndex {
        &self.free
    }
    pub fn quarantine(&self) -> &space::Quarantine {
        &self.quarantine
    }
}

/// A delayed reference head (after the delayed-reference design Btrfs
/// publishes): the count changes to one allocation in this transaction fold
/// into one head, applied to its REF once at settlement. Matching increments
/// and releases cancel, leaving the REF unedited.
struct Head {
    /// The count stored in the space tree when the head opened.
    before: u32,
    /// The release that reaches 0 keeps the node's outgoing references.
    children: bool,
    /// Allocated by this transaction: claim FREE and create the REF.
    new: Option<(Kind, Vec<u32>)>,
}

enum RootChange {
    Delete { id: u64 },
    Put { id: u64, value: Root },
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
    allocated: BTreeSet<u64>,
    allocated_blocks: u64,
    data_allocations: BTreeMap<u64, u64>,
    counts: BTreeMap<(u64, u64), u32>,
    heads: BTreeMap<(u64, u64), Head>,
    // Heads allocated by this transaction whose count is back to 0: their
    // settlement claims FREE and quarantines (metadata_estimate).
    released_new: u64,
    root_changes: VecDeque<RootChange>,
    // This generation's releases not yet written to PENDING, coalesced.
    staged: BTreeMap<u64, u64>,
    // PENDING made allocatable in this transaction (the FREE index already
    // has it); settlement applies the SPACE edits.
    promotion: Option<space::Promotion>,
    promoted: bool,
    failure: Option<Error>,
    next_root_id: u64,
    state: SpaceState,
    reaped: usize,
    // Freeze (601(1)) moves the four private maps above into this shared,
    // immutable layer; the maps above then hold only later private changes.
    frozen: Option<Arc<Frozen>>,
    // A scratch fork names nodes with fake addresses; settle refuses it:
    // N+1 allocates no real block while N is in flight (601(1)).
    scratch: Option<u64>,
    // A scratch fork starts with an empty DEAD index; this records whether
    // the frozen generation's index held any node (`dead_pending`).
    frozen_dead: bool,
}
/// Tree geometry for the caller's reservation policy.
pub struct ReservationPaths {
    pub uncopied_fs_nodes: u64,
    pub private_fs_root: bool,
    pub fs_height: u64,
    pub space_height: u64,
    pub space_nodes: u64,
    /// A node an edit copies may be shared: another root or a DEAD node can
    /// reference it. The copy then takes a reference to each child or data
    /// extent (`prepare`), up to btree::MAX_REFERENCES deferred heads.
    pub shared: bool,
}
impl ReservationPaths {
    /// SPACE changes owed for the references that `copies` copied nodes take
    /// when they may be shared: one deferred head per child or data extent,
    /// at most btree::MAX_REFERENCES a node, each settled in at most three
    /// changes (metadata_estimate). Estimate::blocks caps the total at the
    /// whole-tree bound of design 2.8.
    pub fn shared_copy_changes(&self, copies: u64) -> Option<u64> {
        if !self.shared { return Some(0); }
        copies.checked_mul(3 * btree::MAX_REFERENCES)
    }
}
impl<'a, S: NodeStore> Transaction<'a, S> {
    fn blank(base: Backing<'a, S>, generation: u64, space_root: BlockPtr, state: SpaceState) -> Self {
        Self {
            base,
            generation,
            original_space: space_root,
            space_root,
            space_nodes: state.space_nodes,
            original_space_nodes: state.space_nodes,
            reads: RefCell::new(None),
            free_delta: 0,
            nodes: BTreeMap::new(),
            allocated: BTreeSet::new(),
            allocated_blocks: 0,
            data_allocations: BTreeMap::new(),
            counts: BTreeMap::new(),
            heads: BTreeMap::new(),
            released_new: 0,
            root_changes: VecDeque::new(),
            staged: BTreeMap::new(),
            promotion: None,
            promoted: false,
            failure: None,
            next_root_id: 0,
            state,
            reaped: 0,
            frozen: None,
            scratch: None,
            frozen_dead: false,
        }
    }
    /// Start the first transaction without inventing a durable generation 0.
    /// Block 20 is already private to generation 1; its REF and the remaining
    /// FREE range seed the SPACE tree and the FREE index.
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
        let mut state = SpaceState { space_nodes: 1, ..SpaceState::default() };
        state.free.insert(24, total - 28)?;
        let mut txn = Self::blank(Backing::Owned(base), 1, root, state);
        txn.nodes.insert(20, node);
        txn.allocated.insert(20);
        txn.allocated_blocks = 4;
        txn.counts.insert((20, 4), 1);
        Ok(txn)
    }
    pub fn new(base: &'a S, space_root: BlockPtr, generation: u64) -> Result<Self, Error> {
        Self::with_backing(Backing::Borrowed(base), space_root, generation, None)
    }
    /// Own a read-only store so an adapter can retain a transaction between calls.
    pub fn new_owned(base: S, space_root: BlockPtr, generation: u64) -> Result<Self, Error> {
        Self::with_backing(Backing::Owned(base), space_root, generation, None)
    }
    /// Continue a mount from its preceding transaction's settled space state
    /// at this space root. This constructor walks no part of SPACE.
    pub fn new_owned_continuing(
        base: S,
        space_root: BlockPtr,
        generation: u64,
        state: SpaceState,
    ) -> Result<Self, Error> {
        Self::with_backing(Backing::Owned(base), space_root, generation, Some(state))
    }
    fn with_backing(
        base: Backing<'a, S>,
        space_root: BlockPtr,
        generation: u64,
        continuation: Option<SpaceState>,
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
        let state = match continuation {
            Some(state) => state,
            None => {
                let (free, quarantine) = space::open(base.get(), space_root)?;
                SpaceState {
                    dead: DeadNodes { addresses: space::dead_nodes(base.get(), space_root)? },
                    space_nodes: btree::reservation_nodes(base.get(), space_root, None, 0)?,
                    free,
                    quarantine,
                    data_goal: 0,
                    reuse_bound: None,
                }
            }
        };
        Ok(Self::blank(base, generation, space_root, state))
    }
    /// The read-only store beneath this transaction's private nodes.
    pub fn base(&self) -> &S {
        self.base.get()
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
    /// only, has no FREE index and refuses publication work.
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
        let state = SpaceState { space_nodes: self.original_space_nodes, ..SpaceState::default() };
        let mut scratch = Transaction::blank(Backing::Owned(base), generation, self.original_space, state);
        scratch.next_root_id = self.next_root_id;
        scratch.frozen = Some(frozen);
        scratch.scratch = Some(SCRATCH_BASE);
        scratch.frozen_dead = self.dead_pending();
        Ok(scratch)
    }
    /// Whether a DEAD node still holds references to its children, so a live
    /// node can be shared with it even when only one root remains.
    pub(super) fn dead_pending(&self) -> bool {
        self.state.dead.count() != 0 || self.frozen_dead
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
            // Files::reservation_paths knows the other roots.
            shared: self.dead_pending(),
        })
    }
    pub fn space_node_count(&self) -> u64 {
        self.space_nodes
    }
    /// Blocks already allocated plus the unchanged design-2.8 bound for work
    /// still deferred to publication. Include the mounted ROOT's final update
    /// and FS edits for orphan cleanup; no unused split paths are retained.
    /// A head settles in at most three SPACE item changes (a FREE split and
    /// its REF, or the REF and its PENDING run); one this transaction
    /// allocated and released again settles in a FREE split and a PENDING run,
    /// so it counts twice.
    pub(super) fn metadata_estimate(&self, root: BlockPtr, orphan_count: u64) -> Result<Estimate, Error> {
        let fs_levels = orphan_count.checked_mul(3 * 4 * (u64::from(root.level) + 2))
            .ok_or(space::Error::ENOSPC)?;
        let deferred = (self.heads.len() as u64)
            .checked_add(self.released_new)
            .and_then(|n| n.checked_add(self.root_changes.len() as u64))
            .and_then(|n| n.checked_add(u64::from(self.promotion.is_some())))
            .ok_or(space::Error::ENOSPC)?;
        let changes = deferred.checked_add(1)
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
        self.reserve_extent(length, kind, crcs, None, false)
            .map(|(start, _)| start)
    }
    /// Choose blocks from the FREE index, near `goal` when one is given. FREE
    /// is preferred: the eligible quarantine becomes allocatable only when FREE
    /// cannot hold the request (design 2.5 reuse rule). A short data run then takes
    /// the largest range. Settlement claims the chosen range from FREE.
    fn reserve_extent(
        &mut self,
        length: u64,
        kind: Kind,
        crcs: &[u32],
        goal: Option<u64>,
        short_data: bool,
    ) -> Result<(u64, u64), Error> {
        if !space::valid_shape(length, kind as u8, 1, crcs.len()) {
            return Err(Error::Space(space::Error::Invalid));
        }
        let (start, length) = if let Some(next) = self.scratch.as_mut() {
            // N+1 while N is in flight: a view-only address. Record the same
            // custody and head as a real allocation, so reservation debt still
            // counts it; replay onto N's published view allocates.
            let start = *next;
            *next = next.checked_add(length).ok_or(Error::Invalid)?;
            (start, length)
        } else {
            let mut choice = self.state.free.choose(length, goal, false);
            if choice.is_none() {
                self.promote_eligible(u64::MAX)?;
                choice = self.state.free.choose(length, goal, short_data);
            }
            let (start, chosen) = choice.ok_or(space::Error::ENOSPC)?;
            self.state.free.remove(start, chosen)?;
            if kind == Kind::Data {
                self.state.data_goal = start.checked_add(chosen).ok_or(Error::Invalid)?;
            }
            (start, chosen)
        };
        let crcs = &crcs[..crcs.len().min(length as usize)];
        self.counts.insert((start, length), 1);
        if kind == Kind::Data { self.data_allocations.insert(start, length); }
        self.allocated_blocks = self.allocated_blocks.checked_add(length).ok_or(Error::Invalid)?;
        self.allocated.insert(start);
        let head = Head { before: 0, children: false, new: Some((kind, crcs.to_vec())) };
        if self.heads.insert((start, length), head).is_some() {
            return Err(Error::Invalid);
        }
        Ok((start, length))
    }
    /// Make PENDING released at or before G-2 allocatable, once per
    /// transaction: the FREE index gains it now, SPACE at settlement.
    fn promote_eligible(&mut self, bound: u64) -> Result<(), Error> {
        if self.promoted || self.scratch.is_some() {
            return Ok(());
        }
        self.promoted = true;
        let original = View { base: self.base.get(), nodes: &BTreeMap::new(),
            frozen: None, reads: &self.reads };
        let shared = self.state.reuse_bound.as_ref()
            .map_or(u64::MAX, |bound| bound.load(core::sync::atomic::Ordering::SeqCst));
        let promotion = self.state.quarantine.promote(&original, self.original_space,
            self.generation, &mut self.state.free, bound.min(shared))?;
        if !promotion.is_empty() {
            self.promotion = Some(promotion);
        }
        Ok(())
    }
    /// The initial reference belongs to the caller and is transferred to its
    /// new EXTENT item. Additional items naming that REF need `increment` first.
    pub fn allocate_data(&mut self, crcs: &[u32]) -> Result<u64, Error> {
        self.allocate_data_run(crcs, None).and_then(|(start, length)| {
            if length == crcs.len() as u64 { Ok(start) } else { Err(Error::Invalid) }
        })
    }
    /// Reserve one physical data run, which may cover only a prefix. Record
    /// its actual length so settlement and EXTENT ownership agree. `goal` is
    /// the block that would continue the file's existing data; without one the
    /// run follows this mount's last data allocation.
    pub fn allocate_data_run(&mut self, crcs: &[u32], goal: Option<u64>) -> Result<(u64, u64), Error> {
        self.healthy()?;
        let goal = goal.or((self.state.data_goal != 0).then_some(self.state.data_goal));
        let result = self.reserve_extent(crcs.len() as u64, Kind::Data, crcs, goal, true);
        self.result(result)
    }
    pub fn increment(&mut self, start: u64, length: u64) -> Result<(), Error> {
        self.healthy()?;
        let result = (|| {
            let before = self.count(start, length)?;
            let count = before.checked_add(1).ok_or(space::Error::EMLINK)?;
            self.counts.insert((start, length), count);
            self.heads.entry((start, length)).or_insert(Head { before, children: false, new: None });
            Ok(())
        })();
        self.result(result)
    }
    /// `children` retains outgoing FS references as DEAD for the later reaper.
    /// A COW copy/structural transfer passes false: its replacement owns them.
    pub fn release(&mut self, start: u64, length: u64, children: bool) -> Result<(), Error> {
        self.healthy()?;
        let result = (|| {
            let before = self.count(start, length)?;
            let count = before.checked_sub(1).ok_or(Error::Invalid)?;
            self.counts.insert((start, length), count);
            if count == 0 && children {
                self.state.dead.addresses.insert(start);
            }
            let head = self.heads.entry((start, length))
                .or_insert(Head { before, children: false, new: None });
            head.children = children;
            if count == 0 && head.new.is_some() {
                self.released_new += 1;
            }
            Ok(())
        })();
        self.result(result)
    }
    pub fn dead_nodes(&self) -> &DeadNodes {
        &self.state.dead
    }
    fn settled(&self) -> Result<(), Error> {
        self.publishable()?;
        if !self.heads.is_empty() || !self.root_changes.is_empty() || !self.staged.is_empty()
            || self.promotion.is_some()
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    /// A copy of the settled space state, to continue the mount from it.
    pub fn space_state(&self) -> Result<SpaceState, Error> {
        self.settled()?;
        Ok(SpaceState { space_nodes: self.space_nodes, ..self.state.clone() })
    }
    pub(crate) fn take_space_state(&mut self) -> Result<SpaceState, Error> {
        self.settled()?;
        let mut state = core::mem::take(&mut self.state);
        state.space_nodes = self.space_nodes;
        Ok(state)
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
        self.state.dead.count()
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
            let Some(&addr) = self.state.dead.addresses.first() else {
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
            space::reap(&mut EditStore { txn: self, tree: 2 }, &mut root, addr)?;
            self.space_root = root;
            self.stage(addr, 4)?;
            self.state.dead.addresses.remove(&addr);
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
        self.root_changes.push_back(RootChange::Put { id, value });
        Ok(())
    }
    pub fn delete_root(&mut self, id: u64) -> Result<(), Error> {
        self.healthy()?;
        self.root_changes.push_back(RootChange::Delete { id });
        Ok(())
    }
    pub fn next_root_id(&self) -> u64 {
        self.next_root_id
    }
    /// Bound later promotions of this mount's transactions by the
    /// device's oldest pinned generation (E2 design 7.1).
    pub fn set_reuse_bound(&mut self, bound: alloc::sync::Arc<core::sync::atomic::AtomicU64>) {
        self.state.reuse_bound = Some(bound);
    }
    /// Make PENDING released at or before G-2 FREE in this transaction
    /// (design 2.5 reuse rule): the FREE index at once, SPACE at settlement.
    /// Releases after `bound`, the device's oldest pinned generation, stay
    /// PENDING (E2 design 7.1); `u64::MAX` when no reader holds a pin.
    pub fn promote_pending(&mut self, bound: u64) -> Result<(), Error> {
        self.publishable()?;
        let result = self.promote_eligible(bound);
        self.result(result)
    }
    /// Read after prepare_commit has staged PENDING promotion and settled
    /// the remaining releases into the real SPACE tree.
    pub(super) fn pending_summary(&self) -> space::PendingSummary {
        self.state.quarantine.summary()
    }
    /// Quarantine a range released in this generation; settlement writes the
    /// coalesced runs to PENDING.
    fn stage(&mut self, start: u64, length: u64) -> Result<(), Error> {
        Ok(space::insert_run(&mut self.staged, start, length)?)
    }
    /// Apply deferred changes to the real COW SPACE tree: the promotion, ROOT
    /// changes, one edit per delayed reference head and the quarantine runs.
    /// Its copies/splits add heads; edited paths remain in T, so revisiting them
    /// does not recursively copy them again (design 2.5).
    pub fn settle(&mut self) -> Result<BlockPtr, Error> {
        self.publishable()?;
        let result = self.settle_changes();
        self.result(result)?;
        #[cfg(test)]
        self.check_space_state();
        Ok(self.space_root)
    }
    fn settle_changes(&mut self) -> Result<(), Error> {
        const ROOT_OBJECT: u64 = 0xffff_ffff_ffff_ff10;
        let generation = self.generation;
        loop {
            preemption_point();
            let mut root = self.space_root;
            if let Some(promotion) = self.promotion.take() {
                // First, so a range chosen from promoted space is FREE
                // in SPACE before a head claims it.
                promotion.apply(&mut EditStore { txn: self, tree: 2 }, &mut root)?;
            } else if let Some(change) = self.root_changes.pop_front() {
                let key = |id| Key { object: ROOT_OBJECT, item_type: format::ROOT, offset: id };
                let mut store = EditStore { txn: self, tree: 2 };
                match change {
                    RootChange::Delete { id } => { btree::delete(&mut store, &mut root, key(id))?; }
                    RootChange::Put { id, value } => {
                        btree::insert(&mut store, &mut root, key(id), &value.encode())?;
                    }
                }
            } else if let Some((extent, head)) = self.heads.pop_first() {
                self.apply_head(&mut root, extent, head)?;
            } else if !self.staged.is_empty() {
                // Promotion during these edits only touches older generations.
                let mut staged = core::mem::take(&mut self.staged);
                let mut runs = self.state.quarantine.take_generation(generation);
                let result = runs.write(&mut EditStore { txn: self, tree: 2 }, &mut root, generation,
                    &mut staged);
                self.state.quarantine.put_generation(generation, runs);
                result?;
            } else {
                return Ok(());
            }
            self.space_root = root;
        }
    }
    /// One REF edit for the count changes this transaction made to an
    /// allocation, skipped when they cancel; a FREE claim for a new allocation;
    /// at 0, DEAD for a node with children, else the quarantine.
    fn apply_head(&mut self, root: &mut BlockPtr, (start, length): (u64, u64), head: Head) -> Result<(), Error> {
        let count = self.known_count(start, length).ok_or(Error::Invalid)?;
        let generation = self.generation;
        let mut store = EditStore { txn: self, tree: 2 };
        let quarantine = match head.new {
            Some((kind, crcs)) => {
                space::claim(&mut store, root, start, length)?;
                if count > 0 {
                    let r = Ref { refcount: count, kind: kind as u8, alloc_generation: generation, crcs };
                    space::put_reference(&mut store, root, start, length, &r)?;
                    false
                } else if head.children {
                    if kind != Kind::FsNode {
                        return Err(Error::Invalid);
                    }
                    space::mark_dead(&mut store, root, start)?;
                    false
                } else {
                    true
                }
            }
            None => {
                if count == head.before {
                    return Ok(());
                }
                let mut r = space::reference(&store, *root, start, length)?.ok_or(Error::Invalid)?;
                if r.refcount != head.before
                    || (count > r.refcount && r.kind == Kind::SpaceNode as u8)
                    || (count < r.refcount && generation < r.alloc_generation)
                    || (head.children && r.kind != Kind::FsNode as u8)
                {
                    return Err(Error::Invalid);
                }
                if count == 0 {
                    space::drop_reference(&mut store, root, start, length, head.children)?;
                    !head.children
                } else {
                    r.refcount = count;
                    space::put_reference(&mut store, root, start, length, &r)?;
                    false
                }
            }
        };
        if quarantine {
            self.stage(start, length)?;
        }
        Ok(())
    }
    /// Host oracle after each settlement: the FREE index equals the FREE
    /// items, and each quarantine run this mount wrote is stored as such.
    /// It compares in step, with no vector, so host heap measurements are
    /// not charged for the oracle's own copies.
    #[cfg(test)]
    fn check_space_state(&self) {
        let view = self.view();
        let generation = self.generation;
        let mut indexed = self.state.free.ranges();
        let free = |offset| Key { object: 0xffff_ffff_ffff_ff20, item_type: format::FREE, offset };
        btree::visit_range(&view, self.space_root, free(0), free(u64::MAX), &mut |key, bytes| {
            let stored = (key.offset, Free::decode(bytes)?.length);
            assert_eq!(indexed.next(), Some(stored), "FREE index diverged from SPACE at generation {generation}");
            Ok::<_, Error>(true)
        }).unwrap();
        assert_eq!(indexed.next(), None, "FREE index has ranges SPACE lacks at generation {generation}");
        for (start, length, released) in self.state.quarantine.runs() {
            let key = Key { object: 0xffff_ffff_ffff_ff30, item_type: format::PENDING, offset: start };
            let bytes = btree::search(&view, self.space_root, key).unwrap()
                .unwrap_or_else(|| panic!("quarantine run {start}+{length} is not in PENDING"));
            assert_eq!(format::Pending::decode(&bytes).unwrap(),
                format::Pending { length, released_generation: released });
        }
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
