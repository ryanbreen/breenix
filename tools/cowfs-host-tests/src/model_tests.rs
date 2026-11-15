fn call(fs: &mut impl Fs, action: Action) -> Result<Reply> {
    fs.execute(&Operation {
        at: Time {
            seconds: 1,
            nanos: 0,
        },
        action,
    })
}
fn mkdir(fs: &mut impl Fs, dir: Ino, name: &[u8]) -> Ino {
    match call(
        fs,
        Action::Mkdir {
            dir,
            name: name.to_vec(),
            mode: 0o755,
        },
    )
    .unwrap()
    {
        Reply::Inode(ino) => ino,
        other => panic!("mkdir returned {other:?}"),
    }
}
fn create(fs: &mut impl Fs, dir: Ino, name: &[u8]) -> Ino {
    match call(
        fs,
        Action::Create {
            dir,
            name: name.to_vec(),
            mode: 0o644,
        },
    )
    .unwrap()
    {
        Reply::Inode(ino) => ino,
        other => panic!("create returned {other:?}"),
    }
}

#[test]
fn rmdir_nonempty_enotempty() {
    let mut fs = Model::new();
    let dir = mkdir(&mut fs, ROOT, b"dir");
    create(&mut fs, dir, b"child");
    let before = fs.state().unwrap();
    assert_eq!(
        call(
            &mut fs,
            Action::Rmdir {
                dir: ROOT,
                name: b"dir".to_vec()
            }
        ),
        Err(Errno::ENOTEMPTY.into())
    );
    assert_eq!(fs.state().unwrap(), before);
}

#[test]
fn rename_dir_over_nonempty_dir() {
    let mut fs = Model::new();
    mkdir(&mut fs, ROOT, b"source");
    let dest = mkdir(&mut fs, ROOT, b"dest");
    create(&mut fs, dest, b"child");
    let before = fs.state().unwrap();
    assert_eq!(
        call(
            &mut fs,
            Action::Rename {
                from_dir: ROOT,
                from: b"source".to_vec(),
                to_dir: ROOT,
                to: b"dest".to_vec()
            }
        ),
        Err(Errno::ENOTEMPTY.into())
    );
    assert_eq!(fs.state().unwrap(), before);
}

#[test]
fn rename_into_own_subtree_einval() {
    let mut fs = Model::new();
    let dir = mkdir(&mut fs, ROOT, b"dir");
    let child = mkdir(&mut fs, dir, b"child");
    let before = fs.state().unwrap();
    assert_eq!(
        call(
            &mut fs,
            Action::Rename {
                from_dir: ROOT,
                from: b"dir".to_vec(),
                to_dir: child,
                to: b"moved".to_vec()
            }
        ),
        Err(Errno::EINVAL.into())
    );
    assert_eq!(fs.state().unwrap(), before);
}

#[test]
fn link_to_dir_eperm() {
    let mut fs = Model::new();
    let dir = mkdir(&mut fs, ROOT, b"dir");
    let before = fs.state().unwrap();
    assert_eq!(
        call(
            &mut fs,
            Action::Link {
                ino: dir,
                dir: ROOT,
                name: b"alias".to_vec()
            }
        ),
        Err(Errno::EPERM.into())
    );
    assert_eq!(fs.state().unwrap(), before);
}

#[test]
fn seed_1_create_write_remount() {
    let mut model = Model::new();
    let mut writer = WriterAdapter;
    let operations: alloc::vec::Vec<_> = crate::stream::Stream::new(1).take(4).collect();
    assert_eq!(operations.len(), 4);
    assert!(matches!(operations[0].action, Action::Create { .. }));
    assert!(matches!(operations[1].action, Action::Write { .. }));
    assert!(matches!(operations[2].action, Action::Fsync { .. }));
    assert_eq!(operations[3].action, Action::Remount);
    for op in operations {
        let expected = model.execute(&op).unwrap();
        assert_eq!(writer.execute(&op), Ok(expected), "{op:?}");
        assert!(compare(&model, &writer).unwrap().is_empty(), "{op:?}");
    }
}

#[test]
fn sparse_links_metadata_and_remount() {
    let mut fs = Model::new();
    let at = Time {
        seconds: 12,
        nanos: 34,
    };
    let file = create(&mut fs, ROOT, b"file");
    fs.write(file, 4095, b"ab\0cd", at).unwrap();
    fs.truncate(file, 4097, at).unwrap();
    fs.truncate(file, 4100, at).unwrap();
    let mut bytes = [99; 8];
    assert_eq!(fs.read(file, 4092, &mut bytes).unwrap(), 8);
    assert_eq!(bytes, [0, 0, 0, b'a', b'b', 0, 0, 0]);
    assert_eq!(fs.read(file, 4100, &mut bytes).unwrap(), 0);
    fs.link(file, ROOT, b"alias", at).unwrap();
    fs.rename(ROOT, b"file", ROOT, b"alias", at).unwrap();
    assert_eq!(fs.lookup(ROOT, b"file"), Ok(file)); // Same-inode rename is a no-op.
    fs.unlink(ROOT, b"file", at).unwrap();
    let attrs = SetAttr {
        mode: Some(0o640),
        uid: Some(7),
        gid: Some(8),
        atime: Some(Time {
            seconds: -1,
            nanos: 999,
        }),
        mtime: Some(at),
    };
    fs.setattr(file, &attrs, at).unwrap();
    fs.setxattr(file, b"user.keep", b"value\0", at).unwrap();
    fs.setxattr(file, b"user.remove", b"old", at).unwrap();
    fs.removexattr(file, b"user.remove", at).unwrap();
    let symlink = fs.symlink(ROOT, b"symbol", b"alias", at).unwrap();
    let left = mkdir(&mut fs, ROOT, b"left");
    let right = mkdir(&mut fs, ROOT, b"right");
    let moved = mkdir(&mut fs, left, b"moved");
    let replaced = mkdir(&mut fs, right, b"replaced");
    fs.rename(left, b"moved", right, b"replaced", at).unwrap();
    let state = fs.state().unwrap();
    assert!(!state.inodes.contains_key(&replaced));
    assert_eq!(state.inodes[&left].metadata.nlink, 2);
    assert_eq!(state.inodes[&right].metadata.nlink, 3);
    assert_eq!(
        state.inodes[&moved].contents,
        Contents::Directory {
            parent: right,
            entries: BTreeMap::new()
        }
    );
    let meta = &state.inodes[&file].metadata;
    assert_eq!(
        (meta.mode, meta.uid, meta.gid, meta.nlink),
        (0o100640, 7, 8, 1)
    );
    assert_eq!(
        (meta.atime, meta.mtime, meta.ctime),
        (attrs.atime.unwrap(), at, at)
    );
    assert_eq!(
        state.inodes[&file].xattrs,
        BTreeMap::from([(b"user.keep".to_vec(), b"value\0".to_vec())])
    );
    assert_eq!(
        state.inodes[&symlink].contents,
        Contents::Symlink(b"alias".to_vec())
    );
    fs.fsync(file).unwrap();
    assert_eq!(fs.durable_state(), &state); // fsync includes the shared namespace transaction.
    fs.remount().unwrap();
    assert_eq!(fs.state().unwrap(), state);
    fs.unlink(ROOT, b"alias", at).unwrap();
    fs.unlink(ROOT, b"symbol", at).unwrap();
    fs.rmdir(right, b"replaced", at).unwrap();
    assert!(!fs.state().unwrap().inodes.contains_key(&file));
    assert_eq!(fs.state().unwrap().inodes[&right].metadata.nlink, 2);
    let before = fs.state().unwrap();
    assert_eq!(fs.truncate(symlink, 0, at), Err(Errno::ENOENT.into()));
    assert_eq!(fs.write(left, 0, b"x", at), Err(Errno::EISDIR.into()));
    assert_eq!(
        fs.removexattr(left, b"absent", at),
        Err(Errno::ENOENT.into())
    );
    assert_eq!(fs.state().unwrap(), before);
}

#[test]
fn compare_reports_whole_inode_state() {
    struct Captured(State);
    impl Fs for Captured {
        fn execute(&mut self, _op: &Operation) -> Result<Reply> {
            Err(Error::Unimplemented)
        }
        fn state(&self) -> Result<State> {
            Ok(self.0.clone())
        }
    }
    let mut model = Model::new();
    let file = create(&mut model, ROOT, b"file");
    let original = model.state().unwrap();
    let mut observed = Captured(original.clone());
    assert!(compare(&model, &observed).unwrap().is_empty());
    let inode = observed.0.inodes.get_mut(&file).unwrap();
    inode.metadata.uid = 9;
    inode
        .xattrs
        .insert(b"user.extra".to_vec(), b"extra".to_vec());
    inode.contents = Contents::File {
        size: 1,
        bytes: BTreeMap::from([(0, b'x')]),
    };
    observed.0.inodes.remove(&ROOT);
    observed
        .0
        .inodes
        .insert(100, original.inodes[&file].clone());
    let diff = compare(&model, &observed).unwrap();
    assert_eq!(
        diff.iter().map(|d| d.ino).collect::<alloc::vec::Vec<_>>(),
        [ROOT, file, 100]
    );
    assert_eq!(diff[0].actual, None);
    assert_eq!(diff[1].expected, Some(original.inodes[&file].clone()));
    assert_eq!(diff[1].actual, Some(observed.0.inodes[&file].clone()));
    assert_eq!(diff[2].expected, None);
    assert_eq!(compare(&model, &WriterAdapter), Err(Error::Unimplemented));
}

#[test]
fn seeded_stream_is_reproducible_and_keeps_namespace_consistent() {
    use crate::stream::Stream;
    let first: alloc::vec::Vec<_> = Stream::new(1).take(256).collect();
    assert_eq!(
        first,
        Stream::new(1).take(256).collect::<alloc::vec::Vec<_>>()
    );
    assert_ne!(
        first,
        Stream::new(2).take(256).collect::<alloc::vec::Vec<_>>()
    );
    let mut fs = Model::new();
    for op in first {
        let before = fs.state().unwrap();
        let result = fs.execute(&op);
        let after = fs.state().unwrap();
        if result.is_err() {
            assert_eq!(after, before, "{op:?}");
        }
        for (&ino, node) in &after.inodes {
            let references = after
                .inodes
                .values()
                .filter_map(|n| match &n.contents {
                    Contents::Directory { entries, .. } => {
                        Some(entries.values().filter(|&&i| i == ino).count())
                    }
                    _ => None,
                })
                .sum::<usize>();
            match &node.contents {
                Contents::Directory { parent, entries } => {
                    assert_eq!(references, usize::from(ino != ROOT));
                    let children = entries
                        .values()
                        .filter(|i| matches!(after.inodes[i].contents, Contents::Directory { .. }))
                        .count();
                    assert_eq!(node.metadata.nlink as usize, 2 + children);
                    if ino != ROOT {
                        let Contents::Directory {
                            entries: parents, ..
                        } = &after.inodes[parent].contents
                        else {
                            panic!("non-directory parent")
                        };
                        assert!(parents.values().any(|&i| i == ino));
                    }
                }
                Contents::File { size, bytes } => {
                    assert_eq!(node.metadata.nlink as usize, references);
                    assert!(bytes.iter().all(|(off, value)| off < size && *value != 0));
                }
                Contents::Symlink(_) => assert_eq!(node.metadata.nlink as usize, references),
            }
        }
    }
}
