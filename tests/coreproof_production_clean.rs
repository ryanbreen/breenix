//! The core-proof harness's production-cleanliness ratchet, EXECUTED.
//!
//! The harness's first non-negotiable is that a production build carries zero
//! bytes of it. That is a claim about an ELF, not about a `#[cfg]`, so this test
//! measures the ELF — and it measures the check itself, because a cleanliness
//! scan that cannot go red proves nothing.
//!
//! Two legs, mirroring `kernel_no_neon_guard.rs`'s split of "run the real
//! script against the real binary" from "prove the script discriminates":
//!
//! * `production_kernel_carries_no_harness` builds the production-profile
//!   AArch64 kernel with no features at all and runs
//!   `scripts/check-coreproof-production-clean.sh`, which fails on any harness
//!   symbol or any occurrence of the harness's marker literal.
//! * `the_scan_reddens_on_a_build_that_carries_the_harness` runs the same
//!   script's `--prove` mode, which builds a `coreproof` kernel and requires
//!   both legs to fire. Without this, a scan that had quietly stopped matching
//!   anything would pass forever.
//!
//! The third leg — production `.text` byte-identical with the seams present and
//! with them textually stripped, which is the only leg that distinguishes
//! "expands to nothing" from "the optimiser removed it this time" — lives in the
//! same script behind `--bytes`. It is deliberately NOT run from `cargo test`:
//! it builds the kernel twice more, and the host suite is run on every commit.
//! `docker/qemu/run-coreproof-gate.sh`'s callers run it, and it is stated here
//! so the split is visible rather than silently narrowing what this file claims.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run_ratchet(args: &[&str]) -> (bool, String) {
    let root = repo_root();
    let output = Command::new(root.join("scripts/check-coreproof-production-clean.sh"))
        .current_dir(&root)
        .args(args)
        .output()
        .expect("failed to spawn the core-proof production-cleanliness ratchet");
    let combined = format!(
        "--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    (output.status.success(), combined)
}

#[test]
fn production_kernel_carries_no_harness() {
    let (ok, output) = run_ratchet(&[]);
    assert!(
        ok,
        "the production-profile kernel carries part of the core-proof harness:\n{output}"
    );
}

#[test]
fn the_scan_reddens_on_a_build_that_carries_the_harness() {
    let (ok, output) = run_ratchet(&["--prove"]);
    assert!(
        ok,
        "the production-cleanliness scan did not detect a build that DOES carry \
         the harness, so a clean verdict from it means nothing:\n{output}"
    );
}

/// The seam-placement ratchet is a separate script with its own anti-vacuity
/// mode; running both here keeps the pair wired into the standard host suite
/// rather than depending on someone remembering to invoke them.
#[test]
fn no_prohibited_file_carries_a_seam() {
    let root = repo_root();
    let output = Command::new(root.join("scripts/check-coreproof-seams.sh"))
        .current_dir(&root)
        .output()
        .expect("failed to spawn the core-proof seam ratchet");
    assert!(
        output.status.success(),
        "a prohibited file carries a perturbation seam:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn the_seam_ratchet_reddens_on_a_planted_seam() {
    let root = repo_root();
    let output = Command::new(root.join("scripts/check-coreproof-seams.sh"))
        .current_dir(&root)
        .arg("--prove")
        .output()
        .expect("failed to spawn the core-proof seam ratchet");
    assert!(
        output.status.success(),
        "a seam planted in a prohibited file did not redden the scan:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
