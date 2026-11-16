use cowfs_host_tests::{format as disk, recording::RecordingDevice, *};
use std::{collections::BTreeMap, sync::Arc};
#[path = "../src/test_image.rs"]
mod test_image;

fn call(fs: &mut impl Fs, action: Action, tick: i64) -> Result<Reply> {
    fs.execute(&Operation {
        at: Time {
            seconds: tick,
            nanos: 23,
        },
        action,
    })
}
fn pair(writer: &mut impl Fs, model: &mut impl Fs, action: Action, tick: i64) -> Reply {
    let expected = call(model, action.clone(), tick);
    assert_eq!(call(writer, action, tick), expected);
    assert!(compare(model, writer).unwrap().is_empty());
    expected.unwrap()
}
fn setup() -> (WriterAdapter, model::Model, Arc<RecordingDevice>) {
    let device = Arc::new(RecordingDevice::new(test_image::image()).unwrap());
    (
        WriterAdapter::new(device.clone()).unwrap(),
        model::Model::new(),
        device,
    )
}
fn create(w: &mut impl Fs, m: &mut impl Fs, name: &[u8], tick: i64) -> Ino {
    match pair(
        w,
        m,
        Action::Create {
            dir: ROOT,
            name: name.to_vec(),
            mode: 0o640,
        },
        tick,
    ) {
        Reply::Inode(ino) => ino,
        other => panic!("{other:?}"),
    }
}
fn checked_commit(w: &mut impl Fs, m: &mut impl Fs, d: &RecordingDevice, ino: Ino, tick: i64) {
    pair(w, m, Action::Fsync { ino }, tick);
    test_image::check(&d.durable_image(), &format!("{ino}-{tick}"));
}
fn tree(d: &RecordingDevice) -> (disk::Superblock, Vec<(disk::Key, Vec<u8>)>) {
    use cowfs_host_tests::btree::NodeStore;
    fn items(store: &impl NodeStore, ptr: disk::BlockPtr) -> Vec<(disk::Key, Vec<u8>)> {
        let bytes = store.get(ptr.addr).unwrap();
        let h = disk::NodeHeader::decode(&bytes).unwrap();
        let mut out = Vec::new();
        for n in 0..h.nitems as usize {
            if h.level == 0 {
                let i = disk::LeafItemHeader::decode(&bytes[48 + n * 21..69 + n * 21]).unwrap();
                out.push((
                    i.key,
                    bytes[i.data_off as usize..i.data_off as usize + i.data_len as usize].to_vec(),
                ));
            } else {
                let e = disk::InternalEntry::decode(&bytes[48 + n * 37..85 + n * 37]).unwrap();
                out.extend(items(
                    store,
                    disk::BlockPtr {
                        addr: e.child_addr,
                        generation: e.child_generation,
                        csum: e.child_csum,
                        level: h.level - 1,
                    },
                ));
            }
        }
        out
    }
    let sb = recover::recover(d, false).unwrap();
    let entries = items(&commit::DeviceStore(d), sb.space_root);
    (sb, entries)
}

#[test]
fn sparse_write_reads_holes_as_zero() {
    let (mut w, mut m, d) = setup();
    let ino = create(&mut w, &mut m, b"sparse", 1);
    assert_eq!(ino, 16);
    pair(
        &mut w,
        &mut m,
        Action::Write {
            ino,
            offset: 8191,
            bytes: b"ab\0cd".to_vec(),
        },
        2,
    );
    pair(
        &mut w,
        &mut m,
        Action::Write {
            ino,
            offset: 8192,
            bytes: b"Q".to_vec(),
        },
        2,
    );
    let mut read = vec![99; 8196];
    assert_eq!(w.read(ino, 0, &mut read).unwrap(), 8196);
    assert!(read[..8191].iter().all(|b| *b == 0));
    assert_eq!(&read[8191..], b"aQ\0cd");
    checked_commit(&mut w, &mut m, &d, ino, 3);
    pair(&mut w, &mut m, Action::Remount, 4);
    pair(
        &mut w,
        &mut m,
        Action::Write {
            ino,
            offset: 8192,
            bytes: b"Z".to_vec(),
        },
        5,
    );
    let attrs = SetAttr {
        mode: Some(0o6750),
        uid: Some(7),
        gid: Some(8),
        atime: Some(Time {
            seconds: -2,
            nanos: 4,
        }),
        mtime: None,
    };
    pair(&mut w, &mut m, Action::Setattr { ino, attrs }, 6);
    checked_commit(&mut w, &mut m, &d, ino, 7);
    pair(&mut w, &mut m, Action::Remount, 8);
    assert_eq!(create(&mut w, &mut m, b"next", 9), 17);
    checked_commit(&mut w, &mut m, &d, ino, 10);
}
#[test]
fn truncate_releases_extents_to_pending() {
    let (mut w, mut m, d) = setup();
    let ino = create(&mut w, &mut m, b"shorten", 1);
    pair(
        &mut w,
        &mut m,
        Action::Write {
            ino,
            offset: 0,
            bytes: vec![0x5a; 12288],
        },
        2,
    );
    checked_commit(&mut w, &mut m, &d, ino, 3);
    let (_, old_space) = tree(&d);
    let old: Vec<_> = old_space
        .iter()
        .filter(|(k, v)| k.item_type == disk::REF && disk::Ref::decode(v).unwrap().kind == 3)
        .map(|(k, _)| (k.object, k.offset))
        .collect();
    assert!(!old.is_empty());
    pair(&mut w, &mut m, Action::Truncate { ino, size: 1 }, 4);
    checked_commit(&mut w, &mut m, &d, ino, 5);
    let (sb, space) = tree(&d);
    // Shrinking to one byte releases the old data REFs, including the replaced tail.
    for (start, length) in old {
        for block in start..start + length {
            assert!(space.iter().any(|(k, v)| k.item_type == disk::PENDING && {
                let p = disk::Pending::decode(v).unwrap();
                k.offset <= block
                    && block < k.offset + p.length
                    && p.released_generation == sb.generation
            }));
        }
    }
    pair(&mut w, &mut m, Action::Truncate { ino, size: 12288 }, 6);
    checked_commit(&mut w, &mut m, &d, ino, 7);
    pair(&mut w, &mut m, Action::Remount, 8);
    pair(&mut w, &mut m, Action::Truncate { ino, size: 0 }, 9);
    checked_commit(&mut w, &mut m, &d, ino, 10);
}
#[test]
fn xattr_limits() {
    let (mut w, mut m, d) = setup();
    let ino = create(&mut w, &mut m, b"attrs", 1);
    let name = vec![b'x'; 255];
    pair(
        &mut w,
        &mut m,
        Action::Setxattr {
            ino,
            name: name.clone(),
            value: vec![42; 4096],
        },
        2,
    );
    checked_commit(&mut w, &mut m, &d, ino, 3);
    pair(&mut w, &mut m, Action::Remount, 4);
    for (name, value) in [
        (vec![], vec![]),
        (vec![b'x'; 256], vec![]),
        (b"bad\0".to_vec(), vec![]),
        (b"user.large".to_vec(), vec![0; 4097]),
    ] {
        let before = w.state().unwrap();
        let action = Action::Setxattr { ino, name, value };
        assert_eq!(call(&mut m, action.clone(), 5), Err(Errno::EINVAL.into()));
        assert_eq!(call(&mut w, action, 5), Err(Errno::EINVAL.into()));
        assert_eq!(w.state().unwrap(), before);
    }
    pair(
        &mut w,
        &mut m,
        Action::Setxattr {
            ino,
            name: name.clone(),
            value: b"replacement".to_vec(),
        },
        6,
    );
    pair(&mut w, &mut m, Action::Removexattr { ino, name }, 7);
    checked_commit(&mut w, &mut m, &d, ino, 8);
    pair(&mut w, &mut m, Action::Remount, 9);
}

#[test]
fn namespace_replacement_and_orphan_recovery() {
    use cowfs_host_tests::{
        btree::{self, NodeStore},
        commit::Committer,
        ops,
        txn::Transaction,
    };
    let (mut w, mut m, d) = setup();
    fn made(reply: Reply) -> Ino {
        let Reply::Inode(ino) = reply else {
            panic!("{reply:?}")
        };
        ino
    }
    let left = made(pair(
        &mut w,
        &mut m,
        Action::Mkdir {
            dir: ROOT,
            name: b"left".to_vec(),
            mode: 0o755,
        },
        1,
    ));
    let right = made(pair(
        &mut w,
        &mut m,
        Action::Mkdir {
            dir: ROOT,
            name: b"right".to_vec(),
            mode: 0o700,
        },
        2,
    ));
    let child = made(pair(
        &mut w,
        &mut m,
        Action::Mkdir {
            dir: left,
            name: b"child".to_vec(),
            mode: 0o755,
        },
        3,
    ));
    pair(
        &mut w,
        &mut m,
        Action::Mkdir {
            dir: right,
            name: b"empty".to_vec(),
            mode: 0o755,
        },
        4,
    );
    for action in [
        Action::Rmdir {
            dir: ROOT,
            name: b"left".to_vec(),
        },
        Action::Rename {
            from_dir: ROOT,
            from: b"right".to_vec(),
            to_dir: ROOT,
            to: b"left".to_vec(),
        },
        Action::Rename {
            from_dir: ROOT,
            from: b"left".to_vec(),
            to_dir: child,
            to: b"cycle".to_vec(),
        },
    ] {
        let expected = call(&mut m, action.clone(), 5);
        assert!(expected.is_err());
        let before = w.state().unwrap();
        assert_eq!(call(&mut w, action, 5), expected);
        assert_eq!(w.state().unwrap(), before);
    }
    pair(
        &mut w,
        &mut m,
        Action::Rename {
            from_dir: left,
            from: b"child".to_vec(),
            to_dir: right,
            to: b"empty".to_vec(),
        },
        6,
    );
    pair(
        &mut w,
        &mut m,
        Action::Rmdir {
            dir: right,
            name: b"empty".to_vec(),
        },
        7,
    );
    for (n, len) in [(b"short".as_slice(), 1), (b"long".as_slice(), 4095)] {
        pair(
            &mut w,
            &mut m,
            Action::Symlink {
                dir: ROOT,
                name: n.to_vec(),
                target: vec![b'x'; len],
            },
            8,
        );
    }
    for target in [vec![], vec![b'x'; 4096], b"bad\0target".to_vec()] {
        let a = Action::Symlink {
            dir: ROOT,
            name: b"invalid".to_vec(),
            target,
        };
        let before = w.state().unwrap();
        let expected = call(&mut m, a.clone(), 9);
        assert!(expected.is_err());
        assert_eq!(call(&mut w, a, 9), expected);
        assert_eq!(w.state().unwrap(), before);
    }
    let ino = create(&mut w, &mut m, b"file", 10);
    let replaced = create(&mut w, &mut m, b"replaced", 11);
    for file in [ino, replaced] {
        pair(
            &mut w,
            &mut m,
            Action::Write {
                ino: file,
                offset: 4095,
                bytes: b"payload".to_vec(),
            },
            12,
        );
    }
    checked_commit(&mut w, &mut m, &d, ROOT, 13);
    pair(
        &mut w,
        &mut m,
        Action::Link {
            ino,
            dir: ROOT,
            name: b"alias".to_vec(),
        },
        14,
    );
    pair(
        &mut w,
        &mut m,
        Action::Rename {
            from_dir: ROOT,
            from: b"file".to_vec(),
            to_dir: ROOT,
            to: b"alias".to_vec(),
        },
        15,
    );
    pair(
        &mut w,
        &mut m,
        Action::Rename {
            from_dir: ROOT,
            from: b"file".to_vec(),
            to_dir: ROOT,
            to: b"replaced".to_vec(),
        },
        16,
    );
    pair(
        &mut w,
        &mut m,
        Action::Unlink {
            dir: ROOT,
            name: b"alias".to_vec(),
        },
        17,
    );
    checked_commit(&mut w, &mut m, &d, ROOT, 18);
    pair(&mut w, &mut m, Action::Remount, 19);

    // Construct a valid committed orphan as left by an open unlinked file.
    // Recovery must finish it even when no namespace operation follows mount.
    let sb = recover::recover(d.as_ref(), false).unwrap();
    struct Image(Vec<u8>);
    impl NodeStore for Image {
        fn get(&self, addr: u64) -> std::result::Result<Vec<u8>, btree::Error> {
            let lo = addr as usize * disk::BLOCK_SIZE;
            self.0
                .get(lo..lo + disk::NODE_SIZE)
                .map(|b| b.to_vec())
                .ok_or(btree::Error::Store)
        }
        fn put(&mut self, _: u64, _: Vec<u8>) -> std::result::Result<(), btree::Error> {
            unreachable!()
        }
        fn alloc(&mut self) -> std::result::Result<u64, btree::Error> {
            unreachable!()
        }
    }
    let mut tx =
        Transaction::new_owned(Image(d.durable_image()), sb.space_root, sb.generation + 1).unwrap();
    let root_key = ops::key(0xffff_ffff_ffff_ff10, disk::ROOT, 1);
    let mut root = disk::Root::decode(
        &btree::search(&tx.view(), sb.space_root, root_key)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let entries = ops::items(&tx.view(), root.root).unwrap();
    let data_refs: Vec<_> = entries
        .iter()
        .filter(|(k, _)| k.object == ino && k.item_type == disk::EXTENT)
        .map(|(_, v)| disk::Extent::decode(v).unwrap().phys_start)
        .collect();
    assert!(!data_refs.is_empty());
    let mut parent = disk::Inode::decode(
        &btree::search(&tx.view(), root.root, ops::key(ROOT, disk::INODE, 0))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    for (k, v) in &entries {
        if k.object == ROOT
            && (k.item_type == disk::DIR_ENTRY || k.item_type == disk::DIR_INDEX)
            && ops::dir_records(v)
                .unwrap()
                .iter()
                .any(|r| r.name == b"replaced")
        {
            assert_eq!(ops::dir_records(v).unwrap().len(), 1);
            tx.delete(&mut root.root, *k).unwrap();
        }
    }
    parent.size -= 1;
    tx.insert(
        &mut root.root,
        ops::key(ROOT, disk::INODE, 0),
        &parent.encode(),
    )
    .unwrap();
    let ik = ops::key(ino, disk::INODE, 0);
    let mut inode =
        disk::Inode::decode(&btree::search(&tx.view(), root.root, ik).unwrap().unwrap()).unwrap();
    inode.nlink = 0;
    tx.insert(&mut root.root, ik, &inode.encode()).unwrap();
    tx.insert(
        &mut root.root,
        ops::key(0xffff_ffff_ffff_ff00, disk::ORPHAN, ino),
        &[],
    )
    .unwrap();
    tx.update_root(1, root).unwrap();
    let orphan_sb = Committer::new(sb)
        .commit(d.as_ref(), &mut tx, &BTreeMap::new())
        .unwrap();
    test_image::check(&d.durable_image(), "orphan-before-recovery");
    assert_eq!(
        recover::recover(d.as_ref(), false).unwrap().generation,
        orphan_sb.generation
    );
    let recovered = recover::recover(d.as_ref(), true).unwrap();
    assert_eq!(recovered.generation, orphan_sb.generation + 1);
    let (_, space) = tree(&d);
    let root = disk::Root::decode(&space.iter().find(|(k, _)| *k == root_key).unwrap().1).unwrap();
    let fs = ops::items(&commit::DeviceStore(d.as_ref()), root.root).unwrap();
    assert!(!fs
        .iter()
        .any(|(k, _)| k.object == ino || k.item_type == disk::ORPHAN));
    for addr in data_refs {
        assert!(space.iter().any(|(k, v)| k.item_type == disk::PENDING && {
            let p = disk::Pending::decode(v).unwrap();
            k.offset <= addr
                && addr < k.offset + p.length
                && p.released_generation == recovered.generation
        }));
    }
    test_image::check(&d.durable_image(), "orphan-after-recovery");
}

#[test]
fn siphash_collision_keeps_both_records() {
    use cowfs_host_tests::{btree, ops, siphash};
    // Generated by birthday search over hexadecimal names with SipHash-2-4,
    // key 00000000000000000000000000000000. No truncated hash or test hook.
    let a = b"cdfc07073c9936f4";
    let b = b"bdad17bd9f771c95";
    let hash = siphash::hash(&[0; 16], a);
    assert_ne!(a, b);
    assert_eq!(hash, 0xf4fe9e6f48ac9e1d);
    assert_eq!(hash, siphash::hash(&[0; 16], b));
    let mut image = test_image::image();
    let total = image.len() / disk::BLOCK_SIZE;
    for block in (16..20).chain(total - 4..total) {
        let bytes = &mut image[block * disk::BLOCK_SIZE..(block + 1) * disk::BLOCK_SIZE];
        if let Ok(mut sb) = disk::Superblock::decode(bytes) {
            sb.dirhash_key = [0; 16];
            bytes.copy_from_slice(&sb.encode());
        }
    }
    let d = Arc::new(RecordingDevice::new(image).unwrap());
    let mut w = WriterAdapter::new(d.clone()).unwrap();
    let mut m = model::Model::new();
    let first = create(&mut w, &mut m, a, 1);
    let second = create(&mut w, &mut m, b, 2);
    checked_commit(&mut w, &mut m, &d, ROOT, 3);
    let (_, space) = tree(&d);
    let root = disk::Root::decode(
        &space
            .iter()
            .find(|(k, _)| k.item_type == disk::ROOT)
            .unwrap()
            .1,
    )
    .unwrap();
    let store = commit::DeviceStore(d.as_ref());
    let bucket = btree::search(&store, root.root, ops::key(ROOT, disk::DIR_ENTRY, hash))
        .unwrap()
        .unwrap();
    let records = ops::dir_records(&bucket).unwrap();
    assert_eq!(
        records
            .iter()
            .map(|r| (r.name.as_slice(), r.ino))
            .collect::<Vec<_>>(),
        [(a.as_slice(), first), (b.as_slice(), second)]
    );
    let inode = disk::Inode::decode(
        &btree::search(&store, root.root, ops::key(ROOT, disk::INODE, 0))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!((inode.size, inode.next_index), (2, 4));
    for (index, name) in [(2, a), (3, b)] {
        let v = btree::search(&store, root.root, ops::key(ROOT, disk::DIR_INDEX, index))
            .unwrap()
            .unwrap();
        assert_eq!(ops::dir_records(&v).unwrap()[0].name, name);
    }
    pair(&mut w, &mut m, Action::Remount, 4);
    pair(
        &mut w,
        &mut m,
        Action::Rename {
            from_dir: ROOT,
            from: a.to_vec(),
            to_dir: ROOT,
            to: b.to_vec(),
        },
        5,
    );
    checked_commit(&mut w, &mut m, &d, ROOT, 6);
    pair(
        &mut w,
        &mut m,
        Action::Link {
            ino: first,
            dir: ROOT,
            name: a.to_vec(),
        },
        7,
    );
    pair(
        &mut w,
        &mut m,
        Action::Unlink {
            dir: ROOT,
            name: b.to_vec(),
        },
        8,
    );
    checked_commit(&mut w, &mut m, &d, ROOT, 9);
    pair(&mut w, &mut m, Action::Remount, 10);
    pair(
        &mut w,
        &mut m,
        Action::Unlink {
            dir: ROOT,
            name: a.to_vec(),
        },
        11,
    );
    checked_commit(&mut w, &mut m, &d, ROOT, 12);
    pair(&mut w, &mut m, Action::Remount, 13);
    assert_eq!(w.state().unwrap().inodes.len(), 1);
}

#[test]
fn fsync_preparation_error_completes_waiters_and_latches() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Store<'a> {
        device: &'a RecordingDevice,
        fail: &'a AtomicBool,
    }
    impl btree::NodeStore for Store<'_> {
        fn get(&self, addr: u64) -> std::result::Result<Vec<u8>, btree::Error> {
            if self.fail.load(Ordering::Relaxed) {
                Err(btree::Error::Store)
            } else {
                btree::NodeStore::get(&commit::DeviceStore(self.device), addr)
            }
        }
        fn put(&mut self, _: u64, _: Vec<u8>) -> std::result::Result<(), btree::Error> { unreachable!() }
        fn alloc(&mut self) -> std::result::Result<u64, btree::Error> { unreachable!() }
    }
    let device = RecordingDevice::new(test_image::image()).unwrap();
    let sb = recover::recover(&device, false).unwrap();
    let fail = AtomicBool::new(false);
    let mut files = ops::Files::new(Store { device: &device, fail: &fail }, &sb, 1).unwrap();
    let mut mount = commit::Committer::new(sb);
    let waiters = [mount.waiter(), mount.waiter()];
    fail.store(true, Ordering::Relaxed);
    assert_eq!(files.fsync(&device, &mut mount), Err(ops::Error::EIO));
    assert!(mount.read_only());
    for w in waiters { assert_eq!(w.result(), Some(Err(commit::Error::EIO))); }
    assert_eq!(mount.waiter().result(), Some(Err(commit::Error::EIO)));
    assert!(device.events().is_empty());
    fail.store(false, Ordering::Relaxed);
    assert_eq!(files.fsync(&device, &mut mount), Err(ops::Error::EIO));
    assert!(device.events().is_empty());
}
