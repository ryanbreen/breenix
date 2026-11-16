use cowfs_host_tests::{
    btree::{self, NodeStore},
    format::{self, BlockPtr, Key, Ref},
    space::{self, Error, Kind},
};
use std::collections::BTreeMap;

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
fn tree() -> (Memory, BlockPtr) {
    let mut store = Memory::default();
    let root = btree::create(&mut store, 2, [7; 16], 20).unwrap();
    (store, root)
}
fn item(store: &Memory, root: BlockPtr, object: u64, ty: u8, offset: u64) -> Option<Vec<u8>> {
    btree::search(
        store,
        root,
        Key {
            object,
            item_type: ty,
            offset,
        },
    )
    .unwrap()
}

#[test]
fn pending_waits_two_generations() {
    let (mut store, mut root) = tree();
    space::add_free(&mut store, &mut root, 100, 3).unwrap();
    let crcs = [0x12345678, 0x87654321, 0xe3069283];
    assert_eq!(
        space::allocate(&mut store, &mut root, 3, Kind::Data, &crcs, 5),
        Ok(100)
    );
    let reference = space::reference(&store, root, 100, 3).unwrap().unwrap();
    assert_eq!(reference.crcs, crcs);
    assert_eq!(reference.alloc_generation, 5);
    space::release(&mut store, &mut root, 100, 3, 7, false).unwrap();
    assert!(space::reference(&store, root, 100, 3).unwrap().is_none());
    let pending = item(&store, root, 0xffff_ffff_ffff_ff30, format::PENDING, 100).unwrap();
    assert_eq!(
        format::Pending::decode(&pending).unwrap(),
        format::Pending {
            length: 3,
            released_generation: 7
        }
    );
    let before = store.nodes.clone();
    for generation in [1, 7, 8] {
        assert_eq!(
            space::allocate(&mut store, &mut root, 1, Kind::Data, &[42], generation),
            Err(Error::ENOSPC)
        );
        assert_eq!(store.nodes, before);
    }
    assert_eq!(
        space::allocate(&mut store, &mut root, 1, Kind::Data, &[42], 9),
        Ok(100)
    );
    assert!(item(&store, root, 0xffff_ffff_ffff_ff30, format::PENDING, 100).is_none());
    let free = item(&store, root, 0xffff_ffff_ffff_ff20, format::FREE, 101).unwrap();
    assert_eq!(format::Free::decode(&free).unwrap().length, 2);
}

#[test]
fn free_before_pending() {
    let (mut store, mut root) = tree();
    space::add_free(&mut store, &mut root, 100, 4).unwrap();
    space::allocate(&mut store, &mut root, 4, Kind::FsNode, &[], 2).unwrap();
    space::release(&mut store, &mut root, 100, 4, 3, false).unwrap();
    space::add_free(&mut store, &mut root, 200, 2).unwrap();
    space::add_free(&mut store, &mut root, 202, 2).unwrap();
    assert!(item(&store, root, 0xffff_ffff_ffff_ff20, format::FREE, 202).is_none());
    assert_eq!(
        space::allocate(&mut store, &mut root, 4, Kind::SpaceNode, &[], 5),
        Ok(200)
    );
    assert!(item(&store, root, 0xffff_ffff_ffff_ff30, format::PENDING, 100).is_some());
    let before = store.nodes.clone();
    assert_eq!(
        space::allocate(&mut store, &mut root, 3, Kind::FsNode, &[], 5),
        Err(Error::Invalid)
    );
    assert_eq!(
        space::allocate(&mut store, &mut root, 257, Kind::Data, &[0; 257], 5),
        Err(Error::Invalid)
    );
    assert_eq!(
        space::allocate(&mut store, &mut root, 2, Kind::Data, &[0], 5),
        Err(Error::Invalid)
    );
    assert_eq!(store.nodes, before);
    assert_eq!(
        space::allocate(&mut store, &mut root, 4, Kind::FsNode, &[], 5),
        Ok(100)
    );
}

#[test]
fn refcount_overflow_is_emlink() {
    let (mut store, mut root) = tree();
    let key = Key {
        object: 100,
        item_type: format::REF,
        offset: 4,
    };
    btree::insert(
        &mut store,
        &mut root,
        key,
        &Ref {
            refcount: u32::MAX - 1,
            kind: 1,
            alloc_generation: 1,
            crcs: vec![],
        }
        .encode(),
    )
    .unwrap();
    space::increment(&mut store, &mut root, 100, 4).unwrap();
    let before = store.nodes.clone();
    assert_eq!(
        space::increment(&mut store, &mut root, 100, 4),
        Err(Error::EMLINK)
    );
    assert_eq!(store.nodes, before);
    assert_eq!(
        space::reference(&store, root, 100, 4)
            .unwrap()
            .unwrap()
            .refcount,
        u32::MAX
    );
}

#[test]
fn zero_ref_node_with_children_is_dead() {
    let (mut store, mut root) = tree();
    space::add_free(&mut store, &mut root, 100, 8).unwrap();
    space::allocate(&mut store, &mut root, 4, Kind::FsNode, &[], 2).unwrap();
    space::allocate(&mut store, &mut root, 4, Kind::FsNode, &[], 2).unwrap();
    space::increment(&mut store, &mut root, 100, 4).unwrap();
    space::release(&mut store, &mut root, 100, 4, 3, true).unwrap();
    assert_eq!(
        space::reference(&store, root, 100, 4)
            .unwrap()
            .unwrap()
            .refcount,
        1
    );
    space::release(&mut store, &mut root, 100, 4, 3, true).unwrap();
    assert!(space::reference(&store, root, 100, 4).unwrap().is_none());
    assert_eq!(item(&store, root, 100, format::DEAD, 4), Some(vec![]));
    assert!(item(&store, root, 0xffff_ffff_ffff_ff30, format::PENDING, 100).is_none());
    assert_eq!(
        space::reference(&store, root, 104, 4)
            .unwrap()
            .unwrap()
            .refcount,
        1
    );
    // Reaping owns the child decrements; making DEAD must leave the child alone.
    assert_eq!(
        space::allocate(&mut store, &mut root, 4, Kind::FsNode, &[], 10),
        Err(Error::ENOSPC)
    );
}
