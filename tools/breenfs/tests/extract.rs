use breenfs::block::{BlockDevice, BlockError};
use breenfs::btree::NodeStore;
use breenfs::format::{self, Extent, LeafItemHeader, NodeHeader};
use breenfs::{commit::DeviceStore, format::BLOCK_SIZE, ops::Files, recover};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("breenfs-extract-{}-{name}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest);
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), dest).unwrap();
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}
fn make(image: &Path, source: Option<&Path>) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_breenfs-mkfs"));
    cmd.args([
        "--size",
        "67108864",
        "--uuid",
        "00112233445566778899aabbccddeeff",
        "--hash-key",
        "000102030405060708090a0b0c0d0e0f",
    ]);
    if let Some(source) = source {
        cmd.arg("--populate").arg(source);
    } else {
        cmd.args(["--time", "123456789"]);
    }
    let out = cmd.arg(image).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn extract(args: &[&std::ffi::OsStr]) -> Vec<u8> {
    // Keep this test runnable before the binary exists: absence fails at runtime.
    let fallback = Path::new(env!("CARGO_BIN_EXE_breenfs-mkfs")).with_file_name("breenfs-extract");
    let exe = option_env!("CARGO_BIN_EXE_breenfs-extract")
        .map(Path::new)
        .unwrap_or(&fallback);
    let out = Command::new(exe)
        .args(args)
        .output()
        .expect("breenfs-extract must exist");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
struct Image(Vec<u8>);
impl BlockDevice for Image {
    fn read_block(&self, block: u64, out: &mut [u8]) -> Result<(), BlockError> {
        let start = block as usize * BLOCK_SIZE;
        let bytes = self
            .0
            .get(start..start + out.len())
            .ok_or(BlockError::OutOfBounds)?;
        out.copy_from_slice(bytes);
        Ok(())
    }
    fn write_block(&self, _: u64, _: &[u8]) -> Result<(), BlockError> {
        panic!("read-only check wrote the image")
    }
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        (self.0.len() / BLOCK_SIZE) as u64
    }
    fn flush(&self) -> Result<(), BlockError> {
        panic!("read-only check flushed the image")
    }
}

#[test]
fn manifest_and_sparse_cat_match_the_source() {
    let scratch = Scratch::new("roundtrip");
    let source = scratch.0.join("source");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture"),
        &source,
    );
    fs::hard_link(source.join("small.txt"), source.join("z-hard-link")).unwrap();
    fs::write(source.join("zero-data"), [0; BLOCK_SIZE]).unwrap();
    let raw_name = std::ffi::OsStr::from_bytes(b"raw-\xc3\xa9\t\n");
    fs::write(source.join(raw_name), b"name bytes").unwrap();
    let mut sparse = File::create(source.join("sparse")).unwrap();
    sparse.write_all(&[b'A'; BLOCK_SIZE]).unwrap();
    sparse.seek(SeekFrom::Start(4 * 1024 * 1024)).unwrap();
    sparse.write_all(&[b'Z'; BLOCK_SIZE]).unwrap();
    let size = 8 * 1024 * 1024 + 23;
    sparse.set_len(size).unwrap();
    // macOS may allocate the gaps on extension; explicitly make source holes.
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        #[repr(C)]
        struct Punch {
            flags: u32,
            reserved: u32,
            offset: i64,
            length: i64,
        }
        extern "C" {
            fn fcntl(fd: i32, command: i32, ...) -> i32;
        }
        for (start, end) in [
            (BLOCK_SIZE as i64, 4 * 1024 * 1024),
            (
                4 * 1024 * 1024 + BLOCK_SIZE as i64,
                (size as i64 + 4095) / 4096 * 4096,
            ),
        ] {
            let punch = Punch {
                flags: 0,
                reserved: 0,
                offset: start,
                length: end - start,
            };
            assert_eq!(
                unsafe { fcntl(sparse.as_raw_fd(), 99, &punch) },
                0,
                "F_PUNCHHOLE: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    sparse.sync_all().unwrap();
    fs::set_permissions(source.join("sparse"), fs::Permissions::from_mode(0o644)).unwrap();
    let image = scratch.0.join("image");
    make(&image, Some(&source));
    let before = fs::read(&image).unwrap();
    let device = Image(before.clone());
    let sb = recover::recover(&device, false).unwrap();
    let files = Files::new(DeviceStore(&device), &sb, 1).unwrap();
    let ino = files.lookup(2, b"sparse").unwrap();
    assert_eq!(
        files.inode(ino).unwrap().nblocks,
        2,
        "source holes must remain unmapped"
    );
    let zero = files.lookup(2, b"zero-data").unwrap();
    assert_eq!(
        files.inode(zero).unwrap().nblocks,
        1,
        "allocated zero data is not a hole"
    );
    assert_eq!(
        files.lookup(2, b"small.txt").unwrap(),
        files.lookup(2, b"z-hard-link").unwrap()
    );
    fs::set_permissions(&image, fs::Permissions::from_mode(0o444)).unwrap();
    let actual = extract(&[image.as_os_str(), "manifest".as_ref()]);
    let expected = extract(&["manifest".as_ref(), "--dir".as_ref(), source.as_os_str()]);
    assert_eq!(actual, expected);
    let text = String::from_utf8(actual).unwrap();
    let rows: Vec<Vec<&str>> = text
        .lines()
        .map(|line| line.split('\t').collect())
        .collect();
    assert!(rows.iter().all(|row| row.len() == 11));
    assert!(rows.windows(2).all(|w| w[0][0] < w[1][0]));
    let sparse_row = rows.iter().find(|r| r[0] == "2f737061727365").unwrap();
    assert_eq!(
        &sparse_row[1..7],
        &["reg", "100644", "0", "0", "8388631", "16"]
    );
    let small = rows
        .iter()
        .find(|r| r[0] == "2f736d616c6c2e747874")
        .unwrap();
    assert_eq!(small[10], "2f736d616c6c2e747874");
    assert_eq!(
        small[9],
        "b6b8b1322b941bc55153f4c68d8f594dd3d76e00033e49611600df59d3792fb6"
    );
    let cat = extract(&[image.as_os_str(), "cat".as_ref(), "/sparse".as_ref()]);
    assert_eq!(cat.len() as u64, size);
    assert_eq!(&cat[..BLOCK_SIZE], &[b'A'; BLOCK_SIZE]);
    assert!(cat[BLOCK_SIZE..4 * 1024 * 1024].iter().all(|&b| b == 0));
    assert_eq!(
        &cat[4 * 1024 * 1024..4 * 1024 * 1024 + BLOCK_SIZE],
        &[b'Z'; BLOCK_SIZE]
    );
    assert!(cat[4 * 1024 * 1024 + BLOCK_SIZE..].iter().all(|&b| b == 0));
    let check = Command::new(env!("CARGO_BIN_EXE_breenfs-check"))
        .arg("--full")
        .arg(&image)
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
    assert_eq!(fs::read(image).unwrap(), before);
}

#[test]
fn ls_and_dump_have_fixed_text() {
    let scratch = Scratch::new("text");
    let source = scratch.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join("dir")).unwrap();
    fs::write(source.join("z"), b"abc").unwrap();
    std::os::unix::fs::symlink("z", source.join("link")).unwrap();
    let image = scratch.0.join("image");
    make(&image, Some(&source));
    assert_eq!(
        extract(&[image.as_os_str(), "ls".as_ref(), "/".as_ref()]),
        b"dir\t2f646972\nsymlink\t2f6c696e6b\nreg\t2f7a\n"
    );
    assert_eq!(
        extract(&[image.as_os_str(), "ls".as_ref(), "/z".as_ref()]),
        b"reg\t2f7a\n"
    );
    let empty = scratch.0.join("empty");
    make(&empty, None);
    assert_eq!(extract(&[empty.as_os_str(), "dump".as_ref()]), concat!(
        "superblock generation=1 uuid=00112233445566778899aabbccddeeff total_blocks=16384 free_blocks=16352\n",
        "space object=20 type=80 offset=4 data=01000000020000000100000000000000\n",
        "space object=24 type=80 offset=4 data=01000000010000000100000000000000\n",
        "space object=18446744073709551376 type=90 offset=1 data=18000000000000000100000000000000e1641bc90000000010000000000000000000000000000000010000000000000015cd5b070000000000000000000000000764656661756c74\n",
        "space object=18446744073709551392 type=81 offset=28 data=e03f000000000000\n",
        "fs root=1 object=2 type=01 offset=0 data=ed41000002000000000000000000000000000000000000000000000000000000020000000000000000000000000000000200000000000000010000000000000015cd5b07000000000000000015cd5b07000000000000000015cd5b07000000000000000015cd5b07000000000000000000000000000000000000000000000000\n",
    ).as_bytes());
}

fn node_objects(device: &Image, addr: u64, inode_leaves: &mut BTreeMap<u64, u64>) -> BTreeSet<u64> {
    let bytes = DeviceStore(device).get(addr).unwrap();
    let header = NodeHeader::decode(&bytes).unwrap();
    let mut objects = BTreeSet::new();
    for i in 0..header.nitems as usize {
        if header.level == 0 {
            let item = LeafItemHeader::decode(&bytes[48 + i * 21..]).unwrap();
            objects.insert(item.key.object);
            if item.key.item_type == format::INODE {
                inode_leaves.insert(item.key.object, addr);
            }
        } else {
            let child = format::InternalEntry::decode(&bytes[48 + i * 37..]).unwrap();
            objects.extend(node_objects(device, child.child_addr, inode_leaves));
        }
    }
    objects
}

#[test]
fn blocks_and_owners_cover_the_populated_image() {
    let scratch = Scratch::new("owners");
    let source = scratch.0.join("source");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture"),
        &source,
    );
    fs::hard_link(source.join("small.txt"), source.join("z-hard-link")).unwrap();
    fs::write(source.join("empty"), []).unwrap();
    fs::write(source.join("multi"), vec![b'M'; 2 * BLOCK_SIZE + 17]).unwrap();
    // Enough small items to exercise internal FS nodes as well as leaves.
    for i in 0..64 {
        fs::write(source.join(format!("file-{i:02}")), b"x").unwrap();
    }
    let image = scratch.0.join("image");
    make(&image, Some(&source));
    let before = fs::read(&image).unwrap();
    let device = Image(before.clone());
    let sb = recover::recover(&device, false).unwrap();
    let files = Files::new(DeviceStore(&device), &sb, 1).unwrap();
    let items = files.items().unwrap();
    let space = breenfs::ops::items(&DeviceStore(&device), sb.space_root).unwrap();
    let root = space
        .iter()
        .find(|(k, _)| k.item_type == format::ROOT)
        .unwrap();
    let root = format::Root::decode(&root.1).unwrap().root;
    assert!(root.level > 0, "fixture must exercise internal nodes");
    let mut leaves = BTreeMap::new();
    node_objects(&device, root.addr, &mut leaves);

    let mut names: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
    let mut regular = Vec::new();
    let mut todo = vec![(source.clone(), "/".to_string(), 2)];
    while let Some((host, path, ino)) = todo.pop() {
        names.entry(ino).or_default().insert(path.clone());
        let meta = fs::symlink_metadata(&host).unwrap();
        if meta.is_dir() {
            for entry in fs::read_dir(host).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name();
                let child = files.lookup(ino, name.as_bytes()).unwrap();
                let child_path =
                    format!("{}/{}", path.trim_end_matches('/'), name.to_str().unwrap());
                todo.push((entry.path(), child_path, child));
            }
        } else if meta.is_file() {
            regular.push((path, ino));
        }
    }
    fs::set_permissions(&image, fs::Permissions::from_mode(0o444)).unwrap();
    let owners = |block: u64| {
        String::from_utf8(extract(&[
            "owners".as_ref(),
            image.as_os_str(),
            block.to_string().as_ref(),
        ]))
        .unwrap()
    };
    let mut data_owners: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
    for (path, ino) in &regular {
        let mut expected = String::new();
        for (key, bytes) in items
            .iter()
            .filter(|(k, _)| k.object == *ino && k.item_type == format::EXTENT)
        {
            let extent = Extent::decode(bytes).unwrap();
            for n in 0..extent.num_blocks as u64 {
                let block = extent.phys_start + extent.extent_offset as u64 + n;
                let offset = key.offset + n * BLOCK_SIZE as u64;
                expected.push_str(&format!("data {block} {offset}\n"));
                data_owners
                    .entry(block)
                    .or_default()
                    .insert(format!("data {path} offset={offset}\n"));
            }
        }
        expected.push_str(&format!("inode-leaf {}\n", leaves[ino]));
        assert_eq!(
            extract(&[image.as_os_str(), "blocks".as_ref(), path.as_ref()]),
            expected.as_bytes(),
            "{path} (existing IMAGE COMMAND order)"
        );
        assert_eq!(
            extract(&["blocks".as_ref(), image.as_os_str(), path.as_ref()]),
            expected.as_bytes(),
            "{path}"
        );
    }
    let small = files.lookup(2, b"small.txt").unwrap();
    assert_eq!(
        names[&small],
        BTreeSet::from(["/small.txt".into(), "/z-hard-link".into()])
    );
    assert!(data_owners.values().any(|paths| paths.len() == 2));
    for (block, expected) in &data_owners {
        assert_eq!(owners(*block), expected.iter().cloned().collect::<String>());
    }
    for (key, bytes) in space.iter().filter(|(k, _)| k.item_type == format::REF) {
        let reference = format::Ref::decode(bytes).unwrap();
        let expected = match reference.kind {
            1 => {
                let objects = node_objects(&device, key.object, &mut BTreeMap::new());
                let paths: BTreeSet<_> = objects
                    .iter()
                    .flat_map(|ino| names[ino].iter().cloned())
                    .collect();
                let bytes = DeviceStore(&device).get(key.object).unwrap();
                let level = NodeHeader::decode(&bytes).unwrap().level;
                format!(
                    "fs-node level={level} {}\n",
                    paths.into_iter().collect::<Vec<_>>().join(" ")
                )
            }
            2 => "space-node\n".into(),
            3 => {
                for block in key.object..key.object + key.offset {
                    assert!(
                        data_owners.contains_key(&block),
                        "allocated data block {block}"
                    );
                }
                continue;
            }
            kind => panic!("unexpected REF kind {kind}"),
        };
        for block in key.object..key.object + key.offset {
            assert_eq!(owners(block), expected, "allocated block {block}");
        }
    }
    for block in (16..20).chain(sb.total_blocks - 4..sb.total_blocks) {
        assert_eq!(owners(block), "superblock\n");
    }
    let free = space
        .iter()
        .find(|(k, _)| k.item_type == format::FREE)
        .unwrap()
        .0
        .offset;
    assert_eq!(owners(free), "free\n");
    assert_eq!(fs::read(image).unwrap(), before);
}
