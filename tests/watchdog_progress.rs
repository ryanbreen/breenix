#[path = "../kernel/src/drivers/watchdog_progress.rs"]
mod watchdog_progress;
use watchdog_progress::{Phase, Progress};

#[test]
fn an_independent_observer_reports_a_feeder_stuck_in_the_root_probe() {
    let progress = Progress::new();
    progress.fed(1_000);
    progress.enter(Phase::Waiting);
    assert_eq!(progress.overdue(21_000), None);
    progress.enter(Phase::RootMount);
    assert_eq!(progress.overdue(41_000), Some((Phase::RootMount, 40_000)));
    assert_eq!(progress.overdue(42_000), None);

    // The mount lock released, but the on-disk inode read still did not
    // complete. Report the new phase even though the feeder never returned.
    progress.enter(Phase::RootInode);
    assert_eq!(progress.overdue(61_000), Some((Phase::RootInode, 60_000)));
    progress.fed(62_000);
    assert_eq!(progress.overdue(80_000), None);
}
