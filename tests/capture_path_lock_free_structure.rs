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
use std::process::Command;

const CAPTURE_DIR: &str = "kernel/src/capture";
const GATE_DIR: &str = "docker/qemu";
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
const DENIED: [(&str, &str); 17] = [
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
    // The two spellings the shell script denied and this list did not, found
    // while PR-7 widened the strict scope to the soft-lockup dump. They were a
    // real parity gap, not a cosmetic one: `expect(` is a panic and `to_string`
    // is an allocation, and both were reachable by a capture-path edit that
    // this suite would have passed.
    ("to_string", "heap allocation"),
    ("expect(", "a panic from a capture re-enters the path the capture reports from"),
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
fn every_section_reports_the_verdict_its_own_record_close_returned() {
    let record = read("kernel/src/capture/record.rs");
    let sections = read("kernel/src/capture/sections.rs");
    assert_close_verdicts_are_carried(&record, &sections);
}

/// Shared assertion body: `Writer::close()` hands back whether the record
/// went onto the wire whole, and no section drops that answer.
///
/// `open()` only refuses a record that starts with the budget already spent.
/// A record cut MID-write is reported by `close()` alone, so a section that
/// ignored it would clear its own bit in `sections_skipped=` over a fragment.
/// Two guards, because either alone is weak: `#[must_use]` makes the compiler
/// object anywhere in the tree, and the census below is what a reader of this
/// file can check without building.
fn assert_close_verdicts_are_carried(record: &str, sections: &str) {
    assert!(
        record.contains("#[must_use]\n    pub fn close(&mut self) -> bool {"),
        "Writer::close must return its verdict and be #[must_use]; without the return \
         value a section cannot tell a completed record from one the budget cut, and \
         without the attribute a future edit can drop the answer silently"
    );

    let total = sections.matches("writer.close()").count();
    let branched = sections.matches("if !writer.close()").count();
    let returned = sections.matches("\n    writer.close()\n}").count();
    let discarded = sections.matches("let _ = writer.close()").count();
    assert!(total > 0, "sections.rs closes no records at all");
    assert_eq!(
        total,
        branched + returned + discarded,
        "sections.rs has {total} `writer.close()` call sites but only {} carry the \
         verdict ({branched} branched on, {returned} returned, {discarded} explicitly \
         discarded); a site that drops it reports a fragment as a completed section",
        branched + returned + discarded
    );
    assert_eq!(
        discarded, 1,
        "exactly one call site may discard the verdict -- the `[BXCAP:NOTE \
         sched_lock_held]` refusal, whose section is being reported as skipped \
         either way. Found {discarded}; a new one has to be argued for, not added"
    );
}

#[test]
#[should_panic(expected = "reports a fragment as a completed section")]
fn a_section_that_drops_the_close_verdict_would_be_caught() {
    // The pre-fix shape, applied in memory: a section closes its record and
    // returns `true` unconditionally. The file on disk is untouched.
    let record = read("kernel/src/capture/record.rs");
    let sections = read("kernel/src/capture/sections.rs").replacen(
        "if !writer.close() {\n            return false;\n        }",
        "writer.close();",
        1,
    );
    assert_close_verdicts_are_carried(&record, &sections);
}

#[test]
#[should_panic(expected = "must return its verdict and be #[must_use]")]
fn dropping_the_must_use_on_close_would_be_caught() {
    let record = read("kernel/src/capture/record.rs").replace("#[must_use]\n    pub fn close", "pub fn close");
    let sections = read("kernel/src/capture/sections.rs");
    assert_close_verdicts_are_carried(&record, &sections);
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

    // The COMPLETE strict list, both directions. An earlier revision checked
    // two spellings -- `.lock()` and `try_dump_state` -- on the reasoning that
    // everything else overlapped the shared list. It did not: the shell denied
    // `to_string` and `expect(` and this suite's DENIED array carried neither,
    // so the two guards had drifted where neither was looking. This loop now
    // checks 11 of 11 spellings of the strict list in both directions.
    for needle in [
        ".lock()",
        "try_dump_state",
        "alloc::",
        "Vec<",
        "String",
        "Box<",
        "vec!",
        "to_string",
        "unwrap()",
        "expect(",
        "panic!",
    ] {
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

    // And no gate script builds with it. Census-anchored on
    // `docker/qemu/**/*.sh` as it exists on disk, NOT a literal list: an
    // earlier revision of this test named 4 scripts and the tree has 30-odd,
    // so a gate that grew a `capture_selftest` build would have had to be one
    // of the 4 named to be caught. An empty set is a failure, not a pass.
    let gates = gate_scripts();
    assert!(
        gates.len() >= 20,
        "expected the tree's gate scripts under {GATE_DIR}, found {} -- a census that \
         collapsed to a handful of files is not checking the gates",
        gates.len()
    );
    for (gate, body) in &gates {
        for (lineno, line) in code_lines(body) {
            assert!(
                !(line.contains("--features") && line.contains("capture_selftest")),
                "{gate}:{lineno} builds with capture_selftest; the self-test capture is a \
                 development knob, and a gate that builds it would be scoring a marker no \
                 shipped kernel emits:\n  {line}"
            );
        }
    }
}

/// The `.sh` files under `docker/qemu/`, recursively.
fn gate_scripts() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("docker/qemu must exist") {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("sh") {
                out.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(&repo_path(GATE_DIR), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(repo_path(""))
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let body = fs::read_to_string(&path).expect("readable script");
            (name, body)
        })
        .collect()
}

// ---------------------------------------------------------------------
// The strict soft-lockup scope (failure-capture PR-7).
//
// `kernel/src/arch_impl/aarch64/timer_interrupt.rs` was already a critical
// file, but only for the SHARED logging list: the capture-scoped denylist --
// `.lock()`, `try_dump_state`, the allocation and panic spellings -- applied
// to `kernel/src/capture/` alone. That is how an allocating scheduler dump and
// an allocating process dump sat inside a timer-IRQ report with neither guard
// seeing them.
//
// Adding the whole timer file to the strict list is the wrong repair: it
// legitimately carries a CPU0-regression `panic!` outside the dump's forward
// call path, and a guard that demanded that panic be moved or pinned would be
// demanding the wrong change. The scope is instead the dump's own item body
// plus the bodies of local helpers in the same file reached by syntactically
// resolved calls, derived from the calls rather than from a name list.
//
// WHERE THAT EXTRACTION LIVES, stated plainly: in
// `scripts/check-aarch64-lockup-no-alloc.sh --extract-source`, and this suite
// RUNS it rather than reimplementing it. So this suite does not independently
// re-derive the scope, and a bug in the extractor is not caught by a second
// implementation disagreeing. What catches it instead is the mutation legs
// below, which run the extractor against deliberately mutated source and
// require the mutation to come back.
//
// This is an ADVISORY source boundary either way. Cross-file and indirect
// reachability is the binary mode of that same script, which the three aarch64
// gates run against their own selected kernel.
// ---------------------------------------------------------------------

const TIMER_SOURCE: &str = "kernel/src/arch_impl/aarch64/timer_interrupt.rs";
const LOCKUP_GUARD_SCRIPT: &str = "scripts/check-aarch64-lockup-no-alloc.sh";

/// Run the guard's source mode. `over` optionally replaces the timer file it
/// reads, which is what the mutation legs use instead of writing into the tree.
fn extract_lockup_scope(over: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new(repo_path(LOCKUP_GUARD_SCRIPT));
    command.arg("--extract-source");
    if let Some(path) = over {
        command.arg("--source").arg(path);
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run {LOCKUP_GUARD_SCRIPT}: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(stderr);
    }
    Ok(stdout)
}

/// The `fn` names the extractor put in scope, in emission order.
fn scope_item_names(scope: &str) -> Vec<String> {
    let mut names = Vec::new();
    let marker = "// ---- strict lockup scope: fn ";
    for line in scope.lines() {
        let Some(rest) = line.strip_prefix(marker) else {
            continue;
        };
        let Some(name) = rest.split_whitespace().next() else {
            continue;
        };
        names.push(name.to_string());
    }
    names
}

/// A temp copy of the timer file with `find` replaced by `with`.
fn mutated_timer_source(tag: &str, find: &str, with: &str) -> PathBuf {
    let body = read(TIMER_SOURCE);
    assert!(
        body.contains(find),
        "mutation leg {tag} cannot find its anchor in {TIMER_SOURCE}"
    );
    let dir = std::env::temp_dir().join("breenix-lockup-scope-mutations");
    fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join(format!("{tag}.rs"));
    fs::write(&path, body.replacen(find, with, 1)).expect("write mutated source");
    path
}

#[test]
fn the_strict_lockup_scope_carries_no_lock_allocation_or_formatting() {
    let scope = extract_lockup_scope(None).unwrap();
    assert!(
        !scope.trim().is_empty(),
        "the strict soft-lockup scope extracted to nothing"
    );
    assert_denylist_clean("strict soft-lockup scope", &scope);
}

/// Anti-vacuity for the scope itself. A scope that collapsed to the dump alone
/// would pass the denylist while checking none of the helpers the dump calls,
/// which is the shape this guard exists to reach.
/// claim-lint:ok: the scope reaches 4 of 4 items on this tree --
/// dump_lockup_state, raw_serial_str, print_timer_count_decimal, raw_serial_char --
/// as printed by scripts/check-aarch64-lockup-no-alloc.sh --extract-source
#[test]
fn the_strict_lockup_scope_reaches_the_helpers_the_dump_calls() {
    let scope = extract_lockup_scope(None).unwrap();
    let names = scope_item_names(&scope);
    assert!(
        names.first().map(String::as_str) == Some("dump_lockup_state"),
        "the scope must be seeded by the dump itself, got {names:?}"
    );
    assert!(
        names.len() >= 2,
        "the scope reached only {names:?}; the dump calls local helpers and \
         those bodies are part of what the denylist has to read"
    );
}

const DUMP_ANCHOR: &str = "fn dump_lockup_state(stall_ticks: u64) {";

const ALLOC_LINE: &str = "\n    let _s = alloc::string::String::new()";

#[test]
#[should_panic(expected = "contains `alloc::`")]
fn an_allocation_inserted_into_the_dump_would_be_caught() {
    let mut injected = String::from(DUMP_ANCHOR);
    injected.push_str(ALLOC_LINE);
    let path = mutated_timer_source("alloc-in-dump", DUMP_ANCHOR, &injected);
    let scope = extract_lockup_scope(Some(&path)).unwrap();
    assert_denylist_clean("mutated soft-lockup scope", &scope);
}

const HELPER_DECL: &str = "fn innocuously_named_helper() {";
const HELPER_TAIL: &str = ";\n}\n\n";
const HELPER_CALL: &str = "\n    innocuously_named_helper();";

/// The leg the depth-1 shape cannot do: the allocation is not in the dump, it
/// is in a NEWLY NAMED helper the dump calls. No list names that helper in
/// advance, so only a scope derived from the calls themselves reaches it.
/// claim-lint:ok: this is the mutation leg itself; it reddens the denylist when
/// the helper is introduced, which is what makes the derivation checkable
#[test]
#[should_panic(expected = "contains `alloc::`")]
fn an_allocation_behind_a_newly_named_local_helper_would_be_caught() {
    let mut injected = String::from(HELPER_DECL);
    injected.push_str(ALLOC_LINE);
    injected.push_str(HELPER_TAIL);
    injected.push_str(DUMP_ANCHOR);
    injected.push_str(HELPER_CALL);
    let path = mutated_timer_source("alloc-behind-helper", DUMP_ANCHOR, &injected);
    let scope = extract_lockup_scope(Some(&path)).unwrap();
    assert_denylist_clean("mutated soft-lockup scope", &scope);
}

const DUMP_NAME: &str = "fn dump_lockup_state";
const ABSENT_NAME: &str = "fn dump_lockup_absent";

#[test]
fn a_dump_the_extractor_cannot_find_is_a_failure() {
    let path = mutated_timer_source("root-not-found", DUMP_NAME, ABSENT_NAME);
    let outcome = extract_lockup_scope(Some(&path));
    let message = outcome.expect_err("an absent root must not extract an empty scope");
    assert!(
        message.contains("no `fn dump_lockup_state` item"),
        "the extractor must say the root is missing, got: {message}"
    );
}

/// A new file under `kernel/src/capture/` is covered by the disk census the
/// day it lands. This leg proves the denylist that census feeds actually
/// rejects an allocation, without writing a file into the tree to do it.
/// claim-lint:ok: a mutation leg -- the synthetic module is rejected by the same
/// assert_denylist_clean the disk census feeds
#[test]
#[should_panic(expected = "contains `alloc::`")]
fn an_allocation_in_a_new_capture_module_file_would_be_caught() {
    let mut synthetic = String::from("pub fn sample() {");
    synthetic.push_str(ALLOC_LINE);
    synthetic.push_str(";\n}\n");
    assert_denylist_clean("kernel/src/capture/synthetic_new_module.rs", &synthetic);
}

/// The shell script has to actually consume the strict scope, and it has to
/// offer a clean-exit mode for it. The full scan is a DEBT REPORT whose
/// nonzero status is not a failure signal, so without `--capture-only` a
/// caller has no honest way to gate on these surfaces.
#[test]
fn the_shell_guard_consumes_the_strict_lockup_scope() {
    let script = read(CRITICAL_PATH_SCRIPT);
    let needles = ["--extract-source", "check_lockup_scope", "--capture-only"];
    for needle in needles {
        assert!(
            script.contains(needle),
            "{CRITICAL_PATH_SCRIPT} must carry the strict-scope wiring `{needle}`"
        );
    }
}
