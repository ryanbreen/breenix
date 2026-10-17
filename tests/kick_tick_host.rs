//! Runs the SIGKILL kick-window and x86 timer-vector tick host tests (#1682).
use std::{path::PathBuf, process::Command};

fn run(script: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let status = Command::new("python3")
        .arg(root.join(script))
        .current_dir(&root)
        .status()
        .expect("run python3");
    assert!(status.success(), "{script} failed");
}

#[test]
fn sigkill_kick_window_verdict_and_scorer() {
    run("tests/sigkill_ticks_host.py");
}

#[test]
fn x86_timer_vector_counts_only_timer_expiries() {
    run("tests/x86_timer_ticks_host.py");
}
