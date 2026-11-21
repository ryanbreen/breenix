use breenfs_host_tests::{
    btree,
    commit::{Committer, DeviceStore},
    format::{
        self, BlockPtr, DirRecord, Extent, Inode, InternalEntry, Key, LeafItemHeader, NodeHeader,
        Ref, Root, Superblock, Time, Xattr, BLOCK_SIZE, NODE_SIZE,
    },
    ops::{self, Files},
    recording::RecordingDevice,
    recover,
    reserve::{Ledger, Operation},
    space,
    txn::{Transaction, REAP_PER_COMMIT},
};
use std::{collections::BTreeMap, time::Instant};

#[path = "../src/test_image.rs"]
mod test_image;

const ATTRS: usize = 110;
const DATA_START: u64 = 32 + ATTRS as u64 * 4;

// Encode three-level geometry directly: a root, an internal node and 110
// leaves. 110 internal entries occupy 4118 bytes, above the quarter-full warning.
// One 4 KiB xattr occupies each leaf, including the first leaf's inode/directory/
// extent records. Rewrite 110/110 leaves and reap across transaction budgets.
fn fixture() -> Vec<u8> {
    let template = RecordingDevice::new(test_image::image()).unwrap();
    let mut sb = recover::recover(&template, false).unwrap();
    let mut image = vec![0; sb.total_blocks as usize * BLOCK_SIZE];
    fn node(
        image: &mut [u8],
        sb: &Superblock,
        addr: u64,
        tree: u8,
        level: u8,
        items: &[(Key, Vec<u8>)],
        children: &[(Key, BlockPtr)],
    ) -> BlockPtr {
        let mut bytes = vec![0; NODE_SIZE];
        if level == 0 {
            let mut end = NODE_SIZE;
            for (n, (key, value)) in items.iter().enumerate() {
                end -= value.len();
                bytes[end..end + value.len()].copy_from_slice(value);
                let at = 48 + n * 21;
                bytes[at..at + 21].copy_from_slice(
                    &LeafItemHeader {
                        key: *key,
                        data_off: end as u16,
                        data_len: value.len() as u16,
                    }
                    .encode(),
                );
            }
        } else {
            for (n, (key, ptr)) in children.iter().enumerate() {
                let at = 48 + n * 37;
                bytes[at..at + 37].copy_from_slice(
                    &InternalEntry {
                        key: *key,
                        child_addr: ptr.addr,
                        child_generation: ptr.generation,
                        child_csum: ptr.csum,
                    }
                    .encode(),
                );
            }
        }
        NodeHeader {
            tree,
            level,
            nitems: if level == 0 {
                items.len()
            } else {
                children.len()
            } as u16,
            fs_uuid: sb.fs_uuid,
            addr,
            generation: sb.generation,
        }
        .encode(&mut bytes)
        .unwrap();
        let ptr = BlockPtr {
            addr,
            generation: sb.generation,
            level,
            csum: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        };
        image[addr as usize * BLOCK_SIZE..addr as usize * BLOCK_SIZE + NODE_SIZE]
            .copy_from_slice(&bytes);
        ptr
    }
    let record = DirRecord {
        ino: 16,
        kind: 1,
        name: b"file".to_vec(),
    }
    .encode();
    let mut first = vec![
        (
            ops::key(2, format::INODE, 0),
            Inode {
                mode: 0o40755,
                nlink: 2,
                size: 1,
                parent: 2,
                next_index: 3,
                created_generation: 1,
                ..Default::default()
            }
            .encode()
            .to_vec(),
        ),
        (
            ops::key(
                2,
                format::DIR_ENTRY,
                breenfs_host_tests::siphash::hash(&sb.dirhash_key, b"file"),
            ),
            record.clone(),
        ),
        (ops::key(2, format::DIR_INDEX, 2), record),
        (
            ops::key(16, format::INODE, 0),
            Inode {
                mode: 0o100600,
                nlink: 1,
                size: 8192,
                nblocks: 2,
                created_generation: 1,
                ..Default::default()
            }
            .encode()
            .to_vec(),
        ),
    ];
    let mut space = Vec::new();
    for (n, byte) in [0x37, 0x45].into_iter().enumerate() {
        let addr = DATA_START + n as u64;
        let data = vec![byte; BLOCK_SIZE];
        image[addr as usize * BLOCK_SIZE..(addr as usize + 1) * BLOCK_SIZE].copy_from_slice(&data);
        first.push((
            ops::key(16, format::EXTENT, n as u64 * BLOCK_SIZE as u64),
            Extent {
                phys_start: addr,
                phys_len: 1,
                extent_offset: 0,
                num_blocks: 1,
                flags: 0,
            }
            .encode()
            .to_vec(),
        ));
        space.push((
            ops::key(addr, format::REF, 1),
            Ref {
                refcount: 1,
                kind: 3,
                alloc_generation: 1,
                crcs: vec![breenfs_host_tests::crc32c::checksum(&data)],
            }
            .encode(),
        ));
    }
    let mut attrs: Vec<_> = (0..ATTRS)
        .map(|n| {
            let name = format!("attr{n:04}").into_bytes();
            (
                ops::key(
                    16,
                    format::XATTR,
                    breenfs_host_tests::siphash::hash(&sb.dirhash_key, &name),
                ),
                Xattr {
                    name,
                    value: vec![0x11; 4096],
                }
                .encode(),
            )
        })
        .collect();
    attrs.sort_by_key(|(key, _)| *key);
    let mut leaves = Vec::new();
    for (n, attr) in attrs.into_iter().enumerate() {
        let mut items = if n == 0 {
            core::mem::take(&mut first)
        } else {
            Vec::new()
        };
        items.push(attr);
        let ptr = node(&mut image, &sb, 32 + n as u64 * 4, 1, 0, &items, &[]);
        leaves.push((items[0].0, ptr));
    }
    let mut internal = Vec::new();
    for (n, children) in leaves.chunks(ATTRS).enumerate() {
        let ptr = node(&mut image, &sb, 28 + n as u64 * 4, 1, 1, &[], children);
        internal.push((children[0].0, ptr));
    }
    let fs = node(&mut image, &sb, 24, 1, 2, &[], &internal);
    for addr in (20..DATA_START).step_by(4) {
        space.push((
            ops::key(addr, format::REF, 4),
            Ref {
                refcount: 1,
                kind: if addr == 20 { 2 } else { 1 },
                alloc_generation: 1,
                crcs: Vec::new(),
            }
            .encode(),
        ));
    }
    space.push((
        ops::key(0xffff_ffff_ffff_ff10, format::ROOT, 1),
        Root {
            root: fs,
            next_inode: 17,
            source_root: 0,
            created_generation: 1,
            ctime: Time::default(),
            flags: 0,
            name: b"default".to_vec(),
        }
        .encode(),
    ));
    sb.free_blocks = sb.total_blocks - 4 - (DATA_START + 2);
    space.push((
        ops::key(0xffff_ffff_ffff_ff20, format::FREE, DATA_START + 2),
        format::Free {
            length: sb.free_blocks,
        }
        .encode()
        .to_vec(),
    ));
    space.sort_by_key(|(key, _)| *key);
    sb.space_root = node(&mut image, &sb, 20, 2, 0, &space, &[]);
    for ring in 0..2 {
        sb.ring = ring;
        let addr = if ring == 0 { 16 } else { sb.total_blocks - 4 } + u64::from(sb.slot);
        image[addr as usize * BLOCK_SIZE..(addr as usize + 1) * BLOCK_SIZE]
            .copy_from_slice(&sb.encode());
    }
    image
}

fn root(device: &RecordingDevice, sb: &Superblock) -> Root {
    Root::decode(
        &btree::search(
            &DeviceStore(device),
            sb.space_root,
            ops::key(0xffff_ffff_ffff_ff10, format::ROOT, 1),
        )
        .unwrap()
        .unwrap(),
    )
    .unwrap()
}

fn dead(device: &RecordingDevice, sb: &Superblock) -> Vec<u64> {
    btree::range(
        &DeviceStore(device),
        sb.space_root,
        ops::key(0, 0, 0),
        ops::key(u64::MAX, u8::MAX, u64::MAX),
    )
    .unwrap()
    .into_iter()
    .filter(|(k, _)| k.item_type == format::DEAD)
    .map(|(k, _)| k.object)
    .collect()
}

fn verify(device: &RecordingDevice, sb: &Superblock, shared: (u64, u64)) {
    test_image::check(&device.durable_image(), "reaper");
    assert_eq!(recover::recover(device, false).unwrap(), *sb);
    let files = Files::new(DeviceStore(device), sb, 1, false).unwrap();
    let ino = files.lookup(2, b"file").unwrap();
    let mut bytes = vec![0; 2 * BLOCK_SIZE];
    assert_eq!(files.read(device, ino, 0, &mut bytes).unwrap(), bytes.len());
    assert_eq!(&bytes[..BLOCK_SIZE], vec![0x37; BLOCK_SIZE]);
    assert_eq!(&bytes[BLOCK_SIZE..], vec![0x82; BLOCK_SIZE]);
    let attrs = files.list_xattrs(ino).unwrap();
    assert_eq!(attrs.len(), ATTRS);
    assert!(attrs.iter().all(|attr| attr.value == vec![0x22; 4096]));
    assert!(
        space::reference(&DeviceStore(device), sb.space_root, shared.0, shared.1)
            .unwrap()
            .is_some(),
        "a live extent must never become PENDING or FREE"
    );
}

fn finish(device: &RecordingDevice, mut sb: Superblock, shared: (u64, u64)) -> Superblock {
    let mut mount = Committer::new(sb.clone());
    let mut txn =
        Transaction::new_owned(DeviceStore(device), sb.space_root, sb.generation + 1).unwrap();
    let mut commits = 0;
    while txn.dead_count() != 0 {
        let mut reaped = 0;
        let mut lowest = dead(device, &sb).first().copied();
        while let Some(addr) = txn.reap_step().unwrap() {
            // Materialize the transaction's SPACE view only as a test oracle.
            // The production step uses its mount-time address index.
            let space = txn.settle().unwrap();
            let keys: Vec<_> = btree::range(
                &txn.view(),
                space,
                ops::key(0, 0, 0),
                ops::key(u64::MAX, u8::MAX, u64::MAX),
            )
            .unwrap()
            .into_iter()
            .filter(|(k, _)| k.item_type == format::DEAD)
            .map(|(k, _)| k.object)
            .collect();
            assert_eq!(Some(addr), lowest);
            lowest = keys.first().copied();
            assert_eq!(txn.dead_count(), keys.len() as u64);
            assert!(!keys.contains(&addr));
            reaped += 1;
            assert!(reaped <= 16);
        }
        assert!(reaped != 0);
        assert_eq!(REAP_PER_COMMIT, 16);
        // Calling again in the same transaction cannot obtain another budget.
        assert_eq!(txn.reap_step().unwrap(), None);
        txn.promote_pending().unwrap();
        sb = mount.commit(device, &mut txn, &BTreeMap::new()).unwrap();
        verify(device, &sb, shared);
        assert_eq!(txn.dead_count(), dead(device, &sb).len() as u64);
        commits += 1;
        let state = txn.dead_nodes().clone();
        txn = Transaction::new_owned_with_dead(
            DeviceStore(device),
            sb.space_root,
            sb.generation + 1,
            state,
            txn.space_node_count(),
        )
        .unwrap();
    }
    assert!(commits > 1, "deletion must cross transaction budgets");
    // Drain the two-generation quarantine; SPACE COW itself remains quarantined
    // in steady state and is identical in the no-snapshot baseline.
    for _ in 0..3 {
        txn.promote_pending().unwrap();
        sb = mount.commit(device, &mut txn, &BTreeMap::new()).unwrap();
        verify(device, &sb, shared);
        let state = txn.dead_nodes().clone();
        txn = Transaction::new_owned_with_dead(
            DeviceStore(device),
            sb.space_root,
            sb.generation + 1,
            state,
            txn.space_node_count(),
        )
        .unwrap();
    }
    sb
}

#[test]
fn budgeted_three_level_deletion_recovers_and_returns_space() {
    let start = Instant::now();
    let device = RecordingDevice::new(fixture()).unwrap();
    let sb = recover::recover(&device, false).unwrap();
    let ino = 16;
    let original = root(&device, &sb);
    assert_eq!(original.root.level, 2, "three-level tree");
    test_image::check(&device.durable_image(), "reaper-source");
    let extent = Extent::decode(
        &btree::search(
            &DeviceStore(&device),
            original.root,
            ops::key(ino, format::EXTENT, 0),
        )
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    let shared = (extent.phys_start, u64::from(extent.phys_len));
    eprintln!("reaper fixture {:?}", start.elapsed());
    let source_image = device.durable_image();
    let mut finals = Vec::new();
    let mut deletion_image = None;
    for snapshot in [false, true] {
        let dev = RecordingDevice::new(source_image.clone()).unwrap();
        let mut sb = recover::recover(&dev, true).unwrap();
        let mut mount = Committer::new(sb.clone());
        let mut files = Files::new(DeviceStore(&dev), &sb, 1, true).unwrap();
        if snapshot {
            files.snapshot(b"old", true).unwrap();
        }
        sb = files.fsync(&dev, &mut mount).unwrap();
        let mut files = Files::new(DeviceStore(&dev), &sb, 1, true).unwrap();
        // Rewrite the xattrs to COW the old leaves in live.
        for n in 0..ATTRS {
            files
                .xattr(
                    ino,
                    format!("attr{n:04}").as_bytes(),
                    Some(&vec![0x22; 4096]),
                    Time::default(),
                )
                .unwrap();
        }
        files
            .write(
                &dev,
                ino,
                BLOCK_SIZE as u64,
                &vec![0x82; BLOCK_SIZE],
                Time::default(),
            )
            .unwrap();
        sb = files.fsync(&dev, &mut mount).unwrap();
        let mut files = Files::new(DeviceStore(&dev), &sb, 1, true).unwrap();
        if snapshot {
            files.delete_root(b"old").unwrap();
        }
        sb = files.fsync(&dev, &mut mount).unwrap();
        verify(&dev, &sb, shared);
        eprintln!("reaper snapshot={snapshot} prepared {:?}", start.elapsed());
        if snapshot {
            assert_eq!(dead(&dev, &sb), vec![original.root.addr]);
            // Save after a committed step, while descendants still own refs.
            let mut txn =
                Transaction::new_owned(DeviceStore(&dev), sb.space_root, sb.generation + 1)
                    .unwrap();
            assert_eq!(txn.dead_count(), 1);
            assert_eq!(txn.reap_step().unwrap(), Some(original.root.addr));
            txn.promote_pending().unwrap();
            sb = mount.commit(&dev, &mut txn, &BTreeMap::new()).unwrap();
            verify(&dev, &sb, shared);
            assert!(!dead(&dev, &sb).is_empty());
            deletion_image = Some(dev.durable_image());
            finals.push(finish(&dev, sb, shared).free_blocks);
        } else {
            // Baseline has no DEAD work, only PENDING promotion.
            for _ in 0..3 {
                let mut files = Files::new(DeviceStore(&dev), &sb, 1, true).unwrap();
                sb = files.fsync(&dev, &mut mount).unwrap();
            }
            verify(&dev, &sb, shared);
            finals.push(sb.free_blocks);
        }
    }
    eprintln!("reaper deletion finished {:?}", start.elapsed());
    assert_eq!(
        finals[0], finals[1],
        "snapshot deletion returns the baseline free space"
    );
    let resumed = RecordingDevice::new(deletion_image.unwrap()).unwrap();
    let recovered = recover::recover(&resumed, true).unwrap();
    let final_sb = finish(&resumed, recovered, shared);
    assert_eq!(final_sb.free_blocks, finals[1]);
    assert!(dead(&resumed, &final_sb).is_empty());
    assert!(start.elapsed().as_secs_f64() < 60.0);
}

#[test]
fn reap_uses_the_freeing_reserve_and_selects_lowest_address() {
    let start = Instant::now();
    let dev = RecordingDevice::new(test_image::image()).unwrap();
    let mut sb = recover::recover(&dev, false).unwrap();
    let mut mount = Committer::new(sb.clone());
    let mut files = Files::new(DeviceStore(&dev), &sb, 1, true).unwrap();
    files.snapshot(b"a", false).unwrap();
    files.snapshot(b"b", false).unwrap();
    sb = files.fsync(&dev, &mut mount).unwrap();
    for id in [2, 3] {
        let mut files = Files::new(DeviceStore(&dev), &sb, id, true).unwrap();
        files.create(2, b"owned", 0o600, Time::default()).unwrap();
        sb = files.fsync(&dev, &mut mount).unwrap();
    }
    let mut files = Files::new(DeviceStore(&dev), &sb, 1, true).unwrap();
    files.delete_root(b"a").unwrap();
    files.delete_root(b"b").unwrap();
    sb = files.fsync(&dev, &mut mount).unwrap();
    let expected = dead(&dev, &sb);
    assert_eq!(expected.len(), 2);
    let mut txn =
        Transaction::new_owned(DeviceStore(&dev), sb.space_root, sb.generation + 1).unwrap();
    let mut ledger = Ledger::new(sb.free_blocks);
    assert_eq!(ledger.available(sb.free_blocks), 0);
    assert!(ledger
        .reserve(sb.free_blocks, 4, Operation::Allocate)
        .is_err());
    let blocks = txn.reap_reservation_blocks().unwrap();
    let grant = ledger.reserve(sb.free_blocks, blocks, Operation::DeleteSnapshot).unwrap();
    assert!(grant.blocks() > 0);
    for addr in expected {
        assert_eq!(txn.reap_step().unwrap(), Some(addr));
    }
    assert_eq!(txn.dead_count(), 0);
    txn.promote_pending().unwrap();
    sb = mount.commit(&dev, &mut txn, &BTreeMap::new()).unwrap();
    ledger.release(grant);
    test_image::check(&dev.durable_image(), "reaper-reserve");
    assert_eq!(recover::recover(&dev, false).unwrap(), sb);
    assert!(start.elapsed().as_secs_f64() < 60.0);
}
