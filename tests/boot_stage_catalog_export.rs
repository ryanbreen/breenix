//! Run Inspector boot-stage catalog export ratchet.
//!
//! `tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-<arch>.json` is committed so the
//! Swift CLI/app can inspect stored runs without requiring Cargo at runtime.
//! This test re-runs the live xtask exporter and compares it byte-for-byte with
//! the committed files for both architectures. The assertion contains no
//! literal stage-name list: deleting or editing a `BootStage` changes the live
//! export and reddens the stale committed catalog directly.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn live_catalog(arch: &str) -> String {
    let output = Command::new(cargo())
        .current_dir(repo_root())
        .args([
            "run",
            "-p",
            "xtask",
            "--",
            "dump-boot-stages",
            "--arch",
            arch,
            "--json",
        ])
        .output()
        .expect("run xtask dump-boot-stages");

    if !output.status.success() {
        panic!(
            "xtask dump-boot-stages failed for {arch}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout).expect("xtask output is utf8")
}

fn committed_catalog(arch: &str) -> String {
    fs::read_to_string(
        repo_root().join(format!(
            "tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-{arch}.json"
        )),
    )
    .unwrap_or_else(|_| panic!("read committed boot stage catalog for {arch}"))
}

fn stage_count(catalog: &str) -> usize {
    catalog.matches("\"failureMeaning\"").count()
}

fn assert_fields_non_empty(catalog: &str, arch: &str) {
    for field in ["name", "marker", "failureMeaning", "checkHint"] {
        assert!(
            !catalog.contains(&format!("\"{field}\": \"\"")),
            "{arch} catalog contains an empty {field}"
        );
    }
}

fn assert_catalog_matches_live_export(arch: &str, minimum_stages: usize) {
    let live = live_catalog(arch);
    let committed = committed_catalog(arch);
    assert_eq!(
        committed, live,
        "committed {arch} boot-stage catalog must match live xtask export; run `make catalog` in tools/breenix-runs"
    );

    let count = stage_count(&committed);
    assert!(
        count >= minimum_stages,
        "{arch} catalog has only {count} stages; expected at least {minimum_stages}"
    );
    assert_fields_non_empty(&committed, arch);
}

#[test]
fn committed_aarch64_catalog_matches_live_xtask_export() {
    assert_catalog_matches_live_export("aarch64", 40);
}

#[test]
fn committed_x86_64_catalog_matches_live_xtask_export() {
    assert_catalog_matches_live_export("x86_64", 200);
}
