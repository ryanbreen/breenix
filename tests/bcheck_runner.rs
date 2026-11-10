#[path = "../userspace/programs/src/bcheck/verdict.rs"]
mod verdict;
#[path = "../userspace/programs/src/bcheck/runner.rs"]
mod runner;

use runner::Runner;
use std::time::{Duration, Instant};
use verdict::Verdict;

unsafe extern "C" { fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32; }

fn finish(mut runner: Runner) -> Verdict {
    let limit = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = runner.poll() { return result; }
        assert!(Instant::now() < limit, "runner did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn shell(script: &str, timeout: Duration) -> Runner {
    Runner::start(&["/bin/sh".into(), "-c".into(), script.into()], timeout).unwrap()
}

#[test]
fn child_results_and_spawn_failure() {
    for (script, expected) in [("exit 1", Verdict::Exit(1)), ("exit 127", Verdict::Exit(127)),
        ("kill -TERM $$", Verdict::Signal(15, "SIGTERM")), ("exit 0", Verdict::Pass)] {
        let child = shell(script, Duration::from_secs(2));
        let pid = child.pid();
        let result = finish(child);
        assert_eq!(result.failed(), result != Verdict::Pass);
        assert_eq!(result, expected);
        assert_eq!(unsafe { waitpid(pid as i32, std::ptr::null_mut(), 1) }, -1);
    }
    assert!(matches!(Runner::start(&["/no/such/bcheck-program".into()], Duration::from_secs(1)), Err(Verdict::ExecFailed(_))));
}

#[test]
fn deadline_kills_the_group_and_reaps_the_child() {
    let marker = std::env::temp_dir().join(format!("bcheck-runner-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let script = format!("(sleep 1; touch '{}') & wait", marker.display());
    let child = shell(&script, Duration::from_millis(200));
    let pid = child.pid();
    assert_eq!(finish(child), Verdict::TimedOut);
    assert_eq!(unsafe { waitpid(pid as i32, std::ptr::null_mut(), 1) }, -1);
    std::thread::sleep(Duration::from_millis(1100));
    assert!(!marker.exists(), "grandchild survived the deadline");
}

#[test]
fn cancellation_reaps_the_child() {
    let mut child = shell("sleep 10", Duration::from_secs(20));
    let pid = child.pid();
    child.cancel();
    assert_eq!(finish(child), Verdict::Cancelled);
    assert_eq!(unsafe { waitpid(pid as i32, std::ptr::null_mut(), 1) }, -1);
}
