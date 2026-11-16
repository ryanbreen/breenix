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
