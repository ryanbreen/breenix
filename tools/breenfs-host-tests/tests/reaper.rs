use breenfs_host_tests::{
    btree,
    commit::{Committer, DeviceStore},
    format::{self, Extent, Root, Superblock, Time, BLOCK_SIZE},
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

const ATTRS: usize = 1340;

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
    ops::items(&DeviceStore(device), sb.space_root)
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
        while let Some(addr) = txn.reap_step().unwrap() {
            // Materialize the transaction's SPACE view only as a test oracle.
            // The production step uses its mount-time address index.
            let space = txn.settle().unwrap();
            let keys: Vec<_> = ops::items(&txn.view(), space)
                .unwrap()
                .into_iter()
                .filter(|(k, _)| k.item_type == format::DEAD)
                .map(|(k, _)| k.object)
                .collect();
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
        )
        .unwrap();
    }
    sb
}

#[test]
fn budgeted_three_level_deletion_recovers_and_returns_space() {
    let start = Instant::now();
    let device = RecordingDevice::new(test_image::image()).unwrap();
    let sb = recover::recover(&device, false).unwrap();
    let mut mount = Committer::new(sb.clone());
    let mut files = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    let ino = files.create(2, b"file", 0o600, Time::default()).unwrap();
    // Separate REFs: keep the first shared, replace the second in live.
    files
        .write(&device, ino, 0, &vec![0x37; BLOCK_SIZE], Time::default())
        .unwrap();
    files
        .write(
            &device,
            ino,
            BLOCK_SIZE as u64,
            &vec![0x45; BLOCK_SIZE],
            Time::default(),
        )
        .unwrap();
    for n in 0..ATTRS {
        files
            .xattr(
                ino,
                format!("attr{n:04}").as_bytes(),
                Some(&vec![0x11; 4096]),
                Time::default(),
            )
            .unwrap();
    }
    let sb = files.fsync(&device, &mut mount).unwrap();
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
        // Every old leaf contains a large xattr, so each is COWed in live.
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
    let grant = txn.reserve_reap(&mut ledger, sb.free_blocks).unwrap();
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
