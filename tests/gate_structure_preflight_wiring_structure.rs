//! Wiring ratchet for R191/PR-1 (the gate-tooling round). Pins that the four
//! boot gates `docker/qemu/run-aarch64-boot-test-strict.sh`, `docker/qemu/
//! run-aarch64-prod-profile-boot-test.sh`, `docker/qemu/run-x86-boot-tests.sh`
//! and `docker/qemu/run-x86-prod-profile-boot-test.sh` each call the shared
//! `docker/qemu/lib/gate-structure-preflight.sh::gate_structure_preflight`
//! before booting anything, per `docs/planning/green-program/gates/
//! GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1-2026-09-06.md`.
//!
//! # Why a literal list of four filenames, not a derived census
//!
//! Other wiring ratchets in this tree (e.g. `tests/
//! poll_tcp_gate_wiring_structure.rs`) derive their target set from a shared
//! predicate over each script under `docker/qemu/` and `scripts/`, so a NEW
//! gate that starts asserting some verdict is swept in automatically without
//! anyone updating a list. That shape does not fit here: this suite is not
//! discovering "whichever gates currently do X" -- it is pinning that these
//! four SPECIFIC gates (the four callers this round's own dispatching brief
//! named, and the same four `docker/qemu/lib/gate-structure-preflight.sh`'s
//! own header documents) keep the wiring this round put there. A fifth boot
//! gate this repository adds later is not implicitly in this pin's scope;
//! wiring it in is that future round's own explicit decision, the same way
//! `tests/critical_path_logging_census_structure.rs`'s `ZERO_PIN_FILES` is a
//! deliberate three-file literal list rather than a derived one.
//!
//! Host-side only: a text read of the four gate scripts and the shared lib
//! file, no kernel build or QEMU boot. Run: `cargo test --test
//! gate_structure_preflight_wiring_structure` or `scripts/
//! run-structure-tests.sh gate_structure_preflight_wiring_structure`.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

/// The four gates this round wires the preflight into. See the module doc
/// for why this is a literal list rather than a derived census.
const TARGET_GATES: &[&str] = &[
    "docker/qemu/run-aarch64-boot-test-strict.sh",
    "docker/qemu/run-aarch64-prod-profile-boot-test.sh",
    "docker/qemu/run-x86-boot-tests.sh",
    "docker/qemu/run-x86-prod-profile-boot-test.sh",
];

const SHARED_LIB: &str = "docker/qemu/lib/gate-structure-preflight.sh";

/// The shell function each gate must call. Checked separately from
/// `SOURCE_LITERAL` below so a gate that sources the shared file but does
/// not call the function it defines -- exactly as unprotected as a gate with
/// no wiring at all -- is caught on its own, not masked by the
/// source line still being present.
const CALL_LITERAL: &str = "gate_structure_preflight";
/// The source line each gate must carry, naming the shared lib file itself
/// (not just some other file that happened to define a same-named function).
const SOURCE_LITERAL: &str = "lib/gate-structure-preflight.sh";

/// A gate's text carries this round's wiring only if it does both: sources
/// the shared lib file AND calls the function that file defines.
fn carries_preflight_wiring(gate_text: &str) -> bool {
    gate_text.contains(SOURCE_LITERAL) && gate_text.contains(CALL_LITERAL)
}

fn missing_wiring(gate_texts: &[(&str, String)]) -> Vec<String> {
    gate_texts
        .iter()
        .filter(|(_, text)| !carries_preflight_wiring(text))
        .map(|(name, _)| (*name).to_owned())
        .collect()
}

fn target_gate_texts() -> Vec<(&'static str, String)> {
    TARGET_GATES.iter().map(|path| (*path, repo_text(path))).collect()
}

#[test]
fn every_target_gate_calls_the_structure_preflight() {
    let texts = target_gate_texts();
    let missing = missing_wiring(&texts);
    assert_eq!(
        missing,
        Vec::<String>::new(),
        "gate(s) missing the structure-preflight wiring: {missing:?}"
    );
}

#[test]
fn shared_lib_defines_the_preflight_function_and_its_marker_line() {
    let lib_text = repo_text(SHARED_LIB);
    assert!(
        lib_text.contains("gate_structure_preflight() {"),
        "docker/qemu/lib/gate-structure-preflight.sh no longer defines \
         gate_structure_preflight() -- every caller's wiring now calls nothing"
    );
    assert!(
        lib_text.contains("[GATE_PREFLIGHT:"),
        "the shared lib no longer prints the [GATE_PREFLIGHT:...] marker line \
         the deliverable specifies"
    );
    assert!(
        lib_text.contains("BREENIX_GATE_SKIP_STRUCTURE"),
        "the shared lib no longer honours BREENIX_GATE_SKIP_STRUCTURE, the \
         documented loud opt-out"
    );
}

/// Mutation: strip each occurrence of the call literal from one target
/// gate's in-memory text, leaving the `source` line (and everything else)
/// untouched -- the shape a careless edit that deletes the `if !
/// gate_structure_preflight ...; then ... fi` block's call, but not its
/// header-comment mention or its `source` line, would produce. Must redden
/// on that one gate specifically, not on any of the other three.
#[test]
fn missing_wiring_validator_rejects_a_gate_with_the_call_site_removed() {
    let mut texts = target_gate_texts();
    let target = "docker/qemu/run-aarch64-boot-test-strict.sh";
    let (_, text) = texts
        .iter_mut()
        .find(|(name, _)| *name == target)
        .expect("target gate present in TARGET_GATES");
    *text = text.replace(CALL_LITERAL, "");
    let missing = missing_wiring(&texts);
    assert_eq!(missing, vec![target.to_owned()]);
}

/// Anti-vacuity: a gate script with neither the source line nor the call
/// literal (the shape each of the four gates had before this round)
/// must be rejected outright, not silently treated as "not applicable".
#[test]
fn missing_wiring_validator_rejects_a_gate_with_neither() {
    assert!(!carries_preflight_wiring("#!/bin/bash\necho hello\n"));
}
