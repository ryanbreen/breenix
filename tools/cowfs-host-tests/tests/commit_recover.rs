use cowfs_host_tests::{
    block::{BlockDevice, BlockError},
    btree::{self, NodeStore},
    commit::{Committer, Error},
    crc32c,
    format::{self, *},
    model::Model,
    recording::{Event, RecordingDevice},
    txn::Transaction,
};
use std::{
    collections::BTreeMap,
    fs,
    process::Command,
    sync::{Mutex, OnceLock},
};

struct Image(Vec<u8>);
impl NodeStore for Image {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        let lo = addr as usize * BLOCK_SIZE;
        self.0
            .get(lo..lo + NODE_SIZE)
            .map(|b| b.to_vec())
            .ok_or(btree::Error::Store)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
        unreachable!()
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        unreachable!()
    }
}
fn image() -> Vec<u8> {
    static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let p = std::env::temp_dir().join(format!("cowfs-commit-{}.img", std::process::id()));
            let dir = p.with_extension("source");
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join("file"), b"old").unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(dir.join("file"), fs::Permissions::from_mode(0o644)).unwrap();
            let out = Command::new(env!("CARGO_BIN_FILE_COWFS_cowfs-mkfs"))
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
                .arg(&p)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            let bytes = fs::read(&p).unwrap();
            fs::remove_file(p).unwrap();
            fs::remove_dir_all(dir).unwrap();
            bytes
        })
        .clone()
}
fn sb(bytes: &[u8]) -> Superblock {
    Superblock::decode(&bytes[17 * BLOCK_SIZE..18 * BLOCK_SIZE]).unwrap()
}
fn key(ino: u64, ty: u8) -> Key {
    Key {
        object: ino,
        item_type: ty,
        offset: 0,
    }
}
fn items(store: &impl NodeStore, ptr: BlockPtr) -> BTreeMap<Key, Vec<u8>> {
    let bytes = store.get(ptr.addr).unwrap();
    let h = NodeHeader::decode(&bytes).unwrap();
    assert_eq!(
        (h.addr, h.generation, h.level),
        (ptr.addr, ptr.generation, ptr.level)
    );
    assert_eq!(
        ptr.csum,
        u32::from_le_bytes(bytes[8..12].try_into().unwrap())
    );
    let mut out = BTreeMap::new();
    for i in 0..h.nitems as usize {
        if h.level == 0 {
            let p = LeafItemHeader::decode(&bytes[48 + i * 21..69 + i * 21]).unwrap();
            out.insert(
                p.key,
                bytes[p.data_off as usize..p.data_off as usize + p.data_len as usize].to_vec(),
            );
        } else {
            let e = InternalEntry::decode(&bytes[48 + i * 37..85 + i * 37]).unwrap();
            out.extend(items(
                store,
                BlockPtr {
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
fn root(store: &impl NodeStore, sb: &Superblock) -> Root {
    items(store, sb.space_root)
        .into_iter()
        .find_map(|(k, v)| (k.item_type == ROOT).then(|| Root::decode(&v).unwrap()))
        .unwrap()
}
struct Expected {
    tree: BTreeMap<Key, Vec<u8>>,
    file: Vec<u8>,
}
fn snapshot(bytes: &[u8], sb: &Superblock) -> Expected {
    let store = Image(bytes.to_vec());
    let tree = items(&store, root(&store, sb).root);
    let inode = Inode::decode(&tree[&key(16, INODE)]).unwrap();
    let extent = Extent::decode(&tree[&key(16, EXTENT)]).unwrap();
    let lo = (extent.phys_start + u64::from(extent.extent_offset)) as usize * BLOCK_SIZE;
    Expected {
        tree,
        file: bytes[lo..lo + inode.size as usize].to_vec(),
    }
}
fn stage<'a>(
    base: &'a Image,
    sb: &Superblock,
    payload: &[u8],
) -> (Transaction<'a, Image>, BTreeMap<u64, Vec<u8>>, Expected) {
    let mut r = root(base, sb);
    let mut txn = Transaction::new(base, sb.space_root, sb.generation + 1).unwrap();
    let mut data = vec![0; BLOCK_SIZE];
    data[..payload.len()].copy_from_slice(payload);
    let addr = txn.allocate_data(&[crc32c::checksum(&data)]).unwrap();
    txn.insert(
        &mut r.root,
        key(16, EXTENT),
        &Extent {
            phys_start: addr,
            phys_len: 1,
            extent_offset: 0,
            num_blocks: 1,
            flags: 0,
        }
        .encode(),
    )
    .unwrap();
    let mut inode = Inode::decode(
        &btree::search(&txn.view(), r.root, key(16, INODE))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    inode.size = payload.len() as u64;
    inode.nblocks = 1;
    txn.insert(&mut r.root, key(16, INODE), &inode.encode())
        .unwrap();
    txn.update_root(1, r.clone()).unwrap();
    // Force an internal FS root to exercise bottom-up header/pointer filling.
    for n in 0..4 {
        let name = format!("user.key{n}").into_bytes();
        let value = Xattr {
            name: name.clone(),
            value: vec![n as u8; 4096],
        }
        .encode();
        txn.insert(
            &mut r.root,
            Key {
                offset: cowfs_host_tests::siphash::hash(&sb.dirhash_key, &name),
                ..key(16, XATTR)
            },
            &value,
        )
        .unwrap();
    }
    txn.update_root(1, r.clone()).unwrap();
    let expected = Expected {
        tree: items(&txn.view(), r.root),
        file: payload.to_vec(),
    };
    (txn, BTreeMap::from([(addr, data)]), expected)
}
fn assert_snapshot(bytes: &[u8], sb: &Superblock, expected: &Expected) {
    let actual = snapshot(bytes, sb);
    assert_eq!(actual.tree, expected.tree);
    assert_eq!(actual.file, expected.file);
    // Independent POSIX oracle plus an item decoder compare logical metadata,
    // directory entries, file bytes and xattrs, as well as the physical tree.
    let mut model = Model::new();
    let ino = model.create(2, b"file", 0o644, Default::default()).unwrap();
    model
        .write(ino, 0, &expected.file, Default::default())
        .unwrap();
    let mut read = vec![0; expected.file.len() + 1];
    let n = model.read(ino, 0, &mut read).unwrap();
    assert_eq!(actual.file, read[..n]);
    for (k, value) in &expected.tree {
        if k.item_type == XATTR {
            let x = Xattr::decode(value).unwrap();
            model
                .setxattr(k.object, &x.name, &x.value, Default::default())
                .unwrap();
        }
    }
    use cowfs_host_tests::{Contents, Fs, Metadata, State, Time};
    let time = |t: format::Time| Time {
        seconds: t.seconds,
        nanos: t.nanos,
    };
    let mut state = State::default();
    for (k, value) in &actual.tree {
        if k.item_type != INODE {
            continue;
        }
        let i = Inode::decode(value).unwrap();
        let contents = if i.mode & 0o170000 == 0o040000 {
            let entries = actual
                .tree
                .iter()
                .filter(|(d, _)| d.object == k.object && d.item_type == DIR_INDEX)
                .map(|(_, v)| {
                    let d = DirRecord::decode(v).unwrap();
                    (d.name, d.ino)
                })
                .collect();
            Contents::Directory {
                parent: i.parent,
                entries,
            }
        } else {
            Contents::File {
                size: i.size,
                bytes: actual
                    .file
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(_, b)| *b != 0)
                    .map(|(n, b)| (n as u64, b))
                    .collect(),
            }
        };
        let xattrs = actual
            .tree
            .iter()
            .filter(|(x, _)| x.object == k.object && x.item_type == XATTR)
            .map(|(_, v)| {
                let x = Xattr::decode(v).unwrap();
                (x.name, x.value)
            })
            .collect();
        state.inodes.insert(
            k.object,
            cowfs_host_tests::Inode {
                metadata: Metadata {
                    mode: i.mode,
                    uid: i.uid,
                    gid: i.gid,
                    nlink: i.nlink,
                    atime: time(i.atime),
                    mtime: time(i.mtime),
                    ctime: time(i.ctime),
                    btime: time(i.btime),
                },
                contents,
                xattrs,
            },
        );
    }
    assert_eq!(state, model.state().unwrap());
}
fn visible(dev: &dyn BlockDevice) -> Vec<u8> {
    let mut bytes = vec![0; dev.num_blocks() as usize * dev.block_size()];
    for (n, b) in bytes.chunks_mut(dev.block_size()).enumerate() {
        dev.read_block(n as u64, b).unwrap();
    }
    bytes
}
#[derive(Clone, Copy, Debug)]
enum Fault {
    Data,
    FirstFlush,
    RingA,
    RingB,
    FinalFlush(bool),
}
struct FaultState {
    flushes: usize,
    failed: bool,
    calls: Vec<(bool, u64)>,
}
struct Failing {
    device: RecordingDevice,
    fault: Fault,
    state: Mutex<FaultState>,
}
impl Failing {
    fn new(bytes: Vec<u8>, fault: Fault) -> Self {
        Self {
            device: RecordingDevice::new(bytes).unwrap(),
            fault,
            state: Mutex::new(FaultState {
                flushes: 0,
                failed: false,
                calls: Vec::new(),
            }),
        }
    }
    fn calls(&self) -> Vec<(bool, u64)> {
        self.state.lock().unwrap().calls.clone()
    }
}
impl BlockDevice for Failing {
    fn read_block(&self, n: u64, b: &mut [u8]) -> Result<(), BlockError> {
        self.device.read_block(n, b)
    }
    fn write_block(&self, n: u64, b: &[u8]) -> Result<(), BlockError> {
        let mut s = self.state.lock().unwrap();
        assert!(!s.failed, "write after failed publication");
        s.calls.push((true, n));
        let ring_b = self.device.num_blocks() - 32;
        let fail = match self.fault {
            Fault::Data => s.flushes == 0,
            Fault::RingA => n == 18 * 8 + 7,
            Fault::RingB => n == ring_b + 2 * 8 + 7,
            _ => false,
        };
        self.device.write_block(n, b)?;
        if fail {
            s.failed = true;
            Err(BlockError::IoError)
        } else {
            Ok(())
        }
    }
    fn block_size(&self) -> usize {
        self.device.block_size()
    }
    fn num_blocks(&self) -> u64 {
        self.device.num_blocks()
    }
    fn flush(&self) -> Result<(), BlockError> {
        let mut s = self.state.lock().unwrap();
        assert!(!s.failed, "flush after failed publication");
        s.flushes += 1;
        let count = s.flushes;
        s.calls.push((false, count as u64));
        let fail = matches!(
            (self.fault, count),
            (Fault::FirstFlush, 1) | (Fault::FinalFlush(_), 2)
        );
        if !fail || matches!(self.fault, Fault::FinalFlush(true)) {
            self.device.flush()?;
        }
        if fail {
            s.failed = true;
            Err(BlockError::IoError)
        } else {
            Ok(())
        }
    }
}
mod commit {
    use super::*;
    #[test]
    fn obeys_w1_w3() {
        let bytes = image();
        let initial = sb(&bytes);
        let dev = RecordingDevice::new(bytes.clone()).unwrap();
        let mut mount = Committer::new(initial.clone());
        let base = Image(bytes);
        let (mut txn, data, expected) = stage(&base, &initial, b"new");
        // A newly detached FS leaf remains owned by DEAD until its reaper.
        let dead = txn.create_tree(initial.fs_uuid).unwrap();
        txn.release(dead.addr, 4, true).unwrap();
        let a = mount.waiter();
        let b = mount.waiter();
        assert_eq!(a.result(), None);
        let committed = mount.commit(&dev, &mut txn, &data).unwrap();
        assert_eq!(a.result(), Some(Ok(())));
        assert_eq!(b.result(), Some(Ok(())));
        assert_eq!(dev.durable_generations(), vec![1, 2]);
        assert_eq!(committed.slot, 2);
        assert_snapshot(&dev.durable_image(), &committed, &expected);
        let events = dev.events();
        let flush: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(i, e)| matches!(e, Event::Flush).then_some(i))
            .collect();
        assert_eq!(flush.len(), 2);
        assert_eq!(flush[1], events.len() - 1);
        let sectors: Vec<_> = events[flush[0] + 1..flush[1]]
            .iter()
            .map(|e| match e {
                Event::Write { sector, .. } => *sector,
                _ => panic!("unexpected flush"),
            })
            .collect();
        assert_eq!(
            sectors,
            (18 * 8..19 * 8)
                .chain((initial.total_blocks - 2) * 8..(initial.total_blocks - 1) * 8)
                .collect::<Vec<_>>()
        );
        // A second generation uses the same mount and protects both old trees.
        let bytes = dev.durable_image();
        let base = Image(bytes);
        let (mut txn, data, expected) = stage(&base, &committed, b"third");
        let committed = mount.commit(&dev, &mut txn, &data).unwrap();
        assert_snapshot(&dev.durable_image(), &committed, &expected);
        assert_eq!(dev.durable_generations(), vec![2, 3]);
    }
    fn failure(fault: Fault) -> Failing {
        let bytes = image();
        let initial = sb(&bytes);
        let base = Image(bytes.clone());
        let dev = Failing::new(bytes, fault);
        let mut mount = Committer::new(initial.clone());
        let (mut txn, data, _) = stage(&base, &initial, b"new");
        let waiters = [mount.waiter(), mount.waiter()];
        assert_eq!(mount.commit(&dev, &mut txn, &data), Err(Error::EIO));
        assert!(mount.read_only());
        for w in waiters {
            assert_eq!(w.result(), Some(Err(Error::EIO)));
        }
        assert_eq!(mount.waiter().result(), Some(Err(Error::EIO)));
        let calls = dev.calls();
        assert_eq!(mount.commit(&dev, &mut txn, &data), Err(Error::EIO));
        assert_eq!(dev.calls(), calls);
        dev
    }
    #[test]
    fn failure_before_publication_writes_no_superblock() {
        for f in [Fault::Data, Fault::FirstFlush] {
            let dev = failure(f);
            assert!(dev
                .calls()
                .iter()
                .filter(|(w, _)| *w)
                .all(|(_, n)| !(16 * 8..20 * 8).contains(n) && *n < dev.num_blocks() - 32));
            assert_eq!(dev.device.durable_generations(), vec![1]);
        }
    }
    #[test]
    fn failure_during_publication_stops_all_writes() {
        for f in [
            Fault::RingA,
            Fault::RingB,
            Fault::FinalFlush(false),
            Fault::FinalFlush(true),
        ] {
            let dev = failure(f);
            assert!(dev.state.lock().unwrap().failed);
        }
    }
}
mod recover {
    use super::*;
    #[test]
    fn after_publication_failure_chooses_a_consistent_generation() {
        for fault in [
            Fault::RingA,
            Fault::FinalFlush(false),
            Fault::FinalFlush(true),
        ] {
            let bytes = image();
            let old = sb(&bytes);
            let old_state = snapshot(&bytes, &old);
            let base = Image(bytes.clone());
            let dev = Failing::new(bytes, fault);
            let mut mount = Committer::new(old.clone());
            let (mut txn, data, new_state) = stage(&base, &old, b"new");
            assert_eq!(mount.commit(&dev, &mut txn, &data), Err(Error::EIO));
            // The flushed image and completed volatile sectors can both survive.
            for bytes in [dev.device.durable_image(), visible(&dev)] {
                let recovery = RecordingDevice::new(bytes.clone()).unwrap();
                let chosen = cowfs_host_tests::recover::recover(&recovery, false).unwrap();
                assert!((1..=2).contains(&chosen.generation));
                assert_snapshot(
                    &bytes,
                    &chosen,
                    if chosen.generation == 1 {
                        &old_state
                    } else {
                        &new_state
                    },
                );
            }
        }
    }
    fn committed() -> (Vec<u8>, Superblock, Expected) {
        let bytes = image();
        let old = sb(&bytes);
        let base = Image(bytes.clone());
        let dev = RecordingDevice::new(bytes).unwrap();
        let (mut txn, data, expected) = stage(&base, &old, b"new");
        let sb = Committer::new(old).commit(&dev, &mut txn, &data).unwrap();
        (dev.durable_image(), sb, expected)
    }
    #[test]
    fn read_error_preserves_acknowledged_slots() {
        struct ReadFault {
            inner: RecordingDevice,
            addr: u64,
        }
        impl BlockDevice for ReadFault {
            fn read_block(&self, n: u64, b: &mut [u8]) -> Result<(), BlockError> {
                if n == self.addr * 8 {
                    Err(BlockError::IoError)
                } else {
                    self.inner.read_block(n, b)
                }
            }
            fn write_block(&self, n: u64, b: &[u8]) -> Result<(), BlockError> {
                self.inner.write_block(n, b)
            }
            fn block_size(&self) -> usize { 512 }
            fn num_blocks(&self) -> u64 { self.inner.num_blocks() }
            fn flush(&self) -> Result<(), BlockError> { self.inner.flush() }
        }
        let (bytes, newest, _) = committed();
        for addr in [newest.space_root.addr, root(&Image(bytes.clone()), &newest).root.addr] {
            for writable in [false, true] {
                let dev = ReadFault {
                    inner: RecordingDevice::new(bytes.clone()).unwrap(),
                    addr,
                };
                assert_eq!(cowfs_host_tests::recover::recover(&dev, writable), Err(Error::EIO));
                assert!(dev.inner.events().is_empty());
                assert_eq!(dev.inner.durable_image(), bytes);
            }
        }
    }
    #[test]
    fn chooses_highest_matching_generation() {
        let (bytes, new, expected) = committed();
        let dev = RecordingDevice::new(bytes.clone()).unwrap();
        assert_eq!(
            cowfs_host_tests::recover::recover(&dev, false)
                .unwrap()
                .generation,
            2
        );
        assert_snapshot(&bytes, &new, &expected);
        // Reject a newer slot's mismatching SPACE pointer and its ROOT pointer.
        for fs_root in [false, true] {
            let mut damaged = bytes.clone();
            let addr = if fs_root {
                root(&Image(bytes.clone()), &new).root.addr
            } else {
                new.space_root.addr
            };
            damaged[addr as usize * BLOCK_SIZE + 40] ^= 1;
            let dev = RecordingDevice::new(damaged).unwrap();
            assert_eq!(
                cowfs_host_tests::recover::recover(&dev, false)
                    .unwrap()
                    .generation,
                1
            );
            assert!(dev.events().is_empty());
        }
    }
    #[test]
    fn zeroes_abandoned_slots_first() {
        let (mut bytes, new, _) = committed();
        bytes[new.space_root.addr as usize * BLOCK_SIZE + 40] ^= 1;
        let dev = RecordingDevice::new(bytes.clone()).unwrap();
        let chosen = cowfs_host_tests::recover::recover(&dev, true).unwrap();
        assert_eq!(chosen.generation, 1);
        let events = dev.events();
        assert_eq!(events.last(), Some(&Event::Flush));
        assert_eq!(events.len(), 17);
        for e in &events[..16] {
            assert!(matches!(e,Event::Write{bytes,..} if *bytes == [0;512]));
        }
        let durable = dev.durable_image();
        for block in [18, new.total_blocks - 2] {
            assert!(
                durable[block as usize * BLOCK_SIZE..(block as usize + 1) * BLOCK_SIZE]
                    .iter()
                    .all(|b| *b == 0)
            );
        }
        let base = Image(durable);
        let (mut txn, data, _) = stage(&base, &chosen, b"replacement");
        Committer::new(chosen)
            .commit(&dev, &mut txn, &data)
            .unwrap();
        assert_eq!(dev.events()[16], Event::Flush);
        // A cleanup error refuses the writable mount before any commit can run.
        let dev = Failing::new(bytes, Fault::FirstFlush);
        assert_eq!(
            cowfs_host_tests::recover::recover(&dev, true),
            Err(Error::EIO)
        );
    }
    #[test]
    fn uuid_mismatch_refuses() {
        // Recorder starts with one UUID; a read override models a foreign valid
        // slot without asking its writer oracle to accept a foreign publication.
        struct Foreign {
            inner: RecordingDevice,
            slot: [u8; BLOCK_SIZE],
        }
        impl BlockDevice for Foreign {
            fn read_block(&self, n: u64, b: &mut [u8]) -> Result<(), BlockError> {
                if (18 * 8..19 * 8).contains(&n) {
                    let at = (n - 18 * 8) as usize * 512;
                    b.copy_from_slice(&self.slot[at..at + 512]);
                    Ok(())
                } else {
                    self.inner.read_block(n, b)
                }
            }
            fn write_block(&self, n: u64, b: &[u8]) -> Result<(), BlockError> {
                self.inner.write_block(n, b)
            }
            fn block_size(&self) -> usize {
                512
            }
            fn num_blocks(&self) -> u64 {
                self.inner.num_blocks()
            }
            fn flush(&self) -> Result<(), BlockError> {
                self.inner.flush()
            }
        }
        let bytes = image();
        let mut foreign = sb(&bytes);
        foreign.fs_uuid = [9; 16];
        foreign.generation = 2;
        foreign.slot = 2;
        let dev = Foreign {
            inner: RecordingDevice::new(bytes).unwrap(),
            slot: foreign.encode(),
        };
        assert_eq!(
            cowfs_host_tests::recover::recover(&dev, true),
            Err(Error::UuidMismatch)
        );
        assert!(dev.inner.events().is_empty());
    }
}
