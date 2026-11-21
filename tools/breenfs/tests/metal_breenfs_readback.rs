//! Exercise the firmware's allocation planner with a populated BreenFS image.
#[path = "../../../scripts/metal/efi-stub/src/breenfs_allocation.rs"]
mod allocation;

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn check(image: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_breenfs-check"))
        .arg("--full")
        .arg(image)
        .output()
        .unwrap()
}

#[test]
fn readback_preserves_populated_image_and_checker_detects_a_missing_data_block() {
    let scratch = std::env::temp_dir().join(format!("metal-readback-{}", std::process::id()));
    fs::create_dir(&scratch).unwrap();
    let scratch = Scratch(scratch);
    let tree = scratch.0.join("tree");
    fs::create_dir(&tree).unwrap();
    let data = [0xa5; allocation::BLOCK];
    fs::write(tree.join("payload"), data.repeat(2)).unwrap();
    let image = scratch.0.join("source.img");
    let mkfs = Command::new(env!("CARGO_BIN_EXE_breenfs-mkfs"))
        .args(["--size", "67108864", "--extents", "--populate"])
        .arg(&tree)
        .arg(&image)
        .output()
        .unwrap();
    assert!(
        mkfs.status.success(),
        "{}",
        String::from_utf8_lossy(&mkfs.stderr)
    );
    let expected: Vec<_> = std::str::from_utf8(&mkfs.stdout)
        .unwrap()
        .lines()
        .flat_map(|line| {
            let (first, count) = line.split_once(' ').unwrap();
            let first = first.parse::<usize>().unwrap();
            first..first + count.parse::<usize>().unwrap()
        })
        .collect();

    let mut source = File::open(&image).unwrap();
    let size = source.metadata().unwrap().len();
    let total = size as usize / allocation::BLOCK;
    let mut bitmap = vec![0; total.div_ceil(8)];
    let mut visited = bitmap.clone();
    let mut work = vec![0; allocation::SCRATCH];
    let count = allocation::discover(
        total,
        &mut bitmap,
        &mut visited,
        &mut work,
        |block, bytes| {
            source
                .seek(SeekFrom::Start((block * allocation::BLOCK) as u64))
                .map_err(|_| ())?;
            source.read_exact(bytes).map_err(|_| ())
        },
    )
    .unwrap();
    let blocks: Vec<_> = (0..total)
        .filter(|block| bitmap[block / 8] & (1 << (block % 8)) != 0)
        .collect();
    assert_eq!(count, blocks.len());
    assert_eq!(blocks, expected);

    let cold = scratch.0.join("readback.img");
    let mut output = File::create(&cold).unwrap();
    output.set_len(size).unwrap();
    let mut bytes = [0; allocation::BLOCK];
    let mut data_offset = None;
    for block in blocks {
        let offset = (block * allocation::BLOCK) as u64;
        source.seek(SeekFrom::Start(offset)).unwrap();
        source.read_exact(&mut bytes).unwrap();
        output.seek(SeekFrom::Start(offset)).unwrap();
        output.write_all(&bytes).unwrap();
        if bytes == data {
            data_offset = Some(offset);
        }
    }
    let clean = check(&cold);
    assert!(clean.status.success(), "{clean:?}");

    output.seek(SeekFrom::Start(data_offset.unwrap())).unwrap();
    output.write_all(&[0; allocation::BLOCK]).unwrap();
    let missing = check(&cold);
    assert_eq!(missing.status.code(), Some(4), "{missing:?}");
    assert!(
        String::from_utf8_lossy(&missing.stdout).contains("error C1 "),
        "{missing:?}"
    );
}
