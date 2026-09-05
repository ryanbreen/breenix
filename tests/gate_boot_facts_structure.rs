//! #827 per-boot host facts structural ratchet.
//!
//! The strict and production-profile aarch64 gates used to score a boot
//! purely from serial content, so a boot that ran out of its host-side
//! wall-clock budget and a boot that genuinely wedged scored identically
//! (#826's own 2/40 "Exec smoke did not complete" red, healthy heartbeats
//! to the last line, 0 crash markers). `docker/qemu/lib/gate-boot-facts.sh`
//! is the fix: both gates now print one `[GATE_BOOT_FACTS:...]` line per
//! boot carrying the host wall-clock window, host aarch64 QEMU count and
//! load average at start and at the pre-kill sample, QEMU's own CPU time
//! at that sample, the guest's last heartbeat, and an explicit `ended_by`
//! naming which bound in the caller's own poll loop actually ended the
//! boot.
//!
//! Two properties, each checked against the real files, not merely asserted:
//!
//! 1. `gbf_emit_line`'s own format string carries the 10 required field
//!    labels -- deleting one from the shared emitter is a single edit that
//!    silently drops that fact from BOTH gates at once, so this is checked
//!    against the shared file rather than against each gate's own text.
//! 2. Both gates set `ended_by` (or `ENDED_BY`) to each of the 5
//!    values #827 names, somewhere in their own control flow -- a census,
//!    not a single presence check, so deleting any one of the 5
//!    assignments (a path this gate's poll loop can legitimately take
//!    losing its own `ended_by` classification) reddens by name.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

const GATE_BOOT_FACTS_LIB: &str = "docker/qemu/lib/gate-boot-facts.sh";
const STRICT_GATE: &str = "docker/qemu/run-aarch64-boot-test-strict.sh";
const PROD_GATE: &str = "docker/qemu/run-aarch64-prod-profile-boot-test.sh";

/// The 10 field labels #827 names, in the order `gbf_emit_line` prints
/// them. `GATE_BOOT_FACTS:` itself is checked as an 11th, separate token
/// -- the line's own name, not a field.
const REQUIRED_FIELD_LABELS: &[&str] = &[
    "GATE_BOOT_FACTS:",
    "boot=",
    "host_ms=",
    "qemu_at_start=",
    "load_at_start=",
    "qemu_at_end=",
    "load_at_end=",
    "qemu_cpu_s=",
    "guest_uptime_ms=",
    "ended_by=",
];

/// The 5 values #827's `ended_by` field can take.
const ENDED_BY_VALUES: &[&str] = &[
    "crash_marker",
    "hard_timeout",
    "poll_exhausted",
    "scored_pass",
    "scored_fail",
];

/// Missing field labels in `gbf_emit_line`'s own printf format string --
/// found by locating the function body (from its definition line to the
/// next top-level `}` at the start of a line, the same "next brace at the
/// start of a line" convention this repo's other structural tests use for
/// bash function bodies) and checking each required label appears
/// somewhere in it.
fn missing_field_labels(lib_text: &str) -> Vec<&'static str> {
    // Isolated to the printf format-string LITERAL itself (the text
    // between the first `'` after `printf ` and the next `'`), not the
    // whole function body: the function's own parameter-assignment line
    // (`qemu_cpu_s="$8" ...`) contains the same substrings as the field
    // labels it feeds into that string, so checking the whole body would
    // let a label survive in the parameter line even after it was deleted
    // from the format string that actually decides what gets printed.
    let marker = "printf '[GATE_BOOT_FACTS:";
    let start = lib_text
        .find(marker)
        .expect("gate-boot-facts.sh must have a printf '[GATE_BOOT_FACTS:...' format string");
    let after_quote = start + "printf '".len();
    let rest = &lib_text[after_quote..];
    let end = rest
        .find('\'')
        .expect("the GATE_BOOT_FACTS format string must be closed by a matching '");
    let format_string = &rest[..end];

    REQUIRED_FIELD_LABELS
        .iter()
        .copied()
        .filter(|label| !format_string.contains(label))
        .collect()
}

/// Missing `ended_by`/`ENDED_BY` value assignments in a gate script's own
/// text -- a bare `ended_by="value"` or `ENDED_BY="value"` line (either
/// casing, matching each gate's own local-variable convention), found
/// anywhere in the file rather than tied to one function, since both
/// gates set this variable across more than one code path (the poll loop
/// classification and, for the strict gate, the case statement after it).
fn missing_ended_by_assignments(script_text: &str) -> Vec<&'static str> {
    ENDED_BY_VALUES
        .iter()
        .copied()
        .filter(|value| {
            let upper = format!("ENDED_BY=\"{value}\"");
            let lower = format!("ended_by=\"{value}\"");
            !script_text.contains(&upper) && !script_text.contains(&lower)
        })
        .collect()
}

fn sources_gate_boot_facts(script_text: &str) -> bool {
    script_text.contains("lib/gate-boot-facts.sh")
}

fn calls_gbf_emit_line(script_text: &str) -> bool {
    script_text.contains("gbf_emit_line")
}

#[test]
fn gate_boot_facts_line_has_all_required_fields() {
    let lib_text = repo_text(GATE_BOOT_FACTS_LIB);
    let missing = missing_field_labels(&lib_text);
    assert!(
        missing.is_empty(),
        "gbf_emit_line() in {GATE_BOOT_FACTS_LIB} is missing required GATE_BOOT_FACTS \
         field label(s): {missing:?} (#827)"
    );
}

#[test]
fn both_gates_source_and_call_gate_boot_facts() {
    for path in [STRICT_GATE, PROD_GATE] {
        let text = repo_text(path);
        assert!(
            sources_gate_boot_facts(&text),
            "{path} must source {GATE_BOOT_FACTS_LIB} (#827)"
        );
        assert!(
            calls_gbf_emit_line(&text),
            "{path} must call gbf_emit_line to print a per-boot GATE_BOOT_FACTS line (#827)"
        );
    }
}

#[test]
fn every_kill_path_in_the_strict_gate_sets_ended_by() {
    let text = repo_text(STRICT_GATE);
    let missing = missing_ended_by_assignments(&text);
    assert!(
        missing.is_empty(),
        "{STRICT_GATE} does not set ended_by to: {missing:?} -- each of #827's 5 \
         ended_by values must be reachable from this gate's own poll-loop control flow"
    );
}

#[test]
fn every_kill_path_in_the_prod_gate_sets_ended_by() {
    let text = repo_text(PROD_GATE);
    let missing = missing_ended_by_assignments(&text);
    assert!(
        missing.is_empty(),
        "{PROD_GATE} does not set ended_by to: {missing:?} -- each of #827's 5 \
         ended_by values must be reachable from this gate's own poll-loop control flow"
    );
}

/// ANTI-VACUITY: both census functions above must actually redden when a
/// real field or a real path assignment is removed -- checked with a
/// mutation applied to the real files' own text, in memory, not a
/// synthetic string.
#[test]
fn gate_boot_facts_predicates_are_not_vacuous() {
    let lib_text = repo_text(GATE_BOOT_FACTS_LIB);
    assert!(
        missing_field_labels(&lib_text).is_empty(),
        "sanity: the real lib file must be clean before mutation"
    );

    // Mutation 1: delete one field from the real printf format string
    // (in memory) and confirm the field-completeness check reddens by
    // name, not silently.
    let mutated_missing_field = lib_text.replacen(
        "qemu_cpu_s=%s:guest_uptime_ms=%s:",
        "guest_uptime_ms=%s:",
        1,
    );
    assert_ne!(mutated_missing_field, lib_text, "mutation must apply");
    let missing = missing_field_labels(&mutated_missing_field);
    assert_eq!(
        missing,
        vec!["qemu_cpu_s="],
        "deleting the qemu_cpu_s= field from the format string must redden \
         specifically on that field, not on some other one"
    );

    // Mutation 2: delete one ended_by path assignment from the real
    // strict-gate file (in memory) and confirm the path-census reddens by
    // name.
    let strict_text = repo_text(STRICT_GATE);
    assert!(
        missing_ended_by_assignments(&strict_text).is_empty(),
        "sanity: the real strict gate must be clean before mutation"
    );
    let poll_exhausted_line = "ENDED_BY=\"poll_exhausted\"\n";
    assert!(
        strict_text.contains(poll_exhausted_line),
        "the reconstructed assignment line must match the real file, \
         or this mutation applies to the wrong text"
    );
    let mutated_strict = strict_text.replacen(poll_exhausted_line, "\n", 1);
    assert_ne!(mutated_strict, strict_text, "mutation must apply");
    let missing = missing_ended_by_assignments(&mutated_strict);
    assert_eq!(
        missing,
        vec!["poll_exhausted"],
        "deleting the ENDED_BY=\"poll_exhausted\" assignment must redden \
         specifically on that value"
    );

    // Mutation 3: the same check, on the production gate's own lowercase
    // convention -- this reddens too, so the census is not accidentally
    // case-locked to the strict gate's own uppercase local-variable style.
    let prod_text = repo_text(PROD_GATE);
    assert!(
        missing_ended_by_assignments(&prod_text).is_empty(),
        "sanity: the real prod gate must be clean before mutation"
    );
    // "crash_marker" (not "hard_timeout") because this gate assigns
    // ended_by="hard_timeout" from TWO distinct branches (the qemu_died
    // loop-break and the fallback's dead-QEMU leg) -- replacing only the
    // first occurrence would leave the second standing, so the census
    // would (correctly) still find the value present and this mutation
    // would not redden the test.
    let crash_marker_line = "ended_by=\"crash_marker\"\n";
    assert!(
        prod_text.contains(crash_marker_line),
        "the reconstructed assignment line must match the real file, \
         or this mutation applies to the wrong text"
    );
    let mutated_prod = prod_text.replacen(crash_marker_line, "\n", 1);
    assert_ne!(mutated_prod, prod_text, "mutation must apply");
    let missing = missing_ended_by_assignments(&mutated_prod);
    assert_eq!(
        missing,
        vec!["crash_marker"],
        "deleting the ended_by=\"crash_marker\" assignment must redden \
         specifically on that value"
    );
}
