#[cfg(unix)]
mod mkfs {
    use cowfs::btree::{self, NodeStore};
    use cowfs::format::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::os::unix::{ffi::OsStrExt, fs::MetadataExt, fs::PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicU64, Ordering};

    const SIZE: usize = 64 * 1024 * 1024;
    const ROOT_OBJECT: u64 = 0xffff_ffff_ffff_ff10;
    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "cowfs-mkfs-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn run(image: &Path, source: Option<&Path>) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cowfs-mkfs"));
        c.args([
            "--uuid",
            "00112233445566778899aabbccddeeff",
            "--hash-key",
            "000102030405060708090a0b0c0d0e0f",
            "--time",
            "123456789",
            "--label",
            "fixture",
            "--extents",
            "--size",
            &SIZE.to_string(),
        ]);
        if let Some(p) = source {
            c.arg("--populate").arg(p);
        }
        c.arg(image).output().unwrap()
    }
    fn make(image: &Path, source: Option<&Path>) -> Output {
        let out = run(image, source);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
    fn key(object: u64, item_type: u8, offset: u64) -> Key {
        Key {
            object,
            item_type,
            offset,
        }
    }
    struct Image(Vec<u8>);
    impl NodeStore for Image {
        fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
            let start = addr as usize * BLOCK_SIZE;
            self.0
                .get(start..start + NODE_SIZE)
                .map(|b| b.to_vec())
                .ok_or(btree::Error::Store)
        }
        fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> {
            panic!("read only")
        }
        fn alloc(&mut self) -> Result<u64, btree::Error> {
            panic!("read only")
        }
    }
    impl Image {
        fn sb(&self) -> Superblock {
            Superblock::decode(&self.0[17 * BLOCK_SIZE..18 * BLOCK_SIZE]).unwrap()
        }
        fn get_item(&self, root: BlockPtr, k: Key) -> Vec<u8> {
            btree::search(self, root, k).unwrap().unwrap()
        }
        fn root(&self) -> Root {
            Root::decode(&self.get_item(self.sb().space_root, key(ROOT_OBJECT, ROOT, 1))).unwrap()
        }
        fn walk(
            &self,
            ptr: BlockPtr,
            tree: u8,
            nodes: &mut BTreeSet<u64>,
            items: &mut BTreeMap<Key, Vec<u8>>,
        ) {
            assert!(nodes.insert(ptr.addr));
            let b = self.get(ptr.addr).unwrap();
            let h = NodeHeader::decode(&b).unwrap();
            assert_eq!(
                (h.addr, h.generation, h.level, h.tree, h.fs_uuid),
                (ptr.addr, 1, ptr.level, tree, self.sb().fs_uuid)
            );
            assert_eq!(ptr.csum, u32::from_le_bytes(b[8..12].try_into().unwrap()));
            for i in 0..h.nitems as usize {
                if h.level == 0 {
                    let item = LeafItemHeader::decode(&b[48 + i * 21..48 + (i + 1) * 21]).unwrap();
                    let value = b
                        [item.data_off as usize..item.data_off as usize + item.data_len as usize]
                        .to_vec();
                    assert_eq!(
                        btree::search(self, ptr, item.key).unwrap(),
                        Some(value.clone())
                    );
                    assert!(items.insert(item.key, value).is_none());
                } else {
                    let e = InternalEntry::decode(&b[48 + i * 37..48 + (i + 1) * 37]).unwrap();
                    self.walk(
                        BlockPtr {
                            addr: e.child_addr,
                            generation: e.child_generation,
                            csum: e.child_csum,
                            level: h.level - 1,
                        },
                        tree,
                        nodes,
                        items,
                    );
                }
            }
        }
        fn space(&self) -> (BTreeSet<u64>, BTreeMap<Key, Vec<u8>>) {
            let mut nodes = BTreeSet::new();
            let mut items = BTreeMap::new();
            self.walk(self.sb().space_root, 2, &mut nodes, &mut items);
            (nodes, items)
        }
        fn allocation(&self, extents: &[u8]) {
            let sb = self.sb();
            let (space_nodes, items) = self.space();
            let mut fs_nodes = BTreeSet::new();
            self.walk(self.root().root, 1, &mut fs_nodes, &mut BTreeMap::new());
            let mut covered = vec![false; sb.total_blocks as usize];
            let mut written = BTreeSet::new();
            let mut free = 0;
            for (k, v) in &items {
                let (start, len) = if k.item_type == REF {
                    let r = Ref::decode(v).unwrap();
                    assert_eq!((r.refcount, r.alloc_generation), (1, 1));
                    if r.kind == 3 {
                        assert!((1..=256).contains(&k.offset));
                        assert_eq!(r.crcs.len(), k.offset as usize);
                        for (i, crc) in r.crcs.iter().enumerate() {
                            let p = (k.object as usize + i) * BLOCK_SIZE;
                            assert_eq!(*crc, cowfs::crc32c::checksum(&self.0[p..p + BLOCK_SIZE]));
                        }
                    } else {
                        assert_eq!(k.offset, 4);
                        assert!(if r.kind == 1 {
                            fs_nodes.contains(&k.object)
                        } else {
                            r.kind == 2 && space_nodes.contains(&k.object)
                        });
                    }
                    written.extend(k.object..k.object + k.offset);
                    (k.object, k.offset)
                } else if k.item_type == FREE {
                    let len = Free::decode(v).unwrap().length;
                    free += len;
                    (k.offset, len)
                } else {
                    assert_eq!(k.item_type, ROOT);
                    continue;
                };
                assert!(start >= 20 && start + len <= sb.total_blocks - 4);
                for block in start..start + len {
                    assert!(!covered[block as usize]);
                    covered[block as usize] = true;
                }
            }
            assert!(covered[20..sb.total_blocks as usize - 4].iter().all(|b| *b));
            assert_eq!(free, sb.free_blocks);
            assert_eq!(
                sb.global_reserve,
                (sb.total_blocks / 200).clamp(4096, 262144)
            );
            for addr in fs_nodes.into_iter().chain(space_nodes) {
                assert!(items.contains_key(&key(addr, REF, 4)));
            }
            written.extend(16..20);
            written.extend(sb.total_blocks - 4..sb.total_blocks);
            let mut reported = BTreeSet::new();
            let mut previous = None;
            for line in std::str::from_utf8(extents).unwrap().lines() {
                let n: Vec<u64> = line
                    .split_whitespace()
                    .map(|s| s.parse().unwrap())
                    .collect();
                assert_eq!(n.len(), 2);
                assert!(n[1] > 0);
                if let Some(end) = previous {
                    assert!(end < n[0], "ranges must be merged");
                }
                previous = Some(n[0] + n[1]);
                for b in n[0]..n[0] + n[1] {
                    assert!(reported.insert(b));
                }
            }
            assert_eq!(reported, written);
        }
    }

    #[test]
    fn same_flags_same_bytes() {
        let s = Scratch::new();
        let source = s.path("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("z"), b"last").unwrap();
        fs::create_dir(source.join("a")).unwrap();
        fs::write(source.join("a/file"), b"nested").unwrap();
        std::os::unix::fs::symlink("a/file", source.join("link")).unwrap();
        let a = s.path("a.img");
        let b = s.path("b.img");
        let first = make(&a, Some(&source));
        let second = make(&b, Some(&source));
        assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap());
        assert_eq!(first.stdout, second.stdout);
    }

    #[test]
    fn eight_slots_and_reserved_blocks() {
        let s = Scratch::new();
        let p = s.path("image");
        // A previous image's slots and boot storage record must not leak into mkfs.
        let mut old = vec![0xa5; SIZE];
        old[..16 * BLOCK_SIZE].fill(0x5a);
        fs::write(&p, old).unwrap();
        let out = make(&p, None);
        let image = Image(fs::read(p).unwrap());
        assert_eq!(image.0.len(), SIZE);
        assert!(image.0[..16 * BLOCK_SIZE].iter().all(|b| *b == 0x5a));
        for (ring, start) in [(0, 16), (1, SIZE / BLOCK_SIZE - 4)] {
            for slot in 0..4 {
                let b = &image.0[(start + slot) * BLOCK_SIZE..(start + slot + 1) * BLOCK_SIZE];
                if slot == 1 {
                    assert!(valid_superblock_slot(
                        b,
                        (SIZE / BLOCK_SIZE) as u64,
                        ring,
                        slot as u8
                    ));
                } else {
                    assert!(b.iter().all(|b| *b == 0));
                }
            }
        }
        let sb = image.sb();
        assert_eq!((sb.generation, sb.next_root_id), (1, 2));
        assert_eq!(
            sb.commit_time,
            Time {
                seconds: 123456789,
                nanos: 0
            }
        );
        assert_eq!(sb.mkfs_time, sb.commit_time);
        assert_eq!(&sb.label[..7], b"fixture");
        let root = image.root();
        assert_eq!(
            (
                root.name.as_slice(),
                root.next_inode,
                root.source_root,
                root.flags
            ),
            (b"default".as_slice(), 16, 0, 0)
        );
        let inode = Inode::decode(&image.get_item(root.root, key(2, INODE, 0))).unwrap();
        assert_eq!(
            (
                inode.mode,
                inode.nlink,
                inode.parent,
                inode.size,
                inode.next_index
            ),
            (0o40755, 2, 2, 0, 2)
        );
        image.allocation(&out.stdout);
    }

    fn compare(image: &Image, root: BlockPtr, ino: u64, parent: u64, path: &Path, next: &mut u64) {
        let meta = fs::symlink_metadata(path).unwrap();
        let inode = Inode::decode(&image.get_item(root, key(ino, INODE, 0))).unwrap();
        assert_eq!(
            (inode.mode, inode.uid, inode.gid, inode.created_generation),
            (meta.mode() & 0o177777, 0, 0, 1)
        );
        for t in [inode.atime, inode.mtime, inode.ctime, inode.btime] {
            assert_eq!(
                t,
                Time {
                    seconds: 123456789,
                    nanos: 0
                }
            );
        }
        if meta.is_dir() {
            assert_eq!(inode.parent, parent);
            let mut entries: Vec<_> = fs::read_dir(path).unwrap().map(Result::unwrap).collect();
            entries.sort_by_key(|e| e.file_name());
            assert_eq!(
                (inode.size, inode.next_index),
                (entries.len() as u64, entries.len() as u64 + 2)
            );
            let mut links = 2;
            for (i, e) in entries.iter().enumerate() {
                let name = e.file_name();
                let child = *next;
                *next += 1;
                let d = DirRecord::decode(&image.get_item(root, key(ino, DIR_INDEX, i as u64 + 2)))
                    .unwrap();
                assert_eq!((d.name.as_slice(), d.ino), (name.as_bytes(), child));
                let hashed = image.get_item(
                    root,
                    key(
                        ino,
                        DIR_ENTRY,
                        cowfs::siphash::hash(&image.sb().dirhash_key, name.as_bytes()),
                    ),
                );
                assert_eq!(DirRecord::decode(&hashed).unwrap(), d);
                let m = fs::symlink_metadata(e.path()).unwrap();
                assert_eq!(
                    d.kind,
                    if m.is_dir() {
                        2
                    } else if m.is_file() {
                        1
                    } else {
                        7
                    }
                );
                if m.is_dir() {
                    links += 1;
                }
                compare(image, root, child, ino, &e.path(), next);
            }
            assert_eq!((inode.nlink, inode.nblocks), (links, 0));
        } else if meta.file_type().is_symlink() {
            let target = fs::read_link(path).unwrap();
            let b = image.get_item(root, key(ino, INLINE, 0));
            assert_eq!(b, target.as_os_str().as_bytes());
            assert_eq!(
                (inode.size, inode.nblocks, inode.nlink),
                (b.len() as u64, 0, 1)
            );
            assert!(btree::search(image, root, key(ino, EXTENT, 0))
                .unwrap()
                .is_none());
        } else {
            let expected = fs::read(path).unwrap();
            let mut bytes = Vec::new();
            let mut logical = 0;
            let mut blocks = 0;
            while logical < inode.size {
                let e = Extent::decode(&image.get_item(root, key(ino, EXTENT, logical))).unwrap();
                assert_eq!((e.extent_offset, e.flags), (0, 0));
                assert_eq!(e.num_blocks, e.phys_len);
                assert!((1..=256).contains(&e.num_blocks));
                let lo = e.phys_start as usize * BLOCK_SIZE;
                let len = e.num_blocks as usize * BLOCK_SIZE;
                bytes.extend_from_slice(&image.0[lo..lo + len]);
                logical += len as u64;
                blocks += e.num_blocks as u64;
            }
            assert_eq!(inode.size, expected.len() as u64);
            assert_eq!(&bytes[..expected.len()], expected);
            assert!(bytes[expected.len()..].iter().all(|b| *b == 0));
            assert_eq!((inode.nblocks, inode.nlink), (blocks, 1));
        }
    }
    #[test]
    fn populate_reads_back() {
        let s = Scratch::new();
        let src = s.path("src");
        fs::create_dir(&src).unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o750)).unwrap();
        fs::create_dir(src.join("a-dir")).unwrap();
        fs::write(src.join("a-dir/empty"), []).unwrap();
        let big: Vec<u8> = (0..256 * BLOCK_SIZE + 123)
            .map(|i| (i % 251) as u8)
            .collect();
        fs::write(src.join("big"), big).unwrap();
        std::os::unix::fs::symlink("a-dir/empty", src.join("link")).unwrap();
        std::os::unix::fs::symlink("missing", src.join("dangling")).unwrap();
        for i in 0..400 {
            fs::write(src.join(format!("file-{i:03}")), [i as u8]).unwrap();
        }
        let p = s.path("image");
        let out = make(&p, Some(&src));
        let image = Image(fs::read(&p).unwrap());
        let root = image.root();
        assert!(root.root.level > 0);
        assert!(image.sb().space_root.level > 0);
        let mut next = 16;
        compare(&image, root.root, 2, 2, &src, &mut next);
        assert_eq!(root.next_inode, next);
        image.allocation(&out.stdout);
        let _socket = std::os::unix::net::UnixListener::bind(src.join("socket")).unwrap();
        let refused = run(&s.path("refused"), Some(&src));
        assert!(!refused.status.success());
        let msg = String::from_utf8_lossy(&refused.stderr);
        assert!(
            msg.contains("unsupported") && msg.contains("socket"),
            "{msg}"
        );
    }
}
