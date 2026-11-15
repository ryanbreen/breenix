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
