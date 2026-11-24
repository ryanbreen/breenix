use std::path::Path;
use std::process::Command;

fn run(case: &str) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let output = Command::new("python3")
        .arg(workspace.join("tests/busybox_sh/run.py"))
        .arg(case)
        .arg(env!("CARGO_BIN_EXE_bmake"))
        .output()
        .expect("launch the BusyBox driver check");
    print!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{case} failed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn refuses_digest_mismatch() {
    run("refuses_digest_mismatch");
}

#[test]
fn refuses_failed_patch() {
    run("refuses_failed_patch");
}

#[cfg(target_os = "macos")]
#[test]
fn mac_hosted_build_lists_same_applets() {
    run("mac_hosted_build_lists_same_applets");
}

#[cfg(target_os = "macos")]
#[test]
fn phases_chain_from_predecessor_hashes() {
    run("phases_chain_from_predecessor_hashes");
}

#[cfg(target_os = "macos")]
#[test]
fn phase_refuses_missing_or_altered_predecessor() {
    run("phase_refuses_missing_or_altered_predecessor");
}

#[cfg(target_os = "macos")]
#[test]
fn object_group_chain_equals_directory_goal() {
    run("object_group_chain_equals_directory_goal");
}

#[cfg(target_os = "macos")]
#[test]
fn audit_phases_equal_driver_phases() {
    run("audit_phases_equal_driver_phases");
}
