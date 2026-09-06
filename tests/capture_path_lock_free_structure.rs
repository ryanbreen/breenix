//! Structural ratchet for the BXCAP capture path (`kernel/src/capture/`).
//!
//! The emitter runs from a fault handler or a masked interrupt. What it may
//! not do there -- take a blocking lock, allocate, format, panic -- is not
//! visible in a type signature, so it is pinned here and, from the shell
//! side, in `scripts/check-critical-path-violations.sh`. This suite and that
//! script check overlapping things on purpose: the script is what a gate or
//! a hook runs, this suite is what `cargo test` runs, and neither depends on
//! the other being remembered.
//!
//! # What a source denylist is worth, stated plainly
//!
//! It sees the spellings it lists, in the files it reads, and no more than
//! that. It cannot see an allocation reached two frames down inside a callee
//! that is not itself named here. The plan's answer to that is a binary guard
//! modelled on `scripts/check-x86-dispatch-no-alloc.sh`; PR-3 does not build
//! one (see the round doc's "what is NOT claimed"), so what this suite
//! offers is the fast local signal, not an authority. The one callee that
//! matters most is pinned by name instead: the capture reaches the
//! scheduler through `try_liveness_snapshot`, which fills fixed-size arrays,
//! and not through `try_dump_state`, which builds two `alloc` vectors while
//! holding the guard.
//!
//! # Census-anchored, not line-pinned
//!
//! The file set is `kernel/src/capture/**/*.rs` as it exists on disk, so a
//! file added to the module is covered the day it lands. An empty set is a
//! failure, not a pass.

use std::fs;
use std::path::{Path, PathBuf};

const CAPTURE_DIR: &str = "kernel/src/capture";
const CRITICAL_PATH_SCRIPT: &str = "scripts/check-critical-path-violations.sh";

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    let full = repo_path(rel);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

/// The `.rs` files under `kernel/src/capture/`, recursively.
fn capture_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("kernel/src/capture must exist") {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    let root = repo_path(CAPTURE_DIR);
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(repo_path(""))
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let body = fs::read_to_string(&path).expect("readable source");
            (name, body)
        })
        .collect()
}

/// Lines of `source` that are neither blank nor a `//`-style comment. The
/// denylist is about what the code DOES; a doc comment naming a forbidden
/// construct in order to explain why it is forbidden is not a violation, and
/// treating it as one would push the reasoning out of the file.
fn code_lines(source: &str) -> Vec<(usize, &str)> {
    source
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line))
        .filter(|(_, line)| {
            let trimmed = line.trim_start();
            !trimmed.is_empty()
                && !trimmed.starts_with("//")
                && !trimmed.starts_with("///")
                && !trimmed.starts_with("//!")
        })
        .collect()
}

/// The spellings a capture-path source may not contain, and why.
///
/// Kept in step with `CAPTURE_PROHIBITED_PATTERNS` in
/// `scripts/check-critical-path-violations.sh` by
/// `the_shell_guard_and_this_suite_deny_the_same_shapes` below.
const DENIED: [(&str, &str); 15] = [
    (".lock()", "a BLOCKING lock acquisition; the capture may ask (try_lock) but never wait"),
    ("try_dump_state", "the scheduler's ALLOCATING dump; use try_liveness_snapshot"),
    ("serial_println!", "takes SERIAL1's mutex"),
    ("println!", "formatting machinery and a lock"),
    ("log::", "the logger takes a lock"),
    ("format!", "allocates"),
    ("write!", "formatting machinery"),
    ("writeln!", "formatting machinery"),
    ("alloc::", "heap allocation"),
    ("Vec<", "heap allocation"),
    ("String", "heap allocation"),
    ("Box<", "heap allocation"),
    ("vec!", "heap allocation"),
    ("unwrap()", "a panic from a capture re-enters the path the capture reports from"),
    ("panic!", "a panic from a capture re-enters the path the capture reports from"),
];

fn assert_denylist_clean(name: &str, source: &str) {
    for (needle, why) in DENIED {
        for (lineno, line) in code_lines(source) {
            assert!(
                !line.contains(needle),
                "{name}:{lineno} contains `{needle}` -- {why}:\n  {line}"
            );
        }
    }
}

#[test]
fn the_capture_path_carries_no_lock_allocation_or_formatting() {
    let sources = capture_sources();
    assert!(
        sources.len() >= 3,
        "expected at least mod.rs, record.rs and sections.rs under {CAPTURE_DIR}, found {}",
        sources.len()
    );
    for (name, body) in &sources {
        assert_denylist_clean(name, body);
    }
}

#[test]
fn the_only_output_primitive_is_the_raw_serial_writer() {
    let record = read("kernel/src/capture/record.rs");
    assert!(
        record.contains("use crate::tracing::output::raw_serial_char;"),
        "record.rs must write through raw_serial_char, the lock-free primitive"
    );
    // The other capture sources go through the Writer rather than reaching
    // for a second output path of their own.
    for (name, body) in capture_sources() {
        if name.ends_with("record.rs") {
            continue;
        }
        for (lineno, line) in code_lines(&body) {
            assert!(
                !line.contains("raw_serial_char") && !line.contains("raw_uart_"),
                "{name}:{lineno} writes bytes outside the Writer, so its output is not \
                 counted against the byte budget:\n  {line}"
            );
        }
    }
}

#[test]
fn the_writer_enforces_a_byte_budget_before_every_write() {
    let record = read("kernel/src/capture/record.rs");
    assert!(
        record.contains("pub const BXCAP_BUDGET_BYTES: u32"),
        "record.rs must define BXCAP_BUDGET_BYTES"
    );
    assert_budget_is_enforced(&record);
}

/// The assertion body shared by the real-source test above and the
/// anti-vacuity leg below: given `record.rs`, panic unless `put()` checks
/// the remaining budget BEFORE it reaches the output primitive.
fn assert_budget_is_enforced(record: &str) {
    let put_at = record
        .find("fn put(&mut self, byte: u8)")
        .unwrap_or_else(|| panic!("record.rs must define `fn put(&mut self, byte: u8)`"));
    let body = &record[put_at..];
    let end = body
        .find("\n    }")
        .unwrap_or_else(|| panic!("no terminator for `put`"));
    let body = &body[..end];

    let guard = body.find("self.remaining == 0").unwrap_or_else(|| {
        panic!(
            "`put` must refuse a byte once the budget is spent -- without that test the \
             emitter has no bound and `truncated=` can never be set:\n{body}"
        )
    });
    let sink = body.find("raw_serial_char(byte)").unwrap_or_else(|| {
        panic!("`put` must reach raw_serial_char:\n{body}")
    });
    assert!(
        guard < sink,
        "the budget test must precede the write it guards:\n{body}"
    );
    assert!(
        body.contains("self.truncated = true"),
        "a dropped byte must latch `truncated`, or the loss is silent:\n{body}"
    );
}

#[test]
#[should_panic(expected = "must refuse a byte once the budget is spent")]
fn deleting_the_budget_guard_would_be_caught() {
    // The same assertion body, against a source string with the guard's own
    // test removed. An in-memory mutation: the file on disk is untouched.
    let mutated = read("kernel/src/capture/record.rs").replace("self.remaining == 0", "false");
    assert_budget_is_enforced(&mutated);
}

#[test]
fn the_thread_section_asks_for_the_scheduler_and_never_waits_for_it() {
    let sections = read("kernel/src/capture/sections.rs");
    assert_thr_is_non_blocking_and_states_its_refusal(&sections);
}

/// Shared assertion body: the scheduler read is the non-blocking one, and
/// the refusal arm says so on the wire instead of going quiet.
fn assert_thr_is_non_blocking_and_states_its_refusal(sections: &str) {
    assert!(
        sections.contains("try_liveness_snapshot"),
        "the THR section must read the scheduler through try_liveness_snapshot, the \
         non-blocking, allocation-free snapshot"
    );
    let none_arm = sections.find("        None => {").unwrap_or_else(|| {
        panic!("sections.rs must handle the refused snapshot explicitly, not with `?` or unwrap")
    });
    let tail = &sections[none_arm..];
    let note_at = tail.find("sched_lock_held").unwrap_or_else(|| {
        panic!(
            "a refused scheduler read must be stated as [BXCAP:NOTE sched_lock_held]; \
             going quiet here is the timer_interrupt.rs soft-lockup defect this section \
             was written to invert"
        )
    });
    let return_at = tail.find("return false").unwrap_or_else(|| {
        panic!("the refusal arm must return false so THR's bit stays clear in sections_skipped")
    });
    assert!(
        note_at < return_at,
        "the refusal note must be emitted before the arm returns"
    );
}

#[test]
#[should_panic(expected = "must be stated as [BXCAP:NOTE sched_lock_held]")]
fn silently_dropping_the_scheduler_refusal_would_be_caught() {
    let mutated = read("kernel/src/capture/sections.rs").replace("sched_lock_held", "");
    assert_thr_is_non_blocking_and_states_its_refusal(&mutated);
}

#[test]
fn the_schema_version_is_emitted_on_both_bracket_lines() {
    let module = read("kernel/src/capture/mod.rs");
    assert_version_is_bracketed(&module);
}

/// Shared assertion body: `v=` rides on BEGIN and on END, from the same
/// constant. A decoder that cannot read a version on either line cannot
/// refuse a schema it does not know.
fn assert_version_is_bracketed(module: &str) {
    let emissions = module.matches("kv_dec(\"v\", BXCAP_VERSION)").count();
    assert_eq!(
        emissions, 2,
        "`v=` must be emitted from BXCAP_VERSION on both the BEGIN and the END record \
         (found {emissions} emission sites)"
    );
    let begin_at = module
        .find("writer.open(\"BEGIN\")")
        .unwrap_or_else(|| panic!("mod.rs must emit a BEGIN record"));
    let end_at = module
        .find("writer.open(\"END\")")
        .unwrap_or_else(|| panic!("mod.rs must emit an END record"));
    assert!(begin_at < end_at, "BEGIN must precede END");
}

#[test]
#[should_panic(expected = "must be emitted from BXCAP_VERSION on both")]
fn removing_the_version_field_would_be_caught() {
    let mutated = read("kernel/src/capture/mod.rs")
        .replacen("writer.kv_dec(\"v\", BXCAP_VERSION);\n", "", 1);
    assert_version_is_bracketed(&mutated);
}

#[test]
fn the_shell_guard_covers_the_capture_directory() {
    let script = read(CRITICAL_PATH_SCRIPT);
    assert!(
        script.contains("\"capture/\""),
        "{CRITICAL_PATH_SCRIPT} must carry `capture/` in CRITICAL_FILES, or the emitter \
         is checked by this suite alone"
    );
    assert!(
        script.contains("CAPTURE_PROHIBITED_PATTERNS"),
        "{CRITICAL_PATH_SCRIPT} must carry the capture-scoped denylist; the shared list \
         cannot contain `.lock()` because task/scheduler.rs legitimately does"
    );
    // A directory entry that matches no file must be an error, not a pass.
    assert!(
        script.contains("matched no .rs file"),
        "{CRITICAL_PATH_SCRIPT} must fail when a critical DIRECTORY entry expands to \
         nothing; a renamed directory silently checking zero files is how this class \
         of guard goes vacuous"
    );
}

#[test]
fn the_shell_guard_and_this_suite_deny_the_same_shapes() {
    let script = read(CRITICAL_PATH_SCRIPT);
    let start = script
        .find("CAPTURE_PROHIBITED_PATTERNS=(")
        .expect("capture-scoped denylist must exist");
    let block = &script[start..start + script[start..].find(")\n").expect("unterminated array")];

    // The shapes the shell guard is the one that must catch: a blocking lock
    // and the allocating scheduler dump. Everything else overlaps with the
    // shared list, which the script already applies.
    for needle in [".lock()", "try_dump_state"] {
        assert!(
            block.contains(needle),
            "{CRITICAL_PATH_SCRIPT}'s capture denylist lost `{needle}`, which this suite \
             still denies -- the two guards must not drift apart"
        );
        assert!(
            DENIED.iter().any(|(pattern, _)| *pattern == needle),
            "this suite's DENIED list lost `{needle}`"
        );
    }
}

#[test]
fn the_selftest_edge_is_feature_gated_and_no_gate_builds_it() {
    let manifest = read("kernel/Cargo.toml");
    assert!(
        manifest.contains("capture_selftest = [\"boot_tests\"]"),
        "capture_selftest must ride boot_tests and must not be implied by it"
    );
    assert!(
        !manifest.contains("boot_tests = [\"capture_selftest\"]"),
        "boot_tests must not pull in capture_selftest; that would put the self-test \
         capture into every gated boot"
    );

    // The self-test's only trigger is behind the feature.
    let irq = read("kernel/src/tracing/providers/irq.rs");
    let trigger = irq
        .find("capture::selftest::observe")
        .unwrap_or_else(|| panic!("the self-test edge must be fired from trace_timer_tick"));
    let preceding = &irq[..trigger];
    let cfg_at = preceding
        .rfind("#[cfg(feature = \"capture_selftest\")]")
        .unwrap_or_else(|| panic!("the self-test trigger must be behind #[cfg(feature = \"capture_selftest\")]"));
    assert!(
        preceding[cfg_at..].lines().count() <= 5,
        "the capture_selftest cfg must guard the trigger line itself, not something \
         several lines above it"
    );

    // And no gate script builds with it.
    for gate in [
        "docker/qemu/run-aarch64-boot-test-strict.sh",
        "docker/qemu/run-aarch64-prod-profile-boot-test.sh",
        "docker/qemu/run-x86-boot-tests.sh",
        "docker/qemu/run-x86-prod-profile-boot-test.sh",
    ] {
        let body = read(gate);
        for (lineno, line) in code_lines(&body) {
            assert!(
                !(line.contains("--features") && line.contains("capture_selftest")),
                "{gate}:{lineno} builds with capture_selftest; the self-test capture is a \
                 development knob, and a gate that builds it would be scoring a marker no \
                 shipped kernel emits:\n  {line}"
            );
        }
    }
}
