use cowfs_host_tests::{format as disk, recording::RecordingDevice, *};
use std::sync::Arc;
#[path = "../src/test_image.rs"]
mod test_image;

fn call(fs: &mut impl Fs, action: Action, tick: i64) -> Result<Reply> {
    fs.execute(&Operation {
        at: Time {
            seconds: tick,
            nanos: 23,
        },
        action,
    })
}
fn pair(writer: &mut impl Fs, model: &mut impl Fs, action: Action, tick: i64) -> Reply {
    let expected = call(model, action.clone(), tick);
    assert_eq!(call(writer, action, tick), expected);
    assert!(compare(model, writer).unwrap().is_empty());
    expected.unwrap()
}
fn setup() -> (WriterAdapter, model::Model, Arc<RecordingDevice>) {
    let device = Arc::new(RecordingDevice::new(test_image::image()).unwrap());
    (
        WriterAdapter::new(device.clone()).unwrap(),
        model::Model::new(),
        device,
    )
}
fn create(w: &mut impl Fs, m: &mut impl Fs, name: &[u8], tick: i64) -> Ino {
    match pair(
        w,
        m,
        Action::Create {
            dir: ROOT,
            name: name.to_vec(),
            mode: 0o640,
        },
        tick,
    ) {
        Reply::Inode(ino) => ino,
        other => panic!("{other:?}"),
    }
}
fn checked_commit(w: &mut impl Fs, m: &mut impl Fs, d: &RecordingDevice, ino: Ino, tick: i64) {
    pair(w, m, Action::Fsync { ino }, tick);
    test_image::check(&d.durable_image(), &format!("{ino}-{tick}"));
}
fn tree(d: &RecordingDevice) -> (disk::Superblock, Vec<(disk::Key, Vec<u8>)>) {
    use cowfs_host_tests::btree::NodeStore;
    fn items(store: &impl NodeStore, ptr: disk::BlockPtr) -> Vec<(disk::Key, Vec<u8>)> {
        let bytes = store.get(ptr.addr).unwrap();
        let h = disk::NodeHeader::decode(&bytes).unwrap();
        let mut out = Vec::new();
        for n in 0..h.nitems as usize {
            if h.level == 0 {
                let i = disk::LeafItemHeader::decode(&bytes[48 + n * 21..69 + n * 21]).unwrap();
                out.push((
                    i.key,
                    bytes[i.data_off as usize..i.data_off as usize + i.data_len as usize].to_vec(),
                ));
            } else {
                let e = disk::InternalEntry::decode(&bytes[48 + n * 37..85 + n * 37]).unwrap();
                out.extend(items(
                    store,
                    disk::BlockPtr {
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
    let sb = recover::recover(d, false).unwrap();
    let entries = items(&commit::DeviceStore(d), sb.space_root);
    (sb, entries)
}

#[test]
fn sparse_write_reads_holes_as_zero() {
    let (mut w, mut m, d) = setup();
    let ino = create(&mut w, &mut m, b"sparse", 1);
    assert_eq!(ino, 16);
    pair(
        &mut w,
        &mut m,
        Action::Write {
            ino,
            offset: 8191,
            bytes: b"ab\0cd".to_vec(),
        },
        2,
    );
    pair(
        &mut w,
        &mut m,
        Action::Write {
            ino,
            offset: 8192,
            bytes: b"Q".to_vec(),
        },
        2,
    );
    let mut read = vec![99; 8196];
    assert_eq!(w.read(ino, 0, &mut read).unwrap(), 8196);
    assert!(read[..8191].iter().all(|b| *b == 0));
    assert_eq!(&read[8191..], b"aQ\0cd");
    checked_commit(&mut w, &mut m, &d, ino, 3);
    pair(&mut w, &mut m, Action::Remount, 4);
    pair(
        &mut w,
        &mut m,
        Action::Write {
            ino,
            offset: 8192,
            bytes: b"Z".to_vec(),
        },
        5,
    );
    let attrs = SetAttr {
        mode: Some(0o6750),
        uid: Some(7),
        gid: Some(8),
        atime: Some(Time {
            seconds: -2,
            nanos: 4,
        }),
        mtime: None,
    };
    pair(&mut w, &mut m, Action::Setattr { ino, attrs }, 6);
    checked_commit(&mut w, &mut m, &d, ino, 7);
    pair(&mut w, &mut m, Action::Remount, 8);
    assert_eq!(create(&mut w, &mut m, b"next", 9), 17);
    checked_commit(&mut w, &mut m, &d, ino, 10);
}
#[test]
fn truncate_releases_extents_to_pending() {
    let (mut w, mut m, d) = setup();
    let ino = create(&mut w, &mut m, b"shorten", 1);
    pair(
        &mut w,
        &mut m,
        Action::Write {
            ino,
            offset: 0,
            bytes: vec![0x5a; 12288],
        },
        2,
    );
    checked_commit(&mut w, &mut m, &d, ino, 3);
    let (_, old_space) = tree(&d);
    let old: Vec<_> = old_space
        .iter()
        .filter(|(k, v)| k.item_type == disk::REF && disk::Ref::decode(v).unwrap().kind == 3)
        .map(|(k, _)| (k.object, k.offset))
        .collect();
    assert!(!old.is_empty());
    pair(&mut w, &mut m, Action::Truncate { ino, size: 1 }, 4);
    checked_commit(&mut w, &mut m, &d, ino, 5);
    let (sb, space) = tree(&d);
    // Shrinking to one byte releases the old data REFs, including the replaced tail.
    for (start, length) in old {
        for block in start..start + length {
            assert!(space.iter().any(|(k, v)| k.item_type == disk::PENDING && {
                let p = disk::Pending::decode(v).unwrap();
                k.offset <= block
                    && block < k.offset + p.length
                    && p.released_generation == sb.generation
            }));
        }
    }
    pair(&mut w, &mut m, Action::Truncate { ino, size: 12288 }, 6);
    checked_commit(&mut w, &mut m, &d, ino, 7);
    pair(&mut w, &mut m, Action::Remount, 8);
    pair(&mut w, &mut m, Action::Truncate { ino, size: 0 }, 9);
    checked_commit(&mut w, &mut m, &d, ino, 10);
}
#[test]
fn xattr_limits() {
    let (mut w, mut m, d) = setup();
    let ino = create(&mut w, &mut m, b"attrs", 1);
    let name = vec![b'x'; 255];
    pair(
        &mut w,
        &mut m,
        Action::Setxattr {
            ino,
            name: name.clone(),
            value: vec![42; 4096],
        },
        2,
    );
    checked_commit(&mut w, &mut m, &d, ino, 3);
    pair(&mut w, &mut m, Action::Remount, 4);
    for (name, value) in [
        (vec![], vec![]),
        (vec![b'x'; 256], vec![]),
        (b"bad\0".to_vec(), vec![]),
        (b"user.large".to_vec(), vec![0; 4097]),
    ] {
        let before = w.state().unwrap();
        let action = Action::Setxattr { ino, name, value };
        assert_eq!(call(&mut m, action.clone(), 5), Err(Errno::EINVAL.into()));
        assert_eq!(call(&mut w, action, 5), Err(Errno::EINVAL.into()));
        assert_eq!(w.state().unwrap(), before);
    }
    pair(
        &mut w,
        &mut m,
        Action::Setxattr {
            ino,
            name: name.clone(),
            value: b"replacement".to_vec(),
        },
        6,
    );
    pair(&mut w, &mut m, Action::Removexattr { ino, name }, 7);
    checked_commit(&mut w, &mut m, &d, ino, 8);
    pair(&mut w, &mut m, Action::Remount, 9);
}
