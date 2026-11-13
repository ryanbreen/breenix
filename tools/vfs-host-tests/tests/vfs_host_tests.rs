use crate::{
    adapter::Ext2Adapter,
    attrs::InodeAttrs,
    errno::Errno,
    host_mounts::Fixture,
    syscall_errno::*,
    vfs::{
        FileSystem, NewNode, NodeKind, Remove, RenameMode, SetAttr, Timestamp, WriteAt, Xattr,
        XattrSet,
    },
};

fn attrs(mode: u32) -> InodeAttrs {
    InodeAttrs::new(mode, 70000, 80000)
}
fn regular(fs: &dyn FileSystem, dir: u64, name: &[u8]) -> u64 {
    fs.create(
        dir,
        name,
        NewNode::Regular {
            attrs: attrs(0o640),
        },
    )
    .unwrap()
}

#[test]
fn rename_noreplace_refuses_existing() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let root = fs.root().unwrap();
    let source = regular(fs, root, b"source");
    let target = regular(fs, root, b"target");
    assert_eq!(
        fs.rename(root, b"source", root, b"target", RenameMode::NoReplace),
        Err(Errno(EEXIST))
    );
    assert_eq!(fs.lookup(root, b"source"), Ok(source));
    assert_eq!(fs.lookup(root, b"target"), Ok(target));
    assert_eq!(fs.getattr(source).unwrap().nlink, 1);
    assert_eq!(fs.getattr(target).unwrap().nlink, 1);
    assert_eq!(
        fs.rename(root, b"source", root, b"source", RenameMode::NoReplace),
        Err(Errno(EEXIST))
    );
    assert_eq!(
        fs.rename(root, b"source", root, b"target", RenameMode::Exchange),
        Err(Errno(EINVAL))
    );
    fs.rename(root, b"source", root, b"fresh", RenameMode::NoReplace)
        .unwrap();
    assert_eq!(fs.lookup(root, b"source"), Err(Errno(ENOENT)));
    assert_eq!(fs.lookup(root, b"fresh"), Ok(source));
    fs.rename(root, b"fresh", root, b"target", RenameMode::Replace)
        .unwrap();
    assert_eq!(fs.lookup(root, b"fresh"), Err(Errno(ENOENT)));
    assert_eq!(fs.lookup(root, b"target"), Ok(source));
}

#[test]
fn names_create_lookup_readdir_and_readlink() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let root = fs.root().unwrap();
    let a = attrs(0o750);
    let long_target = vec![b'x'; 120];
    let specs = [
        (
            b"regular".as_slice(),
            NewNode::Regular { attrs: a },
            NodeKind::Regular,
        ),
        (
            b"directory",
            NewNode::Directory { attrs: a },
            NodeKind::Directory,
        ),
        (
            b"symlink",
            NewNode::Symlink {
                attrs: a,
                target: &long_target,
            },
            NodeKind::Symlink,
        ),
        (
            b"character",
            NewNode::CharDevice {
                attrs: a,
                rdev: 0x12345678,
            },
            NodeKind::CharDevice,
        ),
        (
            b"block",
            NewNode::BlockDevice {
                attrs: a,
                rdev: 0x801,
            },
            NodeKind::BlockDevice,
        ),
        (b"fifo", NewNode::Fifo { attrs: a }, NodeKind::Fifo),
        (b"socket", NewNode::Socket { attrs: a }, NodeKind::Socket),
    ];
    let mut expected = std::collections::BTreeMap::new();
    for (name, node, kind) in specs {
        let ino = fs.create(root, name, node).unwrap();
        assert_eq!(fs.lookup(root, name), Ok(ino));
        let meta = fs.getattr(ino).unwrap();
        assert_eq!(meta.ino, ino);
        assert_eq!((meta.attrs.uid, meta.attrs.gid), (a.uid, a.gid));
        assert_eq!(meta.attrs.mode & 0o7777, a.mode);
        assert_eq!(meta.kind, kind);
        assert_eq!(meta.nlink, if kind == NodeKind::Directory { 2 } else { 1 });
        if kind == NodeKind::Symlink {
            assert_eq!(fs.readlink(ino).unwrap(), long_target);
        }
        if kind == NodeKind::CharDevice {
            assert_eq!(meta.rdev, 0x12345678);
        }
        if kind == NodeKind::BlockDevice {
            assert_eq!(meta.rdev, 0x801);
        }
        expected.insert(name.to_vec(), (ino, kind));
    }
    // Names and short symlink targets are bytes, including non-UTF-8.
    let ino = regular(fs, root, b"raw-\xff");
    expected.insert(b"raw-\xff".to_vec(), (ino, NodeKind::Regular));
    let sym = fs
        .create(
            root,
            b"short",
            NewNode::Symlink {
                attrs: a,
                target: b"raw-\xff",
            },
        )
        .unwrap();
    assert_eq!(fs.readlink(sym).unwrap(), b"raw-\xff");
    expected.insert(b"short".to_vec(), (sym, NodeKind::Symlink));
    assert_eq!(fs.lookup(root, b"absent"), Err(Errno(ENOENT)));
    assert_eq!(fs.lookup(ino, b"child"), Err(Errno(ENOTDIR)));
    assert_eq!(
        fs.create(root, b"regular", NewNode::Fifo { attrs: a }),
        Err(Errno(EEXIST))
    );
    assert_eq!(
        fs.create(root, &[b'x'; 256], NewNode::Regular { attrs: a }),
        Err(Errno(ENAMETOOLONG))
    );
    assert_eq!(
        fs.create(root, b"bad\0name", NewNode::Directory { attrs: a }),
        Err(Errno(EINVAL))
    );
    let mut seen = std::collections::BTreeMap::new();
    let mut cookie = 0;
    for _ in 0..expected.len() + 1 {
        let mut accepted = false;
        let next = fs
            .readdir(root, cookie, &mut |entry| {
                if accepted {
                    return Ok(false);
                }
                assert!(
                    seen.insert(entry.name, (entry.ino, entry.kind)).is_none(),
                    "duplicate directory entry"
                );
                accepted = true;
                Ok(true)
            })
            .unwrap();
        assert!(next >= cookie);
        cookie = next;
        if !accepted {
            break;
        }
    }
    assert_eq!(seen, expected);
    assert_eq!(fs.readlink(ino), Err(Errno(EINVAL)));
}

#[test]
fn names_link_counts_and_typed_removal() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let root = fs.root().unwrap();
    let dir = fs
        .create(
            root,
            b"work",
            NewNode::Directory {
                attrs: attrs(0o755),
            },
        )
        .unwrap();
    let file = regular(fs, dir, b"file");
    assert_eq!(fs.getattr(root).unwrap().nlink, 3);
    assert_eq!(fs.remove(root, b"work", Remove::NonDir), Err(Errno(EISDIR)));
    assert_eq!(fs.remove(root, b"work", Remove::Dir), Err(Errno(ENOTEMPTY)));
    assert_eq!(fs.remove(dir, b"file", Remove::Dir), Err(Errno(ENOTDIR)));
    assert_eq!(
        fs.remove(file, b"child", Remove::NonDir),
        Err(Errno(ENOTDIR))
    );
    fs.link(file, root, b"alias").unwrap();
    assert_eq!(fs.getattr(file).unwrap().nlink, 2);
    fs.remove(dir, b"file", Remove::NonDir).unwrap();
    assert_eq!(fs.getattr(file).unwrap().nlink, 1);
    assert_eq!(fs.lookup(root, b"alias"), Ok(file));
    fs.remove(root, b"work", Remove::Dir).unwrap();
    assert_eq!(fs.getattr(root).unwrap().nlink, 2);
    assert_eq!(fs.lookup(root, b"work"), Err(Errno(ENOENT)));
    fs.remove(root, b"alias", Remove::NonDir).unwrap();
    assert_eq!(fs.lookup(root, b"alias"), Err(Errno(ENOENT)));
}

#[test]
fn mount_inode_attributes_and_open() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    assert_eq!(fs.kind(), Ok("ext2"));
    assert_eq!(fs.read_only(), Ok(false));
    assert!(fs.max_file_size().unwrap() > 0);
    let stat = fs.statfs().unwrap();
    assert_eq!(
        (stat.block_size, stat.blocks, stat.free_blocks),
        (4096, 512, 490)
    );
    let root = fs.root().unwrap();
    let ino = regular(fs, root, b"file");
    fs.setattr(
        ino,
        SetAttr {
            mode: Some(0o604),
            uid: Some(90000),
            gid: Some(100000),
            atime: Some(Timestamp { sec: 123, nsec: 0 }),
            mtime: Some(Timestamp { sec: 456, nsec: 0 }),
        },
    )
    .unwrap();
    let meta = fs.getattr(ino).unwrap();
    assert_eq!(meta.attrs, InodeAttrs::new(0o100604, 90000, 100000));
    assert_eq!((meta.times.atime.sec, meta.times.mtime.sec), (123, 456));
    let node = fs.open(ino).unwrap();
    assert_eq!((node.mount, node.ino), (disk.pin().mount_id, ino));
    assert!(node.incarnation > 0);
    assert_eq!(fs.getattr(u32::MAX as u64 + 1), Err(Errno(ENOENT)));
    fs.sync().unwrap();
}

#[test]
fn append_write_extends_and_reads_back() {
    let disk = Fixture::new();
    let fs = Ext2Adapter::new(disk.pin());
    let ino = regular(&fs, fs.root().unwrap(), b"append");
    let node = fs.open(ino).unwrap();
    let other = fs.open(ino).unwrap();
    assert_eq!(fs.write(&node, WriteAt::Offset(0), b"hello"), Ok(5));
    assert_eq!(fs.write(&other, WriteAt::Append, b" world"), Ok(6));
    assert_eq!(fs.write(&node, WriteAt::Append, b"!"), Ok(1));
    let mut bytes = [0xa5; 20];
    assert_eq!(fs.read(&other, 0, &mut bytes), Ok(12));
    assert_eq!(&bytes[..12], b"hello world!");
    assert_eq!(&bytes[12..], &[0xa5; 8]);
    assert_eq!(fs.read(&node, 12, &mut bytes), Ok(0));
    assert_eq!(fs.read(&node, 6, &mut bytes[..5]), Ok(5));
    assert_eq!(&bytes[..5], b"world");
    assert_eq!(fs.getattr(ino).unwrap().size, 12);
    fs.fsync(&node, false).unwrap();
    fs.fsync(&other, true).unwrap();
    let mut value = [0; 8];
    assert_eq!(
        fs.xattr(
            ino,
            Xattr::Get {
                name: b"user.test",
                value: &mut value
            }
        ),
        Err(Errno(EOPNOTSUPP))
    );
    assert_eq!(
        fs.xattr(ino, Xattr::List { names: &mut value }),
        Err(Errno(EOPNOTSUPP))
    );
    assert_eq!(
        fs.xattr(
            ino,
            Xattr::Set {
                name: b"user.test",
                value: b"value",
                mode: XattrSet::Upsert
            }
        ),
        Err(Errno(EOPNOTSUPP))
    );
    assert_eq!(
        fs.xattr(ino, Xattr::Remove { name: b"user.test" }),
        Err(Errno(EOPNOTSUPP))
    );
}

#[test]
fn resize_shrink_then_grow_reads_zeros() {
    let disk = Fixture::new();
    let fs = Ext2Adapter::new(disk.pin());
    let ino = regular(&fs, fs.root().unwrap(), b"resize");
    let node = fs.open(ino).unwrap();
    let data = vec![0x5a; 3 * 4096];
    assert_eq!(fs.write(&node, WriteAt::Offset(0), &data), Ok(data.len()));
    fs.resize(&node, 37).unwrap();
    fs.resize(&node, data.len() as u64).unwrap();
    let mut bytes = vec![0xa5; data.len()];
    assert_eq!(fs.read(&node, 0, &mut bytes), Ok(bytes.len()));
    assert_eq!(&bytes[..37], &[0x5a; 37]);
    assert!(bytes[37..].iter().all(|b| *b == 0));
    assert_eq!(fs.getattr(ino).unwrap().size, data.len() as u64);
}

#[test]
fn write_beyond_max_file_size_is_efbig() {
    let disk = Fixture::new();
    let fs = Ext2Adapter::new(disk.pin());
    let ino = regular(&fs, fs.root().unwrap(), b"limit");
    let node = fs.open(ino).unwrap();
    let before = fs.getattr(ino).unwrap();
    let free = fs.statfs().unwrap().free_blocks;
    let limit = fs.max_file_size().unwrap();
    assert_eq!(
        fs.write(&node, WriteAt::Offset(limit), b"x"),
        Err(Errno(EFBIG))
    );
    assert_eq!(
        fs.write(&node, WriteAt::Offset(u64::MAX), b"x"),
        Err(Errno(EFBIG))
    );
    assert_eq!(fs.getattr(ino).unwrap(), before);
    assert_eq!(fs.statfs().unwrap().free_blocks, free);
}

#[test]
fn statfs_free_blocks_fall_after_write() {
    let disk = Fixture::new();
    let fs = Ext2Adapter::new(disk.pin());
    let ino = regular(&fs, fs.root().unwrap(), b"space");
    let node = fs.open(ino).unwrap();
    let before = fs.statfs().unwrap();
    assert_eq!(
        fs.write(&node, WriteAt::Offset(0), &vec![1; 8192]),
        Ok(8192)
    );
    let after = fs.statfs().unwrap();
    assert_eq!(after.blocks, before.blocks);
    assert_eq!(after.free_blocks, before.free_blocks - 2);
    assert_eq!(after.available_blocks, before.available_blocks - 2);
    fs.sync().unwrap();
}

#[test]
fn setattr_times_round_trip() {
    let disk = Fixture::new();
    let fs = Ext2Adapter::new(disk.pin());
    let ino = regular(&fs, fs.root().unwrap(), b"times");
    let before = fs.getattr(ino).unwrap();
    fs.setattr(
        ino,
        SetAttr {
            atime: Some(Timestamp {
                sec: 123456789,
                nsec: 123456789,
            }),
            mtime: Some(Timestamp {
                sec: 234567890,
                nsec: 999999999,
            }),
            ..SetAttr::default()
        },
    )
    .unwrap();
    let after = fs.getattr(ino).unwrap();
    assert_eq!(
        after.times.atime,
        Timestamp {
            sec: 123456789,
            nsec: 0
        }
    );
    assert_eq!(
        after.times.mtime,
        Timestamp {
            sec: 234567890,
            nsec: 0
        }
    );
    assert_eq!(after.attrs, before.attrs);
    assert_eq!(
        fs.setattr(
            ino,
            SetAttr {
                mode: Some(0o777),
                mtime: Some(Timestamp {
                    sec: 1,
                    nsec: 1_000_000_000
                }),
                ..SetAttr::default()
            }
        ),
        Err(Errno(EINVAL))
    );
    assert_eq!(fs.getattr(ino).unwrap(), after);
}

#[test]
fn read_only_mount_write_is_erofs() {
    let disk = Fixture::new();
    let fs = Ext2Adapter::new(disk.pin());
    let ino = regular(&fs, fs.root().unwrap(), b"readonly");
    let node = fs.open(ino).unwrap();
    disk.remount_read_only();
    assert_eq!(fs.read_only(), Ok(true));
    assert!(fs.statfs().unwrap().flags.read_only);
    assert_eq!(fs.write(&node, WriteAt::Offset(0), b"x"), Err(Errno(EROFS)));
    assert_eq!(fs.write(&node, WriteAt::Append, b"x"), Err(Errno(EROFS)));
    assert_eq!(fs.resize(&node, 1), Err(Errno(EROFS)));
    assert_eq!(
        fs.setattr(
            ino,
            SetAttr {
                mode: Some(0o777),
                ..SetAttr::default()
            }
        ),
        Err(Errno(EROFS))
    );
    assert_eq!(fs.getattr(ino).unwrap().size, 0);
}
