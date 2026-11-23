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

#[cfg(target_os = "macos")]
#[test]
fn mac_hosted_build_lists_same_applets() {
    run("mac_hosted_build_lists_same_applets");
}
