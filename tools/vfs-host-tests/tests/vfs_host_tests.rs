use crate::{
    adapter::Ext2Adapter,
    attrs::InodeAttrs,
    errno::Errno,
    host_mounts::Fixture,
    syscall_errno::*,
    vfs::{
        FileSystem, NewNode, NodeKind, Remove, RenameMode, SetAttr, Timestamp, WriteAt, WriteReq,
        WriteResult, Xattr, XattrSet,
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

fn removal(fs: &dyn FileSystem, dir: u64, expected: u64, directory: bool) -> Remove {
    Remove {
        directory,
        expected,
        owner: fs.getattr(expected).unwrap().attrs.uid,
        parent: fs.getattr(dir).unwrap().attrs,
    }
}

#[test]
fn chown_clears_setid_through_setattr() {
    use crate::vfs::attributes::{attribute_change, AttributeChange};
    use crate::permissions::Credentials;
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let root = fs.root().unwrap();
    let file = regular(fs, root, b"setid");
    fs.setattr(file, SetAttr { mode: Some(0o6755), ..SetAttr::default() }).unwrap();
    let before = fs.getattr(file).unwrap();
    let request = attribute_change(before.attrs, AttributeChange::Owner(90000, 90001),
        &Credentials::kernel()).unwrap();
    fs.setattr(file, request).unwrap();
    let after = fs.getattr(file).unwrap();
    assert_eq!((after.attrs.uid, after.attrs.gid), (90000, 90001));
    assert_eq!(after.attrs.mode, 0o100755);
    assert_eq!(after.times.ctime.sec, 1);

    let mut owner = Credentials::kernel();
    owner.euid = 90000;
    owner.egid = 90001;
    assert_eq!(attribute_change(after.attrs, AttributeChange::Owner(42, u32::MAX), &owner), Err(Errno(EPERM)));
    assert_eq!(attribute_change(after.attrs, AttributeChange::Owner(u32::MAX, 42), &owner), Err(Errno(EPERM)));
    let mode = attribute_change(after.attrs, AttributeChange::Mode(0o6755), &owner).unwrap();
    fs.setattr(file, mode).unwrap();
    // Changing ownership invalidates a previously checked request. It must
    // refuse rather than apply permission bits based on the previous owner.
    assert_eq!(fs.setattr(file, request), Err(Errno(EAGAIN)));
    let fresh = fs.getattr(file).unwrap();
    let keep = attribute_change(fresh.attrs, AttributeChange::Owner(u32::MAX, u32::MAX), &owner).unwrap();
    fs.setattr(file, keep).unwrap();
    assert_eq!(fs.getattr(file).unwrap().attrs.mode, 0o100755);

    let dir = fs.create(root, b"inherit", NewNode::Directory { attrs: attrs(0o2775) }).unwrap();
    let change = attribute_change(fs.getattr(dir).unwrap().attrs,
        AttributeChange::Owner(90000, 90001), &Credentials::kernel()).unwrap();
    fs.setattr(dir, change).unwrap();
    assert_eq!(fs.getattr(dir).unwrap().attrs.mode, 0o042775);
    owner.egid = 42;
    let mode = attribute_change(fs.getattr(dir).unwrap().attrs, AttributeChange::Mode(0o2775), &owner).unwrap();
    fs.setattr(dir, mode).unwrap();
    assert_eq!(fs.getattr(dir).unwrap().attrs.mode, 0o040775);
}

#[test]
fn failed_create_open_leaves_no_name_and_exclusive_retry_succeeds() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let root = fs.root().unwrap();
    let before = fs.statfs().unwrap();
    let policy = |parent: crate::vfs::NodeAttrs| {
        assert_eq!(parent.ino, root);
        Ok(attrs(0)) // The creating open works despite its restrictive mode.
    };
    // The caller receives the prepared VNode before publication and may still
    // fail allocation of its descriptor. No name or inode allocation survives.
    let mut calls = 0;
    assert_eq!(fs.create_open(root, b"exclusive", &policy, &mut |node| {
        calls += 1;
        assert_eq!(node.mount, disk.pin().mount_id);
        assert_eq!(node.size.load(core::sync::atomic::Ordering::Acquire), 0);
        Err(Errno(ENOMEM))
    }), Err(Errno(ENOMEM)));
    assert_eq!(calls, 1);
    assert_eq!(fs.lookup(root, b"exclusive"), Err(Errno(ENOENT)));
    assert_eq!(fs.statfs().unwrap(), before);

    // A publication I/O failure also leaves the exclusive retry possible.
    disk.fail_next_write();
    assert_eq!(fs.create_open(root, b"exclusive", &policy, &mut |_| Ok(())), Err(Errno(EIO)));
    assert_eq!(fs.lookup(root, b"exclusive"), Err(Errno(ENOENT)));
    let mut opened = None;
    fs.create_open(root, b"exclusive", &policy, &mut |node| {
        opened = Some(node);
        Ok(())
    }).unwrap();
    let node = opened.unwrap();
    assert_eq!(fs.lookup(root, b"exclusive"), Ok(node.ino));
    assert_eq!(fs.getattr(node.ino).unwrap().attrs.mode & 0o7777, 0);
    fs.write(&node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false }, b"prepared").unwrap();
    let mut bytes = [0; 8];
    assert_eq!(fs.read(&node, 0, &mut bytes), Ok(8));
    assert_eq!(&bytes, b"prepared");
    assert_eq!(fs.create_open(root, b"exclusive", &policy, &mut |_| panic!("existing name must not prepare")), Err(Errno(EEXIST)));
}

#[test]
fn directory_links_and_descendant_renames_are_refused() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let root = fs.root().unwrap();
    let parent = fs.create(root, b"parent", NewNode::Directory { attrs: attrs(0o755) }).unwrap();
    let child = fs.create(parent, b"child", NewNode::Directory { attrs: attrs(0o755) }).unwrap();
    let before = fs.getattr(parent).unwrap();
    assert_eq!(fs.link(parent, root, b"alias", &|_, _| Ok(())), Err(Errno(EPERM)));
    assert_eq!(fs.lookup(root, b"alias"), Err(Errno(ENOENT)));
    for destination in [parent, child] {
        assert_eq!(fs.rename(root, b"parent", destination, b"cycle", RenameMode::Replace, &|_, _, _, _| Ok(())), Err(Errno(EINVAL)));
        assert_eq!(fs.lookup(root, b"parent"), Ok(parent));
        assert_eq!(fs.lookup(parent, b"child"), Ok(child));
        assert_eq!(fs.lookup(destination, b"cycle"), Err(Errno(ENOENT)));
        assert_eq!(fs.getattr(parent).unwrap(), before);
    }
    let file = regular(fs, parent, b"file");
    // Policy sees current attributes and can refuse before a name changes.
    fs.setattr(parent, SetAttr { mode: Some(0o1777), ..SetAttr::default() }).unwrap();
    fs.setattr(file, SetAttr { uid: Some(90001), ..SetAttr::default() }).unwrap();
    assert_eq!(fs.link(file, parent, b"denied", &|dir, source| {
        assert_eq!(dir.attrs.mode & 0o7777, 0o1777);
        assert_eq!(source.attrs.uid, 90001);
        Err(Errno(EPERM))
    }), Err(Errno(EPERM)));
    assert_eq!(fs.lookup(parent, b"denied"), Err(Errno(ENOENT)));
    assert_eq!(fs.getattr(file).unwrap().nlink, 1);
    let victim = regular(fs, root, b"victim");
    assert_eq!(fs.rename(parent, b"file", root, b"victim", RenameMode::Replace,
        &|from, to, source, dest| {
            assert_eq!(from.attrs.mode & 0o7777, 0o1777);
            assert_eq!(to.ino, root);
            assert_eq!(source.attrs.uid, 90001);
            assert_eq!(dest.unwrap().ino, victim);
            Err(Errno(EPERM))
        }), Err(Errno(EPERM)));
    assert_eq!(fs.lookup(parent, b"file"), Ok(file));
    assert_eq!(fs.lookup(root, b"victim"), Ok(victim));
    fs.link(file, root, b"alias", &|_, _| Ok(())).unwrap();
    fs.rename(parent, b"child", root, b"moved", RenameMode::Replace, &|_, _, _, _| Ok(())).unwrap();
    assert_eq!(fs.lookup(root, b"alias"), Ok(file));
    assert_eq!(fs.lookup(root, b"moved"), Ok(child));
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
        fs.rename(root, b"source", root, b"target", RenameMode::NoReplace, &|_, _, _, _| Ok(())),
        Err(Errno(EEXIST))
    );
    assert_eq!(fs.lookup(root, b"source"), Ok(source));
    assert_eq!(fs.lookup(root, b"target"), Ok(target));
    assert_eq!(fs.getattr(source).unwrap().nlink, 1);
    assert_eq!(fs.getattr(target).unwrap().nlink, 1);
    assert_eq!(
        fs.rename(root, b"source", root, b"source", RenameMode::NoReplace, &|_, _, _, _| Ok(())),
        Err(Errno(EEXIST))
    );
    assert_eq!(
        fs.rename(root, b"source", root, b"target", RenameMode::Exchange, &|_, _, _, _| Ok(())),
        Err(Errno(EINVAL))
    );
    fs.rename(root, b"source", root, b"fresh", RenameMode::NoReplace, &|_, _, _, _| Ok(()))
        .unwrap();
    assert_eq!(fs.lookup(root, b"source"), Err(Errno(ENOENT)));
    assert_eq!(fs.lookup(root, b"fresh"), Ok(source));
    fs.rename(root, b"fresh", root, b"target", RenameMode::Replace, &|_, _, _, _| Ok(()))
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
            .readdir(root, cookie, &mut |entry, _cookie| {
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
    assert_eq!(fs.remove(root, b"work", removal(fs, root, dir, false)), Err(Errno(EISDIR)));
    assert_eq!(fs.remove(root, b"work", removal(fs, root, dir, true)), Err(Errno(ENOTEMPTY)));
    assert_eq!(fs.remove(dir, b"file", removal(fs, dir, file, true)), Err(Errno(ENOTDIR)));
    assert_eq!(
        fs.remove(file, b"child", removal(fs, file, file, false)),
        Err(Errno(ENOTDIR))
    );
    fs.link(file, root, b"alias", &|_, _| Ok(())).unwrap();
    assert_eq!(fs.getattr(file).unwrap().nlink, 2);
    fs.remove(dir, b"file", removal(fs, dir, file, false)).unwrap();
    assert_eq!(fs.getattr(file).unwrap().nlink, 1);
    assert_eq!(fs.lookup(root, b"alias"), Ok(file));
    fs.remove(root, b"work", removal(fs, root, dir, true)).unwrap();
    assert_eq!(fs.getattr(root).unwrap().nlink, 2);
    assert_eq!(fs.lookup(root, b"work"), Err(Errno(ENOENT)));
    fs.remove(root, b"alias", removal(fs, root, file, false)).unwrap();
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
    assert_eq!(
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Offset(0),
                limit: u64::MAX,
                clear_setid: false
            },
            b"hello"
        ).map_err(|error| error.errno())
        .map(|r| r.bytes),
        Ok(5)
    );
    assert_eq!(
        fs.write(
            &other,
            WriteReq {
                at: WriteAt::Append,
                limit: u64::MAX,
                clear_setid: false
            },
            b" world"
        ).map_err(|error| error.errno())
        .map(|r| r.bytes),
        Ok(6)
    );
    assert_eq!(
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Append,
                limit: u64::MAX,
                clear_setid: false
            },
            b"!"
        ).map_err(|error| error.errno())
        .map(|r| r.bytes),
        Ok(1)
    );
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
    assert_eq!(
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Offset(0),
                limit: u64::MAX,
                clear_setid: false
            },
            &data
        ).map_err(|error| error.errno())
        .map(|r| r.bytes),
        Ok(data.len())
    );
    fs.resize(&node, 37, false).unwrap();
    fs.resize(&node, data.len() as u64, false).unwrap();
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
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Offset(limit),
                limit: u64::MAX,
                clear_setid: false
            },
            b"x"
        ).map_err(|error| error.errno())
        .map(|r| r.bytes),
        Err(Errno(EFBIG))
    );
    assert_eq!(
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Offset(u64::MAX),
                limit: u64::MAX,
                clear_setid: false
            },
            b"x"
        ).map_err(|error| error.errno())
        .map(|r| r.bytes),
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
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Offset(0),
                limit: u64::MAX,
                clear_setid: false
            },
            &vec![1; 8192]
        ).map_err(|error| error.errno())
        .map(|r| r.bytes),
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
    assert_eq!(
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Offset(0),
                limit: u64::MAX,
                clear_setid: false
            },
            b"x"
        ).map_err(|error| error.errno())
        .map(|r| r.bytes),
        Err(Errno(EROFS))
    );
    assert_eq!(
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Append,
                limit: u64::MAX,
                clear_setid: false
            },
            b"x"
        ).map_err(|error| error.errno())
        .map(|r| r.bytes),
        Err(Errno(EROFS))
    );
    assert_eq!(fs.resize(&node, 1, false), Err(Errno(EROFS)));
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

#[test]
fn getdents_cookie_survives_concurrent_unlink() {
    use crate::{
        ext2::{generation::Namespace, live_inode::FileHandle},
        host_mounts::write_mount,
        vfs::file::OpenFile,
    };
    use std::{collections::BTreeMap, sync::Arc};

    let disk = Fixture::new();
    let fs: Arc<dyn FileSystem> = Arc::new(Ext2Adapter::new(disk.pin()));
    let generation = fs
        .create(
            2,
            b"generation",
            NewNode::Directory {
                attrs: attrs(0o755),
            },
        )
        .unwrap();
    // The physical parent is inode 2; the selected namespace's parent is itself.
    let ns = Namespace {
        root: generation as u32,
        ..Namespace::RAW
    };
    write_mount(disk.pin())
        .unwrap()
        .as_mut()
        .unwrap()
        .host_set_namespace(ns);
    let mut expected = BTreeMap::new();
    for name in [b"a".as_slice(), b"b", b"pending", b"c", b"d"] {
        expected.insert(
            name.to_vec(),
            (regular(fs.as_ref(), generation, name), NodeKind::Regular),
        );
    }
    expected.insert(b".".to_vec(), (generation, NodeKind::Directory));
    expected.insert(b"..".to_vec(), (u64::from(ns.root), NodeKind::Directory));
    let node = fs.open(generation).unwrap();
    let pin = node.pin::<FileHandle>().expect("ext2 inode pin");
    assert_eq!(pin.object.key.incarnation, node.incarnation);
    assert!(node.pin::<u64>().is_none());
    let file = OpenFile {
        fs: fs.clone(),
        node,
    };
    assert_eq!(file.dirents(0, &mut |_, _| Ok(false)), Ok(0));
    let mut seen = Vec::new();
    let cookie = file
        .dirents(0, &mut |entry, _cookie| {
            if seen.len() == 4 {
                return Ok(false);
            }
            seen.push(entry);
            Ok(true)
        })
        .unwrap();
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[0].name, b".");
    assert_eq!(
        (seen[1].name.as_slice(), seen[1].ino),
        (b"..".as_slice(), u64::from(ns.root))
    );
    let returned = seen[2].name.clone();
    assert_ne!(returned, b"pending");
    fs.remove(generation, &returned, removal(fs.as_ref(), generation, seen[2].ino, false)).unwrap();
    fs.remove(generation, b"pending", removal(fs.as_ref(), generation, expected[b"pending".as_slice()].0, false)).unwrap();
    expected.remove(b"pending".as_slice());
    // Larger than either freed slot, so ext2 inserts it beyond the saved cookie.
    let created = b"created-with-a-long-name-that-does-not-fit-unlinked-slots";
    expected.insert(
        created.to_vec(),
        (regular(fs.as_ref(), generation, created), NodeKind::Regular),
    );
    let mut pos = cookie;
    for _ in 0..expected.len() + 1 {
        let mut count = 0;
        let next = file
            .dirents(pos, &mut |entry, _cookie| {
                if count == 2 {
                    return Ok(false);
                }
                count += 1;
                seen.push(entry);
                Ok(true)
            })
            .unwrap();
        assert!(next >= pos);
        pos = next;
        if count == 0 {
            break;
        }
    }
    let mut actual = BTreeMap::new();
    for entry in seen {
        assert!(
            actual.insert(entry.name, (entry.ino, entry.kind)).is_none(),
            "repeated name after cookie resume"
        );
    }
    assert_eq!(actual, expected);
    let mut dots = Vec::new();
    let after_dots = file
        .dirents(0, &mut |entry, _cookie| {
            if dots.len() == 2 {
                return Ok(false);
            }
            dots.push(entry);
            Ok(true)
        })
        .unwrap();
    assert!(after_dots >= 2);
    assert_eq!(
        dots.iter()
            .map(|e| (e.name.as_slice(), e.ino))
            .collect::<Vec<_>>(),
        vec![
            (b".".as_slice(), generation),
            (b"..".as_slice(), generation)
        ]
    );
    let mut parent_only = Vec::new();
    let after_parent = file
        .dirents(1, &mut |entry, _cookie| {
            if !parent_only.is_empty() {
                return Ok(false);
            }
            parent_only.push(entry);
            Ok(true)
        })
        .unwrap();
    assert!(after_parent >= 2);
    assert_eq!(parent_only, dots[1..]);
}

#[test]
fn append_reports_offset_and_clears_setid() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let ino = regular(fs, fs.root().unwrap(), b"append-mode");
    let node = fs.open(ino).unwrap();
    let other = fs.open(ino).unwrap();
    let req = |clear_setid| WriteReq {
        at: WriteAt::Append,
        limit: u64::MAX,
        clear_setid,
    };
    fs.setattr(
        ino,
        SetAttr {
            mode: Some(0o6750),
            ..SetAttr::default()
        },
    )
    .unwrap();
    assert_eq!(
        fs.write(&other, req(false), b"first").map_err(|error| error.errno()),
        Ok(WriteResult {
            offset: 0,
            bytes: 5,
            limited: false
        })
    );
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o7777, 0o6750);
    // node caches its opening size: append must choose the current inode's EOF.
    assert_eq!(
        fs.write(&node, req(true), b"!").map_err(|error| error.errno()),
        Ok(WriteResult {
            offset: 5,
            bytes: 1,
            limited: false
        })
    );
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o7777, 0o750);
    let mut bytes = [0; 6];
    assert_eq!(fs.read(&other, 0, &mut bytes), Ok(6));
    assert_eq!(&bytes, b"first!");
    fs.setattr(
        ino,
        SetAttr {
            mode: Some(0o6750),
            ..SetAttr::default()
        },
    )
    .unwrap();
    fs.resize(&node, 3, false).unwrap();
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o7777, 0o6750);
    fs.resize(&node, 8, true).unwrap();
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o7777, 0o750);
}

#[test]
fn write_limit_clamps_and_reports_limited() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let ino = regular(fs, fs.root().unwrap(), b"write-limit");
    let node = fs.open(ino).unwrap();
    let other = fs.open(ino).unwrap();
    let req = |at, limit| WriteReq {
        at,
        limit,
        clear_setid: true,
    };
    assert_eq!(
        fs.write(&other, req(WriteAt::Offset(0), 8), b"abcdef").map_err(|error| error.errno()),
        Ok(WriteResult {
            offset: 0,
            bytes: 6,
            limited: false
        })
    );
    assert_eq!(
        fs.write(&node, req(WriteAt::Append, 8), b"ghij").map_err(|error| error.errno()),
        Ok(WriteResult {
            offset: 6,
            bytes: 2,
            limited: true
        })
    );
    fs.setattr(
        ino,
        SetAttr {
            mode: Some(0o6750),
            ..SetAttr::default()
        },
    )
    .unwrap();
    let before = fs.getattr(ino).unwrap();
    let free = fs.statfs().unwrap().free_blocks;
    for at in [WriteAt::Append, WriteAt::Offset(8), WriteAt::Offset(9)] {
        assert_eq!(fs.write(&node, req(at, 8), b"x").map_err(|error| error.errno()), Err(Errno(EFBIG)));
    }
    assert_eq!(
        fs.write(&node, req(WriteAt::Offset(9), 8), b"").map_err(|error| error.errno()),
        Ok(WriteResult {
            offset: 9,
            bytes: 0,
            limited: false
        })
    );
    assert_eq!(fs.getattr(ino).unwrap(), before);
    assert_eq!(fs.statfs().unwrap().free_blocks, free);
    // Explicit-offset writes use the same clamp and preserve the tail.
    assert_eq!(
        fs.write(&node, req(WriteAt::Offset(2), 4), b"XYZ").map_err(|error| error.errno()),
        Ok(WriteResult {
            offset: 2,
            bytes: 2,
            limited: true
        })
    );
    let mut bytes = [0; 8];
    assert_eq!(fs.read(&node, 0, &mut bytes), Ok(8));
    assert_eq!(&bytes, b"abXYefgh");
}

#[test]
fn read_moves_stale_atime_only() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let ino = regular(fs, fs.root().unwrap(), b"read-atime");
    let node = fs.open(ino).unwrap();
    fs.write(
        &node,
        WriteReq {
            at: WriteAt::Offset(0),
            limit: u64::MAX,
            clear_setid: false,
        },
        b"data",
    ).map_err(|error| error.errno())
    .unwrap();
    // The host clock is 1. Write exact times without setattr changing ctime.
    let set_times = |atime, mtime, ctime| {
        let mut guard = crate::host_mounts::write_mount(disk.pin()).unwrap();
        let storage = guard.as_mut().unwrap();
        let mut inode = storage.read_inode(ino as u32).unwrap();
        inode.i_atime = atime;
        inode.i_mtime = mtime;
        inode.i_ctime = ctime;
        storage.write_inode(ino as u32, &inode).unwrap();
    };
    let mut bytes = [0; 4];
    set_times(0, 0, 0);
    assert_eq!(fs.read(&node, 4, &mut bytes), Ok(0));
    assert_eq!(fs.getattr(ino).unwrap().times.atime.sec, 0);
    assert_eq!(fs.read(&node, 0, &mut bytes[..0]), Ok(0));
    assert_eq!(fs.getattr(ino).unwrap().times.atime.sec, 0);
    let before = fs.getattr(ino).unwrap();
    assert_eq!(fs.read(&node, 0, &mut bytes), Ok(4));
    assert_eq!(&bytes, b"data");
    let after = fs.getattr(ino).unwrap();
    assert_eq!(after.times.atime.sec, 1);
    assert_eq!(after.times.mtime, before.times.mtime);
    assert_eq!(after.times.ctime, before.times.ctime);
    assert_eq!(after.attrs, before.attrs);
    assert_eq!(node.times.lock().atime.sec, 1);
    set_times(2, 0, 0);
    assert_eq!(fs.read(&node, 0, &mut bytes), Ok(4));
    assert_eq!(fs.getattr(ino).unwrap().times.atime.sec, 2);
    set_times(0, 0, 0);
    disk.fail_next_write();
    assert_eq!(fs.read(&node, 0, &mut bytes), Ok(4));
    assert_eq!(&bytes, b"data");
    assert_eq!(fs.getattr(ino).unwrap().times.atime.sec, 0);
    disk.remount_read_only();
    assert_eq!(fs.read(&node, 0, &mut bytes), Ok(4));
    assert_eq!(fs.getattr(ino).unwrap().times.atime.sec, 0);
}

#[test]
fn fsize_refusal_is_distinct_from_filesystem_efbig() {
    use crate::vfs::WriteError;
    let disk = Fixture::new();
    let fs = Ext2Adapter::new(disk.pin());
    let ino = regular(&fs, fs.root().unwrap(), b"fsize-signal");
    let node = fs.open(ino).unwrap();
    let req = |at, limit| WriteReq { at, limit, clear_setid: false };
    let short = fs.write(&node, req(WriteAt::Offset(0), 2), b"abcd").unwrap();
    assert_eq!(short.bytes, 2);
    for at in [WriteAt::Append, WriteAt::Offset(2)] {
        assert_eq!(fs.write(&node, req(at, 2), b"x"), Err(WriteError::FileSizeLimit));
    }
    assert_eq!(fs.write(&node, req(WriteAt::Offset(fs.max_file_size().unwrap()), u64::MAX), b"x"),
        Err(WriteError::Filesystem(Errno(EFBIG))));
}

#[test]
fn batched_dirents_supply_resumable_cookies() {
    use std::sync::Arc;
    use crate::vfs::file::OpenFile;
    let disk = Fixture::new();
    let fs = Arc::new(Ext2Adapter::new(disk.pin()));
    let root = fs.create(fs.root().unwrap(), b"batch", NewNode::Directory { attrs: attrs(0o755) }).unwrap();
    for name in [b"a", b"b", b"c"] { regular(fs.as_ref(), root, name); }
    let open = OpenFile { node: fs.open(root).unwrap(), fs };
    let mut batch = Vec::new();
    let end = open.dirents(0, &mut |entry, cookie| { batch.push((entry, cookie)); Ok(true) }).unwrap();
    assert_eq!(batch.len(), 5);
    assert_eq!(end, batch.last().unwrap().1);
    for (index, (_, cookie)) in batch.iter().enumerate() {
        let mut resumed = Vec::new();
        open.dirents(*cookie, &mut |entry, next| { resumed.push((entry, next)); Ok(true) }).unwrap();
        assert_eq!(resumed, batch[index + 1..]);
    }
}

#[test]
fn removal_rechecks_identity_owner_and_parent_policy() {
    let disk = Fixture::new();
    let adapter = Ext2Adapter::new(disk.pin());
    let fs: &dyn FileSystem = &adapter;
    let root = fs.root().unwrap();
    for directory in [false, true] {
        let victim = fs.create(root, b"victim", if directory {
            NewNode::Directory { attrs: attrs(0o755) }
        } else {
            NewNode::Regular { attrs: attrs(0o644) }
        }).unwrap();
        let parent = fs.getattr(root).unwrap().attrs;
        let owner = fs.getattr(victim).unwrap().attrs.uid;
        let request = Remove { directory, expected: victim, owner, parent };
        fs.rename(root, b"victim", root, b"moved", RenameMode::Replace, &|_, _, _, _| Ok(())).unwrap();
        let replacement = fs.create(root, b"victim", if directory {
            NewNode::Directory { attrs: attrs(0o755) }
        } else {
            NewNode::Regular { attrs: attrs(0o644) }
        }).unwrap();
        assert_eq!(fs.remove(root, b"victim", request), Err(Errno(EAGAIN)));
        assert_eq!(fs.lookup(root, b"victim"), Ok(replacement));
        assert_eq!(fs.getattr(replacement).unwrap().nlink, if directory { 2 } else { 1 });

        fs.setattr(victim, SetAttr { uid: Some(owner + 1), ..SetAttr::default() }).unwrap();
        assert_eq!(fs.remove(root, b"moved", request), Err(Errno(EAGAIN)));
        assert_eq!(fs.lookup(root, b"moved"), Ok(victim));
        let request = Remove { owner: owner + 1, ..request };
        fs.setattr(root, SetAttr { mode: Some(0o1777), ..SetAttr::default() }).unwrap();
        assert_eq!(fs.remove(root, b"moved", request), Err(Errno(EAGAIN)));
        assert_eq!(fs.lookup(root, b"moved"), Ok(victim));
        let request = Remove { parent: fs.getattr(root).unwrap().attrs, ..request };
        fs.remove(root, b"moved", request).unwrap();
        assert_eq!(fs.lookup(root, b"moved"), Err(Errno(ENOENT)));
        fs.remove(root, b"victim", Remove { expected: replacement, owner, ..request }).unwrap();
        fs.setattr(root, SetAttr { mode: Some(0o755), ..SetAttr::default() }).unwrap();
    }
}
