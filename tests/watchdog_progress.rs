#[path = "../kernel/src/drivers/watchdog_progress.rs"]
mod watchdog_progress;
use watchdog_progress::{Overdue, Phase, Progress};

#[test]
fn an_independent_observer_reports_a_feeder_stuck_in_the_root_probe() {
    let progress = Progress::new();
    progress.fed(1_000);
    progress.enter(Phase::Waiting);
    assert_eq!(progress.overdue(21_000), None);
    progress.enter(Phase::RootMount);
    assert_eq!(progress.overdue(41_000), Some(Overdue { phase: Phase::RootMount, age_ms: 40_000, has_fed: true }));
    assert_eq!(progress.overdue(42_000), None);

    // The mount lock released, but the on-disk inode read still did not
    // complete. Report the new phase even though the feeder never returned.
    progress.enter(Phase::RootInode);
    assert_eq!(progress.overdue(61_000), Some(Overdue { phase: Phase::RootInode, age_ms: 60_000, has_fed: true }));
    progress.fed(62_000);
    assert_eq!(progress.overdue(80_000), None);
}

#[test]
fn a_stuck_first_round_reports_that_the_feeder_has_never_fed() {
    let progress = Progress::new();
    progress.start(1_000);
    progress.enter(Phase::RootMount);
    assert_eq!(progress.overdue(21_000), None);
    let diagnostic = progress.overdue(41_000).expect("stuck first round");
    assert_eq!(diagnostic.phase, Phase::RootMount);
    assert_eq!(diagnostic.age_ms, 40_000);
    assert!(!diagnostic.has_fed);
}
