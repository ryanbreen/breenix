use breenfs_host_tests::{
    btree::{self, NodeStore},
    commit::{Committer, DeviceStore},
    format::{self, Key, Ref, Superblock, BLOCK_SIZE, NODE_SIZE},
    ops::Files,
    recording::RecordingDevice,
    recover,
    scrub::{BadAddress, Kind, Scrub, Store},
};
use std::{cell::{Cell, RefCell}, collections::BTreeMap, fs, process::Command, sync::OnceLock};

struct ReadStore<'a> {
    base: DeviceStore<'a>,
    flips: BTreeMap<u64, usize>,
    data_reads: RefCell<Vec<u64>>,
    io: Cell<usize>,
}
impl NodeStore for ReadStore<'_> {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.io.set(self.io.get() + NODE_SIZE);
        let mut bytes = self.base.get(addr)?;
        if let Some(&offset) = self.flips.get(&addr) {
            bytes[offset] ^= 1;
        }
        Ok(bytes)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
        unreachable!()
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        unreachable!()
    }
}
impl Store for ReadStore<'_> {
    fn data(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.io.set(self.io.get() + BLOCK_SIZE);
        self.data_reads.borrow_mut().push(addr);
        let mut bytes = Store::data(&self.base, addr)?;
        if let Some(&offset) = self.flips.get(&addr) {
            bytes[offset] ^= 1;
        }
        Ok(bytes)
    }
}
fn store(dev: &RecordingDevice) -> ReadStore<'_> {
    ReadStore {
        base: DeviceStore(dev),
        flips: BTreeMap::new(),
        data_reads: RefCell::new(Vec::new()),
        io: Cell::new(0),
    }
}
fn image() -> Vec<u8> {
    static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("breenfs-scrub-{}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            // One multi-block REF, large enough to pause inside it and cap bad-address reporting.
            fs::write(dir.join("file"), vec![0x39; 70 * BLOCK_SIZE]).unwrap();
            let path = dir.with_extension("img");
            let output = Command::new(env!("CARGO_BIN_FILE_BREENFS_breenfs-mkfs"))
                .args([
                    "--size",
                    "67108864",
                    "--time",
                    "0",
                    "--uuid",
                    "00112233445566778899aabbccddeeff",
                    "--hash-key",
                    "000102030405060708090a0b0c0d0e0f",
                    "--populate",
                ])
                .arg(&dir)
                .arg(&path)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let bytes = fs::read(&path).unwrap();
            fs::remove_file(path).unwrap();
            fs::remove_dir_all(dir).unwrap();
            bytes
        })
        .clone()
}
fn allocations(s: &impl NodeStore, sb: &Superblock) -> Vec<(Key, Ref)> {
    btree::range(
        s,
        sb.space_root,
        Key {
            object: 0,
            item_type: 0,
            offset: 0,
        },
        Key {
            object: u64::MAX,
            item_type: u8::MAX,
            offset: u64::MAX,
        },
    )
    .unwrap()
    .into_iter()
    .filter(|(k, _)| k.item_type == format::REF)
    .map(|(k, b)| (k, Ref::decode(&b).unwrap()))
    .collect()
}
fn finish(scrub: &mut Scrub, s: &impl Store, sb: &Superblock) {
    for _ in 0..512 {
        if scrub.completion().is_some() {
            return;
        }
        assert!(
            scrub
                .step(s, sb.space_root, sb.generation, NODE_SIZE)
                .unwrap()
                <= NODE_SIZE
        );
    }
    panic!("scrub did not finish");
}
fn check(dev: &RecordingDevice, tag: &str) {
    let path = std::env::temp_dir().join(format!(
        "breenfs-scrub-check-{}-{tag}.img",
        std::process::id()
    ));
    fs::write(&path, dev.durable_image()).unwrap();
    let out = Command::new(env!("CARGO_BIN_FILE_BREENFS_breenfs-check"))
        .arg("--full")
        .arg(&path)
        .output()
        .unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}
#[test]
fn visits_allocations_once_and_names_corruption_without_writing() {
    let dev = RecordingDevice::new(image()).unwrap();
    let mut sb = recover::recover(&dev, false).unwrap();
    let mut committer = Committer::new(sb.clone());
    // DEAD is an allocation too, even though no root points at it.
    let mut txn = breenfs_host_tests::txn::Transaction::new_owned(
        DeviceStore(&dev),
        sb.space_root,
        sb.generation + 1,
    )
    .unwrap();
    let dead = txn.create_tree(sb.fs_uuid).unwrap();
    txn.release(dead.addr, 4, true).unwrap();
    sb = committer.commit(&dev, &mut txn, &BTreeMap::new()).unwrap();
    drop(txn);
    let s = store(&dev);
    let refs = allocations(&s, &sb);
    let expected_blocks = refs.iter().map(|(k, _)| k.offset).sum::<u64>() + 4;
    let expected_data: Vec<_> = refs
        .iter()
        .filter(|(_, r)| r.kind == 3)
        .flat_map(|(k, _)| k.object..k.object + k.offset)
        .collect();
    let node = refs.iter().find(|(_, r)| r.kind == 1).unwrap().0.object;
    let events = dev.events();
    let mut scrub = Scrub::new();
    assert_eq!(scrub.step(&s, sb.space_root, sb.generation, 0).unwrap(), 0);
    finish(&mut scrub, &s, &sb);
    assert_eq!(scrub.errors(), 0);
    assert_eq!(scrub.blocks(), expected_blocks);
    assert_eq!(*s.data_reads.borrow(), expected_data);
    assert_eq!(scrub.completion().unwrap().generation, sb.generation);
    let reads = s.data_reads.borrow().len();
    assert_eq!(
        scrub
            .step(&s, sb.space_root, sb.generation, NODE_SIZE)
            .unwrap(),
        0
    );
    assert_eq!(s.data_reads.borrow().len(), reads);
    for (addr, kind) in [
        (expected_data[0], Kind::Data),
        (node, Kind::Node),
        (dead.addr, Kind::Node),
        (sb.space_root.addr, Kind::Node),
    ] {
        let mut bad = store(&dev);
        bad.flips.insert(addr, 80);
        let mut scrub = Scrub::new();
        finish(&mut scrub, &bad, &sb);
        assert_eq!(scrub.errors(), 1);
        assert_eq!(
            scrub.bad(),
            &[BadAddress {
                address: addr,
                kind
            }]
        );
    }
    // A valid CRC cannot hide a misdirected or stale node header.
    for offset in [32, 40] {
        struct HeaderFault<'a> {
            s: ReadStore<'a>,
            addr: u64,
            offset: usize,
        }
        impl NodeStore for HeaderFault<'_> {
            fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
                let mut b = self.s.get(addr)?;
                if addr == self.addr {
                    b[self.offset] ^= 1;
                    let crc = breenfs_host_tests::crc32c::checksum(&b[12..]);
                    b[8..12].copy_from_slice(&crc.to_le_bytes());
                }
                Ok(b)
            }
            fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
                unreachable!()
            }
            fn alloc(&mut self) -> Result<u64, btree::Error> {
                unreachable!()
            }
        }
        impl Store for HeaderFault<'_> {
            fn data(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
                self.s.data(addr)
            }
        }
        let bad = HeaderFault {
            s: store(&dev),
            addr: node,
            offset,
        };
        let mut scrub = Scrub::new();
        finish(&mut scrub, &bad, &sb);
        assert_eq!(
            scrub.bad(),
            &[BadAddress {
                address: node,
                kind: Kind::Node
            }]
        );
    }
    let mut bad = store(&dev);
    for &addr in &expected_data[..65] {
        bad.flips.insert(addr, 80);
    }
    let mut scrub = Scrub::new();
    finish(&mut scrub, &bad, &sb);
    assert_eq!(scrub.errors(), 65);
    assert_eq!(scrub.bad().len(), 64);
    assert_eq!(
        scrub.bad().iter().map(|b| b.address).collect::<Vec<_>>(),
        expected_data[..64]
    );
    assert_eq!(dev.events(), events);
    assert!(!committer.read_only());
    check(&dev, "populated");
}
#[test]
fn each_batch_uses_the_latest_durable_allocation_after_reuse() {
    let dev = RecordingDevice::new(image()).unwrap();
    let mut sb = recover::recover(&dev, false).unwrap();
    let mut committer = Committer::new(sb.clone());
    let s = store(&dev);
    let refs = allocations(&s, &sb);
    let old: Vec<_> = refs
        .iter()
        .filter(|(_, r)| r.kind == 3)
        .flat_map(|(k, _)| k.object..k.object + k.offset)
        .collect();
    let mut scrub = Scrub::new();
    while s.data_reads.borrow().is_empty() {
        scrub
            .step(&s, sb.space_root, sb.generation, NODE_SIZE)
            .unwrap();
    }
    // Stop inside the long data REF, then replace it through real commits.
    assert!(s.data_reads.borrow().len() < old.len());
    let reads_before = s.data_reads.borrow().len();
    let unread: Vec<_> = old
        .iter()
        .copied()
        .filter(|a| !s.data_reads.borrow().contains(a))
        .collect();
    for n in 0..5 {
        let mut files = Files::new(DeviceStore(&dev), &sb, 1, true).unwrap();
        files.truncate(&dev, 16, 0, Default::default()).unwrap();
        files
            .write(
                &dev,
                16,
                0,
                &vec![0x61 + n; 70 * BLOCK_SIZE],
                Default::default(),
            )
            .unwrap();
        sb = files.fsync(&dev, &mut committer).unwrap();
        drop(files);
        // Promotion happens at commit, after allocation. Stop on the first
        // replacement that actually reuses the old run, before a later
        // replacement can release it again. The reuse assertion below stays exact.
        if allocations(&s, &sb).into_iter().any(|(k, r)| {
            r.kind == 3 && (k.object..k.object + k.offset).any(|a| unread.contains(&a))
        }) {
            break;
        }
    }
    let reused = allocations(&s, &sb)
        .into_iter()
        .any(|(k, r)| r.kind == 3 && (k.object..k.object + k.offset).any(|a| unread.contains(&a)));
    assert!(
        reused,
        "the fixture must actually overwrite formerly scrubbed allocations"
    );
    finish(&mut scrub, &s, &sb);
    assert!(
        s.data_reads.borrow()[reads_before..]
            .iter()
            .any(|a| unread.contains(a)),
        "the resumed scanner must read the reused, previously unscanned tail"
    );
    assert_eq!(scrub.errors(), 0);
    assert_eq!(scrub.completion().unwrap().generation, sb.generation);
    check(&dev, "reused");
}
#[test]
fn completion_is_recorded_by_the_next_commit_and_survives_later_commit_and_recovery() {
    let dev = RecordingDevice::new(image()).unwrap();
    let mut sb = recover::recover(&dev, false).unwrap();
    let mut s = store(&dev);
    let addr = allocations(&s, &sb)
        .into_iter()
        .find(|(_, r)| r.kind == 3)
        .unwrap()
        .0
        .object;
    s.flips.insert(addr, 80);
    let mut scrub = Scrub::new();
    finish(&mut scrub, &s, &sb);
    let done = scrub.completion().unwrap();
    assert_eq!(done.errors, 1);
    let mut committer = Committer::new(sb.clone());
    committer.record_scrub(done.generation, done.errors);
    assert_eq!(
        recover::recover(&dev, false).unwrap().last_scrub_generation,
        0
    );
    for n in 0..2 {
        let mut txn = breenfs_host_tests::txn::Transaction::new_owned(
            DeviceStore(&dev),
            sb.space_root,
            sb.generation + 1,
        )
        .unwrap();
        sb = committer.commit(&dev, &mut txn, &BTreeMap::new()).unwrap();
        assert_eq!(
            (sb.last_scrub_generation, sb.last_scrub_errors),
            (done.generation, 1)
        );
        drop(txn);
        check(&dev, &format!("record-{n}"));
    }
    let remounted = RecordingDevice::new(dev.durable_image()).unwrap();
    let recovered = recover::recover(&remounted, true).unwrap();
    assert_eq!(
        (recovered.last_scrub_generation, recovered.last_scrub_errors),
        (done.generation, 1)
    );
    // The split prepare/publish path used by the kernel preserves the same fields.
    let mut committer = Committer::new(recovered.clone());
    let mut files = Files::new(DeviceStore(&remounted), &recovered, 1, true).unwrap();
    let prepared = committer.prepare_files(&remounted, &mut files).unwrap();
    assert_eq!(
        (
            prepared.superblock().last_scrub_generation,
            prepared.superblock().last_scrub_errors
        ),
        (done.generation, 1)
    );
}

#[test]
fn each_step_bounds_and_charges_all_device_reads_including_navigation() {
    let dev = RecordingDevice::new(image()).unwrap();
    let sb = recover::recover(&dev, false).unwrap();
    let s = store(&dev);
    let mut scrub = Scrub::new();
    for _ in 0..512 {
        s.io.set(0);
        let charged = scrub.step(&s, sb.space_root, sb.generation, NODE_SIZE).unwrap();
        assert!(s.io.get() <= NODE_SIZE, "mount exclusion covered {} raw bytes", s.io.get());
        assert_eq!(charged, s.io.get(), "navigation I/O must be charged");
        if scrub.completion().is_some() {
            assert_eq!(scrub.errors(), 0);
            assert_eq!(s.data_reads.borrow().len(), 70);
            return;
        }
    }
    panic!("bounded scanner failed to progress");
}
