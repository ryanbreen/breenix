//! The xHCI recovery exercise's production-cleanliness ratchet, executed (#1931).
//!
//! The exercise is compiled only with `xhci_fault_inject`, so a production
//! metal kernel has no code that reads `/etc/breenix/xhci-fault`. That is a
//! claim about the delivered kernel, so these tests run
//! `scripts/check-xhci-fault-production-clean.sh`:
//!
//! * `production_metal_kernel_carries_no_fault_exercise` builds the x86 kernel
//!   the way the metal build does and fails on any symbol from the exercise's
//!   module or any `xhci-fault` bytes in the UEFI image.
//! * `the_scan_reddens_on_a_fault_test_build` requires both legs to fire on an
//!   `xhci_fault_inject` build, so a scan that stopped matching cannot pass.

use std::path::PathBuf;
use std::process::Command;

fn run(args: &[&str]) -> (bool, String) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(root.join("scripts/check-xhci-fault-production-clean.sh"))
        .current_dir(&root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn the xHCI fault ratchet: {e}"));
    let combined = format!(
        "--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    (output.status.success(), combined)
}

#[test]
fn production_metal_kernel_carries_no_fault_exercise() {
    let (ok, output) = run(&[]);
    assert!(ok, "the production metal kernel carries part of the xHCI fault exercise:\n{output}");
}

#[test]
fn the_scan_reddens_on_a_fault_test_build() {
    let (ok, output) = run(&["--prove"]);
    assert!(
        ok,
        "the scan did not detect a build that does carry the xHCI fault exercise, \
         so a clean verdict from it means nothing:\n{output}"
    );
}
