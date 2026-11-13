//! Store-independent, in-place B-tree editing for the host writer.
//!
//! This layer has no durability, copy-on-write, refcounts or block reclamation.
//! The caller supplies a private store and root. Detached blocks remain owned by
//! that caller. A fallible store must provide rollback for allocation/put failures;
//! the root is published only after the buffered puts succeed.
use super::format::{self, BlockPtr, InternalEntry, Key, LeafItemHeader, NodeHeader, NODE_SIZE};
use alloc::{collections::BTreeMap, vec, vec::Vec};

const HEADER: usize = 48;
const CAPACITY: usize = NODE_SIZE - HEADER;
const MAX_PAYLOAD: usize = 6144;
const MAX_INTERNAL: usize = 441;
const MAX_HEIGHT: u8 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Store,
    Format(format::Error),
    InvalidNode,
    PayloadTooLarge,
    HeightLimit,
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
    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), Error>;
    fn alloc(&mut self) -> Result<u64, Error>;
}

struct Node {
    header: NodeHeader,
    items: Vec<(Key, Vec<u8>)>,
    children: Vec<InternalEntry>,
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
    if bytes.len() != NODE_SIZE {
        return Err(Error::InvalidNode);
    }
    let header = NodeHeader::decode(bytes)?;
    let n = usize::from(header.nitems);
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
    let mut node = decode(&store.get(root.addr)?, root, true, None, None)?;
    let mut upper = None;
    while node.header.level > 0 {
        let i = node.route(key);
        if key < node.key(0) {
            return Ok(None);
        }
        let lower = Some(node.key(i));
        upper = node.children.get(i + 1).map(|e| e.key).or(upper);
        let ptr = node.child(i);
        let child = decode(&store.get(ptr.addr)?, ptr, false, lower, upper)?;
        same_tree(&node, &child)?;
        node = child;
    }
    Ok(node
        .items
        .binary_search_by_key(&key, |e| e.0)
        .ok()
        .map(|i| node.items[i].1.clone()))
}

struct Edit<'a, S> {
    store: &'a mut S,
    writes: BTreeMap<u64, Vec<u8>>,
}
impl<S: NodeStore> Edit<'_, S> {
    fn load(
        &self,
        ptr: BlockPtr,
        root: bool,
        lower: Option<Key>,
        upper: Option<Key>,
    ) -> Result<Node, Error> {
        if let Some(bytes) = self.writes.get(&ptr.addr) {
            decode(bytes, ptr, root, lower, upper)
        } else {
            decode(&self.store.get(ptr.addr)?, ptr, root, lower, upper)
        }
    }
    fn save(&mut self, node: &mut Node) -> Result<BlockPtr, Error> {
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
    fn pack(&mut self, mut node: Node) -> Result<Vec<InternalEntry>, Error> {
        if node.len() == 0 {
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
        left.items.append(&mut right.items);
        left.children.append(&mut right.children);
        let mut links = Vec::new();
        if !left.fits() {
            right = self.split(&mut left, right.header.addr)?;
            let ptr = self.save(&mut right)?;
            links.push(entry(right.key(0), ptr));
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
        Ok((self.pack(node)?, old))
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
    };
    let original = edit.load(*root, true, None, None)?;
    let (links, old) = edit.change(*root, true, None, None, key, value)?;
    let mut new_root = match links.len() {
        0 => {
            let mut empty = Node {
                header: NodeHeader {
                    level: 0,
                    ..original.header
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
        new_root = node.child(0);
    }
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
