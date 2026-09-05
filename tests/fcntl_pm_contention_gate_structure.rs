//! Census-shaped ratchet for R157/F3 (#796 review round, finding F3).
//!
//! `fcntl_pm_contention_oracle` measures whether an `fcntl()` issued while a
//! peer CPU holds `PROCESS_MANAGER` *waits* for the lock instead of reporting
//! `EAGAIN`. Two fields carry that reading, and before this file existed 1 of
//! the 2 was defended anywhere:
//! claim-lint:ok: 2 of 2 fields are named in the two bullets below; the oracle's
//! red-on-main / green-on-branch runs are in the #796 doc's STEP 3.
//!
//! * `eagain=0` -- the property under test. Pinned by the strict gate.
//! * `first_wait_us` -- the anti-vacuity floor. Without it, a boot whose peer
//!   failed to arm, or armed and released early, prints `eagain=0` and scores
//!   green having measured an *uncontended* call.
//!
//! The review found the floor unratcheted and unpinned: the strict gate's
//! pattern accepted `first_wait_us=[0-9]+`, i.e. `first_wait_us=0`, so the sole
//! authority making "the call actually waited" true was the in-kernel conjunct
//! `first_wait_us >= FCNTL_PM_MIN_WAIT_US` in the oracle's own `passed`
//! predicate. Deleting that one conjunct left the gate and 30 of the 30
//! structure suites then in the tree green -- a grep for fcntl over tests/
//! returned 0 files.
//! claim-lint:ok: R157 finding F3, reproduced as mutation M1 in the #796 doc's
//! STEP 3 anti-vacuity block.
//!
//! This file closes both halves, and closes them by CENSUS rather than by
//! naming today's script (the campaign's standing rule after #549/#551/#527-r1):
//!
//! 1. `oracle_pass_predicate_carries_a_wait_floor_conjunct` derives the
//!    `passed` predicate that feeds the emitted verdict from the registry
//!    source and requires it to compare `first_wait_us` against a floor of at
//!    least `MIN_DEFENSIBLE_FLOOR_US`. Deleting the conjunct, or zeroing the
//!    constant it names, reddens it.
//! 2. `every_gate_pinning_the_aarch64_pass_verdict_selfchecks_its_pattern`
//!    derives the set of scripts under docker/qemu/ and scripts/ that pin the
//!    oracle's aarch64 `PASS` verdict, and requires each one to carry the
//!    `FCNTL_PM_WAIT_SELFCHECK` block. A gate added later is swept into the
//!    census automatically.
//!
//! The division of labour matters. This file does not evaluate anybody's
//! regular expression: whether a pattern actually rejects `first_wait_us=0` is
//! decided at gate time by the gate's own matcher, in the self-check block,
//! which runs before the pattern is used to score any boot. So a loosened
//! pattern fails the gate that uses it rather than a ratchet that models it,
//! and this file's job is only to make sure no gate can quietly drop the
//! self-check.
//!
//! Host-side only: a text read of the tree, no kernel build and no QEMU boot.
//! Run: `cargo test --test fcntl_pm_contention_gate_structure`.

use std::fs;
use std::path::{Path, PathBuf};

/// The oracle's aarch64 verdict prefix. A gate script mentioning this string
/// and `PASS` is treating the oracle's green verdict as gate-relevant.
const ORACLE_AARCH64_PREFIX: &str = "FCNTL_PM_CONTENTION_ORACLE:aarch64";
/// A floor below this would not distinguish a contended call from an
/// uncontended one on this hardware: the uncontended calls recorded on
/// origin/main returned in 2-21 us.
const MIN_DEFENSIBLE_FLOOR_US: u64 = 1_000;
/// The token a gate script carries when it proves, at gate time and in its own
/// matcher, that its pinned pattern rejects `first_wait_us=0` and accepts a
/// real wait. Keyed on rather than reproduced here, so this ratchet asserts the
/// self-check exists and the gate itself asserts that it works.
const GATE_SELFCHECK_TOKEN: &str = "FCNTL_PM_WAIT_SELFCHECK";

const REGISTRY: &str = "kernel/src/test_framework/registry.rs";
const GATE_ROOTS: [&str; 2] = ["docker/qemu", "scripts"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    String::from_utf8_lossy(
        &fs::read(path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e)),
    )
    .into_owned()
}

/// Each regular file under `root`, recursively.
fn discover_files(root: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("read entry in {}: {}", root.display(), e))
            .path();
        if path.is_dir() {
            discover_files(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

/// The `passed = ...;` assignment that decides the emitted verdict: the last
/// one before the marker literal is printed. Derived from the emit site rather
/// than pinned to a line, so moving the oracle's internals around does not
/// redden this.
fn verdict_predicate(source: &str) -> &str {
    let emit = source
        .find(ORACLE_AARCH64_PREFIX)
        .expect("the registry no longer emits the oracle aarch64 marker");
    let before = &source[..emit];
    let assign = before
        .rfind("passed =")
        .expect("no `passed =` assignment before the oracle verdict emit");
    let tail = &before[assign..];
    let end = tail
        .find(';')
        .expect("unterminated `passed =` assignment before the verdict emit");
    &tail[..end]
}

/// The `u64` value of a `const NAME: ... = <literal>;` declaration.
fn const_u64(source: &str, name: &str) -> u64 {
    let needle = format!("const {}:", name);
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("const {} is not declared in {}", name, REGISTRY));
    let tail = &source[start..];
    let eq = start + tail.find('=').expect("const without an initializer");
    let semi = eq + source[eq..].find(';').expect("const without a terminator");
    source[eq + 1..semi]
        .trim()
        .replace('_', "")
        .parse::<u64>()
        .unwrap_or_else(|e| panic!("const {} is not a plain u64 literal: {}", name, e))
}

/// The registry source, read once per test.
fn registry_source() -> String {
    let path = repo_root().join(REGISTRY);
    read(&path)
}

#[test]
fn oracle_pass_predicate_carries_a_wait_floor_conjunct() {
    let registry = registry_source();
    let predicate = verdict_predicate(&registry);

    assert!(
        predicate.contains("first_wait_us"),
        "the oracle verdict predicate does not constrain first_wait_us at all, so a \
         boot whose peer never really held the lock can still print PASS. Predicate was:\n{}",
        predicate
    );

    let at = predicate
        .find("first_wait_us")
        .expect("checked immediately above");
    let after = predicate[at + "first_wait_us".len()..].trim_start();
    assert!(
        after.starts_with(">="),
        "first_wait_us appears in the verdict predicate but not as a floor. Predicate was:\n{}",
        predicate
    );

    let rhs: String = after[2..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    assert!(
        !rhs.is_empty(),
        "the first_wait_us floor has no right-hand side. Predicate was:\n{}",
        predicate
    );

    let floor = match rhs.replace('_', "").parse::<u64>() {
        Ok(literal) => literal,
        Err(_) => const_u64(&registry, &rhs),
    };
    assert!(
        floor >= MIN_DEFENSIBLE_FLOOR_US,
        "the first_wait_us floor is {} us, below the {} us that separates a contended \
         call from the 2-21 us an uncontended one took on origin/main",
        floor,
        MIN_DEFENSIBLE_FLOOR_US
    );
}

/// Scripts under the gate roots that pin the oracle's aarch64 PASS verdict.
/// Derived rather than listed, so a gate added later is swept in without anyone
/// editing this file.
fn gates_pinning_the_aarch64_pass_verdict() -> Vec<(PathBuf, String)> {
    let root = repo_root();
    let mut files = Vec::new();
    for gate_root in GATE_ROOTS {
        discover_files(&root.join(gate_root), &mut files);
    }
    files.sort();
    let mut gates = Vec::new();
    for path in files {
        let text = read(&path);
        if text.contains(ORACLE_AARCH64_PREFIX) && text.contains("PASS") {
            gates.push((path, text));
        }
    }
    gates
}

#[test]
fn every_gate_pinning_the_aarch64_pass_verdict_selfchecks_its_pattern() {
    let gates = gates_pinning_the_aarch64_pass_verdict();
    assert!(
        !gates.is_empty(),
        "no script under {:?} pins the oracle aarch64 PASS verdict -- either the \
         oracle lost its only gate or its marker was renamed, and this ratchet would \
         have passed vacuously",
        GATE_ROOTS
    );

    for (path, text) in &gates {
        assert!(
            text.contains(GATE_SELFCHECK_TOKEN),
            "{} pins the oracle aarch64 PASS verdict but carries no {} block, so \
             nothing proves its pattern can tell a real wait from a zero wait",
            path.display(),
            GATE_SELFCHECK_TOKEN
        );
    }
}
