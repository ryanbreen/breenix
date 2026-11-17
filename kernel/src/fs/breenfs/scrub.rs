//! Read-only, address-ordered scrub of durable REF and DEAD allocations.
//!
//! The caller holds publication/reuse exclusion for each `step`, and supplies
//! the latest durable root and generation together. The cursor stores an
//! address and key bound; step resolves pointers and checksums from that root.
use super::{
    btree::{self, NodeStore},
    commit::DeviceStore,
    crc32c,
    format::{
        self, BlockPtr, InternalEntry, Key, LeafItemHeader, NodeHeader, Ref, BLOCK_SIZE, NODE_SIZE,
    },
};
use alloc::{collections::VecDeque, vec::Vec};
use core::cell::{Cell, RefCell};

// Keep a bounded navigation working set across batches of one durable generation.
// Every uncached node and every data read consumes the same raw-I/O budget.
struct BudgetStore<'a, S> {
    store: &'a S,
    nodes: RefCell<VecDeque<(u64, Vec<u8>)>>,
    used: Cell<usize>,
    budget: usize,
    exhausted: Cell<bool>,
}
impl<S> BudgetStore<'_, S> {
    fn charge(&self, bytes: usize) -> Result<(), btree::Error> {
        if bytes > self.budget - self.used.get() {
            self.exhausted.set(true);
            return Err(btree::Error::Store);
        }
        self.used.set(self.used.get() + bytes);
        Ok(())
    }
}
impl<S: Store> NodeStore for BudgetStore<'_, S> {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        let mut nodes = self.nodes.borrow_mut();
        if let Some(i) = nodes.iter().position(|(a, _)| *a == addr) {
            let entry = nodes.remove(i).unwrap();
            let bytes = entry.1.clone();
            nodes.push_back(entry);
            return Ok(bytes);
        }
        self.charge(NODE_SIZE)?;
        let bytes = self.store.get(addr)?;
        if nodes.len() == 16 { nodes.pop_front(); }
        nodes.push_back((addr, bytes.clone()));
        Ok(bytes)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> { Err(btree::Error::Store) }
    fn alloc(&mut self) -> Result<u64, btree::Error> { Err(btree::Error::Store) }
}
impl<S: Store> Store for BudgetStore<'_, S> {
    fn data(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.charge(BLOCK_SIZE)?;
        self.store.data(addr)
    }
}

/// Immutable device view. Navigation reads nodes through NodeStore; data reads
/// must likewise see durable bytes while the caller excludes publication/reuse.
pub trait Store: NodeStore {
    fn data(&self, address: u64) -> Result<Vec<u8>, btree::Error>;
}
impl Store for DeviceStore<'_> {
    fn data(&self, address: u64) -> Result<Vec<u8>, btree::Error> {
        super::commit::read(self.0, address, BLOCK_SIZE).map_err(|_| btree::Error::Store)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Node,
    Data,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BadAddress {
    pub address: u64,
    pub kind: Kind,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Completion {
    pub generation: u64,
    pub errors: u64,
}

pub struct Scrub {
    address: u64,
    floor: Key,
    generation: u64,
    blocks: u64,
    errors: u64,
    bad: Vec<BadAddress>,
    completion: Option<Completion>,
    nodes: VecDeque<(u64, Vec<u8>)>,
}
impl Default for Scrub {
    fn default() -> Self {
        Self::new()
    }
}
impl Scrub {
    pub fn new() -> Self {
        Self {
            address: 20,
            floor: key(0),
            generation: 0,
            blocks: 0,
            errors: 0,
            bad: Vec::new(),
            completion: None,
            nodes: VecDeque::new(),
        }
    }
    pub fn blocks(&self) -> u64 {
        self.blocks
    }
    pub fn errors(&self) -> u64 {
        self.errors
    }
    pub fn bad(&self) -> &[BadAddress] {
        &self.bad
    }
    pub fn completion(&self) -> Option<Completion> {
        self.completion
    }
    fn record(&mut self, address: u64, kind: Kind) {
        let bad = BadAddress { address, kind };
        // A corrupt SPACE node can be encountered on its search path before
        // its own REF. Do not count that same diagnosed node twice.
        if self.bad.contains(&bad) {
            return;
        }
        self.errors = self.errors.saturating_add(1);
        if self.bad.len() < 64 {
            self.bad.push(bad);
        }
    }
    fn finish(&mut self, generation: u64) {
        self.completion = Some(Completion {
            generation,
            errors: self.errors,
        });
    }
    /// Bound all raw reads, including navigation, to `budget` bytes. A node
    /// needs NODE_SIZE; cached navigation permits progress across small batches.
    /// Generation changes discard cached pointers before publication/reuse.
    pub fn step<S: Store>(
        &mut self, store: &S, root: BlockPtr, generation: u64, budget: usize,
    ) -> Result<usize, btree::Error> {
        if generation != self.generation { self.nodes.clear(); }
        let limited = BudgetStore {
            store, nodes: RefCell::new(core::mem::take(&mut self.nodes)),
            used: Cell::new(0), budget, exhausted: Cell::new(false),
        };
        let result = self.step_inner(&limited, root, generation, budget);
        let used = limited.used.get();
        self.nodes = limited.nodes.into_inner();
        result.map(|_| used)
    }
    fn step_inner<S: Store>(
        &mut self, store: &BudgetStore<'_, S>, root: BlockPtr,
        generation: u64, budget: usize,
    ) -> Result<usize, btree::Error> {
        if self.completion.is_some() || budget == 0 {
            return Ok(0);
        }
        if generation == 0 || generation < self.generation || root.generation > generation {
            return Err(btree::Error::InvalidNode);
        }
        self.generation = generation;
        let bytes = store.get(root.addr);
        if store.exhausted.get() { return Ok(0); }
        let header = bytes
            .as_ref()
            .ok()
            .and_then(|b| verify_node(b, root, 2, None, generation).ok());
        let Some(header) = header else {
            self.record(root.addr, Kind::Node);
            self.finish(generation);
            return Ok(0);
        };
        let reader = Reader {
            store,
            uuid: header.fs_uuid,
            generation,
        };
        let mut used = 0;
        loop {
            // A new allocation may straddle the cursor after commits between
            // batches. Data REFs are at most 256 blocks; look back far enough
            // to rediscover that allocation without retaining its old REF.
            let lower = key(self.address.saturating_sub(255)).max(self.floor);
            let found = reader.next(root, lower, None, self.address);
            if store.exhausted.get() { break; }
            let (k, value) = match found {
                Ok(Some(item)) => item,
                Ok(None) => {
                    self.finish(generation);
                    break;
                }
                Err(fault) => {
                    self.record(fault.address, Kind::Node);
                    // No checksum can enumerate a corrupt SPACE subtree. Skip
                    // its key interval, but continue checking readable branches.
                    if let Some(next) = fault.resume {
                        self.floor = next;
                        continue;
                    }
                    self.finish(generation);
                    break;
                }
            };
            let reference = if k.item_type == format::REF {
                Ref::decode(&value).ok()
            } else {
                None
            };
            let data = reference.as_ref().is_some_and(|r| r.kind == 3);
            let length = k.offset;
            let valid = if k.item_type == format::DEAD {
                length == 4 && value.is_empty()
            } else {
                reference.as_ref().is_some_and(|r| {
                    r.refcount != 0
                        && r.alloc_generation != 0
                        && r.alloc_generation <= generation
                        && match r.kind {
                            1 => length == 4 && r.crcs.is_empty(),
                            2 => length == 4 && r.crcs.is_empty() && r.refcount == 1,
                            3 => (1..=256).contains(&length) && r.crcs.len() as u64 == length,
                            _ => false,
                        }
                })
            };
            let Some(end) = k.object.checked_add(length).filter(|_| valid) else {
                self.record(k.object, if data { Kind::Data } else { Kind::Node });
                self.floor = successor(k);
                continue;
            };
            let cost = if data { BLOCK_SIZE } else { NODE_SIZE };
            if budget - used < cost {
                break;
            }
            if data {
                let r = reference.as_ref().unwrap();
                let address = self.address.max(k.object);
                let good = store.data(address).is_ok_and(|bytes| {
                    bytes.len() == BLOCK_SIZE
                        && crc32c::checksum(&bytes) == r.crcs[(address - k.object) as usize]
                });
                if store.exhausted.get() { break; }
                if !good {
                    self.record(address, Kind::Data);
                }
                self.address = address + 1;
                self.blocks += 1;
            } else {
                let good = store.get(k.object).is_ok_and(|bytes| {
                    let Ok(h) = NodeHeader::decode(&bytes) else {
                        return false;
                    };
                    let ptr = BlockPtr {
                        addr: k.object,
                        generation: reference
                            .as_ref()
                            .map_or(h.generation, |r| r.alloc_generation),
                        level: h.level,
                        csum: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
                    };
                    let tree = reference.as_ref().map_or(1, |r| r.kind);
                    verify_node(&bytes, ptr, tree, Some(reader.uuid), generation).is_ok()
                });
                if store.exhausted.get() { break; }
                if !good {
                    self.record(k.object, Kind::Node);
                }
                self.address = end;
                self.blocks += 4;
            }
            used += cost;
            if used == budget {
                break;
            }
        }
        Ok(used)
    }
}
fn key(object: u64) -> Key {
    Key {
        object,
        item_type: 0,
        offset: 0,
    }
}
fn successor(k: Key) -> Key {
    if k.offset != u64::MAX {
        Key {
            offset: k.offset + 1,
            ..k
        }
    } else if k.item_type != u8::MAX {
        Key {
            item_type: k.item_type + 1,
            offset: 0,
            ..k
        }
    } else {
        key(k.object.saturating_add(1))
    }
}
fn verify_node(
    bytes: &[u8],
    ptr: BlockPtr,
    tree: u8,
    uuid: Option<[u8; 16]>,
    generation: u64,
) -> Result<NodeHeader, btree::Error> {
    btree::decode_node(bytes, ptr)?;
    let h = NodeHeader::decode(bytes)?;
    if h.tree != tree || uuid.is_some_and(|id| h.fs_uuid != id) || h.generation > generation {
        return Err(btree::Error::InvalidNode);
    }
    Ok(h)
}
struct Fault {
    address: u64,
    resume: Option<Key>,
}
struct Reader<'a, S> {
    store: &'a S,
    uuid: [u8; 16],
    generation: u64,
}
impl<S: Store> Reader<'_, S> {
    fn next(
        &self,
        ptr: BlockPtr,
        lower: Key,
        upper: Option<Key>,
        address: u64,
    ) -> Result<Option<(Key, Vec<u8>)>, Fault> {
        let load = || -> Result<_, btree::Error> {
            let bytes = self.store.get(ptr.addr)?;
            let h = verify_node(&bytes, ptr, 2, Some(self.uuid), self.generation)?;
            Ok((bytes, h))
        };
        let (bytes, h) = load().map_err(|_| Fault {
            address: ptr.addr,
            resume: upper,
        })?;
        for i in 0..usize::from(h.nitems) {
            if h.level == 0 {
                let at = 48 + i * 21;
                let item = LeafItemHeader::decode(&bytes[at..at + 21]).map_err(|_| Fault {
                    address: ptr.addr,
                    resume: upper,
                })?;
                let k = item.key;
                if k < lower || !matches!(k.item_type, format::REF | format::DEAD) {
                    continue;
                }
                if k.object
                    .checked_add(k.offset)
                    .is_some_and(|end| end <= address)
                {
                    continue;
                }
                let at = usize::from(item.data_off);
                return Ok(Some((
                    k,
                    bytes[at..at + usize::from(item.data_len)].to_vec(),
                )));
            }
            let at = 48 + i * 37;
            let entry = InternalEntry::decode(&bytes[at..at + 37]).map_err(|_| Fault {
                address: ptr.addr,
                resume: upper,
            })?;
            let bound = if i + 1 < usize::from(h.nitems) {
                Some(Key::decode(&bytes[at + 37..at + 54]).map_err(|_| Fault {
                    address: ptr.addr,
                    resume: upper,
                })?)
            } else {
                upper
            };
            if bound.is_some_and(|k| k <= lower) {
                continue;
            }
            let child = BlockPtr {
                addr: entry.child_addr,
                generation: entry.child_generation,
                csum: entry.child_csum,
                level: h.level - 1,
            };
            if let Some(found) = self.next(child, lower, bound, address)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
}
