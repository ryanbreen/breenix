use breenfs::block::{BlockDevice, BlockError};
use breenfs::commit::{Committer, DeviceStore};
use breenfs::format::{Time, BLOCK_SIZE};
use breenfs::{ops::Files, recover};
use std::fs::{self, File};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Image(File);

impl BlockDevice for Image {
    fn read_block(&self, block: u64, out: &mut [u8]) -> Result<(), BlockError> {
        self.0
            .read_exact_at(out, block * BLOCK_SIZE as u64)
            .map_err(|_| BlockError::IoError)
    }

    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        self.0
            .write_all_at(bytes, block * BLOCK_SIZE as u64)
            .map_err(|_| BlockError::IoError)
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn num_blocks(&self) -> u64 {
        self.0.metadata().unwrap().len() / BLOCK_SIZE as u64
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.0.sync_all().map_err(|_| BlockError::IoError)
    }
}

#[test]
fn migration_manifest_selects_a_snapshot_by_name() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("breenfs-migration-manifest-{}", std::process::id())),
    );
    fs::create_dir(&scratch.0).unwrap();
    let image = scratch.0.join("breenfs.img");
    let built = Command::new(env!("CARGO_BIN_EXE_breenfs-mkfs"))
        .args(["--size", "67108864", "--time", "1234567890"])
        .arg(&image)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );

    let device = Image(File::options().read(true).write(true).open(&image).unwrap());
    let sb = recover::recover(&device, true).unwrap();
    let mut files = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    let time = Time {
        seconds: 1234567890,
        nanos: 0,
    };
    files.create(2, b"original", 0o644, time).unwrap();
    files.snapshot(b"before-change", true).unwrap();
    files.create(2, b"live-only", 0o644, time).unwrap();
    files.fsync(&device, &mut Committer::new(sb)).unwrap();

    let extract = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_breenfs-extract"))
            .arg(&image)
            .arg("migration-manifest")
            .args(args)
            .output()
            .unwrap()
    };
    let paths = |args: &[&str]| {
        let result = extract(args);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout)
            .unwrap()
            .lines()
            .map(|line| line.split('\t').next().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        paths(&["--root", "before-change"]),
        ["2f", "2f6f726967696e616c"]
    );
    assert_eq!(
        paths(&[]),
        ["2f", "2f6c6976652d6f6e6c79", "2f6f726967696e616c"]
    );
    let missing = extract(&["--root", "missing"]);
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
}
