//! #855 oracle print-site and strict-gate structural ratchets.

use std::fs;
use std::path::PathBuf;

const IRQ_PROVIDER_SOURCE: &str = "kernel/src/tracing/providers/irq.rs";
const REGISTRY_SOURCE: &str = "kernel/src/test_framework/registry.rs";

/// The marker's fixed prefix. Everything after it is per-boot numbers, so
/// this is the whole of the literal a print site can be recognised by.
const MARKER_PREFIX: &str = "[RING_SPAN_UNFILTERED:cpu=";

#[test]
fn unfiltered_report_is_released_after_the_synchronous_capture() {
    let source = read(IRQ_PROVIDER_SOURCE);
    let oracle = item_body(&source, "mod unfiltered_ring_span_self_check {");
    let observe = item_body(&oracle, "fn observe(");
    let claim = observe.find("CHECKED.swap(true, Ordering::AcqRel)").unwrap();
    let snapshot = observe.find("publish();").unwrap();
    let capture = observe.find("crate::capture::selftest::observe(tick_count);").unwrap();
    let ready = observe.find("READY.store(true, Ordering::Release);").unwrap();
    assert!(claim < snapshot && snapshot < capture && capture < ready);
    assert!(!item_body(&oracle, "fn publish(").contains("READY.store"));
    let tick = item_body(&source, "fn trace_timer_tick(");
    assert!(tick.contains(
        "#[cfg(feature = \"capture_selftest\")]\n    #[cfg(not(target_arch = \"aarch64\"))]\n    crate::capture::selftest::observe(tick_count);"
    ));
}

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
    assert!(
        !source.contains(MARKER_PREFIX),
        "{what} must not carry the `{MARKER_PREFIX}...` marker text: the print site belongs \
         in the registry test, which emits it through the locked serial writer (#847)"
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
        "the unfiltered_ring_span_self_check module",
        &item_body(&source, "mod unfiltered_ring_span_self_check {"),
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
        source.contains(r#"name: "ring_span_unfiltered_report","#),
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
    let body = item_body(&source, "mod unfiltered_ring_span_self_check {");
    let anchor = "        READY.store(true, Ordering::Release);";
    assert!(
        body.contains(anchor),
        "test fixture assumption broken -- the publication line this test inserts a serial \
         write beside no longer matches the real source:\n{body}"
    );
    let mutated = body.replace(
        anchor,
        "        raw_serial_str(\"[RING_SPAN_UNFILTERED:cpu=\");\n        READY.store(true, Ordering::Release);",
    );
    assert_no_unlocked_serial_write("the unfiltered_ring_span_self_check module", &mutated);
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
        "        raw_serial_str(\"[RING_SPAN_UNFILTERED:cpu=0:span_ms=\");\n        READY.store(true, Ordering::Release);",
    );
    assert_marker_is_not_emitted_here("the irq trace provider", &mutated);
}

const GATE_SOURCE: &str = "docker/qemu/run-aarch64-boot-test-strict.sh";
const GATE_HEADER: &str = "if [ \"$KERNEL_HAS_CAPTURE_SELFTEST\" = \"1\" ]; then";

fn compact(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}

// Shell counterpart of item_body: the enclosing `fi` has the header's
// indentation, just as Rust's closing brace does in the helper above.
fn gate_block(source: &str) -> String {
    let lines: Vec<_> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| compact(line) == compact(GATE_HEADER))
        .expect("capture-selftest scoring block must exist");
    let indent = lines[start].len() - lines[start].trim_start().len();
    let end = start
        + lines[start..]
            .iter()
            .position(|line| line.trim() == "fi" && line.len() - line.trim_start().len() == indent)
            .expect("capture-selftest scoring block must close");
    lines[start..=end].join("\n")
}

fn assert_unfiltered_floor(block: &str) {
    // Pin the complete executable block, omitting only comments and echo
    // wording. In particular the same parsed span variable must reach the
    // 1000 ms comparison, and each rejection must return failure.
    let code = block
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter(|line| !line.trim_start().starts_with("echo "))
        .collect::<Vec<_>>()
        .join("\n");
    let expected = r#"
    if [ "$KERNEL_HAS_CAPTURE_SELFTEST" = "1" ]; then
        RING_SPAN_UNFILTERED_FLOOR_MS=1000
        ring_span_unfiltered_line=$(grep -aoE '\[RING_SPAN_UNFILTERED:cpu=[0-9]+:span_ms=[0-9]+:writes=[0-9]+:dropped=[0-9]+\]' "$serial_file" 2>/dev/null | tail -1 || true)
        if [ -z "$ring_span_unfiltered_line" ]; then
            return 1
        fi
        ring_span_unfiltered_ms=$(echo "$ring_span_unfiltered_line" | sed -n 's/.*:span_ms=\([0-9][0-9]*\):.*/\1/p')
        if [ -z "$ring_span_unfiltered_ms" ]; then
            return 1
        fi
        if [ "$ring_span_unfiltered_ms" -lt "$RING_SPAN_UNFILTERED_FLOOR_MS" ]; then
            return 1
        fi
    fi
    "#;
    assert_eq!(
        compact(&code),
        compact(expected),
        "unfiltered gate must wire the parsed span_ms to a 1000 ms floor"
    );
}

#[test]
fn strict_gate_checks_the_parsed_unfiltered_span() {
    let source = read(GATE_SOURCE);
    assert!(source.contains(MARKER_PREFIX));
    let scoring = item_body(&source, "score_serial() {");
    assert_unfiltered_floor(&gate_block(&scoring));
}

#[test]
fn capture_selftest_detection_reads_the_kernel_once_outside_scoring() {
    let source = read(GATE_SOURCE);
    let detection = "grep -aqF '[RING_SPAN_UNFILTERED:cpu=' \"$KERNEL\"";
    assert_eq!(
        source.matches(detection).count(),
        1,
        "feature detection must grep the kernel file once"
    );
    let scoring = item_body(&source, "score_serial() {");
    assert!(
        !scoring.contains(detection),
        "feature detection is a per-kernel fact"
    );
    let start = source
        .find("require_boot_tests_kernel \"$KERNEL\"")
        .unwrap();
    let detection_pos = source.find(detection).unwrap();
    assert!(
        start < detection_pos,
        "detect capture_selftest after requiring boot_tests"
    );
    assert!(
        compact(&source).contains(&compact(
            r#"
        KERNEL_HAS_CAPTURE_SELFTEST=0
        if grep -aqF '[RING_SPAN_UNFILTERED:cpu=' "$KERNEL" 2>/dev/null; then
            KERNEL_HAS_CAPTURE_SELFTEST=1
        fi
    "#
        )),
        "kernel feature detection must control the scoring flag"
    );
}

#[test]
#[should_panic(expected = "unfiltered gate must wire the parsed span_ms to a 1000 ms floor")]
fn weakening_the_unfiltered_floor_would_be_caught() {
    let block = gate_block(&read(GATE_SOURCE));
    assert_unfiltered_floor(&block);
    assert_unfiltered_floor(&block.replace("1000", "1"));
}
