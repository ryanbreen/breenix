extern crate self as libbreenix;
#[path = "../libs/libbreenix/src/ui_clock.rs"]
pub mod ui_clock;
#[path = "../libs/libuitest/src/clock.rs"]
mod clock;

#[test]
fn recorded_commits_complete_only_the_required_epoch_and_name_a_stopped_window() {
    let path = std::env::temp_dir().join(format!("ui-step-{}", std::process::id()));
    ui_clock::ClockPage::create(&path, 1_704_067_200_000_000_000).unwrap();
    let page = ui_clock::ClockPage::open(&path).unwrap();
    let a = page.register(41).unwrap();
    let b = page.register(42).unwrap();
    let idle = page.register(43).unwrap();
    a.schedule(16_000_000, false);
    b.schedule(u64::MAX, true);
    idle.schedule(17_000_000, false);
    let driver = clock::Clock::open(&path).unwrap();
    let header = std::fs::read(&path).unwrap()[..64].to_vec();
    let mut step = driver.begin(16, 100, |_| false).unwrap();
    assert_eq!(step.stamp(), ui_clock::Stamp { epoch: 1, now_ns: 16_000_000 });
    assert_eq!(page.stamp(), step.stamp());
    assert_eq!(step.waiting(), &[41, 42]);
    assert_eq!(page.participants().len(), 3);
    // READ_META fixtures: old commit, current pixel commit, current no-change commit.
    for (window, epoch) in [(41, 0), (43, 1), (41, 1)] { step.observe(window, epoch); }
    assert!(!step.complete());
    step.observe(42, 2);
    assert!(!step.complete(), "a future epoch is not the requested acknowledgment");
    b.accepted(step.stamp());
    assert!(!step.complete(), "participant self-report is not kernel metadata");
    assert!(step.check(2_000_000_099).is_ok());
    let error = step.check(2_000_000_100).unwrap_err();
    assert!(error.contains("window=42") && error.contains("epoch=1"), "{error}");
    step.observe(42, 1);
    assert!(step.complete());
    step.check(2_000_000_100).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], &header[..8]);
    assert_eq!(&bytes[24..32], &header[24..32]);
    assert_eq!(&bytes[36..64], &header[36..64]);
    let mut next = driver.begin(0, 200, |id| id == 42).unwrap();
    assert_eq!(next.stamp(), ui_clock::Stamp { epoch: 2, now_ns: 16_000_000 });
    next.observe(41, 1);
    assert!(!next.complete(), "a previous acknowledgment cannot satisfy a zero step");
    next.observe(41, 2);
    assert!(next.complete());
    drop(idle); drop(b); drop(a);
    assert!(page.participants().is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn provider_shares_frozen_time_without_real_reads_and_samples_frame_at_acquire() {
    let path = std::env::temp_dir().join(format!("ui-provider-{}", std::process::id()));
    ui_clock::ClockPage::create(&path, 123_000_000_000).unwrap();
    std::env::set_var("BREENIX_UI_CLOCK", &path);
    let before = ui_clock::real_clock_reads();
    let acquired = ui_clock::stamp();
    for _ in 0..10 { assert_eq!(ui_clock::stamp(), acquired); }
    assert_eq!(ui_clock::wall_ns(), 123_000_000_000);
    let driver = clock::Clock::open(&path).unwrap();
    let step = driver.begin(16, 0, |_| false).unwrap();
    assert!(step.complete());
    assert_eq!(ui_clock::stamp(), step.stamp());
    assert_eq!(ui_clock::now_ns(), 16_000_000);
    assert_eq!(ui_clock::wall_ns(), 123_016_000_000);
    assert_eq!(acquired.epoch, 0, "a frame begun before the step keeps the old epoch");
    assert_eq!(ui_clock::real_clock_reads(), before);
    std::env::remove_var("BREENIX_UI_CLOCK");
    std::fs::remove_file(path).unwrap();
}
