//! Writable namespace checks through the production FileSystem boundary.
use crate::{
    attrs::InodeAttrs,
    block::cache::BufferCache,
    breenfs::{
        commit::{Committer, DeviceStore},
        format::{self, Superblock, Time},
        ops::Files,
        recover,
        vfs::BreenFs,
    },
    errno::Errno,
    read_conformance::{Disk, NoWait},
    syscall_errno::*,
    vfs::*,
};
use std::sync::{Arc, Mutex};

fn fixture() -> (Arc<Disk>, u64) {
    let disk = Arc::new(Disk {
        bytes: Mutex::new(vec![0; 64 * 1024 * 1024]),
        writable: true,
    });
    let sb = Superblock {
        fs_uuid: [0x73; 16],
        generation: 1,
        slot: 1,
        total_blocks: 16384,
        global_reserve: 4096,
        next_root_id: 2,
        dirhash_key: [0x31; 16],
        ..Superblock::default()
    };
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let ino = files.create(2, b"seed", 0o640, Time::default()).unwrap();
    files
        .write(
            disk.as_ref(),
            ino,
            0,
            &vec![0x5a; 256 * 4096],
            Time::default(),
        )
        .unwrap();
    files
        .fsync(disk.as_ref(), &mut Committer::format(sb))
        .unwrap();
    drop(files);
    (disk, ino)
}
fn mount(disk: Arc<Disk>) -> Arc<BreenFs<NoWait>> {
    BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 71, 1).unwrap()
}
fn remove(fs: &dyn FileSystem, dir: u64, name: &[u8], directory: bool) -> Result<(), Errno> {
    let ino = fs.lookup(dir, name)?;
    fs.remove(
        dir,
        name,
        Remove {
            directory,
            expected: ino,
            owner: fs.getattr(ino)?.attrs.uid,
            parent: fs.getattr(dir)?.attrs,
        },
    )
}
// Extended-attribute absence uses ABI errno 61 (no shared constant yet).
const ENODATA: i32 = 61;
const ATTRS: InodeAttrs = InodeAttrs::new(0o6750, 70000, 80000);
const LINK: &LinkCheck<'_> = &|_, _| Ok(());
const RENAME: &RenameCheck<'_> = &|_, _, _, _| Ok(());

#[test]
fn breenfs_namespace_round_trips_a_remount() {
    let (disk, seed) = fixture();
    let fs = mount(disk.clone());
    assert_eq!(fs.read_only(), Ok(false));
    assert!(!fs.statfs().unwrap().flags.read_only);
    let dir = fs
        .create(2, b"dir", NewNode::Directory { attrs: ATTRS })
        .unwrap();
    let child = fs
        .create(dir, b"child", NewNode::Directory { attrs: ATTRS })
        .unwrap();
    let nodes = [
        (
            b"file".as_slice(),
            NewNode::Regular { attrs: ATTRS },
            NodeKind::Regular,
            0,
        ),
        (
            b"symlink",
            NewNode::Symlink {
                attrs: ATTRS,
                target: b"../seed",
            },
            NodeKind::Symlink,
            0,
        ),
        (
            b"char",
            NewNode::CharDevice {
                attrs: ATTRS,
                rdev: 0x1234,
            },
            NodeKind::CharDevice,
            0x1234,
        ),
        (
            b"block",
            NewNode::BlockDevice {
                attrs: ATTRS,
                rdev: 0x123456789,
            },
            NodeKind::BlockDevice,
            0x123456789,
        ),
        (b"fifo", NewNode::Fifo { attrs: ATTRS }, NodeKind::Fifo, 0),
        (
            b"socket",
            NewNode::Socket { attrs: ATTRS },
            NodeKind::Socket,
            0,
        ),
    ];
    let mut expected = Vec::new();
    for (name, new, kind, rdev) in nodes {
        let ino = fs.create(dir, name, new).unwrap();
        expected.push((name, ino, kind, rdev));
        assert_eq!(
            fs.getattr(ino).unwrap().attrs,
            InodeAttrs::new(ATTRS.mode | mode(kind), ATTRS.uid, ATTRS.gid)
        );
    }
    assert_eq!(fs.readlink(expected[1].1).unwrap(), b"../seed");
    for kind in [NodeKind::Regular, NodeKind::Socket] {
        let name = if kind == NodeKind::Regular {
            b"opened".as_slice()
        } else {
            b"opened-socket"
        };
        let mut opened = None;
        fs.create_open(
            dir,
            name,
            kind,
            &|parent| {
                assert_eq!(parent.ino, dir);
                Ok(ATTRS)
            },
            &mut |node| {
                opened = Some(node);
                Ok(())
            },
        )
        .unwrap();
        let node = opened.unwrap();
        assert_eq!(fs.lookup(dir, name), Ok(node.ino));
        assert!(Arc::ptr_eq(&node, &fs.open(node.ino).unwrap()));
    }
    assert_eq!(
        fs.create_open(
            dir,
            b"failed",
            NodeKind::Regular,
            &|_| Ok(ATTRS),
            &mut |_| Err(Errno(ENOMEM))
        ),
        Err(Errno(ENOMEM))
    );
    assert_eq!(fs.lookup(dir, b"failed"), Err(Errno(ENOENT)));
    assert_eq!(
        fs.link(seed, dir, b"denied", &|_, _| Err(Errno(EACCES))),
        Err(Errno(EACCES))
    );
    fs.link(seed, dir, b"alias", LINK).unwrap();
    assert_eq!(fs.getattr(seed).unwrap().nlink, 2);
    assert_eq!(fs.link(dir, 2, b"bad", LINK), Err(Errno(EPERM)));
    assert_eq!(remove(fs.as_ref(), 2, b"dir", true), Err(Errno(ENOTEMPTY)));
    assert_eq!(remove(fs.as_ref(), 2, b"dir", false), Err(Errno(EISDIR)));
    let stale = Remove {
        directory: false,
        expected: seed,
        owner: 1,
        parent: fs.getattr(dir).unwrap().attrs,
    };
    assert_eq!(fs.remove(dir, b"alias", stale), Err(Errno(EAGAIN)));
    assert_eq!(
        fs.rename(2, b"dir", child, b"loop", RenameMode::Replace, RENAME),
        Err(Errno(EINVAL))
    );
    assert_eq!(
        fs.rename(dir, b"alias", dir, b"file", RenameMode::NoReplace, RENAME),
        Err(Errno(EEXIST))
    );
    assert_eq!(
        fs.rename(
            dir,
            b"alias",
            dir,
            b"new",
            RenameMode::Replace,
            &|_, _, _, _| Err(Errno(EACCES))
        ),
        Err(Errno(EACCES))
    );
    fs.rename(dir, b"alias", dir, b"file", RenameMode::Replace, RENAME)
        .unwrap();
    assert_eq!(fs.lookup(dir, b"file"), Ok(seed));
    assert_eq!(fs.lookup(dir, b"alias"), Err(Errno(ENOENT)));
    fs.rename(dir, b"child", 2, b"moved", RenameMode::NoReplace, RENAME)
        .unwrap();
    assert_eq!(fs.parent(child), Ok(2));
    remove(fs.as_ref(), 2, b"moved", true).unwrap();
    let held = fs.open(seed).unwrap();
    let changed = SetAttr {
        expected: Some(fs.getattr(seed).unwrap().attrs),
        mode: Some(0o600),
        uid: Some(90000),
        gid: Some(91000),
        atime: Some(Timestamp { sec: -5, nsec: 123 }),
        mtime: Some(Timestamp {
            sec: 42,
            nsec: 999_999_999,
        }),
    };
    fs.setattr(seed, changed).unwrap();
    assert_eq!(fs.setattr(seed, changed), Err(Errno(EAGAIN)));
    assert_eq!(held.times.lock().mtime, changed.mtime.unwrap());
    assert_eq!(
        fs.xattr(
            seed,
            Xattr::Set {
                name: b"user.a",
                value: b"one",
                mode: XattrSet::Create
            }
        ),
        Ok(0)
    );
    assert_eq!(
        fs.xattr(
            seed,
            Xattr::Set {
                name: b"user.a",
                value: b"bad",
                mode: XattrSet::Create
            }
        ),
        Err(Errno(EEXIST))
    );
    assert_eq!(
        fs.xattr(
            seed,
            Xattr::Set {
                name: b"user.absent",
                value: b"bad",
                mode: XattrSet::Replace
            }
        ),
        Err(Errno(ENODATA))
    );
    fs.xattr(
        seed,
        Xattr::Set {
            name: b"user.a",
            value: b"two",
            mode: XattrSet::Replace,
        },
    )
    .unwrap();
    fs.xattr(
        seed,
        Xattr::Set {
            name: b"user.gone",
            value: b"x",
            mode: XattrSet::Upsert,
        },
    )
    .unwrap();
    fs.xattr(seed, Xattr::Remove { name: b"user.gone" })
        .unwrap();
    let mut tiny = [0; 1];
    assert_eq!(
        fs.xattr(
            seed,
            Xattr::Get {
                name: b"user.a",
                value: &mut tiny
            }
        ),
        Err(Errno(ERANGE))
    );
    fs.sync().unwrap();
    // Change the namespace again after the transaction has committed.
    fs.create(2, b"after-commit", NewNode::Regular { attrs: ATTRS })
        .unwrap();
    fs.sync().unwrap();
    drop(held);
    drop(fs);
    let fs = mount(disk);
    assert_eq!(fs.lookup(2, b"dir"), Ok(dir));
    assert!(fs.lookup(2, b"after-commit").is_ok());
    for (name, ino, kind, rdev) in expected {
        let ino = if name == b"file" { seed } else { ino };
        assert_eq!(fs.lookup(dir, name), Ok(ino));
        let attrs = fs.getattr(ino).unwrap();
        assert_eq!((attrs.kind, attrs.rdev), (kind, rdev));
    }
    let attrs = fs.getattr(seed).unwrap();
    assert_eq!(attrs.attrs, InodeAttrs::new(0o100600, 90000, 91000));
    assert_eq!(attrs.times.atime, changed.atime.unwrap());
    assert_eq!(attrs.times.mtime, changed.mtime.unwrap());
    let mut value = [0; 8];
    assert_eq!(
        fs.xattr(
            seed,
            Xattr::Get {
                name: b"user.a",
                value: &mut value
            }
        ),
        Ok(3)
    );
    assert_eq!(&value[..3], b"two");
    assert_eq!(fs.xattr(seed, Xattr::List { names: &mut value }), Ok(7));
    assert_eq!(&value[..7], b"user.a\0");
}
fn mode(kind: NodeKind) -> u32 {
    match kind {
        NodeKind::Regular => 0o100000,
        NodeKind::Directory => 0o040000,
        NodeKind::Symlink => 0o120000,
        NodeKind::CharDevice => 0o020000,
        NodeKind::BlockDevice => 0o060000,
        NodeKind::Fifo => 0o010000,
        NodeKind::Socket => 0o140000,
    }
}

#[test]
fn unlinked_open_file_survives_until_close_then_reclaims() {
    let (disk, ino) = fixture();
    let fs = mount(disk.clone());
    let file = fs.open(ino).unwrap();
    let second = fs.open(ino).unwrap();
    remove(fs.as_ref(), 2, b"seed", false).unwrap();
    assert_eq!(fs.lookup(2, b"seed"), Err(Errno(ENOENT)));
    // A link lookup racing the unlink cannot resurrect a zero-link inode.
    assert_eq!(fs.link(ino, 2, b"resurrect", LINK), Err(Errno(ENOENT)));
    fs.sync().unwrap();
    let before = fs.statfs().unwrap().free_blocks;
    let sb = recover::recover(disk.as_ref(), false).unwrap();
    let items = Files::new(DeviceStore(disk.as_ref()), &sb, 1)
        .unwrap()
        .items()
        .unwrap();
    assert!(items
        .iter()
        .any(|(k, _)| k.item_type == format::ORPHAN && k.offset == ino));
    drop(file);
    fs.sync().unwrap();
    let mut bytes = vec![0; 256 * 4096];
    assert_eq!(fs.read(&second, 0, &mut bytes), Ok(bytes.len()));
    assert!(bytes.iter().all(|&b| b == 0x5a));
    assert_eq!(fs.getattr(ino).unwrap().nlink, 0);
    drop(second);
    fs.sync().unwrap();
    let sb = recover::recover(disk.as_ref(), false).unwrap();
    let items = Files::new(DeviceStore(disk.as_ref()), &sb, 1)
        .unwrap()
        .items()
        .unwrap();
    assert!(!items
        .iter()
        .any(|(k, _)| k.item_type == format::ORPHAN || k.object == ino));
    // The reuse rule quarantines released blocks for two durable generations.
    fs.sync().unwrap();
    fs.sync().unwrap();
    assert!(fs.statfs().unwrap().free_blocks >= before + 256);
    drop(fs);
    let fs = mount(disk);
    assert_eq!(fs.lookup(2, b"seed"), Err(Errno(ENOENT)));
    assert_eq!(fs.getattr(ino), Err(Errno(ENOENT)));
    assert!(fs.statfs().unwrap().free_blocks >= before + 256);
}

fn write_at(fs: &dyn FileSystem, node: &VNode, at: WriteAt, bytes: &[u8]) -> WriteResult {
    fs.write(
        node,
        WriteReq {
            at,
            limit: u64::MAX,
            clear_setid: false,
        },
        bytes,
    )
    .unwrap()
}
fn equal_bytes(fs: &dyn FileSystem, node: &VNode, expected: &[u8]) {
    let mut actual = vec![0xcc; expected.len() + 13];
    assert_eq!(fs.read(node, 0, &mut actual), Ok(expected.len()));
    assert_eq!(&actual[..expected.len()], expected);
    assert!(actual[expected.len()..].iter().all(|&b| b == 0));
}

#[test]
fn breenfs_data_reads_dirty_pages_then_extents() {
    let (disk, ino) = fixture();
    let fs = mount(disk.clone());
    let node = fs.open(ino).unwrap();
    let mut expected = vec![0x5a; 256 * 4096];
    // Prime the clean read path, then overwrite an unaligned, page-spanning range.
    equal_bytes(fs.as_ref(), &node, &expected);
    let before = disk.bytes.lock().unwrap().clone();
    let patch: Vec<_> = (0..9001).map(|i| (i % 251) as u8).collect();
    let result = write_at(fs.as_ref(), &node, WriteAt::Offset(4093), &patch);
    assert_eq!(
        result,
        WriteResult {
            offset: 4093,
            bytes: patch.len(),
            limited: false
        }
    );
    expected[4093..4093 + patch.len()].copy_from_slice(&patch);
    write_at(fs.as_ref(), &node, WriteAt::Offset(8190), b"overlap");
    expected[8190..8197].copy_from_slice(b"overlap");
    fs.setattr(
        ino,
        SetAttr {
            mode: Some(0o6750),
            ..Default::default()
        },
    )
    .unwrap();
    let end = expected.len() as u64;
    let req = WriteReq {
        at: WriteAt::Append,
        limit: end + 3,
        clear_setid: true,
    };
    assert_eq!(
        fs.write(&node, req, b"abcdef"),
        Ok(WriteResult {
            offset: end,
            bytes: 3,
            limited: true
        })
    );
    expected.extend_from_slice(b"abc");
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o6000, 0);
    assert_eq!(fs.write(&node, req, b"x"), Err(WriteError::FileSizeLimit));
    let max = fs.max_file_size().unwrap();
    assert_eq!(
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Offset(max),
                limit: u64::MAX,
                clear_setid: false
            },
            b"x"
        ),
        Err(WriteError::Filesystem(Errno(EFBIG)))
    );
    assert_eq!(
        *disk.bytes.lock().unwrap(),
        before,
        "write must wait for commit"
    );
    equal_bytes(fs.as_ref(), &node, &expected);
    drop(node);
    let node = fs.open(ino).unwrap();
    equal_bytes(fs.as_ref(), &node, &expected);
    fs.fsync(&node, false).unwrap();
    equal_bytes(fs.as_ref(), &node, &expected);
    // Another transaction overwrites data already read through the clean cache.
    write_at(
        fs.as_ref(),
        &node,
        WriteAt::Offset(4095),
        b"new transaction",
    );
    expected[4095..4110].copy_from_slice(b"new transaction");
    fs.sync().unwrap();
    equal_bytes(fs.as_ref(), &node, &expected);
    fs.setattr(
        ino,
        SetAttr {
            mode: Some(0o6750),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Append,
                limit: 0,
                clear_setid: true
            },
            b""
        ),
        Ok(WriteResult {
            offset: expected.len() as u64,
            bytes: 0,
            limited: false
        })
    );
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o6000, 0o6000);
    fs.sync().unwrap();
    drop(node);
    drop(fs);
    let fs = mount(disk);
    equal_bytes(fs.as_ref(), &fs.open(ino).unwrap(), &expected);
}

#[test]
fn truncate_sparse_reads_zeros_and_counts_mapped_blocks() {
    let (disk, _) = fixture();
    let fs = mount(disk.clone());
    let ino = fs
        .create(2, b"sparse", NewNode::Regular { attrs: ATTRS })
        .unwrap();
    let node = fs.open(ino).unwrap();
    fs.resize(&node, 9 * 4096 + 17, false, &|_| Ok(())).unwrap();
    let mut expected = vec![0; 9 * 4096 + 17];
    equal_bytes(fs.as_ref(), &node, &expected);
    assert_eq!(fs.getattr(ino).unwrap().blocks, 0);
    write_at(fs.as_ref(), &node, WriteAt::Offset(4094), b"four");
    expected[4094..4098].copy_from_slice(b"four");
    write_at(fs.as_ref(), &node, WriteAt::Offset(8 * 4096 + 3), b"tail");
    expected[8 * 4096 + 3..8 * 4096 + 7].copy_from_slice(b"tail");
    assert_eq!(fs.getattr(ino).unwrap().blocks, 3 * 8);
    equal_bytes(fs.as_ref(), &node, &expected);
    fs.sync().unwrap();
    assert_eq!(fs.getattr(ino).unwrap().blocks, 3 * 8);
    // Refused resize leaves content and set-ID bits alone.
    let attrs = fs.getattr(ino).unwrap();
    assert_eq!(
        fs.resize(&node, 0, true, &|a| {
            assert_eq!(a, attrs);
            Err(Errno(EACCES))
        }),
        Err(Errno(EACCES))
    );
    assert_eq!(fs.getattr(ino).unwrap(), attrs);
    write_at(
        fs.as_ref(),
        &node,
        WriteAt::Offset(7 * 4096),
        b"discard dirty page",
    );
    fs.resize(&node, 4095, true, &|_| Ok(())).unwrap();
    expected.truncate(4095);
    assert_eq!(fs.getattr(ino).unwrap().blocks, 8);
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o6000, 0);
    fs.resize(&node, 9 * 4096 + 17, false, &|_| Ok(())).unwrap();
    expected.resize(9 * 4096 + 17, 0);
    equal_bytes(fs.as_ref(), &node, &expected);
    fs.sync().unwrap();
    equal_bytes(fs.as_ref(), &node, &expected);
    assert_eq!(fs.getattr(ino).unwrap().blocks, 8);
    drop(node);
    drop(fs);
    let fs = mount(disk);
    let node = fs.open(ino).unwrap();
    equal_bytes(fs.as_ref(), &node, &expected);
    assert_eq!(fs.getattr(ino).unwrap().blocks, 8);
    fs.resize(&node, 0, false, &|_| Ok(())).unwrap();
    fs.resize(&node, 8192, false, &|_| Ok(())).unwrap();
    equal_bytes(fs.as_ref(), &node, &[0; 8192]);
    assert_eq!(fs.getattr(ino).unwrap().blocks, 0);
    fs.sync().unwrap();
    assert_eq!(fs.getattr(ino).unwrap().blocks, 0);
}

#[test]
fn commit_drains_into_extents_of_at_most_1_mib() {
    let (disk, _) = fixture();
    let fs = mount(disk.clone());
    let ino = fs
        .create(2, b"large", NewNode::Regular { attrs: ATTRS })
        .unwrap();
    let node = fs.open(ino).unwrap();
    let data: Vec<_> = (0..(531 * 4096 - 7)).map(|i| (i % 239) as u8).collect();
    let before = disk.bytes.lock().unwrap().clone();
    write_at(fs.as_ref(), &node, WriteAt::Offset(0), &data);
    write_at(
        fs.as_ref(),
        &node,
        WriteAt::Offset(540 * 4096 + 5),
        b"island",
    );
    assert_eq!(*disk.bytes.lock().unwrap(), before);
    assert_eq!(fs.getattr(ino).unwrap().blocks, 532 * 8);
    fs.sync().unwrap();
    let sb = recover::recover(disk.as_ref(), false).unwrap();
    let files = Files::new(DeviceStore(disk.as_ref()), &sb, 1).unwrap();
    let extents: Vec<_> = files
        .items()
        .unwrap()
        .into_iter()
        .filter(|(k, _)| k.object == ino && k.item_type == format::EXTENT)
        .map(|(k, v)| (k.offset, format::Extent::decode(&v).unwrap()))
        .collect();
    assert_eq!(
        extents
            .iter()
            .map(|(off, e)| (*off, e.num_blocks))
            .collect::<Vec<_>>(),
        vec![
            (0, 256),
            (256 * 4096, 256),
            (512 * 4096, 19),
            (540 * 4096, 1)
        ]
    );
    assert!(extents.iter().all(|(_, e)| e.phys_len <= 256));
    let mut expected = data;
    expected.resize(540 * 4096 + 11, 0);
    expected[540 * 4096 + 5..].copy_from_slice(b"island");
    equal_bytes(fs.as_ref(), &node, &expected);
    drop(files);
    drop(node);
    drop(fs);
    let fs = mount(disk);
    equal_bytes(fs.as_ref(), &fs.open(ino).unwrap(), &expected);
}

#[test]
fn writer_enters_next_generation_while_fsync_waits_outside_mount_lock() {
    use crate::block::{BlockDevice, BlockError};
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Condvar,
    };
    use std::time::Duration;
    struct Gate {
        disk: Arc<Disk>,
        armed: AtomicBool,
        flushes: AtomicUsize,
        entered: mpsc::Sender<()>,
        released: (Mutex<bool>, Condvar),
    }
    impl BlockDevice for Gate {
        fn read_block(&self, n: u64, b: &mut [u8]) -> Result<(), BlockError> {
            self.disk.read_block(n, b)
        }
        fn write_block(&self, n: u64, b: &[u8]) -> Result<(), BlockError> {
            self.disk.write_block(n, b)
        }
        fn block_size(&self) -> usize {
            self.disk.block_size()
        }
        fn num_blocks(&self) -> u64 {
            self.disk.num_blocks()
        }
        fn flush(&self) -> Result<(), BlockError> {
            if self.armed.load(Ordering::Acquire) {
                match self.flushes.fetch_add(1, Ordering::AcqRel) {
                    0 => {
                        self.entered.send(()).unwrap();
                        let (lock, ready) = &self.released;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = ready.wait(released).unwrap();
                        }
                    }
                    2 => return Err(BlockError::IoError),
                    _ => (),
                }
            }
            self.disk.flush()
        }
    }
    let (disk, ino) = fixture();
    let (entered, barrier) = mpsc::channel();
    let device = Arc::new(Gate {
        disk,
        armed: AtomicBool::new(false),
        flushes: AtomicUsize::new(0),
        entered,
        released: (Mutex::new(false), Condvar::new()),
    });
    let fs =
        BreenFs::mount_writable(device.clone(), Arc::new(BufferCache::new(NoWait)), 71, 1).unwrap();
    let node = fs.open(ino).unwrap();
    write_at(fs.as_ref(), &node, WriteAt::Offset(0), b"generation two");
    device.armed.store(true, Ordering::Release);
    let first = {
        let fs = fs.clone();
        let node = node.clone();
        std::thread::spawn(move || fs.fsync(&node, false))
    };
    barrier.recv_timeout(Duration::from_secs(3)).unwrap();
    let (written, observed) = mpsc::channel();
    let next = {
        let fs = fs.clone();
        let node = node.clone();
        std::thread::spawn(move || {
            let mut data = [0; 14];
            fs.read(&node, 0, &mut data).unwrap();
            assert_eq!(&data, b"generation two");
            write_at(fs.as_ref(), &node, WriteAt::Offset(0), b"G3");
            fs.read(&node, 0, &mut data).unwrap();
            assert_eq!(&data, b"G3neration two");
            written.send(()).unwrap();
            fs.fsync(&node, true)
        })
    };
    // This is a hang watchdog: publication is held until the writer has run.
    let progressed = observed.recv_timeout(Duration::from_secs(3));
    *device.released.0.lock().unwrap() = true;
    device.released.1.notify_all();
    let first = first.join().unwrap();
    let next = next.join().unwrap();
    assert!(
        progressed.is_ok(),
        "writer blocked behind fsync's mount lock"
    );
    assert_eq!(first, Ok(()));
    assert_eq!(next, Err(Errno(EIO)));
    assert_eq!(fs.read_only(), Ok(true));
    let mapping = fs.mapping_io(&node).unwrap();
    assert_eq!(mapping.sync(0, 1), Err(Errno(EIO)));
    assert!(mapping.needs_flush());
    assert_eq!(fs.sync(), Err(Errno(EIO)));
    assert_eq!(
        fs.create(2, b"refused", NewNode::Regular { attrs: ATTRS }),
        Err(Errno(EROFS))
    );
    let mut report = String::new();
    fs.report(Report::State, "test", &mut report).unwrap();
    assert!(report.contains("durable_generation=2"));
}

#[test]
fn writeback_pages_joins_the_transaction_and_msync_is_durable() {
    let (disk, ino) = fixture();
    let fs = mount(disk.clone());
    let node = fs.open(ino).unwrap();
    fs.resize(&node, 2 * 4096 + 37, false, &|_| Ok(())).unwrap();
    fs.setattr(
        ino,
        SetAttr {
            mode: Some(0o6750),
            ..Default::default()
        },
    )
    .unwrap();
    fs.sync().unwrap();
    let before = disk.bytes.lock().unwrap().clone();
    let generation = recover::recover(disk.as_ref(), false).unwrap().generation;
    let mapping = fs.mapping_io(&node).unwrap();
    assert!(mapping.needs_flush());
    let mut bound = false;
    mapping
        .publish(&mut |append_only| {
            bound = !append_only;
            Ok(())
        })
        .unwrap();
    assert!(bound, "writable mount must publish a writable binding");
    let mut expected = vec![0x5a; 2 * 4096 + 37];
    write_at(fs.as_ref(), &node, WriteAt::Offset(7), b"ordinary");
    expected[7..15].copy_from_slice(b"ordinary");
    fs.writeback_pages(&node, 1, &[0x61; 4096], mapping.as_ref())
        .unwrap();
    expected[4096..8192].fill(0x61);
    fs.writeback_pages(&node, 2, &[0x62; 37], mapping.as_ref())
        .unwrap();
    expected[8192..].fill(0x62);
    assert_eq!(fs.dirty_bytes(), 3 * 4096);
    assert_eq!(
        *disk.bytes.lock().unwrap(),
        before,
        "writeback must join, not publish, the transaction"
    );
    equal_bytes(fs.as_ref(), &node, &expected);
    fs.clear_mapping_privilege(&node).unwrap();
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o6000, 0);
    // The mapping layer obtains a fresh transfer after staging its page batches.
    fs.mapping_io(&node).unwrap().sync(1, 3).unwrap();
    assert_eq!(
        recover::recover(disk.as_ref(), false).unwrap().generation,
        generation + 1
    );
    assert_eq!(fs.dirty_bytes(), 0);
    drop(mapping);
    drop(node);
    drop(fs);
    let fs = BreenFs::mount(disk, Arc::new(BufferCache::new(NoWait)), 71, 1).unwrap();
    equal_bytes(fs.as_ref(), &fs.open(ino).unwrap(), &expected);
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o6000, 0);
}

#[test]
fn writeback_bounces_before_checksum() {
    use crate::breenfs::{crc32c::checksum, space};
    let (disk, ino) = fixture();
    let fs = mount(disk.clone());
    let node = fs.open(ino).unwrap();
    let mapping = fs.mapping_io(&node).unwrap();
    let mut caller: Vec<_> = (0..8192).map(|i| (i % 251) as u8).collect();
    let snapshot = caller.clone();
    fs.writeback_pages(&node, 3, &caller, mapping.as_ref())
        .unwrap();
    caller.fill(0xee);
    let mut expected = vec![0x5a; 256 * 4096];
    expected[3 * 4096..5 * 4096].copy_from_slice(&snapshot);
    equal_bytes(fs.as_ref(), &node, &expected);
    mapping.sync(3, 5).unwrap();
    let sb = recover::recover(disk.as_ref(), false).unwrap();
    let store = DeviceStore(disk.as_ref());
    let files = Files::new(store, &sb, 1).unwrap();
    for page in 3..5 {
        let offset = page * 4096;
        let (start, extent) = files
            .items()
            .unwrap()
            .into_iter()
            .filter(|(k, _)| k.object == ino && k.item_type == format::EXTENT)
            .map(|(k, v)| (k, format::Extent::decode(&v).unwrap()))
            .find(|(k, e)| k.offset <= offset && offset < k.offset + u64::from(e.num_blocks) * 4096)
            .unwrap();
        let reference = space::reference(
            &DeviceStore(disk.as_ref()),
            sb.space_root,
            extent.phys_start,
            u64::from(extent.phys_len),
        )
        .unwrap()
        .unwrap();
        let index = u64::from(extent.extent_offset) + (offset - start.offset) / 4096;
        let block = (extent.phys_start + index) as usize * 4096;
        let want = &snapshot[(page as usize - 3) * 4096..(page as usize - 2) * 4096];
        assert_eq!(&disk.bytes.lock().unwrap()[block..block + 4096], want);
        assert_eq!(reference.crcs[index as usize], checksum(want));
    }
    drop(files);
    drop(mapping);
    drop(node);
    drop(fs);
    let fs = BreenFs::mount(disk, Arc::new(BufferCache::new(NoWait)), 71, 1).unwrap();
    equal_bytes(fs.as_ref(), &fs.open(ino).unwrap(), &expected);
}
