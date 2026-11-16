use breenfs_host_tests::{
    corrupt::{self, Kind},
    format::BLOCK_SIZE,
    recording::SECTOR_SIZE,
    recover, Fs, WriterAdapter,
};
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    process::Command,
    sync::Arc,
};
#[path = "../src/test_image.rs"]
mod test_image;

#[test]
fn bit_flips_error_never_panic_and_checker_names_the_block() {
    let dir = std::env::temp_dir().join(format!("breenfs-corruption-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let source = dir.join("source");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join("nested")).unwrap();
    fs::write(source.join("nested/file"), vec![0x5a; BLOCK_SIZE * 3 + 17]).unwrap();
    fs::write(source.join("empty"), []).unwrap();
    std::os::unix::fs::symlink("nested/file", source.join("link")).unwrap();
    let populated_path = dir.join("populated.img");
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
        .arg(&populated_path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let populated = fs::read(&populated_path).unwrap();
    test_image::check(&populated, "corruption-populated");
    let path = dir.join("flipped.img");
    let mut artifact = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut checks = 0;
    let report = corrupt::bit_flips(
        populated,
        test_image::image(),
        &[1, 2],
        8,
        |image, flip, files| {
            // Score the original corruption, before writable recovery can repair a
            // slot or choose and clean an older consistent generation.
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
            let output = Command::new(env!("CARGO_BIN_FILE_BREENFS_breenfs-check"))
                .arg("--full")
                .arg(&path)
                .output()
                .unwrap();
            let diagnostic = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                matches!(output.status.code(), Some(1 | 4 | 8)),
                "checker missed {flip:?}: {output:?}"
            );
            let names = |line: &str, number: u64| {
                // Compare decimal tokens, so block 20 cannot match block 200.
                line.split(|c: char| !c.is_ascii_digit())
                    .any(|s| s.parse::<u64>() == Ok(number))
            };
            assert!(
                diagnostic.lines().any(|line| match flip.kind {
                    Kind::Data => line.starts_with("error C1 ") && names(line, flip.block),
                    Kind::Node { addr } => names(line, flip.block) || names(line, addr),
                    Kind::Slot => names(line, flip.block),
                }),
                "checker did not name {flip:?}: {diagnostic}"
            );

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
                // Do not use the corrupted walk's result to choose the files. Read
                // holes and the partial tail too, and continue after read errors.
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
            checks += 1;
            Ok(())
        },
    )
    .unwrap();
    assert!(report.committed_images >= 3, "{report:?}");
    assert!(report.slots >= report.committed_images * 2, "{report:?}");
    assert!(report.nodes >= report.committed_images * 8, "{report:?}");
    assert!(report.data >= 4, "{report:?}");
    assert_eq!(checks, report.slots + report.nodes + report.data);
    drop(artifact);
    fs::remove_dir_all(dir).unwrap();
    eprintln!("corruption corpus: {checks} full checker invocations; {report:?}");
}
