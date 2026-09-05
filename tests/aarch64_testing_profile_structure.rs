//! Structural ratchet for the aarch64 `--features testing` boot gate (#763).
//!
//! This file pins the SCORING of `docker/qemu/run-aarch64-testing-profile-boot-test.sh`,
//! not the kernel behaviour that gate measures. #562 and #761 are both still
//! open and this file makes no claim about either: it requires only that the
//! gate's own classifier keeps the terms that make its verdicts meaningful.
//!
//! The rules are about behaviour-bearing shapes in the script's text rather
//! than line numbers.
//!
//! Ported per hunk from `fix/562-761-aarch64-testing-profile`
//! (`tests/aarch64_testing_profile_structure.rs` there: the shared helpers at
//! the top of that file and the 2 tests at the bottom). The tests that pinned
//! #562 and #761 kernel functions are NOT ported: `main` carries no such
//! functions, so those rules would be vacuous here. Two of that file's four
//! shared helpers (`rust_sources_below`, `function_body`) are not ported
//! either, for the same reason -- with only these 2 tests present `rustc`
//! reports them, and the `BTreeSet` import, as dead code.
//!
//! Run with `scripts/run-structure-tests.sh aarch64_testing_profile_structure`.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

// ---------------------------------------------------------------------------
// The testing-profile gate's own scoring (#728, review finding R7-004)
// ---------------------------------------------------------------------------

#[test]
fn the_testing_profile_gate_detects_and_classifies_a_soft_lockup() {
    let gate = repo_text("docker/qemu/run-aarch64-testing-profile-boot-test.sh");
    let classifier = gate
        .split("classify_serial()")
        .nth(1)
        .expect("the gate must keep its scoring in one function");

    assert!(
        gate.contains("SOFT LOCKUP DETECTED"),
        "round 7 scored this profile with a term list that had no lockup in it, and called a boot \
         carrying a five-second lockup dump a clean pass; that is what this gate exists to stop"
    );
    assert!(
        classifier.contains("$LOCKUP_LINE") && classifier.contains("$EXT2_STALL_LINE"),
        "the scoring itself must read both the lockup line and the signature that attributes one"
    );
    for verdict in ["728-signature", "UNATTRIBUTED"] {
        assert!(
            classifier.contains(verdict),
            "the classifier must be able to reach the {verdict} verdict"
        );
    }
    assert!(
        classifier.contains("unattributed-lockup"),
        "an unattributed lockup has to be a red, not a note"
    );
    // Attribution is an ORDERING question: a stall printed after the dump does
    // not explain it.
    assert!(
        classifier.contains("$1 < l"),
        "attribution must compare line numbers, not merely ask whether both strings appear"
    );
    assert!(
        classifier.contains("userspace_panic=") && classifier.contains("kernel_panic="),
        "panics are counted and reported, and a kernel-side panic is scored separately"
    );
}

#[test]
fn the_testing_profile_gate_scores_committed_serials_with_the_same_code() {
    let gate = repo_text("docker/qemu/run-aarch64-testing-profile-boot-test.sh");
    assert!(
        gate.contains("--classify"),
        "an evidence serial has to be readable back by the code that scored it live, or a \
         committed classification is a second implementation nobody runs"
    );
    let live_mode = gate
        .split("for i in $(seq 1 \"$ITERATIONS\")")
        .nth(1)
        .expect("the gate must still boot the profile");
    assert!(
        live_mode.contains("classify_serial"),
        "the live path must call the same scoring function the offline path does"
    );
}
