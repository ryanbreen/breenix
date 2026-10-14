//! Behavioral tests of the process-manager release protocol used by quiet waits.
mod process {
    mod wait_epoch {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/kernel/src/process/wait_epoch.rs"));
    }
    #[test]
    fn readonly_holders_and_two_empty_pollers_keep_cached_scan_valid() {
        let epoch = wait_epoch::WaitEpoch::new();
        let cached = epoch.load();
        for _ in 0..100 {
            epoch.release(false); // read-only lookup
            epoch.release(false); // first empty scan
            epoch.release(false); // second empty scan
        }
        assert_eq!(epoch.load(), cached);
    }
    #[test]
    fn mutable_holders_and_consumed_reports_invalidate_both_pollers() {
        let epoch = wait_epoch::WaitEpoch::new();
        let first = epoch.load();
        let second = epoch.load();
        epoch.release(true);
        assert_ne!(epoch.load(), first);
        assert_ne!(epoch.load(), second);
        let refreshed = epoch.load();
        epoch.release(false); // WNOWAIT leaves a report intact
        assert_eq!(epoch.load(), refreshed);
        epoch.release(true); // consume stop/continue report
        assert_ne!(epoch.load(), refreshed);
    }
}
