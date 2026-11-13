use vfs_host_tests::{
    adapter::Ext2Adapter,
    attrs::InodeAttrs,
    errno::Errno,
    host_mounts::Fixture,
    syscall_errno::*,
    vfs::{FileSystem, NewNode, NodeKind, Remove, RenameMode, SetAttr, Timestamp},
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
