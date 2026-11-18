//! Store-independent B-tree editing for the host writer and transactions.
//!
//! The default store hooks edit in place; transactions supply copy-on-write and
//! detached-node accounting. The caller supplies a private store and root.
//! A fallible store must provide rollback for allocation/put failures;
//! the root is published only after the buffered puts succeed.
use super::preempt::preemption_point;
use super::format::{self, BlockPtr, InternalEntry, Key, LeafItemHeader, NodeHeader, NODE_SIZE};
use alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec};

const HEADER: usize = 48;
const CAPACITY: usize = NODE_SIZE - HEADER;
const MAX_PAYLOAD: usize = 6144;
const MAX_INTERNAL: usize = 441;
const MAX_HEIGHT: u8 = 8;

// Host measurement of the production decoder, isolated from parallel tests.
#[cfg(test)]
pub mod measurement {
    extern crate std;
    std::thread_local! {
        static CHANGES: core::cell::RefCell<Option<alloc::vec::Vec<ChangeWork>>> = const { core::cell::RefCell::new(None) };
        static REFRESH_CALLS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
        static MUTATIONS: core::cell::Cell<[u64; 8]> = const { core::cell::Cell::new([0; 8]) };
        static DECODES: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
        static ITEMS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
        static FS_RESERVATIONS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
        static SPACE_RESERVATIONS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
        static NODES: core::cell::RefCell<alloc::collections::BTreeSet<u64>> = const {
            core::cell::RefCell::new(alloc::collections::BTreeSet::new())
        };
        static HEIGHT: core::cell::Cell<u8> = const { core::cell::Cell::new(0) };
    }
    #[derive(Clone, Copy, Debug)]
    pub struct ChangeWork { pub height: u8, pub reads: usize, pub writes: usize }
    pub(super) fn changed(work: ChangeWork) {
        CHANGES.with(|rows| { if let Some(rows) = rows.borrow_mut().as_mut() { rows.push(work); } });
    }
    pub fn take_changes() -> alloc::vec::Vec<ChangeWork> {
        CHANGES.with(|rows| rows.replace(Some(alloc::vec::Vec::new())).unwrap_or_default())
    }
    pub(crate) fn refresh_call() { REFRESH_CALLS.with(|n| n.set(n.get() + 1)); }
    pub fn take_refresh_calls() -> u64 { REFRESH_CALLS.with(|n| n.replace(0)) }
    // Reservations, granted/retained/returned blocks, inode writes, node writes,
    // node reads and VNode refresh visits. Host-only; no guest-path cost.
    pub(crate) fn mutation(index: usize, amount: u64) {
        MUTATIONS.with(|cell| { let mut n = cell.get(); n[index] += amount; cell.set(n); });
    }
    pub fn take_mutations() -> [u64; 8] { MUTATIONS.with(|cell| cell.replace([0; 8])) }
    pub fn take_decodes() -> u64 {
        DECODES.with(|count| count.replace(0))
    }
    pub(super) fn decoded() {
        DECODES.with(|count| count.set(count.get() + 1));
    }
    pub(super) fn node(addr: u64, level: u8) {
        NODES.with(|nodes| { nodes.borrow_mut().insert(addr); });
        HEIGHT.with(|height| height.set(height.get().max(level + 1)));
    }
    pub fn take_geometry() -> (usize, u8) {
        let nodes = NODES.with(|nodes| core::mem::take(&mut *nodes.borrow_mut()).len());
        (nodes, HEIGHT.with(|height| height.replace(0)))
    }
    pub(super) fn items(n: u64) { ITEMS.with(|count| count.set(count.get() + n)); }
    pub(super) fn reservation(space: bool) {
        let visit = |count: &core::cell::Cell<u64>| count.set(count.get() + 1);
        if space { SPACE_RESERVATIONS.with(visit); } else { FS_RESERVATIONS.with(visit); }
    }
    pub fn take_work() -> [u64; 4] {
        [take_decodes(), ITEMS.with(|n| n.replace(0)), FS_RESERVATIONS.with(|n| n.replace(0)),
            SPACE_RESERVATIONS.with(|n| n.replace(0))]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Store,
    Read(ReadFailure),
    Format(format::Error),
    InvalidNode,
    PayloadTooLarge,
    HeightLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadCause { Device, Driver(crate::block::read_failure::VirtioReadFailure), Verification, Allocation }
impl ReadCause {
    pub fn name(self) -> &'static str {
        match self {
            Self::Driver(failure) if failure.context_refused() => "io-context",
            Self::Device | Self::Driver(_) => "device-read",
            Self::Verification => "verification",
            Self::Allocation => "allocation",
        }
    }
}
impl core::fmt::Display for ReadCause {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.name())?;
        if let Self::Driver(failure) = self { write!(f, " {}", failure)?; }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadFailure {
    pub cause: ReadCause,
    pub addr: u64,
}
impl Error {
    pub fn at_node(self, addr: u64) -> Self {
        match self {
            Self::Read(_) => self,
            _ => Self::Read(ReadFailure { cause: ReadCause::Verification, addr }),
        }
    }
}
impl From<format::Error> for Error {
    fn from(error: format::Error) -> Self {
        Self::Format(error)
    }
}

/// Addresses name four-block nodes. `alloc` reserves a fresh, disjoint address;
/// `put` replaces the bytes at that address, and `get` returns its encoded node.
/// Allocation and reclamation policy belong to the caller, not the B-tree.
pub trait NodeStore {
    fn get(&self, addr: u64) -> Result<Vec<u8>, Error>;
    /// Immutable stores may retain a verified decoding, keyed by the
    /// complete parent pointer. Mutable stores decode their current bytes.
    fn read_node(&self, ptr: BlockPtr) -> Result<Arc<Node>, Error> {
        decode_node(&self.get(ptr.addr)?, ptr)
    }
    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), Error>;
    fn alloc(&mut self) -> Result<u64, Error>;
    /// Called top-down before modifying a node, including rebalance siblings.
    /// A transaction may replace its address/generation and transfer references.
    fn prepare_edit(&mut self, _header: &mut NodeHeader) -> Result<(), Error> {
        Ok(())
    }
    /// The node's outgoing references have been transferred or removed already.
    fn retire(&mut self, _header: NodeHeader) -> Result<(), Error> {
        Ok(())
    }
}

pub struct Node {
    header: NodeHeader,
    items: Vec<(Key, Vec<u8>)>,
    children: Vec<InternalEntry>,
}

pub(crate) fn decode_node(bytes: &[u8], ptr: BlockPtr) -> Result<Arc<Node>, Error> {
    Ok(Arc::new(decode(bytes, ptr, true, None, None)?))
}

// Bounds and parent identity belong to this traversal, not to the cached node.
// Recheck them even when the exact node/pointer was verified on an earlier path.
fn read_node<S: NodeStore>(
    store: &S,
    ptr: BlockPtr,
    root: bool,
    lower: Option<Key>,
    upper: Option<Key>,
) -> Result<Arc<Node>, Error> {
    #[cfg(test)]
    measurement::mutation(6, 1);
    let node = store.read_node(ptr)?;
    if (node.len() == 0 && !root)
        || (node.len() != 0
            && (lower.is_some_and(|k| node.key(0) < k)
                || upper.is_some_and(|k| node.key(node.len() - 1) >= k)))
    {
        return Err(Error::InvalidNode.at_node(ptr.addr));
    }
    Ok(node)
}
impl Node {
    fn len(&self) -> usize {
        if self.header.level == 0 {
            self.items.len()
        } else {
            self.children.len()
        }
    }
    fn key(&self, i: usize) -> Key {
        if self.header.level == 0 {
            self.items[i].0
        } else {
            self.children[i].key
        }
    }
    fn weight(&self, i: usize) -> usize {
        if self.header.level == 0 {
            21 + self.items[i].1.len()
        } else {
            37
        }
    }
    fn used(&self) -> usize {
        (0..self.len()).map(|i| self.weight(i)).sum()
    }
    fn fits(&self) -> bool {
        self.used() <= CAPACITY && (self.header.level == 0 || self.len() <= MAX_INTERNAL)
    }
    fn child(&self, i: usize) -> BlockPtr {
        let e = self.children[i];
        BlockPtr {
            addr: e.child_addr,
            generation: e.child_generation,
            csum: e.child_csum,
            level: self.header.level - 1,
        }
    }
    fn route(&self, key: Key) -> usize {
        self.children
            .partition_point(|e| e.key <= key)
            .saturating_sub(1)
    }
    fn encode(&mut self) -> Result<(BlockPtr, Vec<u8>), Error> {
        if !self.fits() || self.header.level >= MAX_HEIGHT {
            return Err(Error::InvalidNode);
        }
        let mut bytes = vec![0; NODE_SIZE];
        self.header.nitems = self.len() as u16;
        if self.header.level == 0 {
            let mut tail = NODE_SIZE;
            for (i, (key, value)) in self.items.iter().enumerate() {
                tail -= value.len();
                bytes[tail..tail + value.len()].copy_from_slice(value);
                bytes[HEADER + i * 21..HEADER + (i + 1) * 21].copy_from_slice(
                    &LeafItemHeader {
                        key: *key,
                        data_off: tail as u16,
                        data_len: value.len() as u16,
                    }
                    .encode(),
                );
            }
        } else {
            for (i, entry) in self.children.iter().enumerate() {
                bytes[HEADER + i * 37..HEADER + (i + 1) * 37].copy_from_slice(&entry.encode());
            }
        }
        self.header.encode(&mut bytes)?;
        Ok((
            BlockPtr {
                addr: self.header.addr,
                generation: self.header.generation,
                level: self.header.level,
                csum: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            },
            bytes,
        ))
    }
}

fn decode(
    bytes: &[u8],
    ptr: BlockPtr,
    root: bool,
    lower: Option<Key>,
    upper: Option<Key>,
) -> Result<Node, Error> {
    #[cfg(test)]
    measurement::decoded();
    if bytes.len() != NODE_SIZE {
        return Err(Error::InvalidNode);
    }
    let header = NodeHeader::decode(bytes)?;
    #[cfg(test)]
    measurement::node(header.addr, header.level);
    let n = usize::from(header.nitems);
    #[cfg(test)]
    measurement::items(n as u64);
    if header.addr != ptr.addr
        || header.generation != ptr.generation
        || header.level != ptr.level
        || ptr.csum != u32::from_le_bytes(bytes[8..12].try_into().unwrap())
        || header.level >= MAX_HEIGHT
        || !(1..=2).contains(&header.tree)
        || header.generation == 0
        || (n == 0 && !(root && header.level == 0))
        || (header.level > 0 && n > MAX_INTERNAL)
    {
        return Err(Error::InvalidNode);
    }
    let width = if header.level == 0 { 21 } else { 37 };
    let end = HEADER + n * width;
    if end > NODE_SIZE {
        return Err(Error::InvalidNode);
    }
    let mut used = vec![false; NODE_SIZE];
    used[..end].fill(true);
    let mut node = Node {
        header,
        items: Vec::new(),
        children: Vec::new(),
    };
    for i in 0..n {
        let start = HEADER + i * width;
        let key = Key::decode(&bytes[start..start + 17])?;
        if (i > 0 && node.key(i - 1) >= key)
            || lower.is_some_and(|l| key < l)
            || upper.is_some_and(|u| key >= u)
        {
            return Err(Error::InvalidNode);
        }
        if header.level == 0 {
            let h = LeafItemHeader::decode(&bytes[start..start + 21])?;
            let lo = usize::from(h.data_off);
            let hi = lo + usize::from(h.data_len);
            if usize::from(h.data_len) > MAX_PAYLOAD
                || lo < end
                || hi > NODE_SIZE
                || used[lo..hi].iter().any(|v| *v)
            {
                return Err(Error::InvalidNode);
            }
            used[lo..hi].fill(true);
            node.items.push((key, bytes[lo..hi].to_vec()));
        } else {
            let entry = InternalEntry::decode(&bytes[start..start + 37])?;
            if entry.child_generation > header.generation {
                return Err(Error::InvalidNode);
            }
            node.children.push(entry);
        }
    }
    if bytes.iter().zip(used).any(|(v, u)| !u && *v != 0) {
        return Err(Error::InvalidNode);
    }
    Ok(node)
}

fn entry(key: Key, ptr: BlockPtr) -> InternalEntry {
    InternalEntry {
        key,
        child_addr: ptr.addr,
        child_generation: ptr.generation,
        child_csum: ptr.csum,
    }
}
fn same_tree(parent: &Node, child: &Node) -> Result<(), Error> {
    if child.header.tree != parent.header.tree || child.header.fs_uuid != parent.header.fs_uuid {
        return Err(Error::InvalidNode);
    }
    Ok(())
}

/// Make an empty root leaf. The format's item-type/payload semantics are a layer
/// above this generic tree; here both FS and SPACE trees use the same machinery.
pub fn create<S: NodeStore>(
    store: &mut S,
    tree: u8,
    fs_uuid: [u8; 16],
    generation: u64,
) -> Result<BlockPtr, Error> {
    if !(1..=2).contains(&tree) || generation == 0 {
        return Err(Error::InvalidNode);
    }
    let mut node = Node {
        header: NodeHeader {
            tree,
            fs_uuid,
            generation,
            addr: store.alloc()?,
            level: 0,
            nitems: 0,
        },
        items: Vec::new(),
        children: Vec::new(),
    };
    let (ptr, bytes) = node.encode()?;
    store.put(ptr.addr, bytes)?;
    Ok(ptr)
}

pub fn search<S: NodeStore>(store: &S, root: BlockPtr, key: Key) -> Result<Option<Vec<u8>>, Error> {
    let mut node = read_node(store, root, true, None, None)?;
    let mut upper = None;
    while node.header.level > 0 {
        let i = node.route(key);
        if key < node.key(0) {
            return Ok(None);
        }
        let lower = Some(node.key(i));
        upper = node.children.get(i + 1).map(|e| e.key).or(upper);
        let ptr = node.child(i);
        let child = read_node(store, ptr, false, lower, upper)?;
        same_tree(&node, &child)?;
        node = child;
    }
    Ok(node
        .items
        .binary_search_by_key(&key, |e| e.0)
        .ok()
        .map(|i| node.items[i].1.clone()))
}

/// Count nodes on an inode's paths, or a whole tree for the space bound.
/// Private nodes need no further COW copy in the open generation.
pub(super) fn reservation_nodes<S: NodeStore>(
    store: &S,
    root: BlockPtr,
    object: Option<u64>,
    generation: u64,
) -> Result<u64, Error> {
    #[cfg(test)]
    measurement::reservation(object.is_none());
    let node = store.read_node(root)?;
    let mut count = u64::from(root.generation != generation);
    for (i, child) in node.children.iter().enumerate() {
        preemption_point();
        if object.is_some_and(|ino| child.key.object > ino
            || node.children.get(i + 1).is_some_and(|next| next.key.object < ino))
        {
            continue;
        }
        count += reservation_nodes(store, node.child(i), object, generation)?;
    }
    Ok(count)
}

/// Return items in an inclusive key range, visiting only intersecting branches.
pub fn range<S: NodeStore>(
    store: &S,
    root: BlockPtr,
    lower: Key,
    upper: Key,
) -> Result<Vec<(Key, Vec<u8>)>, Error> {
    range_limit(store, root, lower, upper, usize::MAX)
}

/// Read at most `limit` items, stopping before visiting later branches.
/// Resuming at the successor key re-descends from the durable root.
pub fn range_limit<S: NodeStore>(
    store: &S,
    root: BlockPtr,
    lower: Key,
    upper: Key,
    limit: usize,
) -> Result<Vec<(Key, Vec<u8>)>, Error> {
    let mut out = Vec::new();
    if limit != 0 {
        visit_range(store, root, lower, upper, &mut |key, bytes| {
            preemption_point();
            out.push((key, bytes.to_vec()));
            Ok::<_, Error>(out.len() < limit)
        })?;
    }
    Ok(out)
}

/// Visit an inclusive range once, stopping as soon as the sink refuses an item.
/// This call retains its ancestors; a later call resumes from its current root.
/// The sink's error type keeps caller errors distinct from B-tree failures.
pub fn visit_range<S: NodeStore, E: From<Error>>(
    store: &S,
    root: BlockPtr,
    lower: Key,
    upper: Key,
    visit: &mut dyn FnMut(Key, &[u8]) -> Result<bool, E>,
) -> Result<(), E> {
    fn walk<S: NodeStore, E: From<Error>>(
        store: &S,
        node: Arc<Node>,
        bound: Option<Key>,
        lower: Key,
        upper: Key,
        visit: &mut dyn FnMut(Key, &[u8]) -> Result<bool, E>,
    ) -> Result<bool, E> {
        preemption_point();
        if node.header.level == 0 {
            let start = node.items.partition_point(|(key, _)| *key < lower);
            for (key, bytes) in &node.items[start..] {
                preemption_point();
                if *key > upper { break; }
                if !visit(*key, bytes)? { return Ok(false); }
            }
        } else {
            for (i, entry) in node.children.iter().enumerate() {
                preemption_point();
                let next = node.children.get(i + 1).map(|e| e.key).or(bound);
                if entry.key <= upper && next.is_none_or(|k| k > lower) {
                    let ptr = node.child(i);
                    let child = read_node(store, ptr, false, Some(entry.key), next)?;
                    same_tree(&node, &child)?;
                    if !walk(store, child, next, lower, upper, visit)? {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
    if lower <= upper {
        let node = read_node(store, root, true, None, None)?;
        walk(store, node, None, lower, upper, visit)?;
    }
    Ok(())
}

struct Edit<'a, S> {
    store: &'a mut S,
    writes: BTreeMap<u64, Vec<u8>>,
    empty_root: Option<NodeHeader>,
    #[cfg(test)]
    reads: core::cell::Cell<usize>,
    #[cfg(test)]
    writes_count: usize,
}
impl<S: NodeStore> Edit<'_, S> {
    fn load(
        &self,
        ptr: BlockPtr,
        root: bool,
        lower: Option<Key>,
        upper: Option<Key>,
    ) -> Result<Node, Error> {
        #[cfg(test)]
        self.reads.set(self.reads.get() + 1);
        if let Some(bytes) = self.writes.get(&ptr.addr) {
            decode(bytes, ptr, root, lower, upper)
        } else {
            decode(&self.store.get(ptr.addr)?, ptr, root, lower, upper)
        }
    }
    fn save(&mut self, node: &mut Node) -> Result<BlockPtr, Error> {
        #[cfg(test)]
        { self.writes_count += 1; }
        let (ptr, bytes) = node.encode()?;
        self.writes.insert(ptr.addr, bytes);
        Ok(ptr)
    }
    // Pick the closest byte balance among legal cuts. The payload bound ensures
    // an overflowing leaf can be divided into two nonempty nodes that fit.
    fn split(&mut self, node: &mut Node, right_addr: u64) -> Result<Node, Error> {
        let total = node.used();
        let mut left = 0;
        let mut best = None;
        for i in 1..node.len() {
            left += node.weight(i - 1);
            let right = total - left;
            if left <= CAPACITY
                && right <= CAPACITY
                && (node.header.level == 0 || (i <= MAX_INTERNAL && node.len() - i <= MAX_INTERNAL))
            {
                let score = left.abs_diff(right);
                if best.is_none_or(|(_, old)| score < old) {
                    best = Some((i, score));
                }
            }
        }
        let cut = best.ok_or(Error::InvalidNode)?.0;
        Ok(Node {
            header: NodeHeader {
                addr: right_addr,
                ..node.header
            },
            items: if node.header.level == 0 {
                node.items.split_off(cut)
            } else {
                Vec::new()
            },
            children: if node.header.level > 0 {
                node.children.split_off(cut)
            } else {
                Vec::new()
            },
        })
    }
    fn pack(&mut self, mut node: Node, root: bool) -> Result<Vec<InternalEntry>, Error> {
        if node.len() == 0 {
            if root {
                self.empty_root = Some(node.header);
            } else {
                self.store.retire(node.header)?;
            }
            return Ok(Vec::new());
        }
        let mut links = Vec::new();
        let right = if node.fits() {
            None
        } else {
            let addr = self.store.alloc()?;
            Some(self.split(&mut node, addr)?)
        };
        let ptr = self.save(&mut node)?;
        links.push(entry(node.key(0), ptr));
        if let Some(mut right) = right {
            let ptr = self.save(&mut right)?;
            links.push(entry(right.key(0), ptr));
        }
        Ok(links)
    }
    fn rebalance(&mut self, parent: &mut Node, i: usize, upper: Option<Key>) -> Result<(), Error> {
        if parent.len() < 2 {
            return Ok(());
        }
        let child = self.load(
            parent.child(i),
            false,
            Some(parent.key(i)),
            parent.children.get(i + 1).map(|e| e.key).or(upper),
        )?;
        same_tree(parent, &child)?;
        if child.used() >= CAPACITY / 4 {
            return Ok(());
        }
        let left_i = if i == 0 { 0 } else { i - 1 };
        let mut left = self.load(
            parent.child(left_i),
            false,
            Some(parent.key(left_i)),
            Some(parent.key(left_i + 1)),
        )?;
        let mut right = self.load(
            parent.child(left_i + 1),
            false,
            Some(parent.key(left_i + 1)),
            parent.children.get(left_i + 2).map(|e| e.key).or(upper),
        )?;
        same_tree(parent, &left)?;
        same_tree(parent, &right)?;
        self.store.prepare_edit(&mut left.header)?;
        self.store.prepare_edit(&mut right.header)?;
        left.items.append(&mut right.items);
        left.children.append(&mut right.children);
        let mut links = Vec::new();
        if !left.fits() {
            right = self.split(&mut left, right.header.addr)?;
            let ptr = self.save(&mut right)?;
            links.push(entry(right.key(0), ptr));
        } else {
            self.store.retire(right.header)?;
        }
        let ptr = self.save(&mut left)?;
        links.insert(0, entry(left.key(0), ptr));
        parent.children.splice(left_i..left_i + 2, links);
        Ok(())
    }
    fn change(
        &mut self,
        ptr: BlockPtr,
        root: bool,
        lower: Option<Key>,
        upper: Option<Key>,
        key: Key,
        value: Option<&[u8]>,
    ) -> Result<(Vec<InternalEntry>, Option<Vec<u8>>), Error> {
        let mut node = self.load(ptr, root, lower, upper)?;
        self.store.prepare_edit(&mut node.header)?;
        let old;
        if node.header.level == 0 {
            match node.items.binary_search_by_key(&key, |e| e.0) {
                Ok(i) => {
                    old = Some(if let Some(value) = value {
                        core::mem::replace(&mut node.items[i].1, value.to_vec())
                    } else {
                        node.items.remove(i).1
                    });
                }
                Err(i) => {
                    old = None;
                    if let Some(value) = value {
                        node.items.insert(i, (key, value.to_vec()));
                    }
                }
            }
        } else {
            let i = node.route(key);
            let child = self.load(
                node.child(i),
                false,
                Some(node.key(i)),
                node.children.get(i + 1).map(|e| e.key).or(upper),
            )?;
            same_tree(&node, &child)?;
            let (links, previous) = self.change(
                node.child(i),
                false,
                Some(node.key(i)),
                node.children.get(i + 1).map(|e| e.key).or(upper),
                key,
                value,
            )?;
            old = previous;
            let replaced = links.len();
            node.children.splice(i..i + 1, links);
            if replaced == 1 {
                self.rebalance(&mut node, i, upper)?;
            }
        }
        Ok((self.pack(node, root)?, old))
    }
}

fn change<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    key: Key,
    value: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, Error> {
    let mut edit = Edit {
        store,
        writes: BTreeMap::new(),
        empty_root: None,
        #[cfg(test)]
        reads: core::cell::Cell::new(0),
        #[cfg(test)]
        writes_count: 0,
    };
    let original = edit.load(*root, true, None, None)?;
    let (links, old) = edit.change(*root, true, None, None, key, value)?;
    let mut new_root = match links.len() {
        0 => {
            let mut empty = Node {
                header: NodeHeader {
                    level: 0,
                    ..edit.empty_root.take().ok_or(Error::InvalidNode)?
                },
                items: Vec::new(),
                children: Vec::new(),
            };
            edit.save(&mut empty)?
        }
        1 => BlockPtr {
            addr: links[0].child_addr,
            generation: links[0].child_generation,
            csum: links[0].child_csum,
            level: root.level,
        },
        _ => {
            if root.level + 1 >= MAX_HEIGHT {
                return Err(Error::HeightLimit);
            }
            let mut node = Node {
                header: NodeHeader {
                    addr: edit.store.alloc()?,
                    level: root.level + 1,
                    generation: links[0].child_generation,
                    ..original.header
                },
                items: Vec::new(),
                children: links,
            };
            edit.save(&mut node)?
        }
    };
    loop {
        let node = edit.load(new_root, true, None, None)?;
        if node.header.level == 0 || node.len() != 1 {
            break;
        }
        edit.store.retire(node.header)?;
        new_root = node.child(0);
    }
    #[cfg(test)]
    measurement::changed(measurement::ChangeWork {
        height: root.level + 1, reads: edit.reads.get(), writes: edit.writes_count,
    });
    for (addr, bytes) in edit.writes {
        edit.store.put(addr, bytes)?;
    }
    *root = new_root;
    Ok(old)
}

/// Insert or replace a payload, returning its previous value.
pub fn insert<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    key: Key,
    value: &[u8],
) -> Result<Option<Vec<u8>>, Error> {
    if value.len() > MAX_PAYLOAD {
        return Err(Error::PayloadTooLarge);
    }
    change(store, root, key, Some(value))
}

/// Remove a key, returning the optional previous value.
pub fn delete<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    key: Key,
) -> Result<Option<Vec<u8>>, Error> {
    change(store, root, key, None)
}
