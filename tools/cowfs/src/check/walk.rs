//! The walk of M's trees (format sections 2.2-2.4) with invariants N1-N6 and the length half
//! of C1.
//!
//! The walk starts at `space_root`, then each ROOT item's FS tree, then each DEAD node: the
//! set L of section 3. Each address is read once; a later pointer to the same address is
//! compared with what the first visit recorded. A node's contents are read only after its
//! magic, CRC, address, uuid and tree check out.

use std::collections::BTreeMap;
use std::io;

use super::image::Image;
use super::node::{self, Expect, Node, HEADER, INTERNAL_ENTRY, LEAF_ITEM, MAX_INTERNAL_ENTRIES};
use super::raw::{all_zero, tree_name, u32_at, u64_at, BlockPtr, Key, NODE, TREE_FS, TREE_SPACE};
use super::recovery::{ROOT_OBJECT, ROOT_TYPE};
use super::report::Report;
use super::slots::Slot;

const MAX_LEVEL: u8 = 7;
const MAX_PAYLOAD: usize = 6144;
pub const ORPHAN_OBJECT: u64 = 0xFFFF_FFFF_FFFF_FF00;
const FREE_OBJECT: u64 = 0xFFFF_FFFF_FFFF_FF20;
const PENDING_OBJECT: u64 = 0xFFFF_FFFF_FFFF_FF30;

pub struct DataRef {
    pub start: u64,
    pub length: u64,
    pub crcs: Vec<u32>,
}

/// A REF item (format 2.4): blocks `start .. start + length`, its kind and refcount.
pub struct RefItem {
    pub start: u64,
    pub length: u64,
    pub kind: u8,
    pub refcount: u32,
}

/// An EXTENT item in a leaf in L, as A3 and A4 read it.
pub struct ExtentItem {
    pub key: Key,
    pub phys_start: u64,
    pub phys_len: u64,
}

/// What the per-root F checks need of a trusted FS node.
pub enum FsNode {
    /// The leaf's items whose payloads lie inside the node.
    Leaf(Vec<(Key, Vec<u8>)>),
    /// The child addresses, in entry order.
    Internal(Vec<u64>),
}

#[derive(Clone)]
pub struct RootItem {
    pub id: u64,
    pub ptr: BlockPtr,
    pub next_inode: u64,
    pub name: Vec<u8>,
}

/// What the first visit to an address learned, for comparing later pointers to it.
struct Seen {
    trusted: bool,
    tree: u8,
    level: u8,
    generation: u64,
    csum: u32,
    first: Option<Key>,
    last: Option<Key>,
}

struct Visit {
    addr: u64,
    tree: u8,
    generation: Option<u64>,
    csum: Option<u32>,
    level: Option<(u8, &'static str)>,
    parent_generation: u64,
    lo: Option<Key>,
    hi: Option<Key>,
    is_root: bool,
    from: String,
}

pub struct Walk<'i> {
    pub image: &'i Image,
    uuid: [u8; 16],
    pub total_blocks: u64,
    /// M's generation.
    pub m: u64,
    pub dirhash_key: [u8; 16],
    seen: BTreeMap<u64, Seen>,
    /// L: each node address reached, once, with the tree the pointer to it names.
    pub nodes: BTreeMap<u64, u8>,
    /// References to each address from `space_root`, ROOT items, internal entries of nodes
    /// in L and EXTENT items in leaves in L, one per pointer or item (A4).
    pub refs: BTreeMap<u64, u64>,
    pub dead: Vec<u64>,
    pub ref_items: Vec<RefItem>,
    /// FREE extents as (start, length).
    pub free: Vec<(u64, u64)>,
    /// PENDING extents as (start, length, released_generation).
    pub pending: Vec<(u64, u64, u64)>,
    pub extents: Vec<ExtentItem>,
    pub fs_nodes: BTreeMap<u64, FsNode>,
    /// The sum of FREE lengths; absent when the sum overflows 64 bits.
    pub free_total: Option<u64>,
    pub roots: Vec<RootItem>,
    pub data_refs: Vec<DataRef>,
}

pub fn run<'i>(image: &'i Image, m: &Slot, report: &mut Report) -> io::Result<Walk<'i>> {
    let mut walk = Walk {
        image,
        uuid: m.uuid,
        total_blocks: m.total_blocks,
        m: m.generation,
        dirhash_key: m.dirhash_key,
        seen: BTreeMap::new(),
        nodes: BTreeMap::new(),
        refs: BTreeMap::new(),
        dead: Vec::new(),
        ref_items: Vec::new(),
        free: Vec::new(),
        pending: Vec::new(),
        extents: Vec::new(),
        fs_nodes: BTreeMap::new(),
        free_total: Some(0),
        roots: Vec::new(),
        data_refs: Vec::new(),
    };
    let ptr = m.space_root;
    walk.reference(ptr.addr);
    walk.visit(report, root_visit(ptr, TREE_SPACE, m.generation, "space_root".into()))?;
    let roots = walk.roots.clone();
    for root in roots {
        walk.reference(root.ptr.addr);
        walk.visit(report, root_visit(root.ptr, TREE_FS, m.generation, format!("ROOT item {}", root.id)))?;
    }
    let dead = walk.dead.clone();
    for addr in dead {
        let visit = Visit {
            addr,
            tree: TREE_FS,
            generation: None,
            csum: None,
            level: None,
            parent_generation: m.generation,
            lo: None,
            hi: None,
            is_root: true,
            from: "DEAD item".into(),
        };
        walk.visit(report, visit)?;
    }
    Ok(walk)
}

fn root_visit(ptr: BlockPtr, tree: u8, m: u64, from: String) -> Visit {
    Visit {
        addr: ptr.addr,
        tree,
        generation: Some(ptr.generation),
        csum: Some(ptr.csum),
        level: Some((ptr.level, "N1")),
        parent_generation: m,
        lo: None,
        hi: None,
        is_root: true,
        from,
    }
}

impl Walk<'_> {
    fn reference(&mut self, addr: u64) {
        *self.refs.entry(addr).or_default() += 1;
    }

    fn visit(&mut self, report: &mut Report, v: Visit) -> io::Result<()> {
        let at = format!("node {} ({} tree, from {})", v.addr, tree_name(v.tree), v.from);
        if !node::in_node_area(v.addr, self.total_blocks) {
            report.error("N1", format!("{at}: address outside the node area, blocks 20..{}", self.total_blocks - 5));
            return Ok(());
        }
        if let Some(seen) = self.seen.get(&v.addr) {
            revisit(seen, &v, &at, report);
            return Ok(());
        }
        self.nodes.insert(v.addr, v.tree);
        let n = node::read(self.image, v.addr)?;
        let uuid = self.uuid;
        let expect = Expect {
            addr: v.addr,
            tree: v.tree,
            generation: v.generation,
            csum: v.csum,
            level: v.level,
            uuid: &uuid,
        };
        let mismatches = node::compare(&n, &expect);
        let trusted = !mismatches.iter().any(|m| m.fatal);
        for m in mismatches {
            report.error(m.id, format!("{at}: {}", m.detail));
        }
        let mut seen = Seen {
            trusted,
            tree: n.tree,
            level: n.level,
            generation: n.generation,
            csum: n.csum,
            first: None,
            last: None,
        };
        if !trusted {
            self.seen.insert(v.addr, seen);
            return Ok(());
        }
        if n.generation > v.parent_generation {
            report.error("N2", format!("{at}: generation {} above its parent's {}", n.generation, v.parent_generation));
        } else if n.generation > self.m {
            report.error("N2", format!("{at}: generation {} above M = {}", n.generation, self.m));
        }
        if n.level > MAX_LEVEL {
            report.error("N3", format!("{at}: level {} makes the tree more than 8 levels deep", n.level));
        }
        if n.nitems == 0 && !(v.is_root && n.level == 0) {
            let what = if n.level == 0 { "leaf" } else { "internal node" };
            report.error("N3", format!("{at}: empty {what} that is not a tree's root leaf"));
        }
        if n.level == 0 {
            let keys = self.leaf(report, &n, &v, &at);
            seen.first = keys.first().copied();
            seen.last = keys.last().copied();
            self.seen.insert(v.addr, seen);
        } else {
            let entries = node::entries(&n);
            seen.first = entries.first().map(|e| e.key);
            seen.last = entries.last().map(|e| e.key);
            self.seen.insert(v.addr, seen);
            if n.tree == TREE_FS {
                self.fs_nodes.insert(v.addr, FsNode::Internal(entries.iter().map(|e| e.child).collect()));
            }
            self.internal(report, &n, &entries, &v, &at)?;
        }
        Ok(())
    }

    /// N4-N6 for a leaf, and the items the later checks need. Returns the leaf's keys.
    fn leaf(&mut self, report: &mut Report, n: &Node, v: &Visit, at: &str) -> Vec<Key> {
        let items = node::leaf_items(n);
        if n.nitems > items.len() {
            report.error("N5", format!("{at}: {} item headers overrun the node", n.nitems));
        }
        let keys: Vec<Key> = items.iter().map(|i| i.key).collect();
        check_keys(report, at, &keys, v.lo, v.hi);
        let headers_end = HEADER + LEAF_ITEM * items.len();
        let mut used = vec![(0, headers_end)];
        let mut in_bounds = vec![false; items.len()];
        for (i, item) in items.iter().enumerate() {
            let end = item.data_off + item.data_len;
            if item.data_off < headers_end || end > NODE {
                report.error(
                    "N5",
                    format!("{at}: item {i} {} payload {}+{} outside the payload area", item.key, item.data_off, item.data_len),
                );
                continue;
            }
            if item.data_len > MAX_PAYLOAD {
                report.error("N5", format!("{at}: item {i} {} payload of {} bytes exceeds 6144", item.key, item.data_len));
            }
            in_bounds[i] = true;
            used.push((item.data_off, end));
        }
        let used_bytes = check_layout(report, at, &n.raw, used);
        if !v.is_root && used_bytes < NODE / 4 {
            report.warning(format!("{at}: leaf under a quarter full ({used_bytes} of {NODE} bytes)"));
        }
        let mut fs_items = Vec::new();
        for (i, item) in items.iter().enumerate() {
            if let Some(payload) = item.payload(n).filter(|_| in_bounds[i]) {
                self.item(report, at, n.tree, item.key, payload);
                if n.tree == TREE_FS {
                    fs_items.push((item.key, payload.to_vec()));
                }
            }
        }
        if n.tree == TREE_FS {
            self.fs_nodes.insert(v.addr, FsNode::Leaf(fs_items));
        }
        keys
    }

    fn internal(&mut self, report: &mut Report, n: &Node, entries: &[node::Entry], v: &Visit, at: &str) -> io::Result<()> {
        if n.nitems > MAX_INTERNAL_ENTRIES {
            report.error("N5", format!("{at}: {} entries exceed 441", n.nitems));
        }
        let keys: Vec<Key> = entries.iter().map(|e| e.key).collect();
        check_keys(report, at, &keys, v.lo, v.hi);
        let used_bytes = check_layout(report, at, &n.raw, vec![(0, HEADER + INTERNAL_ENTRY * entries.len())]);
        if !v.is_root && used_bytes < NODE / 4 {
            report.warning(format!("{at}: internal node under a quarter full ({used_bytes} of {NODE} bytes)"));
        }
        for (i, entry) in entries.iter().enumerate() {
            self.reference(entry.child);
            let child = Visit {
                addr: entry.child,
                tree: v.tree,
                generation: Some(entry.generation),
                csum: Some(entry.csum),
                level: Some((n.level - 1, "N3")),
                parent_generation: n.generation,
                lo: Some(entry.key),
                hi: entries.get(i + 1).map(|e| e.key).or(v.hi),
                is_root: false,
                from: format!("entry {i} of node {}", v.addr),
            };
            self.visit(report, child)?;
        }
        Ok(())
    }

    /// N6 (item types, key shapes and fixed payload sizes) and C1's CRC count; collects the
    /// FREE, ROOT, DEAD and data REF items.
    fn item(&mut self, report: &mut Report, at: &str, tree: u8, key: Key, p: &[u8]) {
        let mut bad: Vec<String> = Vec::new();
        let size = |want: usize, bad: &mut Vec<String>| {
            if p.len() != want {
                bad.push(format!("payload of {} bytes, expected {want}", p.len()));
                false
            } else {
                true
            }
        };
        match (tree, key.ty) {
            (TREE_SPACE, 0x80) if p.len() < 16 => {
                bad.push(format!("REF payload of {} bytes, expected at least 16", p.len()));
            }
            (TREE_SPACE, 0x80) => {
                if !all_zero(&p[5..8]) {
                    bad.push("REF padding not zero".into());
                }
                self.ref_items.push(RefItem { start: key.object, length: key.offset, kind: p[4], refcount: u32_at(p, 0) });
                match p[4] {
                    1 | 2 => {
                        size(16, &mut bad);
                    }
                    3 => {
                        let want = key.offset.checked_mul(4).and_then(|b| b.checked_add(16));
                        if want == Some(p.len() as u64) {
                            let crcs = p[16..].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
                            self.data_refs.push(DataRef { start: key.object, length: key.offset, crcs });
                        } else {
                            report.error(
                                "C1",
                                format!("{at}: data REF {key} holds {} bytes of CRCs for {} blocks", p.len() - 16, key.offset),
                            );
                        }
                    }
                    kind => bad.push(format!("REF kind {kind}")),
                }
            }
            (TREE_SPACE, 0x81) => {
                if key.object != FREE_OBJECT {
                    bad.push("FREE item outside the FREE object".into());
                } else if size(8, &mut bad) {
                    self.free_total = self.free_total.and_then(|t| t.checked_add(u64_at(p, 0)));
                    self.free.push((key.offset, u64_at(p, 0)));
                }
            }
            (TREE_SPACE, 0x82) => {
                if key.object != PENDING_OBJECT {
                    bad.push("PENDING item outside the PENDING object".into());
                } else if size(16, &mut bad) {
                    self.pending.push((key.offset, u64_at(p, 0), u64_at(p, 8)));
                }
            }
            (TREE_SPACE, 0x83) => {
                if key.offset != 4 {
                    bad.push("DEAD offset is not 4".into());
                } else if size(0, &mut bad) {
                    self.dead.push(key.object);
                }
            }
            (TREE_SPACE, ROOT_TYPE) => {
                if key.object != ROOT_OBJECT {
                    bad.push("ROOT item outside the ROOT object".into());
                } else if p.len() < 66 || p.len() != 65 + p[64] as usize {
                    bad.push(format!("ROOT payload of {} bytes does not match its name length", p.len()));
                } else {
                    let ptr = BlockPtr::parse(p, 0);
                    if !ptr.pad_zero {
                        bad.push("ROOT pointer padding not zero".into());
                    }
                    let (next_inode, name) = (u64_at(p, 24), p[65..].to_vec());
                    self.roots.push(RootItem { id: key.offset, ptr, next_inode, name });
                }
            }
            (TREE_FS, 0x01) => {
                if key.offset != 0 {
                    bad.push("INODE offset is not 0".into());
                }
                if size(128, &mut bad) && !all_zero(&p[112..128]) {
                    bad.push("INODE bytes 112..127 not zero".into());
                }
            }
            (TREE_FS, 0x20) => {
                if size(24, &mut bad) {
                    let (phys_start, phys_len) = (u64_at(p, 0), u32_at(p, 8) as u64);
                    self.extents.push(ExtentItem { key, phys_start, phys_len });
                    self.reference(phys_start);
                }
            }
            (TREE_FS, 0x21) => {
                if key.offset != 0 {
                    bad.push("INLINE offset is not 0".into());
                }
            }
            (TREE_FS, 0x40) => {
                if key.object != ORPHAN_OBJECT {
                    bad.push("ORPHAN item outside the orphan object".into());
                }
                size(0, &mut bad);
            }
            (TREE_FS, 0x10 | 0x11 | 0x30) => {}
            (_, ty) => bad.push(format!("type {ty:#04x} does not belong in a {} tree", tree_name(tree))),
        }
        for what in bad {
            report.error("N6", format!("{at}: item {key}: {what}"));
        }
    }
}

/// Compares a later pointer to an address with what its first visit recorded.
fn revisit(seen: &Seen, v: &Visit, at: &str, report: &mut Report) {
    if !seen.trusted {
        return;
    }
    if seen.tree != v.tree {
        report.error("N1", format!("{at}: already reached as a {} tree node", tree_name(seen.tree)));
    }
    if v.generation.is_some_and(|g| g != seen.generation) {
        report.error("N1", format!("{at}: node generation {}, pointer says {}", seen.generation, v.generation.unwrap_or(0)));
    }
    if v.csum.is_some_and(|c| c != seen.csum) {
        report.error("N1", format!("{at}: node CRC {:#010x}, pointer says {:#010x}", seen.csum, v.csum.unwrap_or(0)));
    }
    if let Some((level, id)) = v.level.filter(|(l, _)| *l != seen.level) {
        report.error(id, format!("{at}: node level {}, expected {level}", seen.level));
    }
    if seen.generation > v.parent_generation {
        report.error("N2", format!("{at}: generation {} above its parent's {}", seen.generation, v.parent_generation));
    }
    let below = seen.first.zip(v.lo).is_some_and(|(k, lo)| k < lo);
    let above = seen.last.zip(v.hi).is_some_and(|(k, hi)| k >= hi);
    if below || above {
        report.error("N4", format!("{at}: keys outside this parent's bounds"));
    }
}

/// N4: keys strictly increase and lie in [lo, hi). Reports the first violation of each kind.
fn check_keys(report: &mut Report, at: &str, keys: &[Key], lo: Option<Key>, hi: Option<Key>) {
    if let Some(i) = (1..keys.len()).find(|&i| keys[i] <= keys[i - 1]) {
        report.error("N4", format!("{at}: key {i} {} does not follow {}", keys[i], keys[i - 1]));
    }
    let outside = |k: &Key| lo.is_some_and(|lo| *k < lo) || hi.is_some_and(|hi| *k >= hi);
    if let Some((i, k)) = keys.iter().enumerate().find(|(_, k)| outside(k)) {
        let show = |b: Option<Key>| b.map_or("-".to_string(), |k| k.to_string());
        report.error("N4", format!("{at}: key {i} {k} outside the parent's bounds [{}, {})", show(lo), show(hi)));
    }
}

/// N5: the used ranges (header, item headers or entries, payloads) do not overlap and every
/// other byte is zero. Returns the number of bytes in use.
/// claim-lint:ok: docs/fs/cowfs-format.md section 2.2 (unused bytes are zero) and N5.
fn check_layout(report: &mut Report, at: &str, raw: &[u8], mut used: Vec<(usize, usize)>) -> usize {
    used.sort_unstable();
    let mut cursor = 0;
    let mut total = 0;
    let mut overlap_reported = false;
    let mut nonzero_reported = false;
    for &(start, end) in &used {
        if start < cursor && !overlap_reported {
            report.error("N5", format!("{at}: payloads overlap at byte {start}"));
            overlap_reported = true;
        }
        if start > cursor && !nonzero_reported {
            if let Some(i) = (cursor..start).find(|&i| raw[i] != 0) {
                report.error("N5", format!("{at}: unused byte {i} not zero"));
                nonzero_reported = true;
            }
        }
        total += end - start;
        cursor = cursor.max(end);
    }
    if cursor < NODE && !nonzero_reported {
        if let Some(i) = (cursor..NODE).find(|&i| raw[i] != 0) {
            report.error("N5", format!("{at}: unused byte {i} not zero"));
        }
    }
    total
}
