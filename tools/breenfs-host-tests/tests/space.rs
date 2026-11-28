use breenfs_host_tests::{
    btree::{self, NodeStore},
    format::{self, BlockPtr, InternalEntry, Key, NodeHeader, Pending, Ref},
    space::{self, Error},
    txn::{self, Transaction},
};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
};

const FREE: u64 = 0xffff_ffff_ffff_ff20;
const PENDING: u64 = 0xffff_ffff_ffff_ff30;

#[derive(Default)]
struct Memory {
    nodes: BTreeMap<u64, Vec<u8>>,
    next: u64,
    reads: Cell<usize>,
}
impl NodeStore for Memory {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.reads.set(self.reads.get() + 1);
        self.nodes.get(&addr).cloned().ok_or(btree::Error::Store)
    }
    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), btree::Error> {
        self.nodes.insert(addr, bytes);
        Ok(())
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        self.next = self.next.max(1_000_000) + 4;
        Ok(self.next)
    }
}
fn tree() -> (Memory, BlockPtr) {
    let mut store = Memory::default();
    let root = btree::create(&mut store, 2, [7; 16], 1).unwrap();
    (store, root)
}
fn item(store: &impl NodeStore, root: BlockPtr, object: u64, ty: u8, offset: u64) -> Option<Vec<u8>> {
    btree::search(store, root, Key { object, item_type: ty, offset }).unwrap()
}
fn put(store: &mut Memory, root: &mut BlockPtr, object: u64, ty: u8, offset: u64, value: &[u8]) {
    btree::insert(store, root, Key { object, item_type: ty, offset }, value).unwrap();
}
fn node_ref(count: u32) -> Vec<u8> {
    Ref { refcount: count, kind: 1, alloc_generation: 1, crcs: vec![] }.encode()
}
fn node_addrs(store: &Memory, ptr: BlockPtr, out: &mut BTreeSet<u64>) {
    out.insert(ptr.addr);
    let bytes = store.get(ptr.addr).unwrap();
    let h = NodeHeader::decode(&bytes).unwrap();
    for i in 0..usize::from(h.nitems) {
        if h.level == 0 {
            break;
        }
        let e = InternalEntry::decode(&bytes[48 + i * 37..48 + (i + 1) * 37]).unwrap();
        let child = BlockPtr { addr: e.child_addr, generation: e.child_generation, csum: e.child_csum, level: h.level - 1 };
        node_addrs(store, child, out);
    }
}
/// A SPACE tree for transactions: `items` plus room for node copies at 5000,
/// and a REF for each SPACE node (invariant A3) so a transaction can copy it.
fn txn_tree(items: &[(u64, u8, u64, Vec<u8>)]) -> (Memory, BlockPtr) {
    let (mut store, mut root) = tree();
    put(&mut store, &mut root, FREE, format::FREE, 5_000, &format::Free { length: 100 }.encode());
    for (object, ty, offset, value) in items {
        put(&mut store, &mut root, *object, *ty, *offset, value);
    }
    seal(&mut store, &mut root);
    (store, root)
}
fn seal(store: &mut Memory, root: &mut BlockPtr) {
    loop {
        let mut nodes = BTreeSet::new();
        node_addrs(store, *root, &mut nodes);
        let missing: Vec<_> = nodes.into_iter()
            .filter(|&a| space::reference(store, *root, a, 4).unwrap().is_none())
            .collect();
        if missing.is_empty() {
            return;
        }
        for addr in missing {
            let r = Ref { refcount: 1, kind: 2, alloc_generation: 1, crcs: vec![] }.encode();
            put(store, root, addr, format::REF, 4, &r);
        }
    }
}

#[test]
fn promote_between_free_neighbors_without_scanning_the_free_tree() {
    let (mut store, mut root) = tree();
    // Multiple FREE leaves, with a gap that closes between two existing runs.
    // Seed the ranges directly so setup is independent of the coalescer.
    for i in 0..1500 {
        put(&mut store, &mut root, FREE, format::FREE, 20_000 + 12 * i, &format::Free { length: 4 }.encode());
    }
    assert_eq!(root.level, 1);
    let start = 20_000 + 12 * 750;
    store.reads.set(0);
    space::add_free(&mut store, &mut root, start + 4, 8).unwrap();
    let coalesce = store.reads.replace(0);
    space::claim(&mut store, &mut root, 20_000, 4).unwrap();
    let claim = store.reads.replace(0);
    assert!(item(&store, root, FREE, format::FREE, 20_000).is_none());
    assert_eq!(item(&store, root, FREE, format::FREE, start).unwrap(), format::Free { length: 16 }.encode());
    assert!(item(&store, root, FREE, format::FREE, start + 12).is_none());
    // Three B-tree edits and two range endpoints, or one edit and the FREE
    // record containing the claim. A height-one edit loads at most ten nodes.
    println!("FREE_READS coalesce={coalesce} claim={claim}");
    assert!(coalesce <= 38 && claim <= 34, "FREE reads: {coalesce}/{claim}");
}

#[test]
fn free_index_mirrors_items_and_prefers_goal_then_best_fit() {
    let (mut store, mut root) = tree();
    for (start, length) in [(100, 4), (200, 64), (300, 8), (400, 1000)] {
        space::add_free(&mut store, &mut root, start, length).unwrap();
    }
    let (mut free, _) = space::open(&store, root).unwrap();
    assert_eq!(free.ranges().collect::<Vec<_>>(), vec![(100, 4), (200, 64), (300, 8), (400, 1000)]);
    // Best fit by length, lowest address first.
    assert_eq!(free.choose(4, None, false), Some((100, 4)));
    assert_eq!(free.choose(5, None, false), Some((300, 5)));
    // A goal inside a range with room starts there; otherwise the next range
    // after it that fits, else the nearest before it.
    assert_eq!(free.choose(8, Some(210), false), Some((210, 8)));
    assert_eq!(free.choose(8, Some(260), false), Some((300, 8)));
    assert_eq!(free.choose(16, Some(1500), false), Some((1384, 16)));
    // A short data run falls back to the largest range.
    assert_eq!(free.choose(2000, None, true), Some((400, 1000)));
    assert_eq!(free.choose(2000, None, false), None);
    // Removal splits as claim does; insertion coalesces as add_free does.
    free.remove(210, 8).unwrap();
    space::claim(&mut store, &mut root, 210, 8).unwrap();
    free.insert(104, 96).unwrap();
    space::add_free(&mut store, &mut root, 104, 96).unwrap();
    assert_eq!(free.remove(205, 10), Err(Error::Invalid));
    assert_eq!(free.insert(150, 4), Err(Error::Invalid));
    let (stored, _) = space::open(&store, root).unwrap();
    assert_eq!(free.ranges().collect::<Vec<_>>(), stored.ranges().collect::<Vec<_>>());
    assert_eq!(free.ranges().collect::<Vec<_>>(), vec![(100, 110), (218, 46), (300, 8), (400, 1000)]);
}

#[test]
fn oversized_free_record_is_rejected_before_allocation() {
    let (mut store, mut root) = tree();
    let mut payload = format::Free { length: 4 }.encode().to_vec();
    payload.push(0);
    put(&mut store, &mut root, FREE, format::FREE, 100, &payload);
    let before = store.nodes.clone();
    assert!(matches!(space::open(&store, root), Err(Error::Invalid)));
    assert_eq!(space::claim(&mut store, &mut root, 100, 4), Err(Error::Invalid));
    assert_eq!(store.nodes, before);
}

#[test]
fn pending_waits_two_generations() {
    let (mut store, mut root) = tree();
    let pending = Pending { length: 3, released_generation: 7 }.encode();
    put(&mut store, &mut root, PENDING, format::PENDING, 100, &pending);
    for generation in [1, 7, 8] {
        let (mut free, mut quarantine) = space::open(&store, root).unwrap();
        let promotion = quarantine.promote(&store, root, generation, &mut free).unwrap();
        assert!(promotion.is_empty(), "generation {generation}");
        assert_eq!(free.ranges().count(), 0);
        assert_eq!(quarantine.summary().census(), Some((3, 7)));
    }
    let (mut free, mut quarantine) = space::open(&store, root).unwrap();
    let promotion = quarantine.promote(&store, root, 9, &mut free).unwrap();
    assert_eq!(free.ranges().collect::<Vec<_>>(), vec![(100, 3)]);
    promotion.apply(&mut store, &mut root).unwrap();
    assert!(item(&store, root, PENDING, format::PENDING, 100).is_none());
    assert_eq!(item(&store, root, FREE, format::FREE, 100).unwrap(), format::Free { length: 3 }.encode());
}

/// Design 2.5: a transaction allocates FREE before it converts eligible
/// PENDING, rejects shapes no REF may have, and the count changes it makes to
/// one allocation settle as one REF edit.
#[test]
fn free_before_pending_and_one_ref_edit_per_allocation() {
    let (store, root) = txn_tree(&[
        (PENDING, format::PENDING, 1_000, Pending { length: 256, released_generation: 3 }.encode().to_vec()),
        (FREE, format::FREE, 200, format::Free { length: 4 }.encode().to_vec()),
    ]);
    let mut t = Transaction::new(&store, root, 5).unwrap();
    assert_eq!(t.allocate_data(&[0; 257]), Err(txn::Error::Space(Error::Invalid)));
    let mut t = Transaction::new(&store, root, 5).unwrap();
    // FREE first, although G-2's PENDING at 1000 is eligible.
    assert_eq!(t.allocate_data(&[1, 2, 3, 4]), Ok(200));
    // An increment and a release of the new extent cancel.
    t.increment(200, 4).unwrap();
    t.release(200, 4, false).unwrap();
    // No FREE range holds 256 blocks: the eligible PENDING becomes FREE.
    assert_eq!(t.allocate_data(&[6; 256]), Ok(1_000));
    let space = t.settle().unwrap();
    let view = t.view();
    assert!(item(&view, space, PENDING, format::PENDING, 1_000).is_none());
    assert!(item(&view, space, FREE, format::FREE, 1_000).is_none());
    assert!(item(&view, space, FREE, format::FREE, 200).is_none());
    let r = space::reference(&view, space, 200, 4).unwrap().unwrap();
    assert_eq!((r.refcount, r.kind, r.alloc_generation, r.crcs), (1, 3, 5, vec![1, 2, 3, 4]));
    assert_eq!(space::reference(&view, space, 1_000, 256).unwrap().unwrap().refcount, 1);
}

/// Delayed references (audit R13): count changes that cancel within one
/// transaction leave a stored REF unedited, so settlement copies no SPACE
/// node and keeps the root. Replaying each change in order edits the REF to
/// 2 and back to 1, which copies the leaf and ends at the same count.
#[test]
fn cancelled_ref_changes_leave_space_unedited() {
    let (store, root) = txn_tree(&[(100, format::REF, 4, node_ref(1)), (104, format::REF, 4, node_ref(2))]);
    let mut t = Transaction::new(&store, root, 3).unwrap();
    t.increment(100, 4).unwrap();
    t.increment(100, 4).unwrap();
    t.release(100, 4, false).unwrap();
    t.release(100, 4, false).unwrap();
    t.release(104, 4, false).unwrap();
    t.increment(104, 4).unwrap();
    assert_eq!(t.settle().unwrap(), root, "cancelled changes edited SPACE");
    assert_eq!(t.writes().count(), 0, "cancelled changes copied a SPACE node");
    assert_eq!(space::reference(&t.view(), root, 100, 4).unwrap().unwrap().refcount, 1);
    assert_eq!(space::reference(&t.view(), root, 104, 4).unwrap().unwrap().refcount, 2);
}

/// A sparse write aims at the block its preceding extent would map it to,
/// which can fall inside a FREE range. Taking one block from the middle of a
/// seven-block range leaves two three-block holes, too small for a node, so
/// such writes can use up every node-sized range while most of the space is
/// still free, and settlement then has nowhere to copy a SPACE node.
#[test]
fn goal_inside_a_free_range_leaves_room_for_nodes() {
    let (mut store, mut root) = tree();
    let runs: Vec<u64> = (0..8).map(|i| 1_000 + 16 * i).collect();
    for &start in &runs {
        put(&mut store, &mut root, FREE, format::FREE, start, &format::Free { length: 7 }.encode());
    }
    seal(&mut store, &mut root);
    let mut t = Transaction::new(&store, root, 5).unwrap();
    for &start in &runs {
        t.allocate_data_run(&[9], Some(start + 3)).unwrap();
    }
    let state = (|| { t.settle().ok()?; t.space_state().ok() })();
    let state = state.expect("settlement found no four-block range for a SPACE node copy");
    let free: u64 = state.free().ranges().map(|(_, l)| l).sum();
    assert!(free >= 40, "{free} blocks left free");
}

#[test]
fn refcount_overflow_is_emlink() {
    let r = Ref { refcount: u32::MAX - 1, kind: 1, alloc_generation: 1, crcs: vec![] };
    let (store, root) = txn_tree(&[(100, format::REF, 4, r.encode())]);
    let mut t = Transaction::new(&store, root, 2).unwrap();
    t.increment(100, 4).unwrap();
    assert_eq!(t.increment(100, 4), Err(txn::Error::Space(Error::EMLINK)));
    let mut t = Transaction::new(&store, root, 2).unwrap();
    t.increment(100, 4).unwrap();
    let space = t.settle().unwrap();
    assert_eq!(space::reference(&t.view(), space, 100, 4).unwrap().unwrap().refcount, u32::MAX);
}

#[test]
fn zero_ref_node_with_children_is_dead() {
    let (store, root) = txn_tree(&[(100, format::REF, 4, node_ref(1)), (104, format::REF, 4, node_ref(1))]);
    let mut t = Transaction::new(&store, root, 3).unwrap();
    t.increment(100, 4).unwrap();
    t.release(100, 4, true).unwrap();
    t.release(100, 4, true).unwrap();
    let space = t.settle().unwrap();
    let view = t.view();
    assert!(space::reference(&view, space, 100, 4).unwrap().is_none());
    assert_eq!(item(&view, space, 100, format::DEAD, 4), Some(vec![]));
    assert!(item(&view, space, PENDING, format::PENDING, 100).is_none());
    assert_eq!(space::reference(&view, space, 104, 4).unwrap().unwrap().refcount, 1);
    // Reaping owns the child decrements; making DEAD must leave the child
    // alone, and neither node's blocks become allocatable.
    assert_eq!(t.dead_count(), 1);
    let state = t.space_state().unwrap();
    assert!(state.free().ranges().all(|(s, l)| s + l <= 100 || s >= 108));
    assert!(state.quarantine().runs().all(|(s, l, _)| s + l <= 100 || s >= 108));
}
