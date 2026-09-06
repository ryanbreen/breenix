//! Structural ratchet for #847 (ruling R188): the timer tick does not print.
//!
//! `ring_span_self_check` (`kernel/src/tracing/providers/irq.rs`) runs inside
//! `trace_timer_tick`, which both timer interrupt handlers call directly on
//! each tick. It used to serialize its `[RING_SPAN:...]` marker to the UART
//! from there, with the lock-free `raw_serial_*` writers, because the
//! logger's lock is unavailable in an ISR. On a `-smp 4` aarch64 boot that
//! lock-freedom let another CPU's serial line interleave byte-for-byte with
//! the marker on the shared UART, corrupting it; the strict gate then scored
//! an otherwise-passing boot "Ring-span self-check marker missing" (#847).
//!
//! The fix moves the PRINT, not the measurement: the tick publishes its
//! numbers to atomics, and a boot-test in `kernel/src/test_framework/registry.rs`
//! claims them from thread context and emits the marker through
//! `serial_println!` -- the locked, interrupt-masked writer this framework's
//! `[TEST:...]` markers go through.
//!
//! This suite pins that SHAPE, not a line number:
//!
//! - no `raw_serial*` call anywhere in `trace_timer_tick`, or in the
//!   `ring_span_self_check` module it calls, so no path from the tick can
//!   reach an unlocked serial write again;
//! - the `[RING_SPAN:` marker text does not appear in the provider at all;
//! - it does appear in the registry, on a `serial_println!` invocation, and
//!   the test that emits it is registered in the test table.
//!
//! The two `#[should_panic]` legs are the anti-vacuity proof: they run the
//! same assertion bodies against in-memory sources with the print moved back
//! into the provider, and assert the assertions fail. The round doc records
//! the same mutation applied to the real files on disk and re-run, for the
//! mutation table's cmd/exit/assertion record:
//! docs/planning/green-program/failure-capture/847-RING-SPAN-THREAD-PRINT-2026-09-06.md

use std::fs;
use std::path::PathBuf;

const IRQ_PROVIDER_SOURCE: &str = "kernel/src/tracing/providers/irq.rs";
const REGISTRY_SOURCE: &str = "kernel/src/test_framework/registry.rs";

/// The marker's fixed prefix. Everything after it is per-boot numbers, so
/// this is the whole of the literal a print site can be recognised by.
const MARKER_PREFIX: &str = "[RING_SPAN:cpu=";

fn read(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

/// The text of the first item whose opening line contains `header`, up to the
/// closing brace at that line's own indentation. Same helper shape
/// tests/trace_ring_depth_structure.rs and
/// tests/dispatch_path_lock_free_structure.rs use: it finds the item by what
/// it is called, not by where it sits.
fn item_body(source: &str, header: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains(header))
        .unwrap_or_else(|| panic!("no `{header}` in the source under test"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let terminator = format!("{}}}", " ".repeat(indent));
    let end = lines[start..]
        .iter()
        .position(|line| *line == terminator)
        .unwrap_or_else(|| panic!("no terminator for `{header}`"))
        + start;
    lines[start..=end].join("\n")
}

/// Lines of `body` that are code (not a `//` comment) and mention `needle`.
fn code_lines_mentioning<'a>(body: &'a str, needle: &str) -> Vec<&'a str> {
    body.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| line.contains(needle))
        .collect()
}

/// The assertion body shared by the real-source tests and the anti-vacuity
/// mutation legs: given a chunk of the tick provider, panic if it can reach
/// an unlocked serial writer.
fn assert_no_unlocked_serial_write(what: &str, body: &str) {
    let offenders = code_lines_mentioning(body, "raw_serial");
    assert!(
        offenders.is_empty(),
        "{what} must not write to serial: the timer tick reaches it on every tick, and an \
         unlocked multi-byte write there interleaves with other CPUs' serial lines (#847). \
         Publish to an atomic and let the registry test print. Offending line(s): {offenders:?}"
    );
}

/// The assertion body for "the marker text is not printed from the provider".
fn assert_marker_is_not_emitted_here(what: &str, source: &str) {
    let offenders = code_lines_mentioning(source, MARKER_PREFIX);
    assert!(
        offenders.is_empty(),
        "{what} must not carry the `{MARKER_PREFIX}...` marker text: the print site moved to \
         the registry test, which emits it through the locked serial writer (#847). \
         Offending line(s): {offenders:?}"
    );
}

#[test]
fn the_tick_provider_reaches_no_unlocked_serial_writer() {
    let source = read(IRQ_PROVIDER_SOURCE);
    assert_no_unlocked_serial_write(
        "trace_timer_tick",
        &item_body(&source, "fn trace_timer_tick("),
    );
    assert_no_unlocked_serial_write(
        "the ring_span_self_check module",
        &item_body(&source, "mod ring_span_self_check {"),
    );
}

#[test]
fn the_marker_text_is_not_in_the_tick_provider() {
    let source = read(IRQ_PROVIDER_SOURCE);
    assert_marker_is_not_emitted_here("the irq trace provider", &source);
}

/// The statement the marker literal at `offset` belongs to: everything back
/// to the nearest preceding statement or block boundary. The marker sits in a
/// macro's argument list, which spans several lines, so the writer that emits
/// it cannot be identified from the marker's own line.
fn enclosing_statement(source: &str, offset: usize) -> &str {
    let start = source[..offset]
        .rfind([';', '{', '}'])
        .map(|boundary| boundary + 1)
        .unwrap_or(0);
    &source[start..offset]
}

#[test]
fn the_registry_prints_the_marker_through_the_locked_writer() {
    let source = read(REGISTRY_SOURCE);
    let emitters = code_lines_mentioning(&source, MARKER_PREFIX);
    assert_eq!(
        emitters.len(),
        1,
        "exactly one print site for `{MARKER_PREFIX}...` is expected in the registry, found: \
         {emitters:?}"
    );
    let offset = source
        .find(MARKER_PREFIX)
        .expect("the marker literal was just located by line");
    let statement = enclosing_statement(&source, offset);
    assert!(
        statement.contains("serial_println!"),
        "the ring-span marker must be emitted through `serial_println!` -- the locked, \
         interrupt-masked writer every `[TEST:...]` line uses -- not through any other \
         writer: {statement}"
    );
}

#[test]
fn the_printing_test_is_registered_in_the_test_table() {
    let source = read(REGISTRY_SOURCE);
    assert!(
        source.contains(r#"name: "ring_span_report","#),
        "the test that prints the ring-span marker must be registered in the test table, \
         or the aarch64 executor never runs it and the marker never appears"
    );
}

/// Anti-vacuity for `assert_no_unlocked_serial_write`: the exact regression
/// this ratchet exists to catch is someone moving the print back into the
/// tick. Built from the real module body with one line inserted, so it stays
/// byte-for-byte the real shape plus the one change under test.
#[test]
#[should_panic(expected = "must not write to serial")]
fn moving_the_print_back_into_the_tick_would_be_caught() {
    let source = read(IRQ_PROVIDER_SOURCE);
    let body = item_body(&source, "mod ring_span_self_check {");
    let anchor = "        READY.store(true, Ordering::Release);";
    assert!(
        body.contains(anchor),
        "test fixture assumption broken -- the publication line this test inserts a serial \
         write beside no longer matches the real source:\n{body}"
    );
    let mutated = body.replace(
        anchor,
        "        raw_serial_str(\"[RING_SPAN:cpu=\");\n        READY.store(true, Ordering::Release);",
    );
    assert_no_unlocked_serial_write("the ring_span_self_check module", &mutated);
}

/// Anti-vacuity for `assert_marker_is_not_emitted_here`.
#[test]
#[should_panic(expected = "must not carry the")]
fn moving_the_marker_text_back_into_the_provider_would_be_caught() {
    let source = read(IRQ_PROVIDER_SOURCE);
    let anchor = "        READY.store(true, Ordering::Release);";
    assert!(
        source.contains(anchor),
        "test fixture assumption broken -- the publication line this test inserts the marker \
         text beside no longer matches the real source"
    );
    let mutated = source.replace(
        anchor,
        "        raw_serial_str(\"[RING_SPAN:cpu=0:span_ms=\");\n        READY.store(true, Ordering::Release);",
    );
    assert_marker_is_not_emitted_here("the irq trace provider", &mutated);
}
