use cowfs_host_tests::{
    btree::{self, NodeStore},
    format::{self, BlockPtr, Extent, InternalEntry, Key, LeafItemHeader, NodeHeader, Ref, Root},
    space,
    txn::Transaction,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Memory {
    nodes: BTreeMap<u64, Vec<u8>>,
    next: u64,
}
impl NodeStore for Memory {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.nodes.get(&addr).cloned().ok_or(btree::Error::Store)
    }
    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), btree::Error> {
        self.nodes.insert(addr, bytes);
        Ok(())
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        self.next = self.next.max(20) + 4;
        Ok(self.next)
    }
}
fn key(n: u64) -> Key {
    Key {
        object: 16,
        item_type: format::XATTR,
        offset: n,
    }
}
fn walk(
    store: &impl NodeStore,
    ptr: BlockPtr,
    nodes: &mut BTreeSet<u64>,
    items: &mut BTreeMap<Key, Vec<u8>>,
) {
    assert!(nodes.insert(ptr.addr));
    let bytes = store.get(ptr.addr).unwrap();
    let h = NodeHeader::decode(&bytes).unwrap();
    assert_eq!(
        (h.addr, h.generation, h.level),
        (ptr.addr, ptr.generation, ptr.level)
    );
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        ptr.csum
    );
    for i in 0..usize::from(h.nitems) {
        if h.level == 0 {
            let at = 48 + i * 21;
            let item = LeafItemHeader::decode(&bytes[at..at + 21]).unwrap();
            let lo = usize::from(item.data_off);
            assert!(items
                .insert(
                    item.key,
                    bytes[lo..lo + usize::from(item.data_len)].to_vec()
                )
                .is_none());
        } else {
            let at = 48 + i * 37;
            let e = InternalEntry::decode(&bytes[at..at + 37]).unwrap();
            assert!(e.child_generation <= h.generation);
            walk(
                store,
                BlockPtr {
                    addr: e.child_addr,
                    generation: e.child_generation,
                    csum: e.child_csum,
                    level: h.level - 1,
                },
                nodes,
                items,
            );
        }
    }
}
fn contents(store: &impl NodeStore, root: BlockPtr) -> BTreeMap<Key, Vec<u8>> {
    let mut out = BTreeMap::new();
    walk(store, root, &mut BTreeSet::new(), &mut out);
    out
}
fn add_ref(store: &mut Memory, space: &mut BlockPtr, addr: u64, kind: u8, count: u32) {
    btree::insert(
        store,
        space,
        Key {
            object: addr,
            item_type: format::REF,
            offset: 4,
        },
        &Ref {
            refcount: count,
            kind,
            alloc_generation: 1,
            crcs: Vec::new(),
        }
        .encode(),
    )
    .unwrap();
}
fn fixture(
    count: u64,
    shared: bool,
    extra_space_items: u64,
) -> (Memory, BlockPtr, BlockPtr, BTreeMap<Key, Vec<u8>>) {
    let mut store = Memory::default();
    let mut fs = btree::create(&mut store, 1, [7; 16], 1).unwrap();
    let mut model = BTreeMap::new();
    for i in 0..count {
        let value = vec![i as u8; 6144];
        btree::insert(&mut store, &mut fs, key(i), &value).unwrap();
        model.insert(key(i), value);
    }
    let mut space = btree::create(&mut store, 2, [7; 16], 1).unwrap();
    let mut nodes = BTreeSet::new();
    walk(&store, fs, &mut nodes, &mut BTreeMap::new());
    for addr in nodes {
        add_ref(
            &mut store,
            &mut space,
            addr,
            1,
            if shared && addr == fs.addr { 2 } else { 1 },
        );
    }
    for i in 0..extra_space_items {
        add_ref(&mut store, &mut space, 200_000 + i * 4, 1, 1);
    }
    space::add_free(&mut store, &mut space, 10_000, 100_000).unwrap();
    loop {
        let mut nodes = BTreeSet::new();
        walk(&store, space, &mut nodes, &mut BTreeMap::new());
        let missing: Vec<_> = nodes
            .into_iter()
            .filter(|a| space::reference(&store, space, *a, 4).unwrap().is_none())
            .collect();
        if missing.is_empty() {
            break;
        }
        for addr in missing {
            add_ref(&mut store, &mut space, addr, 2, 1);
        }
    }
    (store, fs, space, model)
}
fn root_record(root: BlockPtr) -> Root {
    Root {
        root,
        next_inode: 17,
        source_root: 0,
        created_generation: 1,
        ctime: format::Time::default(),
        flags: 0,
        name: b"test".to_vec(),
    }
}

#[test]
fn second_change_does_not_copy_again() {
    let (store, mut root, space, _) = fixture(12, false, 0);
    let old = root;
    let before = contents(&store, old);
    let mut txn = Transaction::new(&store, space, 2).unwrap();
    txn.insert(&mut root, key(3), b"first").unwrap();
    assert_ne!(root.addr, old.addr);
    assert_eq!(root.generation, 2);
    let nodes: BTreeSet<_> = txn.writes().keys().copied().collect();
    let first_root = root.addr;
    txn.insert(&mut root, key(3), b"second").unwrap();
    assert_eq!(root.addr, first_root);
    assert_eq!(txn.writes().keys().copied().collect::<BTreeSet<_>>(), nodes);
    assert_eq!(contents(&txn.view(), old), before);
    let space = txn.settle().unwrap();
    assert!(space::reference(&txn.view(), space, old.addr, 4)
        .unwrap()
        .is_none());
    assert_eq!(
        space::reference(&txn.view(), space, root.addr, 4)
            .unwrap()
            .unwrap()
            .refcount,
        1
    );
    assert!(contents(&txn.view(), space)
        .iter()
        .any(|(k, v)| k.item_type == format::PENDING
            && k.offset == old.addr
            && format::Pending::decode(v).unwrap().released_generation == 2));
}

#[test]
fn shared_leaf_copy_increments_extents() {
    let (mut store, mut root, mut space, _) = fixture(0, true, 0);
    let extent_key = Key {
        object: 16,
        item_type: format::EXTENT,
        offset: 0,
    };
    let extent = Extent {
        phys_start: 300_000,
        phys_len: 4,
        extent_offset: 1,
        num_blocks: 2,
        flags: 0,
    };
    btree::insert(&mut store, &mut root, extent_key, &extent.encode()).unwrap();
    // Two items name the same physical REF, so each copied item adds a reference.
    btree::insert(
        &mut store,
        &mut root,
        Key {
            offset: 8192,
            ..extent_key
        },
        &extent.encode(),
    )
    .unwrap();
    btree::insert(
        &mut store,
        &mut space,
        Key {
            object: extent.phys_start,
            item_type: format::REF,
            offset: 4,
        },
        &Ref {
            refcount: 2,
            kind: 3,
            alloc_generation: 1,
            crcs: vec![1, 2, 3, 4],
        }
        .encode(),
    )
    .unwrap();
    let old = root;
    let mut txn = Transaction::new(&store, space, 2).unwrap();
    txn.insert(&mut root, key(1), b"changed").unwrap();
    assert_eq!(
        space::reference(&txn.view(), space, extent.phys_start, 4)
            .unwrap()
            .unwrap()
            .refcount,
        2,
        "space changes must be deferred"
    );
    let space = txn.settle().unwrap();
    assert_eq!(
        space::reference(&txn.view(), space, old.addr, 4)
            .unwrap()
            .unwrap()
            .refcount,
        1
    );
    let data = space::reference(&txn.view(), space, extent.phys_start, 4)
        .unwrap()
        .unwrap();
    assert_eq!(data.refcount, 4);
    assert_eq!(data.crcs, vec![1, 2, 3, 4]);
    assert_eq!(
        btree::search(&txn.view(), old, extent_key).unwrap(),
        Some(extent.encode().to_vec())
    );
}

#[test]
fn fixpoint_terminates() {
    let (store, mut root, old_space, _) = fixture(18, false, 800);
    assert!(old_space.level > 0);
    let original = contents(&store, old_space);
    let mut txn = Transaction::new(&store, old_space, 2).unwrap();
    for i in 0..18 {
        txn.insert(&mut root, key(i), b"edited").unwrap();
    }
    txn.update_root(1, root_record(root)).unwrap();
    let space = txn.settle().unwrap();
    assert_ne!(space.addr, old_space.addr);
    assert_eq!(contents(&txn.view(), old_space), original);
    let mut nodes = BTreeSet::new();
    walk(&txn.view(), space, &mut nodes, &mut BTreeMap::new());
    let live_space = nodes.clone();
    for addr in nodes {
        let r = space::reference(&txn.view(), space, addr, 4)
            .unwrap()
            .unwrap();
        assert_eq!((r.kind, r.refcount), (2, 1));
    }
    let recorded_space: BTreeSet<_> = contents(&txn.view(), space)
        .into_iter()
        .filter(|(k, v)| k.item_type == format::REF && Ref::decode(v).unwrap().kind == 2)
        .map(|(k, _)| k.object)
        .collect();
    assert_eq!(recorded_space, live_space);
    let root_key = Key {
        object: 0xffff_ffff_ffff_ff10,
        item_type: format::ROOT,
        offset: 1,
    };
    assert_eq!(
        Root::decode(
            &btree::search(&txn.view(), space, root_key)
                .unwrap()
                .unwrap()
        )
        .unwrap()
        .root,
        root
    );
    let writes = txn.writes().clone();
    assert_eq!(txn.settle().unwrap(), space);
    assert_eq!(txn.writes(), &writes);
}

#[test]
fn edits_match_btreemap_and_preserve_old_root() {
    for mut seed in [1u64, 0xdead_beef_cafe_babe] {
        let (store, mut root, old_space, mut model) = fixture(64, true, 0);
        let old = root;
        let before = model.clone();
        let mut txn = Transaction::new(&store, old_space, 2).unwrap();
        for _ in 0..250 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let k = key(seed % 100);
            if seed % 3 == 0 {
                assert_eq!(txn.delete(&mut root, k).unwrap(), model.remove(&k));
            } else {
                let value = vec![seed as u8; [1, 128, 4096, 6144][(seed >> 8) as usize % 4]];
                assert_eq!(
                    txn.insert(&mut root, k, &value).unwrap(),
                    model.insert(k, value)
                );
            }
            assert_eq!(contents(&txn.view(), root), model);
            assert_eq!(contents(&txn.view(), old), before);
        }
        for k in model.keys().copied().collect::<Vec<_>>() {
            txn.delete(&mut root, k).unwrap();
        }
        assert!(contents(&txn.view(), root).is_empty());
        assert_eq!(root.level, 0);
        let space = txn.settle().unwrap();
        assert_eq!(contents(&txn.view(), old), before);
        // A shared root was copied before descending: untouched children gain
        // a reference, while copied children transfer only the new parent's one.
        let mut incoming = BTreeMap::<u64, u32>::new();
        let mut seen = BTreeSet::new();
        let mut todo = vec![old, root];
        for p in &todo {
            *incoming.entry(p.addr).or_default() += 1;
        }
        while let Some(p) = todo.pop() {
            if !seen.insert(p.addr) {
                continue;
            }
            let bytes = txn.view().get(p.addr).unwrap();
            let h = NodeHeader::decode(&bytes).unwrap();
            if h.level == 0 {
                continue;
            }
            for i in 0..usize::from(h.nitems) {
                let at = 48 + i * 37;
                let e = InternalEntry::decode(&bytes[at..at + 37]).unwrap();
                *incoming.entry(e.child_addr).or_default() += 1;
                todo.push(BlockPtr {
                    addr: e.child_addr,
                    generation: e.child_generation,
                    csum: e.child_csum,
                    level: h.level - 1,
                });
            }
        }
        let recorded_fs: BTreeMap<_, _> = contents(&txn.view(), space)
            .into_iter()
            .filter(|(k, v)| k.item_type == format::REF && Ref::decode(v).unwrap().kind == 1)
            .map(|(k, v)| (k.object, Ref::decode(&v).unwrap().refcount))
            .collect();
        assert_eq!(recorded_fs, incoming);
        for (addr, count) in incoming {
            assert_eq!(
                space::reference(&txn.view(), space, addr, 4)
                    .unwrap()
                    .unwrap()
                    .refcount,
                count,
                "node {addr}"
            );
        }
    }
}
