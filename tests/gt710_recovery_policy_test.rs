//! Watchdog boundaries operate on hardware publication and ordered progress times.
#[path="../kernel/src/gpu/gk208/recovery_policy.rs"] mod policy;
use policy::{Watchdog,Action};
#[test]
fn quarantine_then_a_further_two_seconds_without_progress() {
    let mut w=Watchdog::new(100);
    assert_eq!(w.scan(2_000_099),Action::None);
    assert_eq!(w.scan(2_000_100),Action::Quarantine);
    assert_eq!(w.scan(4_000_099),Action::None);
    assert_eq!(w.scan(4_000_100),Action::Reset);
}
#[test]
fn surviving_ordered_completions_restart_only_the_no_progress_clock() {
    let mut w=Watchdog::new(100);
    assert_eq!(w.scan(2_000_100),Action::Quarantine);
    w.progress(3_900_100);
    assert_eq!(w.scan(4_000_100),Action::None);
    assert_eq!(w.scan(5_900_099),Action::None);
    assert_eq!(w.scan(5_900_100),Action::Reset);
}
