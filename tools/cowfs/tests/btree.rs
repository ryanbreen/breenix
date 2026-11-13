use cowfs::btree::{self, Error, NodeStore};
use cowfs::format::{BlockPtr, InternalEntry, Key, LeafItemHeader, NodeHeader, NODE_SIZE};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Memory {
    nodes: BTreeMap<u64, Vec<u8>>,
    next: u64,
}
impl NodeStore for Memory {
    fn get(&self, addr: u64) -> Result<Vec<u8>, Error> {
        self.nodes.get(&addr).cloned().ok_or(Error::Store)
    }
    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), Error> {
        assert_eq!(bytes.len(), NODE_SIZE);
        self.nodes.insert(addr, bytes);
        Ok(())
    }
    fn alloc(&mut self) -> Result<u64, Error> {
        self.next = self.next.max(20) + 4;
        Ok(self.next)
    }
}
fn key(n: u64) -> Key {
    Key {
        object: n / 7,
        item_type: 0x30 + (n % 3) as u8,
        offset: n,
    }
}
fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

// Read the encoded layout independently of the B-tree's traversal and balancing.
fn walk(
    store: &Memory,
    ptr: BlockPtr,
    lower: Option<Key>,
    upper: Option<Key>,
    root: bool,
    seen: &mut BTreeSet<u64>,
    out: &mut BTreeMap<Key, Vec<u8>>,
) {
    assert!(seen.insert(ptr.addr), "cycle or shared child");
    let b = &store.nodes[&ptr.addr];
    let h = NodeHeader::decode(b).unwrap();
    assert_eq!(
        (h.addr, h.generation, h.level),
        (ptr.addr, ptr.generation, ptr.level)
    );
    assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), ptr.csum);
    assert!(h.level < 8, "N3 height");
    assert!(h.nitems > 0 || (root && h.level == 0), "N3 empty node");
    let n = usize::from(h.nitems);
    let width = if h.level == 0 { 21 } else { 37 };
    let end = 48 + n * width;
    assert!(end <= NODE_SIZE, "N5 headers");
    assert!(h.level == 0 || n <= 441, "N5 internal capacity");
    let mut used = vec![false; NODE_SIZE];
    used[..end].fill(true);
    let mut keys = Vec::new();
    for i in 0..n {
        let start = 48 + i * width;
        let k = Key::decode(&b[start..start + 17]).unwrap();
        assert!(keys.last().is_none_or(|p| *p < k), "N4 order");
        assert!(
            lower.is_none_or(|l| l <= k) && upper.is_none_or(|u| k < u),
            "N4 bounds"
        );
        keys.push(k);
    }
    for i in 0..n {
        let start = 48 + i * width;
        if h.level == 0 {
            let item = LeafItemHeader::decode(&b[start..start + 21]).unwrap();
            let lo = usize::from(item.data_off);
            let hi = lo + usize::from(item.data_len);
            assert!(
                item.data_len <= 6144 && lo >= end && hi <= NODE_SIZE,
                "N5 payload"
            );
            assert!(used[lo..hi].iter().all(|v| !v), "N5 overlap");
            used[lo..hi].fill(true);
            assert!(out.insert(item.key, b[lo..hi].to_vec()).is_none());
        } else {
            let e = InternalEntry::decode(&b[start..start + 37]).unwrap();
            walk(
                store,
                BlockPtr {
                    addr: e.child_addr,
                    generation: e.child_generation,
                    csum: e.child_csum,
                    level: h.level - 1,
                },
                Some(e.key),
                keys.get(i + 1).copied().or(upper),
                false,
                seen,
                out,
            );
        }
    }
    assert!(
        b.iter().zip(used).all(|(v, u)| u || *v == 0),
        "N5 unused bytes"
    );
}
fn verify(store: &Memory, root: BlockPtr, model: &BTreeMap<Key, Vec<u8>>) {
    let mut actual = BTreeMap::new();
    walk(
        store,
        root,
        None,
        None,
        true,
        &mut BTreeSet::new(),
        &mut actual,
    );
    assert_eq!(&actual, model);
}

#[test]
fn random_ops_match_btreemap() {
    for seed in [1, 0x1234_5678_9abc_def0, 0xdead_beef_cafe_babe] {
        let mut rng = seed;
        let mut store = Memory::default();
        let mut root = btree::create(&mut store, 1, [42; 16], 1).unwrap();
        let mut model = BTreeMap::new();
        // Two maximum-size items fit per leaf: this also forces an internal split.
        for n in 0..1000 {
            let k = key(n);
            let payload = vec![next(&mut rng) as u8; 6144];
            assert_eq!(
                btree::insert(&mut store, &mut root, k, &payload).unwrap(),
                model.insert(k, payload)
            );
            verify(&store, root, &model);
        }
        assert!(root.level >= 2, "internal split was not exercised");
        for _ in 0..3000 {
            let k = key(next(&mut rng) % 1400);
            match next(&mut rng) % 5 {
                0..=1 => {
                    let lengths = [0, 1, 21, 128, 1024, 4096, 6144];
                    let len = lengths[(next(&mut rng) as usize) % lengths.len()];
                    let payload = vec![next(&mut rng) as u8; len];
                    assert_eq!(
                        btree::insert(&mut store, &mut root, k, &payload).unwrap(),
                        model.insert(k, payload)
                    );
                }
                2..=3 => assert_eq!(
                    btree::delete(&mut store, &mut root, k).unwrap(),
                    model.remove(&k)
                ),
                _ => assert_eq!(
                    btree::search(&store, root, k).unwrap(),
                    model.get(&k).cloned()
                ),
            }
            verify(&store, root, &model);
        }
        // Random draining crosses separators, merges internal nodes and collapses the root.
        while !model.is_empty() {
            let i = next(&mut rng) as usize % model.len();
            let k = *model.keys().nth(i).unwrap();
            assert_eq!(
                btree::delete(&mut store, &mut root, k).unwrap(),
                model.remove(&k)
            );
            verify(&store, root, &model);
        }
        assert_eq!(root.level, 0);
        assert_eq!(btree::delete(&mut store, &mut root, key(0)).unwrap(), None);
        verify(&store, root, &model);
    }
}

#[test]
fn oversize_payload_refused() {
    let mut store = Memory::default();
    let mut root = btree::create(&mut store, 2, [7; 16], 9).unwrap();
    btree::insert(&mut store, &mut root, key(1), &[5; 6144]).unwrap();
    let before = store.nodes.clone();
    let old_root = root;
    for k in [key(1), key(2)] {
        assert_eq!(
            btree::insert(&mut store, &mut root, k, &[3; 6145]),
            Err(Error::PayloadTooLarge)
        );
        assert_eq!(root, old_root);
        assert_eq!(store.nodes, before);
    }
    verify(&store, root, &BTreeMap::from([(key(1), vec![5; 6144])]));
}
