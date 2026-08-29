//! The ext2/VFS fault-injection leg's production-cleanliness ratchets, EXECUTED.
//!
//! The leg's first non-negotiable is that a production build carries zero bytes
//! of it. That is a claim about an ELF and about the source that produces it, so
//! this file runs both halves of the pair rather than restating the `#[cfg]`s:
//!
//! * `production_kernel_carries_no_fault_leg` builds the production-profile
//!   AArch64 kernel with no features at all and runs
//!   `scripts/check-fs-fault-production-clean.sh`, which fails on any symbol
//!   belonging to the leg or any occurrence of its `[FSFAULT:` marker prefix.
//! * `the_scan_reddens_on_a_build_that_carries_the_leg` runs the same script's
//!   `--prove` mode against an `fs_fault_inject` build and requires both legs to
//!   fire, so a scan that had quietly stopped matching cannot pass forever.
//! * The seam ratchet's two legs do the same for the source half: every
//!   reference to the leg outside its own module is cfg-guarded, no hot-path
//!   file references it at all, and a planted unguarded call is detected.
//!
//! What is deliberately NOT here: the `.text` byte-identity leg the core-proof
//! harness needs. That leg exists to distinguish "the seam macro expands to
//! nothing" from "the optimiser removed it this time" at PRODUCTION call sites.
//! This leg has no production call sites — the module and both of its call sites
//! are inside `#[cfg(feature = "fs_fault_inject")]` blocks — so there is nothing
//! for such a comparison to be about, and the seam ratchet is what guards the
//! property that makes it so.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run(script: &str, args: &[&str]) -> (bool, String) {
    let root = repo_root();
    let output = Command::new(root.join(script))
        .current_dir(&root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {script}: {e}"));
    let combined = format!(
        "--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    (output.status.success(), combined)
}

#[test]
fn production_kernel_carries_no_fault_leg() {
    let (ok, output) = run("scripts/check-fs-fault-production-clean.sh", &[]);
    assert!(
        ok,
        "the production-profile kernel carries part of the ext2 fault-injection leg:\n{output}"
    );
}

#[test]
fn the_scan_reddens_on_a_build_that_carries_the_leg() {
    let (ok, output) = run("scripts/check-fs-fault-production-clean.sh", &["--prove"]);
    assert!(
        ok,
        "the production-cleanliness scan did not detect a build that DOES carry the \
         fault-injection leg, so a clean verdict from it means nothing:\n{output}"
    );
}

#[test]
fn every_reference_to_the_leg_is_cfg_guarded() {
    let (ok, output) = run("scripts/check-fs-fault-seams.sh", &[]);
    assert!(
        ok,
        "a reference to the fault-injection leg is unguarded or sits in a hot path:\n{output}"
    );
}

#[test]
fn the_seam_ratchet_reddens_on_a_planted_unguarded_call() {
    let (ok, output) = run("scripts/check-fs-fault-seams.sh", &["--prove"]);
    assert!(
        ok,
        "the seam ratchet did not detect a planted unguarded call to the leg, so a \
         clean verdict from it means nothing:\n{output}"
    );
}
