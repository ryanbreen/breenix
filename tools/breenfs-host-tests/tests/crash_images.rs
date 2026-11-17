use breenfs_host_tests::{
    crash::{self, Masks},
    recording::SECTOR_SIZE,
};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    process::Command,
    time::Instant,
};
#[path = "../src/test_image.rs"]
mod test_image;

// The full corpora take minutes, so the default run validates a deterministic
// sample and the opt-in breenfs battery (scripts/breenfs-battery.py) runs the
// full forms. The sample keeps both seeds and both torn tails, and in each
// flush window its writes+1 prefixes, its writes omissions and one random
// selection in SAMPLE_RANDOM; each sampled image gets writable recovery, the
// external checker and the model commit bound, as in the full corpus.
const SAMPLE: Masks = Masks::Sample(SAMPLE_RANDOM);
const SAMPLE_RANDOM: usize = 64;

/// Enumerate the crash images of seeds 1 and 2 (subset images when a tail is
/// absent, else torn images) and check the ones `masks` keeps. Returns the
/// reports and the checker invocations.
fn corpus(
    name: &str,
    tails: &[Option<crash::TornTail>],
    masks: Masks,
    deadline: Option<Instant>,
) -> (Vec<crash::Report>, usize) {
    let path = std::env::temp_dir().join(format!("breenfs-{name}-{}.img", std::process::id()));
    let mut artifact = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .unwrap();
    let initial = test_image::image();
    test_image::check(&initial, "crash-base");
    let mut images = 0;
    let mut checks = 0;
    let mut reports = Vec::new();
    for seed in [1, 2] {
        for &tail in tails {
            let check = |image: &crash::Image| {
                images += 1;
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
                    "seed={seed} tail={tail:?} image={checks}: {output:?}"
                );
                checks += 1;
                if let Some(started) = deadline {
                    assert!(
                        started.elapsed().as_secs_f64() < 600.0,
                        "torn corpus exceeded 10 minutes"
                    );
                }
                Ok(())
            };
            let report = match tail {
                None => crash::subset_images_of(initial.clone(), seed, 8, masks, check),
                Some(tail) => crash::torn_images_of(initial.clone(), seed, 8, tail, masks, check),
            }
            .unwrap();
            assert!(report.commits >= 1, "seed={seed} tail={tail:?}: {report:?}");
            assert_eq!(report.boundaries.len(), report.commits * 2);
            for boundary in &report.boundaries {
                let writes = boundary.writes;
                let expected = if writes <= 12 {
                    1 << writes
                } else {
                    writes * 2 + 4097
                };
                assert_eq!(
                    boundary.images, expected,
                    "seed={seed} tail={tail:?}: {boundary:?}"
                );
                assert_eq!(
                    boundary.validated,
                    (0..expected)
                        .filter(|&case| masks.keeps(writes, case))
                        .count(),
                    "seed={seed} tail={tail:?}: {boundary:?}"
                );
                assert!(boundary.acknowledged <= boundary.issued);
            }
            reports.push(report);
        }
    }
    assert!(reports
        .iter()
        .any(|r| r.boundaries.iter().any(|b| b.acknowledged > 1)));
    assert_eq!(
        images,
        reports
            .iter()
            .flat_map(|r| &r.boundaries)
            .map(|b| b.validated)
            .sum::<usize>()
    );
    assert_eq!(checks, images);
    drop(artifact);
    std::fs::remove_file(path).unwrap();
    (reports, checks)
}

/// Each sampled window above 12 writes validated its prefixes, its omissions
/// and random selections, as the full corpus does.
fn sample_reaches_every_selection_kind(reports: &[crash::Report]) {
    for boundary in reports.iter().flat_map(|r| &r.boundaries) {
        if boundary.writes > 12 {
            assert!(boundary.validated > boundary.writes * 2 + 1, "{boundary:?}");
        }
    }
}

fn masks_cover_both_regimes() {
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
}

#[test]
fn subset_images_sample_recovers_to_an_acknowledged_commit() {
    masks_cover_both_regimes();
    let (reports, checks) = corpus("crash-sample", &[None], SAMPLE, None);
    sample_reaches_every_selection_kind(&reports);
    eprintln!("subset sample: {checks} full checker invocations; {reports:?}");
}

#[test]
#[ignore = "full crash corpus (minutes): run by the breenfs battery, or with --ignored when fs/breenfs commit, recovery or the crash corpus change"]
fn subset_images_recover_to_an_acknowledged_commit() {
    masks_cover_both_regimes();
    let (reports, checks) = corpus("crash", &[None], Masks::Every, None);
    assert!(checks > 8192);
    eprintln!("subset corpus: {checks} full checker invocations; {reports:?}");
}

#[test]
fn torn_images_sample_recover_to_an_acknowledged_commit() {
    let tails = [Some(crash::TornTail::Old), Some(crash::TornTail::Garbage)];
    let (reports, checks) = corpus("torn-sample", &tails, SAMPLE, None);
    sample_reaches_every_selection_kind(&reports);
    eprintln!("torn sample: {checks} full checker invocations; {reports:?}");
}

#[test]
#[ignore = "full crash corpus (minutes): run by the breenfs battery, or with --ignored when fs/breenfs commit, recovery or the crash corpus change"]
fn torn_images_recover_to_an_acknowledged_commit() {
    let started = Instant::now();
    let tails = [Some(crash::TornTail::Old), Some(crash::TornTail::Garbage)];
    let (reports, checks) = corpus("torn", &tails, Masks::Every, Some(started));
    assert!(checks > 16384);
    eprintln!(
        "torn corpus: {checks} full checker invocations in {:?}; {reports:?}",
        started.elapsed()
    );
}
