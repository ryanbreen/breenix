//! E3-1: the radix tree (plan section 4.3).
use std::alloc::{alloc, dealloc, Layout};
use std::collections::BTreeMap;
use std::ptr::NonNull;

use pagecache_host_tests::page_tree::{
    height_for, worst_case_nodes, Entry, Node, NodeList, NodeSource, PageTree, Slot, Tag, TreeError,
};
use pagecache_host_tests::Lcg;

/// Nodes from the system allocator, refusing the allocation numbered
/// `fail_at` (0-based, counted across the source's life).
struct Source {
    calls: usize,
    live: usize,
    fail_at: Option<usize>,
}

impl Source {
    fn new() -> Source {
        Source {
            calls: 0,
            live: 0,
            fail_at: None,
        }
    }
}

unsafe impl NodeSource for Source {
    fn alloc_node(&mut self) -> Option<NonNull<Node>> {
        let call = self.calls;
        self.calls += 1;
        if self.fail_at == Some(call) {
            return None;
        }
        self.live += 1;
        NonNull::new(unsafe { alloc(Layout::new::<Node>()) }.cast())
    }

    unsafe fn free_node(&mut self, node: NonNull<Node>) {
        self.live -= 1;
        unsafe { dealloc(node.as_ptr().cast(), Layout::new::<Node>()) };
    }
}

fn page(pfn: u64) -> Entry {
    Entry::page(pfn).unwrap()
}

/// Store with an exact reservation, checking the tree took exactly what it
/// said it needed.
fn store(tree: &mut PageTree, source: &mut Source, index: u64, entry: Entry) -> Slot {
    let needed = tree.nodes_needed(index);
    let before = tree.nodes();
    let mut reserved = NodeList::new();
    reserved.reserve(source, needed).unwrap();
    let old = tree.store(index, entry, &mut reserved).unwrap();
    assert_eq!(reserved.len(), 0, "store at {index:#x} left reserved nodes");
    assert_eq!(tree.nodes(), before + needed, "store at {index:#x}");
    old
}

fn remove(tree: &mut PageTree, source: &mut Source, index: u64) -> Slot {
    let mut freed = NodeList::new();
    let before = tree.nodes();
    let old = tree.remove(index, &mut freed);
    assert_eq!(tree.nodes() + freed.len(), before, "remove at {index:#x} lost nodes");
    freed.release(source);
    old
}

fn clear(tree: &mut PageTree, source: &mut Source) {
    let mut freed = NodeList::new();
    tree.clear(&mut freed);
    freed.release(source);
}

/// The entries, with their tags, in index order, plus the shape.
fn snapshot(tree: &PageTree) -> (u32, usize, Vec<(u64, Slot, bool, bool)>) {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((index, slot)) = tree.next_present(from) {
        out.push((index, slot, tree.tagged(index, Tag::Dirty), tree.tagged(index, Tag::Writeback)));
        match index.checked_add(1) {
            Some(next) => from = next,
            None => break,
        }
    }
    (tree.height(), tree.nodes(), out)
}

fn tagged_walk(tree: &PageTree, tag: Tag) -> Vec<u64> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((index, _)) = tree.next_tagged(from, tag) {
        out.push(index);
        match index.checked_add(1) {
            Some(next) => from = next,
            None => break,
        }
    }
    out
}

const EDGES: [u64; 5] = [0, 63, 64, 1 << 32, (1 << 52) - 1];

#[test]
fn insert_lookup_remove_across_height_changes() {
    let mut source = Source::new();
    let mut tree = PageTree::new();
    for (n, &index) in EDGES.iter().enumerate() {
        assert_eq!(store(&mut tree, &mut source, index, page(100 + n as u64)), Slot::Absent);
        assert_eq!(tree.height(), height_for(index), "height after storing {index:#x}");
        for (m, &seen) in EDGES.iter().enumerate().take(n + 1) {
            assert_eq!(tree.lookup(seen), Slot::Page { pfn: 100 + m as u64 });
        }
        for &neighbour in &[1, 62, 65, (1 << 32) + 1, (1 << 52) - 2, 1 << 52, u64::MAX] {
            assert_eq!(tree.lookup(neighbour), Slot::Absent, "neighbour {neighbour:#x}");
        }
    }
    assert_eq!(store(&mut tree, &mut source, 64, Entry::hole()), Slot::Page { pfn: 102 });
    assert_eq!(tree.lookup(64), Slot::Hole);

    // Removing the largest index each time shrinks the height to the next.
    for n in (0..EDGES.len()).rev() {
        let expected = if n == 2 { Slot::Hole } else { Slot::Page { pfn: 100 + n as u64 } };
        assert_eq!(remove(&mut tree, &mut source, EDGES[n]), expected);
        assert_eq!(remove(&mut tree, &mut source, EDGES[n]), Slot::Absent);
        let height = if n == 0 { 0 } else { height_for(EDGES[n - 1]) };
        assert_eq!(tree.height(), height, "height after removing {:#x}", EDGES[n]);
        for &kept in &EDGES[..n] {
            assert_ne!(tree.lookup(kept), Slot::Absent);
        }
    }
    assert!(tree.is_empty());
    assert_eq!(tree.nodes(), 0);
    assert_eq!(source.live, 0, "nodes leaked");

    // Removing the smallest first keeps the height until the largest goes.
    for &index in &EDGES {
        store(&mut tree, &mut source, index, page(index & 0xffff));
    }
    for &index in &EDGES[..4] {
        remove(&mut tree, &mut source, index);
        assert_eq!(tree.height(), height_for(EDGES[4]));
        assert_eq!(tree.lookup(EDGES[4]), Slot::Page { pfn: EDGES[4] & 0xffff });
    }
    assert_eq!(tree.nodes(), height_for(EDGES[4]) as usize);
    remove(&mut tree, &mut source, EDGES[4]);
    assert_eq!((tree.height(), tree.nodes(), source.live), (0, 0, 0));
}

#[test]
fn tags_propagate_and_tagged_iteration_visits_exactly_the_tagged_pages_seed_e3c0ffee() {
    let mut source = Source::new();
    let mut tree = PageTree::new();
    let mut rng = Lcg(0xe3c0_ffee);
    let mut present: BTreeMap<u64, Slot> = BTreeMap::new();
    let mut dirty: BTreeMap<u64, ()> = BTreeMap::new();
    let mut writeback: BTreeMap<u64, ()> = BTreeMap::new();
    // Indices clustered in a few subtrees, so set and clear meet shared
    // ancestors, plus a few far ones that change the height.
    let pick = |rng: &mut Lcg| -> u64 {
        match rng.below(8) {
            0 => rng.below(64),
            1..=4 => 4096 + rng.below(300),
            5 | 6 => (1 << 32) + rng.below(200),
            _ => (1 << 52) - 1 - rng.below(3),
        }
    };
    for step in 0..4000 {
        let index = pick(&mut rng);
        let tag = if rng.below(2) == 0 { Tag::Dirty } else { Tag::Writeback };
        let model = if tag == Tag::Dirty { &mut dirty } else { &mut writeback };
        match rng.below(6) {
            0 => {
                let entry = if rng.below(5) == 0 { Entry::hole() } else { page(step) };
                store(&mut tree, &mut source, index, entry);
                present.insert(index, entry.slot());
                dirty.remove(&index);
                writeback.remove(&index);
            }
            1 => {
                remove(&mut tree, &mut source, index);
                present.remove(&index);
                dirty.remove(&index);
                writeback.remove(&index);
            }
            2 | 3 => {
                let result = tree.set_tag(index, tag);
                match present.get(&index) {
                    Some(Slot::Page { .. }) => {
                        assert_eq!(result, Ok(()));
                        model.insert(index, ());
                    }
                    _ => assert_eq!(result, Err(TreeError::NotAPage), "step {step}"),
                }
            }
            _ => {
                tree.clear_tag(index, tag);
                model.remove(&index);
            }
        }
        if step % 16 == 0 || step > 3900 {
            assert_eq!(tagged_walk(&tree, Tag::Dirty), dirty.keys().copied().collect::<Vec<_>>(), "step {step}");
            assert_eq!(
                tagged_walk(&tree, Tag::Writeback),
                writeback.keys().copied().collect::<Vec<_>>(),
                "step {step}"
            );
            // Sets and clears reach the root.
            assert_eq!(tree.any_tagged(Tag::Dirty), !dirty.is_empty(), "step {step}");
            assert_eq!(tree.any_tagged(Tag::Writeback), !writeback.is_empty(), "step {step}");
            let (_, _, entries) = snapshot(&tree);
            let expected: Vec<_> = present
                .iter()
                .map(|(&i, &s)| (i, s, dirty.contains_key(&i), writeback.contains_key(&i)))
                .collect();
            assert_eq!(entries, expected, "step {step}");
            // A start inside a run finds the next tagged page, not an earlier one.
            let probe = pick(&mut rng);
            assert_eq!(
                tree.next_tagged(probe, Tag::Dirty).map(|(i, _)| i),
                dirty.range(probe..).next().map(|(&i, _)| i),
                "step {step} probe {probe:#x}"
            );
        }
    }
    // Clearing the remaining tags clears them to the root: a search from 0 finds no page.
    for &index in present.keys() {
        tree.clear_tag(index, Tag::Dirty);
        tree.clear_tag(index, Tag::Writeback);
    }
    assert!(!tree.any_tagged(Tag::Dirty) && !tree.any_tagged(Tag::Writeback));
    assert_eq!(tree.next_tagged(0, Tag::Dirty), None);
    assert_eq!(tree.next_tagged(0, Tag::Writeback), None);
    clear(&mut tree, &mut source);
    assert_eq!(source.live, 0);
}

#[test]
fn a_failed_reservation_or_a_short_one_changes_nothing() {
    let mut source = Source::new();
    let mut tree = PageTree::new();
    for &index in &[0u64, 63, 64, 4096] {
        store(&mut tree, &mut source, index, page(index + 1));
    }
    tree.set_tag(64, Tag::Dirty).unwrap();
    let before = snapshot(&tree);
    for &index in &[1u64 << 32, (1 << 52) - 1, 200_000] {
        let needed = tree.nodes_needed(index);
        assert!(needed >= 2, "index {index:#x} needs new nodes");
        assert!(needed <= worst_case_nodes(height_for(index), 1));

        // A reservation one short is refused before anything changes.
        let mut short = NodeList::new();
        short.reserve(&mut source, needed - 1).unwrap();
        assert_eq!(
            tree.store(index, page(7), &mut short),
            Err(TreeError::NeedNodes {
                needed,
                reserved: needed - 1
            })
        );
        assert_eq!(short.len(), needed - 1);
        short.release(&mut source);
        assert_eq!(snapshot(&tree), before);

        // Failing the Nth node of the reservation, for each N, returns the
        // nodes already taken and leaves the reservation empty.
        for nth in 0..needed {
            let live = source.live;
            source.fail_at = Some(source.calls + nth);
            let mut reserved = NodeList::new();
            assert!(reserved.reserve(&mut source, needed).is_err(), "nth {nth}");
            assert_eq!(reserved.len(), 0);
            assert_eq!(source.live, live, "nth {nth} kept nodes");
            source.fail_at = None;
            assert_eq!(snapshot(&tree), before);
        }

        // An existing reservation is not lost when a later top-up fails.
        let mut reserved = NodeList::new();
        reserved.reserve(&mut source, 1).unwrap();
        source.fail_at = Some(source.calls);
        assert!(reserved.reserve(&mut source, needed - 1).is_err());
        assert_eq!(reserved.len(), 1);
        source.fail_at = None;
        reserved.release(&mut source);
    }
    // With the full reservation the store succeeds and the old entries stay.
    store(&mut tree, &mut source, (1 << 52) - 1, page(9));
    assert!(tree.tagged(64, Tag::Dirty));
    for &index in &[0u64, 63, 64, 4096] {
        assert_eq!(tree.lookup(index), Slot::Page { pfn: index + 1 });
    }
    clear(&mut tree, &mut source);
    assert_eq!(source.live, 0);
}
