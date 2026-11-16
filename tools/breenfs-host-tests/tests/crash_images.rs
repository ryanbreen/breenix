use breenfs_host_tests::{crash, recording::SECTOR_SIZE};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    process::Command,
};
#[path = "../src/test_image.rs"]
mod test_image;

#[test]
fn subset_images_recover_to_an_acknowledged_commit() {
    // Test both enumeration regimes without increasing the seeded stream corpus.
    for writes in [0, 1, 12, 13] {
        let masks = crash::subsets(writes, 0x24_09).unwrap();
        assert!(masks.iter().all(|mask| mask.len() == writes));
        if writes <= 12 {
            let unique: BTreeSet<_> = masks.iter().collect();
            assert_eq!(masks.len(), 1 << writes);
            assert_eq!(unique.len(), 1 << writes);
        } else {
            assert_eq!(masks.len(), writes * 2 + 4097);
            for prefix in 0..=writes {
                assert_eq!(
                    masks[prefix],
                    (0..writes).map(|i| i < prefix).collect::<Vec<_>>()
                );
            }
            for omission in 0..writes {
                assert_eq!(
                    masks[writes + 1 + omission],
                    (0..writes).map(|i| i != omission).collect::<Vec<_>>()
                );
            }
            let random = &masks[writes * 2 + 1..];
            assert_eq!(random.len(), 4096);
            assert!(random
                .iter()
                .any(|mask| mask.windows(2).any(|pair| pair == [false, true])));
        }
        assert_eq!(masks, crash::subsets(writes, 0x24_09).unwrap());
    }

    let path = std::env::temp_dir().join(format!("breenfs-crash-{}.img", std::process::id()));
    let mut artifact = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut checks = 0;
    let mut reports = Vec::new();
    let initial = test_image::image();
    test_image::check(&initial, "crash-base");
    for seed in [1, 2] {
        let report = crash::subset_images(initial.clone(), seed, 8, |image| {
            // Recreate a sparse file from this recovered image, including zeroed
            // recovery sectors. Truncation discards the prior checker input.
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
            assert_eq!(
                output.status.code(),
                Some(0),
                "seed={seed} image={checks}: {output:?}"
            );
            checks += 1;
            Ok(())
        })
        .unwrap();
        assert!(report.commits >= 1, "seed={seed}: {report:?}");
        assert_eq!(report.boundaries.len(), report.commits * 2);
        for boundary in &report.boundaries {
            let writes = boundary.writes;
            let expected = if writes <= 12 {
                1 << writes
            } else {
                writes * 2 + 4097
            };
            assert_eq!(boundary.images, expected, "seed={seed}: {boundary:?}");
            assert!(boundary.acknowledged <= boundary.issued);
        }
        reports.push(report);
    }
    assert!(reports
        .iter()
        .any(|r| r.boundaries.iter().any(|b| b.acknowledged > 1)));
    assert_eq!(
        checks,
        reports
            .iter()
            .flat_map(|r| &r.boundaries)
            .map(|b| b.images)
            .sum::<usize>()
    );
    assert!(checks > 8192);
    drop(artifact);
    std::fs::remove_file(path).unwrap();
    eprintln!("subset corpus: {checks} full checker invocations; {reports:?}");
}
