//! Read subset shared by any read-only FileSystem implementation.
use crate::{
    attrs::InodeAttrs,
    block::{
        cache::{BufferCache, Wait},
        BlockDevice, BlockError,
    },
    breenfs::{
        commit::{Committer, DeviceStore},
        format::{self, Inode, Superblock, Time, BLOCK_SIZE, NODE_SIZE},
        ops::Files,
        vfs::BreenFs,
    },
    errno::Errno,
    syscall_errno::*,
    vfs::*,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        panic!("sequential conformance reads must not wait")
    }
    fn wake_all(&self) {}
}
struct Disk {
    bytes: Mutex<Vec<u8>>,
    writable: bool,
}
impl BlockDevice for Disk {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        assert_eq!(buf.len(), BLOCK_SIZE);
        let bytes = self.bytes.lock().unwrap();
        let start = usize::try_from(block)
            .ok()
            .and_then(|b| b.checked_mul(BLOCK_SIZE))
            .ok_or(BlockError::OutOfBounds)?;
        buf.copy_from_slice(
            bytes
                .get(start..start + BLOCK_SIZE)
                .ok_or(BlockError::OutOfBounds)?,
        );
        Ok(())
    }
    fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        assert!(self.writable, "read-only mount wrote to disk");
        assert_eq!(buf.len(), BLOCK_SIZE);
        let mut bytes = self.bytes.lock().unwrap();
        let start = block as usize * BLOCK_SIZE;
        bytes
            .get_mut(start..start + BLOCK_SIZE)
            .ok_or(BlockError::OutOfBounds)?
            .copy_from_slice(buf);
        Ok(())
    }
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        (self.bytes.lock().unwrap().len() / BLOCK_SIZE) as u64
    }
    fn flush(&self) -> Result<(), BlockError> {
        assert!(self.writable, "read-only mount flushed disk");
        Ok(())
    }
}
struct Tree {
    image: Vec<u8>,
    attrs: BTreeMap<u64, Inode>,
    dirs: BTreeMap<u64, BTreeMap<Vec<u8>, u64>>,
    sparse: u64,
    symlink: u64,
    big: u64,
    nested: u64,
    deep: u64,
    leaf: u64,
    sb: Superblock,
}
fn tree() -> &'static Tree {
    static TREE: OnceLock<Tree> = OnceLock::new();
    TREE.get_or_init(|| {
        let disk = Disk {
            bytes: Mutex::new(vec![0; 64 * 1024 * 1024]),
            writable: true,
        };
        let at = Time {
            seconds: 1_700_000_000,
            nanos: 123_456_789,
        };
        let sb = Superblock {
            fs_uuid: [0x42; 16],
            generation: 1,
            slot: 1,
            total_blocks: 16384,
            global_reserve: 4096,
            next_root_id: 2,
            dirhash_key: [0x19; 16],
            mkfs_time: at,
            commit_time: at,
            ..Superblock::default()
        };
        let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
        let sparse = files.create(2, b"sparse", 0o6751, at).unwrap();
        files.write(&disk, sparse, 0, b"head", at).unwrap();
        files.write(&disk, sparse, 8195, b"tail", at).unwrap();
        files.link(sparse, 2, b"alias", at).unwrap();
        files
            .setattr(
                sparse,
                &crate::breenfs::ops::SetAttr {
                    uid: Some(70000),
                    gid: Some(80000),
                    atime: Some(Time {
                        seconds: -7,
                        nanos: 999_999_999,
                    }),
                    mtime: Some(Time {
                        seconds: 42,
                        nanos: 987_654_321,
                    }),
                    ..Default::default()
                },
                at,
            )
            .unwrap();
        let symlink = files
            .symlink(2, b"symlink", b"nested/deep/leaf", at)
            .unwrap();
        let nested = files.mkdir(2, b"nested", 0o2750, at).unwrap();
        let deep = files.mkdir(nested, b"deep", 0o711, at).unwrap();
        let leaf = files.create(deep, b"leaf", 0o640, at).unwrap();
        files.write(&disk, leaf, 0, b"nested data", at).unwrap();
        let big = files.mkdir(2, b"big", 0o755, at).unwrap();
        let mut names = BTreeMap::new();
        let mut ids = vec![2, sparse, symlink, nested, deep, leaf, big];
        for n in 0..5000 {
            let name = format!("name-{n:04}").into_bytes();
            let ino = files.create(big, &name, 0o600, at).unwrap();
            ids.push(ino);
            names.insert(name, ino);
        }
        let attrs = ids
            .into_iter()
            .map(|ino| (ino, files.inode(ino).unwrap()))
            .collect();
        let dirs = BTreeMap::from([
            (
                2,
                BTreeMap::from([
                    (b"sparse".to_vec(), sparse),
                    (b"alias".to_vec(), sparse),
                    (b"symlink".to_vec(), symlink),
                    (b"nested".to_vec(), nested),
                    (b"big".to_vec(), big),
                ]),
            ),
            (nested, BTreeMap::from([(b"deep".to_vec(), deep)])),
            (deep, BTreeMap::from([(b"leaf".to_vec(), leaf)])),
            (big, names),
        ]);
        let sb = files.fsync(&disk, &mut Committer::format(sb)).unwrap();
        drop(files);
        Tree {
            image: disk.bytes.into_inner().unwrap(),
            attrs,
            dirs,
            sparse,
            symlink,
            big,
            nested,
            deep,
            leaf,
            sb,
        }
    })
}
fn stamp(t: Time) -> Timestamp {
    Timestamp {
        sec: t.seconds,
        nsec: t.nanos,
    }
}
fn kind(mode: u32) -> NodeKind {
    match mode & 0o170000 {
        0o100000 => NodeKind::Regular,
        0o040000 => NodeKind::Directory,
        0o120000 => NodeKind::Symlink,
        _ => panic!("unexpected fixture type"),
    }
}
fn attributes_equal(fs: &dyn FileSystem, tree: &Tree) {
    for (&ino, i) in &tree.attrs {
        assert_eq!(
            fs.getattr(ino).unwrap(),
            NodeAttrs {
                ino,
                kind: kind(i.mode),
                attrs: InodeAttrs::new(i.mode, i.uid, i.gid),
                nlink: u64::from(i.nlink),
                size: i.size,
                blocks: i.nblocks * 8,
                rdev: i.rdev,
                times: NodeTimes {
                    atime: stamp(i.atime),
                    mtime: stamp(i.mtime),
                    ctime: stamp(i.ctime),
                    btime: Some(stamp(i.btime))
                },
            }
        );
    }
    let sparse = fs.getattr(tree.sparse).unwrap();
    assert_eq!((sparse.nlink, sparse.size, sparse.blocks), (2, 8199, 16));
    assert_eq!(fs.parent(2), Ok(2));
    assert_eq!(fs.parent(tree.nested), Ok(2));
    assert_eq!(fs.parent(tree.deep), Ok(tree.nested));
    assert_eq!(fs.parent(tree.sparse), Err(Errno(ENOTDIR)));
    assert_eq!(fs.lookup(2, b"missing"), Err(Errno(ENOENT)));
    assert_eq!(fs.lookup(tree.sparse, b"child"), Err(Errno(ENOTDIR)));
    let root = fs.open(2).unwrap();
    for (name, ino) in &tree.dirs[&2] {
        assert_eq!(fs.lookup(2, name), Ok(*ino));
        let node = fs.lookup_open(&root, name).unwrap();
        assert_eq!(
            (
                node.ino,
                node.mount,
                node.size.load(std::sync::atomic::Ordering::Acquire)
            ),
            (*ino, root.mount, tree.attrs[ino].size)
        );
    }
    assert_eq!(fs.readlink(tree.symlink).unwrap(), b"nested/deep/leaf");
    assert_eq!(fs.readlink(tree.sparse), Err(Errno(EINVAL)));
}
fn reads_equal(fs: &dyn FileSystem, tree: &Tree) {
    let file = fs.open(tree.sparse).unwrap();
    let mut expected = vec![0; 3 * BLOCK_SIZE];
    expected[..4].copy_from_slice(b"head");
    expected[8195..8199].copy_from_slice(b"tail");
    let mut bytes = vec![0xa5; expected.len()];
    assert_eq!(fs.read(&file, 0, &mut bytes), Ok(8199));
    assert_eq!(
        bytes, expected,
        "holes and the partial block tail must be zero"
    );
    for off in [4096, 8192, 8198, 8199, 12288, u64::MAX] {
        bytes.fill(0xa5);
        let n = fs.read(&file, off, &mut bytes).unwrap();
        assert_eq!(
            n,
            8199u64.saturating_sub(off).min(bytes.len() as u64) as usize
        );
        let mut want = vec![0; bytes.len()];
        if off < 8199 {
            want[..n].copy_from_slice(&expected[off as usize..off as usize + n]);
        }
        assert_eq!(bytes, want, "offset {off}");
    }
    let leaf = fs.open(tree.leaf).unwrap();
    let mut data = [0xa5; 11];
    assert_eq!(fs.read(&leaf, 0, &mut data), Ok(11));
    assert_eq!(&data, b"nested data");
}
fn directory_equal(
    fs: &dyn FileSystem,
    dir: u64,
    expected: &BTreeMap<Vec<u8>, u64>,
    attrs: &BTreeMap<u64, Inode>,
    batch: usize,
) {
    let mut cookie = 0;
    let mut seen = BTreeMap::new();
    let mut saved = Vec::new();
    for _ in 0..=expected.len() {
        let before = cookie;
        let mut accepted = Vec::new();
        cookie = fs
            .readdir(dir, cookie, &mut |ent, next| {
                if accepted.len() == batch {
                    return Ok(false);
                }
                assert!(next > before);
                assert_eq!(ent.kind, kind(attrs[&ent.ino].mode));
                accepted.push((ent, next));
                Ok(true)
            })
            .unwrap();
        if accepted.is_empty() {
            assert_eq!(cookie, before);
            break;
        }
        assert_eq!(cookie, accepted.last().unwrap().1);
        for (ent, next) in accepted {
            assert_eq!(expected.get(&ent.name), Some(&ent.ino));
            assert!(
                seen.insert(ent.name.clone(), ent.ino).is_none(),
                "duplicate entry"
            );
            saved.push((ent, next));
        }
    }
    assert_eq!(&seen, expected);
    assert_eq!(
        fs.readdir(dir, cookie, &mut |_, _| panic!("entry after EOF")),
        Ok(cookie)
    );
    // Seek to earlier returned cookies and replay the following entry.
    for index in [0, saved.len() / 2, saved.len().saturating_sub(2)] {
        if index + 1 >= saved.len() {
            continue;
        }
        let mut replay = None;
        let end = fs
            .readdir(dir, saved[index].1, &mut |ent, next| {
                if replay.is_some() {
                    return Ok(false);
                }
                replay = Some((ent, next));
                Ok(true)
            })
            .unwrap();
        assert_eq!(replay.as_ref(), Some(&saved[index + 1]));
        assert_eq!(end, saved[index + 1].1);
    }
}
fn refusals(fs: &dyn FileSystem, tree: &Tree) {
    let node = fs.open(tree.sparse).unwrap();
    let ro = Errno(EROFS);
    assert_eq!(
        fs.write(
            &node,
            WriteReq {
                at: WriteAt::Offset(0),
                limit: u64::MAX,
                clear_setid: true
            },
            b"changed"
        ),
        Err(WriteError::Filesystem(ro))
    );
    assert_eq!(
        fs.create(
            2,
            b"new",
            NewNode::Regular {
                attrs: InodeAttrs::new(0o600, 0, 0)
            }
        ),
        Err(ro)
    );
    assert_eq!(
        fs.remove(
            2,
            b"alias",
            Remove {
                directory: false,
                expected: tree.sparse,
                owner: 70000,
                parent: fs.getattr(2).unwrap().attrs
            }
        ),
        Err(ro)
    );
    assert_eq!(
        fs.rename(
            2,
            b"alias",
            2,
            b"renamed",
            RenameMode::Replace,
            &|_, _, _, _| panic!("mutation policy called")
        ),
        Err(ro)
    );
    assert_eq!(
        fs.setattr(
            tree.sparse,
            SetAttr {
                mode: Some(0o777),
                ..Default::default()
            }
        ),
        Err(ro)
    );
    assert_eq!(
        fs.resize(&node, 0, false, &|_| panic!("resize policy called")),
        Err(ro)
    );
    assert_eq!(
        fs.link(tree.sparse, 2, b"link", &|_, _| panic!(
            "link policy called"
        )),
        Err(ro)
    );
    assert_eq!(
        fs.create_open(
            2,
            b"new",
            NodeKind::Regular,
            &|_| panic!("create policy called"),
            &mut |_| panic!("prepare called")
        ),
        Err(ro)
    );
    assert_eq!(
        fs.xattr(
            tree.sparse,
            Xattr::Set {
                name: b"user.test",
                value: b"x",
                mode: XattrSet::Upsert
            }
        ),
        Err(ro)
    );
    assert_eq!(
        fs.xattr(tree.sparse, Xattr::Remove { name: b"user.test" }),
        Err(ro)
    );
    assert_eq!(fs.clear_mapping_privilege(&node), Err(ro));
    assert_eq!(fs.sync(), Ok(()));
    assert_eq!(fs.fsync(&node, false), Ok(()));
}
#[test]
fn breenfs_read_subset_conformance() {
    let tree = tree();
    assert_eq!(tree.dirs[&tree.big].len(), 5000);
    let disk = Arc::new(Disk {
        bytes: Mutex::new(tree.image.clone()),
        writable: false,
    });
    let adapter = BreenFs::mount(disk, Arc::new(BufferCache::new(NoWait)), 17, 1).unwrap();
    let fs: &dyn FileSystem = adapter.as_ref();
    assert_eq!(fs.kind(), Ok("breenfs"));
    assert_eq!(fs.root(), Ok(2));
    assert_eq!(fs.read_only(), Ok(true));
    assert_eq!(fs.max_file_size(), Ok((1 << 63) - 4096));
    let stat = fs.statfs().unwrap();
    assert_ne!(stat.magic, 0);
    assert_eq!(
        (
            stat.block_size,
            stat.blocks,
            stat.free_blocks,
            stat.available_blocks,
            stat.max_name_len
        ),
        (
            4096,
            tree.sb.total_blocks,
            tree.sb.free_blocks,
            tree.sb.free_blocks.saturating_sub(tree.sb.global_reserve),
            255
        )
    );
    assert!(stat.flags.read_only);
    attributes_equal(fs, tree);
    reads_equal(fs, tree);
    for (&dir, names) in &tree.dirs {
        for batch in [1, 7] {
            directory_equal(fs, dir, names, &tree.attrs, batch);
        }
    }
    assert_eq!(fs.readdir(tree.big, 0, &mut |_, _| Ok(false)), Ok(0));
    assert_eq!(
        fs.readdir(tree.big, 0, &mut |_, _| Err(Errno(ENOMEM))),
        Err(Errno(ENOMEM))
    );
    refusals(fs, tree);
    attributes_equal(fs, tree);
    reads_equal(fs, tree);
}
#[test]
fn corrupt_descendant_returns_eio() {
    let tree = tree();
    let mut bytes = tree.image.clone();
    let leaf = (20..tree.sb.total_blocks as usize - 7)
        .find(|&block| {
            let node = &bytes[block * BLOCK_SIZE..block * BLOCK_SIZE + NODE_SIZE];
            let Ok(h) = format::NodeHeader::decode(node) else {
                return false;
            };
            h.tree == 1
                && h.level == 0
                && (0..h.nitems as usize).any(|i| {
                    let at = 48 + i * 21;
                    let item = format::LeafItemHeader::decode(&node[at..at + 21]).unwrap();
                    item.key == crate::breenfs::ops::key(tree.sparse, format::INODE, 0)
                })
        })
        .expect("fixture must have a descendant leaf containing the sparse inode");
    bytes[leaf * BLOCK_SIZE + NODE_SIZE - 1] ^= 1;
    let disk = Arc::new(Disk {
        bytes: Mutex::new(bytes),
        writable: false,
    });
    let adapter = BreenFs::mount(disk, Arc::new(BufferCache::new(NoWait)), 18, 1).unwrap();
    let fs: &dyn FileSystem = adapter.as_ref();
    assert_eq!(fs.getattr(tree.sparse), Err(Errno(EIO)));
    assert_eq!(fs.open(tree.sparse).unwrap_err(), Errno(EIO));
}

// Small fixture for the production omissions found by the first read boot.
fn small_read_mount() -> (Arc<BreenFs<NoWait>>, Arc<BufferCache<NoWait>>, u64, u64) {
    let disk = Arc::new(Disk {
        bytes: Mutex::new(vec![0; 64 * 1024 * 1024]),
        writable: true,
    });
    let sb = Superblock {
        fs_uuid: [0x51; 16],
        generation: 1,
        slot: 1,
        total_blocks: 16384,
        global_reserve: 4096,
        next_root_id: 2,
        ..Superblock::default()
    };
    let at = Time {
        seconds: 1,
        nanos: 0,
    };
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let tail = files.create(2, b"tail", 0o644, at).unwrap();
    files.write(disk.as_ref(), tail, 8192, b"tail", at).unwrap();
    let empty = files.create(2, b"empty", 0o644, at).unwrap();
    files
        .fsync(disk.as_ref(), &mut Committer::format(sb))
        .unwrap();
    drop(files);
    let cache = Arc::new(BufferCache::new(NoWait));
    let fs = BreenFs::mount(disk, cache.clone(), 91, 1).unwrap();
    (fs, cache, tail, empty)
}

#[test]
fn readonly_mapping_reads_holes_and_zero_fills_the_partial_tail() {
    let (fs, _, ino, _) = small_read_mount();
    let node = fs.open(ino).unwrap();
    let transfer = fs
        .mapping_io(&node)
        .expect("read-only mmap needs a transfer");
    drop(node);
    drop(fs);
    assert_eq!(transfer.size(), 8196);
    assert_eq!(transfer.read(4096, 4096).unwrap(), vec![0; 4096]);
    let bytes = transfer.read(8192, 4096).unwrap();
    assert_eq!(bytes.len(), 4096);
    assert_eq!(&bytes[..4], b"tail");
    assert!(bytes[4..].iter().all(|&byte| byte == 0));
    assert!(!transfer.needs_flush());
    assert_eq!(transfer.sync(0, u64::MAX), Ok(()));
    let mut published = 0;
    transfer
        .publish(&mut |append_only| {
            assert!(!append_only);
            published += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(published, 1);
}

#[test]
fn repeated_inode_reads_reuse_immutable_metadata_and_empty_reads_stop_at_eof() {
    let (fs, cache, _, empty) = small_read_mount();
    let expected = fs.getattr(empty).unwrap();
    let node = fs.open(empty).unwrap();
    let before = cache.counters();
    for _ in 0..16 {
        assert_eq!(fs.getattr(empty), Ok(expected));
        let mut byte = [0xa5];
        assert_eq!(fs.read(&node, 0, &mut byte), Ok(0));
        assert_eq!(byte, [0]);
    }
    assert_eq!(
        cache.counters().hits,
        before.hits,
        "repeated stat/empty reads must not decode the B-tree again"
    );
    assert_eq!(cache.counters().misses, before.misses);
}
