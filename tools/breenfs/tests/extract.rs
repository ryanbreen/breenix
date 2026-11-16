use breenfs::block::{BlockDevice, BlockError};
use breenfs::{commit::DeviceStore, format::BLOCK_SIZE, ops::Files, recover};
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
