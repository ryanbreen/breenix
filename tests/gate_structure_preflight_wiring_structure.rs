//! Pins the shared preflight in the four original boot gates and the service
//! sequence gate added by issue 947 review V-3. The explicit target list
//! identifies the five gates covered by this suite.
//! Run through scripts/run-structure-tests.sh.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

/// The five explicitly covered gates.
const TARGET_GATES: &[&str] = &[
    "docker/qemu/run-aarch64-service-sequence-gate.sh",
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
/// on that one gate specifically, not on any of the other four.
#[test]
fn missing_wiring_validator_rejects_a_gate_with_the_call_site_removed() {
    for target in TARGET_GATES {
        let mut texts = target_gate_texts();
        let (_, text) = texts.iter_mut().find(|(name, _)| name == target).unwrap();
        *text = text.replace(CALL_LITERAL, "");
        assert_eq!(missing_wiring(&texts), vec![target.to_string()]);
    }
}

/// Anti-vacuity: a gate script with neither the source line nor the call
/// literal (the shape each of the original four gates had before this round)
/// must be rejected outright, not silently treated as "not applicable".
#[test]
fn missing_wiring_validator_rejects_a_gate_with_neither() {
    assert!(!carries_preflight_wiring("#!/bin/bash\necho hello\n"));
}

// Both gates boot the timed isolation fixtures before the service oracles.
fn capture_window_defaults(strict: &str, service: &str) -> (u64, u64) {
    let strict_values: Vec<_> = strict.lines()
        .filter_map(|line| line.trim().strip_prefix("timeout \"${BREENIX_STRICT_TIMEOUT_SECONDS:-"))
        .map(|tail| tail.split_once('}').expect("strict timeout expansion").0
            .parse::<u64>().expect("numeric strict capture default"))
        .collect();
    let service_values: Vec<_> = service.lines()
        .filter_map(|line| line.strip_prefix("BOOT_TIMEOUT="))
        .map(|value| value.parse::<u64>().expect("numeric service capture default"))
        .collect();
    assert_eq!(strict_values.len(), 1, "unique active strict capture default");
    assert_eq!(service_values.len(), 1, "unique service capture default");
    assert!(strict_values[0] > 0);
    (strict_values[0], service_values[0])
}

#[test]
fn service_sequence_capture_window_covers_strict() {
    let (strict, service) = capture_window_defaults(
        &repo_text("docker/qemu/run-aarch64-boot-test-strict.sh"),
        &repo_text("docker/qemu/run-aarch64-service-sequence-gate.sh"),
    );
    assert!(service >= strict,
        "service-sequence capture {service}s is shorter than strict {strict}s");
}

#[test]
fn service_sequence_capture_window_45_mutation_is_rejected() {
    let strict_source = repo_text("docker/qemu/run-aarch64-boot-test-strict.sh");
    let service_source = repo_text("docker/qemu/run-aarch64-service-sequence-gate.sh");
    let (strict, service) = capture_window_defaults(&strict_source, &service_source);
    assert!(service >= strict);
    let mutated = service_source.replacen(
        &format!("BOOT_TIMEOUT={service}"), "BOOT_TIMEOUT=45", 1);
    assert_ne!(mutated, service_source, "mutation must change the shipped source");
    let (strict, service) = capture_window_defaults(&strict_source, &mutated);
    assert_eq!(service, 45);
    assert!(service < strict, "45s regression must fail the capture-window ordering");
}

#[test]
fn service_preflight_precedes_build_and_aborts_on_failure() {
    let service = repo_text("docker/qemu/run-aarch64-service-sequence-gate.sh");
    let call = service.find(CALL_LITERAL).expect("service preflight call");
    assert!(call < service.find("if $REBUILD; then").unwrap());
    assert!(service[..call].trim_end().ends_with("if !"));
    let failure = &service[call..service[call..].find("\nfi").unwrap() + call];
    assert!(failure.contains("exit 1"));
}
