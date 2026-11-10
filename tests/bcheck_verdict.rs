#[path = "../userspace/programs/src/bcheck/verdict.rs"]
mod verdict;

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use verdict::Verdict;

#[test]
fn pass() {
    let verdict = Verdict::from_status(ExitStatus::from_raw(0));
    assert_eq!(verdict, Verdict::Pass);
    assert!(!verdict.failed());
}
#[test]
fn exit() {
    for code in [1, 127] {
        let verdict = Verdict::from_status(ExitStatus::from_raw(code << 8));
        assert_eq!(verdict, Verdict::Exit(code));
        assert!(verdict.failed());
    }
}
#[test]
fn signal() {
    let verdict = Verdict::from_status(ExitStatus::from_raw(9));
    assert_eq!(verdict, Verdict::Signal(9, "SIGKILL"));
    assert!(verdict.failed());
}
#[test]
fn exec_failed() {
    let error = std::io::Error::from_raw_os_error(2);
    let verdict = Verdict::from_spawn_error(&error);
    assert_eq!(verdict, Verdict::ExecFailed(error.to_string()));
    assert!(verdict.failed());
}
#[test]
fn timed_out() { assert!(Verdict::TimedOut.failed()); }
#[test]
fn cancelled() { assert!(Verdict::Cancelled.failed()); }
