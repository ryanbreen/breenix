//! Minimized crash and bit-flip cases, each replayed through the acceptance of
//! the corpus that found it. Rows come from `minimize::Case::regression_row`.
use breenfs_host_tests::{
    corrupt::{self, Flip},
    crash::{self, Image},
    format::BLOCK_SIZE,
    minimize::{Case, Fault, Outcome},
    recording::SECTOR_SIZE,
    recover, Fs, WriterAdapter,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path,
    process::{Command, Output},
    sync::Arc,
};
#[path = "../src/test_image.rs"]
mod test_image;

#[rustfmt::skip] // One emitted row per line.
fn cases() -> Vec<Case> {
    vec![
        // 24.9-b: garbage-tailed A2/B2 at the publication flush survived writable
        // recovery as invalid slots (checker exit 4). Located by crash::case_at
        // (seed 1, 8 operations, boundary 1, image 0) and minimized against the
        // recovery of 021da596. 24.9-c's bit-flip corpus had no failing case.
        Case { seed: 1, operation_count: 1, keep: vec![0], fault: Fault::Crash { boundary: 1, writes: 16, selected: vec![], tail: Some(crash::TornTail::Garbage), tear_seed: 0xd1b54a32d192ed02 } },
    ]
}

/// Write the sparse image to the artifact file and run the full checker on it.
fn checker(artifact: &mut File, path: &Path, image: &Image) -> Output {
    artifact.set_len(0).unwrap();
    artifact
        .set_len(image.sector_count * SECTOR_SIZE as u64)
        .unwrap();
    for (&sector, bytes) in &image.sectors {
        artifact
            .seek(SeekFrom::Start(sector * SECTOR_SIZE as u64))
            .unwrap();
        artifact.write_all(bytes).unwrap();
    }
    artifact.flush().unwrap();
    Command::new(env!("CARGO_BIN_FILE_BREENFS_breenfs-check"))
        .arg("--full")
        .arg(path)
        .output()
        .unwrap()
}

/// The populated formatter image of tests/corruption.rs, which bit-flip rows
/// with no operation flip directly.
fn populated(dir: &Path) -> Vec<u8> {
    let source = dir.join("source");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join("nested")).unwrap();
    fs::write(source.join("nested/file"), vec![0x5a; BLOCK_SIZE * 3 + 17]).unwrap();
    fs::write(source.join("empty"), []).unwrap();
    std::os::unix::fs::symlink("nested/file", source.join("link")).unwrap();
    let path = dir.join("populated.img");
    let output = Command::new(env!("CARGO_BIN_FILE_BREENFS_breenfs-mkfs"))
        .args([
            "--size",
            "67108864",
            "--time",
            "0",
            "--uuid",
            "00112233445566778899aabbccddeeff",
            "--hash-key",
            "000102030405060708090a0b0c0d0e0f",
            "--populate",
        ])
        .arg(&source)
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let populated = fs::read(&path).unwrap();
    test_image::check(&populated, "regressions-populated");
    populated
}

/// tests/corruption.rs's acceptance for one flipped image.
fn flip_accepted(output: Output, image: &Image, flip: Flip, files: &[(u64, u64)]) {
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        corrupt::checker_detected(output.status.code(), &diagnostic, flip),
        "checker did not report {flip:?}: {diagnostic}"
    );
    if matches!(flip.kind, corrupt::Kind::Node { .. }) {
        assert_eq!(
            corrupt::read_flipped_node(image, flip),
            Err(breenfs_host_tests::Errno::EIO.into()),
            "node read accepted {flip:?}"
        );
    }
    for writable in [false, true] {
        let device = Arc::new(corrupt::device(image.clone()));
        let result = catch_unwind(AssertUnwindSafe(|| {
            recover::recover(device.as_ref(), writable)
        }));
        assert!(
            result.is_ok(),
            "recovery panicked writable={writable} {flip:?}"
        );
        if !writable {
            assert_eq!(device.image(), *image, "read-only recovery wrote {flip:?}");
        }
    }
    let device = Arc::new(corrupt::device(image.clone()));
    let mounted = catch_unwind(AssertUnwindSafe(|| WriterAdapter::new(device)));
    assert!(mounted.is_ok(), "adapter construction panicked {flip:?}");
    if let Ok(writer) = mounted.unwrap() {
        assert!(
            catch_unwind(AssertUnwindSafe(|| writer.state())).is_ok(),
            "state walk panicked {flip:?}"
        );
        for &(ino, size) in files {
            let result = catch_unwind(AssertUnwindSafe(|| {
                for offset in (0..=size).step_by(BLOCK_SIZE) {
                    let mut bytes = [0; BLOCK_SIZE];
                    let _ = writer.read(ino, offset, &mut bytes);
                }
            }));
            assert!(result.is_ok(), "file read panicked ino={ino} {flip:?}");
        }
    }
}

#[test]
fn every_minimized_case_replays() {
    let dir = std::env::temp_dir().join(format!("breenfs-regressions-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let path = dir.join("case.img");
    let mut artifact = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .unwrap();
    for case in cases() {
        assert!(case.well_formed(), "{case:?}");
        let outcome = match &case.fault {
            // tests/crash_images.rs: the recovered image passes the full checker;
            // replay_case checks recovery and the model commit bounds.
            Fault::Crash { .. } => crash::replay_case(test_image::image(), &case, |image| {
                let output = checker(&mut artifact, &path, image);
                assert_eq!(output.status.code(), Some(0), "{case:?}: {output:?}");
                Ok(())
            }),
            Fault::Flip(flip) => {
                let base = match flip.operation {
                    None => populated(&dir),
                    Some(_) => test_image::image(),
                };
                corrupt::replay_flip(base, &case, |image, flip, files| {
                    flip_accepted(checker(&mut artifact, &path, image), image, flip, files);
                    Ok(())
                })
            }
        };
        assert_eq!(outcome.unwrap(), Outcome::Accepted, "{case:?}");
    }
    drop(artifact);
    fs::remove_dir_all(dir).unwrap();
}
