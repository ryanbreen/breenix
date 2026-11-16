//! In-memory copy-on-write transactions (design 2.5, commit step 1).
//!
//! Old node bytes stay in the backing store. Edits and the space-tree fixpoint
//! accumulate private writes; only a successful `settle` supplies a space root
//! to the later commit engine. An error poisons the transaction: discard it and
//! its unpublished FS roots. Reservation policy and device I/O are higher layers.
use super::{
    btree::{self, NodeStore},
    format::{self, BlockPtr, Extent, InternalEntry, Key, LeafItemHeader, NodeHeader, Root},
    space::{self, Kind},
};
use alloc::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    vec,
    vec::Vec,
};

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

enum Change {
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
    nodes: BTreeMap<u64, Vec<u8>>,
    reserved_nodes: BTreeMap<u64, Vec<u8>>,
    reservation_root: BlockPtr,
    scratch_next: u64,
    allocated: BTreeSet<u64>,
    counts: BTreeMap<(u64, u64), u32>,
    deferred: VecDeque<Change>,
    failure: Option<Error>,
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
            reservation_root: root,
            nodes: BTreeMap::from([(20, bytes.clone())]),
            reserved_nodes: BTreeMap::from([(20, bytes)]),
            scratch_next: u64::MAX - 3,
            allocated: BTreeSet::from([20]),
            counts: BTreeMap::from([((20, 4), 1)]),
            deferred: VecDeque::new(),
            failure: None,
        })
    }
    pub fn new(base: &'a S, space_root: BlockPtr, generation: u64) -> Result<Self, Error> {
        Self::with_backing(Backing::Borrowed(base), space_root, generation)
    }
    /// Own a read-only store so an adapter can retain a transaction between calls.
    pub fn new_owned(base: S, space_root: BlockPtr, generation: u64) -> Result<Self, Error> {
        Self::with_backing(Backing::Owned(base), space_root, generation)
    }
    fn with_backing(
        base: Backing<'a, S>,
        space_root: BlockPtr,
        generation: u64,
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
        Ok(Self {
            base,
            generation,
            original_space: space_root,
            space_root,
            nodes: BTreeMap::new(),
            reserved_nodes: BTreeMap::new(),
            reservation_root: space_root,
            scratch_next: u64::MAX - 3,
            allocated: BTreeSet::new(),
            counts: BTreeMap::new(),
            deferred: VecDeque::new(),
            failure: None,
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
        let mut reservations = Reservations {
            view: View {
                base: self.base.get(),
                nodes: &BTreeMap::new(),
            },
            changes: &mut self.reserved_nodes,
            next: &mut self.scratch_next,
        };
        let start = space::allocate(
            &mut reservations,
            &mut self.reservation_root,
            length,
            kind,
            crcs,
            self.generation,
        )?;
        self.counts.insert((start, length), 1);
        self.allocated.insert(start);
        self.deferred.push_back(Change::Allocate {
            start,
            length,
            kind,
            crcs: crcs.to_vec(),
        });
        Ok(start)
    }
    /// The initial reference belongs to the caller and is transferred to its
    /// new EXTENT item. Additional items naming that REF need `increment` first.
    pub fn allocate_data(&mut self, crcs: &[u32]) -> Result<u64, Error> {
        self.healthy()?;
        let result = self.reserve(crcs.len() as u64, Kind::Data, crcs);
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
            self.deferred.push_back(Change::Release {
                start,
                length,
                children,
            });
            Ok(())
        })();
        self.result(result)
    }
    fn prepare(&mut self, header: &mut NodeHeader) -> Result<(), Error> {
        if header.generation == self.generation && self.allocated.contains(&header.addr) {
            return Ok(());
        }
        if header.generation >= self.generation {
            return Err(Error::Invalid);
        }
        let count = self.count(header.addr, 4)?;
        if count == 0 {
            return Err(Error::Invalid);
        }
        // Ancestors call this before descending. Their increments must be in
        // the effective counts before a child decides whether it is shared.
        if count > 1 {
            if header.tree != 1 {
                return Err(Error::Invalid);
            }
            let bytes = self.view().get(header.addr)?;
            for (start, length) in outgoing(&bytes)? {
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
    fn change(
        &mut self,
        root: &mut BlockPtr,
        key: Key,
        value: Option<&[u8]>,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.healthy()?;
        let result = (|| {
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
        self.deferred.push_back(Change::Root { id, value });
        Ok(())
    }
    /// Apply deferred changes to the real COW SPACE tree. Its copies/splits add
    /// more allocation/release work to this queue. Edited paths remain in T,
    /// so revisiting them does not recursively copy them again (design 2.5).
    pub fn settle(&mut self) -> Result<BlockPtr, Error> {
        self.healthy()?;
        while let Some(change) = self.deferred.pop_front() {
            let mut root = self.space_root;
            let generation = self.generation;
            let mut store = EditStore { txn: self, tree: 2 };
            let result = match change {
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
        self.txn.result(result).map_err(|_| btree::Error::Store)
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
