//! Full-provider formatting regression for issue 947 finding V-2.
use std::path::Path;
use std::process::Command;

#[test]
fn teardown_provider_is_rustfmt_clean() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("rustfmt")
        .current_dir(root)
        .args(["--edition", "2021", "--check", "kernel/src/tracing/providers/teardown.rs"])
        .output().expect("run rustfmt");
    assert!(output.status.success(), "full-provider rustfmt failed: {}{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
