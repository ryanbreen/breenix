//! Census-shaped ratchet for R93/R96 (#693 review round 3, finding F1).
//!
//! R93 demoted the userspace poll oracle's `late_wake` verdicts to reports:
//! the ONLY gate-failing authority for a lost TCP-readiness wake is now the
//! kernel's own `[POLL_TCP_READY_LOST]` marker, not anything the oracle
//! prints. That ruling is only safe if EVERY gate script that used to fail on
//! the oracle's `[POLL_TCP_ORACLE:FAIL` verdict was also given the kernel
//! marker -- round 2 wired 7 of the 8 aarch64 gates (plus
//! `scripts/x86-gate-verdict.sh`) by hand and missed the 8th,
//! `docker/qemu/run-aarch64-percpu-stack-custody-gate.sh`, which shipped with
//! no lost-wake detector at all until this round's F1 fix (see the
//! committed specimen
//! `docs/planning/green-program/sockets/serials/693-fix-r3/aarch64-mutK-boot1-serial-20260903.txt`,
//! which has 1 `[POLL_TCP_READY_LOST]` and 0 `[POLL_TCP_ORACLE:FAIL` and
//! would have PASSED that one gate).
//!
//! R96 is the standing rule that this kind of wiring must be proved by a
//! CENSUS over the gate scripts, never by a literal list of their names --
//! the campaign's own recurring lesson (this exact arc failed it twice: once
//! against the oracle verdict itself in round 1, and again against the gate
//! wiring in round 2/3). So this test hardcodes no script name anywhere: it
//! derives the set of scripts under `docker/qemu/` and `scripts/` that assert
//! the oracle's `[POLL_TCP_ORACLE:FAIL` verdict, and requires every member of
//! that derived set to also carry the kernel's `[POLL_TCP_READY_LOST]`
//! marker. A future gate that starts asserting the oracle's verdict is swept
//! into the census automatically and must carry the kernel marker too, or
//! this test reddens without anyone updating a list.
//!
//! Shell equivalent of the derivation below (the reviewer's own repro, round
//! 3 finding F1):
//! ```sh
//! comm -23 <(grep -rl 'POLL_TCP_ORACLE:FAIL' docker/qemu/ scripts/ | sort) \
//!          <(grep -rl 'POLL_TCP_READY_LOST'  docker/qemu/ scripts/ | sort)
//! ```
//! An empty `comm -23` output is exactly `missing_ready_lost_wiring` below
//! returning an empty vector.
//!
//! Host-side only: a text read of the tree, no kernel build or QEMU boot.
//! Run: `cargo test --test poll_tcp_gate_wiring_structure`.
//! claim-lint:ok: 3 of 3 tests in this file pass at the bytes this ships at;
//! the RED-at-a875230a/GREEN-after-F1 split is recorded in this round's fix
//! notes (`fix-r4-notes.md`), not in this file, because reproducing RED
//! requires checking out the pre-F1 gate script this file only reads.

use std::fs;
use std::path::{Path, PathBuf};

/// The oracle's gate-failing verdict literal (`fail()` -> `emit()` in
/// `userspace/programs/src/poll_tcp_oracle.rs` reaches `main()`'s `Err(f)`
/// arm, which prints exactly this). Any script asserting this string is
/// treating the oracle as a lost-wake authority and must also require the
/// kernel's own marker per R93.
const ORACLE_FAIL_LITERAL: &str = "POLL_TCP_ORACLE:FAIL";
/// The kernel's own, sole gate-failing authority for a lost wake (R93).
const READY_LOST_LITERAL: &str = "POLL_TCP_READY_LOST";

fn repo_root() -> PathBuf {
    // tests/ lives one level below the workspace root this Cargo.toml sits
    // in, matching the convention other `*_structure.rs` ratchets use.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Each regular file under `root`, found recursively -- the same shape other
/// `discover_files`-shaped helpers in this tree's structure ratchets use
/// (e.g. `tests/block_request_lifetime_structure.rs`).
fn discover_files(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(directory)
            .unwrap_or_else(|_| panic!("read repository directory {}", directory.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|_| panic!("read entry in {}", directory.display()))
                .path();
            if path.is_dir() {
                visit(&path, files);
            } else {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    if root.is_dir() {
        visit(root, &mut files);
    }
    files.sort();
    files
}

/// The derived set: each file under `docker/qemu/` or `scripts/` that
/// contains the oracle's gate-failing FAIL literal. No name list -- this is
/// the computed census the R96 ratchet requires.
fn scripts_asserting_oracle_fail() -> Vec<PathBuf> {
    let mut candidates = discover_files(&repo_root().join("docker/qemu"));
    candidates.extend(discover_files(&repo_root().join("scripts")));
    candidates.sort();

    candidates
        .into_iter()
        .filter(|path| {
            // A file that is not valid UTF-8 cannot carry the literal, so it is
            // not a member of this census and reading it is not an error. The
            // specimen: `python3 scripts/test_claim_lint.py` -- 1 of the 2
            // claim-discipline commands a round runs -- imports
            // `scripts/claim-lint.py` and leaves
            // `scripts/__pycache__/claim-lint.cpython-*.pyc` behind, and the
            // panic this replaces then reddened this ratchet on a tree
            // whose 8 of 8 censused scripts were correctly wired.
            fs::read_to_string(path)
                .map(|text| text.contains(ORACLE_FAIL_LITERAL))
                .unwrap_or(false)
        })
        .collect()
}

/// Each member of `scripts` that does NOT also carry the kernel's
/// READY_LOST marker -- i.e. this call's own `comm -23` result, computed
/// in-process rather than shelled out.
///
/// R157: this reader used `read_to_string(...).unwrap_or_else(panic!)`,
/// which panicked on any non-UTF8 file reaching this filter (e.g. a stray
/// `scripts/__pycache__/*.pyc` left by `python3 scripts/claim-lint.py`). A
/// gate script is text by construction, so a file that fails to decode as
/// UTF-8 is not a member of this census and is skipped rather than fatal.
fn missing_ready_lost_wiring(scripts: &[PathBuf]) -> Vec<PathBuf> {
    scripts
        .iter()
        .filter(|path| {
            let bytes =
                fs::read(path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
            let Ok(text) = String::from_utf8(bytes) else {
                return false;
            };
            !text.contains(READY_LOST_LITERAL)
        })
        .cloned()
        .collect()
}

fn relative(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[test]
fn every_oracle_fail_gate_also_requires_the_kernel_ready_lost_marker() {
    let asserting = scripts_asserting_oracle_fail();

    // Anti-vacuity: a census over an empty set is vacuously satisfied (this
    // campaign's own recurring failure mode -- see MEMORY.md's "census
    // shapes in ratchets, not literal lists" note). At least one script
    // has to assert the oracle's FAIL literal for this ratchet to be
    // checking anything at all.
    assert!(
        !asserting.is_empty(),
        "found 0 scripts under docker/qemu/ or scripts/ asserting {ORACLE_FAIL_LITERAL:?} \
         -- this ratchet would be vacuously true, which is exactly the failure mode it exists \
         to prevent"
    );
    println!(
        "R96 census: {} script(s) assert {ORACLE_FAIL_LITERAL:?}",
        asserting.len()
    );

    let missing = missing_ready_lost_wiring(&asserting);
    assert!(
        missing.is_empty(),
        "{} of {} script(s) asserting the oracle's {ORACLE_FAIL_LITERAL:?} verdict do not also \
         require the kernel's {READY_LOST_LITERAL:?} marker (R93): {}. Per R93 the userspace \
         oracle's verdicts are demoted to reports and the kernel marker is the ONLY \
         gate-failing authority for a lost wake -- a gate that checks the oracle without also \
         checking the kernel marker has no lost-wake detector at all.",
        missing.len(),
        asserting.len(),
        missing.iter().map(|p| relative(p)).collect::<Vec<_>>().join(", ")
    );
}

#[test]
fn discover_files_finds_the_known_wired_gate() {
    // Sanity check on the directory walk itself, independent of the content
    // filter above: the always-present, already-correctly-wired
    // refusal-drain gate must be reachable from the recursive scan.
    let files = discover_files(&repo_root().join("docker/qemu"));
    let expect = repo_root().join("docker/qemu/run-aarch64-refusal-drain-gate.sh");
    assert!(
        files.contains(&expect),
        "recursive scan of docker/qemu/ did not find run-aarch64-refusal-drain-gate.sh"
    );
}

#[test]
fn negative_a_script_missing_the_kernel_marker_is_detected() {
    // Proves `missing_ready_lost_wiring` actually reddens on a script that
    // asserts the oracle FAIL literal without the kernel marker -- the exact
    // shape `run-aarch64-percpu-stack-custody-gate.sh` had before this
    // round's F1 fix. Built from a temp file rather than a tracked one so
    // this test does not depend on any script's current wiring state.
    let dir = std::env::temp_dir().join(format!(
        "poll_tcp_gate_wiring_structure_negative_{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    let unwired = dir.join("unwired-gate.sh");
    fs::write(&unwired, "grep -qF '[POLL_TCP_ORACLE:FAIL' serial.txt && exit 1\n")
        .expect("write synthetic unwired gate");
    let wired = dir.join("wired-gate.sh");
    let wired_body = "grep -qF '[POLL_TCP_ORACLE:FAIL' serial.txt && exit 1\ngrep -qF '[POLL_TCP_READY_LOST]' serial.txt && exit 1\n";
    fs::write(&wired, wired_body).expect("write synthetic wired gate");

    let scripts = vec![unwired.clone(), wired.clone()];
    let missing = missing_ready_lost_wiring(&scripts);
    assert_eq!(
        missing,
        vec![unwired.clone()],
        "expected only the synthetic unwired gate to be flagged"
    );

    fs::remove_file(&unwired).ok();
    fs::remove_file(&wired).ok();
    fs::remove_dir(&dir).ok();
}
