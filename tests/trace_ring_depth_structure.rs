//! Structural ratchet for the failure-trace-capture PR-2 tick-sampling fix.
//!
//! `trace_timer_tick` (`kernel/src/tracing/providers/irq.rs`) must keep
//! incrementing `TIMER_TICK_TOTAL` on each call -- the soft-lockup detector,
//! per-CPU idle-tick accounting and `/proc/stat` depend on it -- while
//! recording a `TIMER_TICK` ring EVENT only once per `TICK_SAMPLE` ticks. The
//! plan and round doc for this PR are at
//! docs/planning/green-program/failure-capture/PR-2-2026-09-05.md.
//!
//! This suite pins the SHAPE of that guard, not a line number: the counter
//! increment must sit outside any `TICK_SAMPLE` condition, and the ring-write
//! call must sit textually after, and inside, an `if` whose condition
//! mentions `% TICK_SAMPLE`. A future edit that records each tick again,
//! unconditionally -- silently collapsing the ring back to its pre-fix span,
//! the exact regression this PR closes -- fails here before it ever reaches
//! a gate.
//!
//! `tick_sample_guard_deletion_would_be_caught` is the anti-vacuity leg: it
//! runs the same assertion body against a source string with the guard's
//! `% TICK_SAMPLE` condition deleted (an in-memory mutation, not a rebuild)
//! and asserts that mutation panics. The round doc also records the same
//! mutation applied to the real file on disk and rebuilt, for the mutation
//! table's cmd/exit/assertion record.

use std::fs;
use std::path::PathBuf;

const IRQ_PROVIDER_SOURCE: &str = "kernel/src/tracing/providers/irq.rs";

fn read(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

/// The body of the first `fn <name>(` in `source`, up to the closing brace at
/// the same indentation as the signature line. Copied from the identical
/// helper in tests/dispatch_path_lock_free_structure.rs; `trace_timer_tick`
/// is written at one indentation level, which is what makes that terminator
/// exact.
fn function_body(source: &str, name: &str) -> String {
    let needle = format!("fn {name}(");
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("no `{needle}` in the source under test"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let terminator = format!("{}}}", " ".repeat(indent));
    let end = lines[start..]
        .iter()
        .position(|line| *line == terminator)
        .unwrap_or_else(|| panic!("no terminator for `{needle}`"))
        + start;
    lines[start..=end].join("\n")
}

/// The assertion body shared by the real-source test and the anti-vacuity
/// mutation leg below: given `trace_timer_tick`'s function body as a string,
/// panic unless the ring-write sits inside the sampling guard.
fn assert_record_is_sampling_guarded(body: &str) {
    let record_pos = body
        .find("record_event(TIMER_TICK")
        .unwrap_or_else(|| panic!("trace_timer_tick must record a TIMER_TICK ring event:\n{body}"));
    let guard_pos = body
        .find("% TICK_SAMPLE")
        .unwrap_or_else(|| {
            panic!("trace_timer_tick must gate its ring write on `tick_count % TICK_SAMPLE`:\n{body}")
        });
    assert!(
        guard_pos < record_pos,
        "the `% TICK_SAMPLE` guard must appear before the record_event call it protects, not after:\n{body}"
    );

    // The two must share one unterminated `if` block: no line between the
    // guard and the call may close a brace at or above the guard's own
    // indentation. This is what stops a guard written for something
    // unrelated, earlier in the function, from satisfying the position check
    // above by coincidence.
    let guard_line_idx = body[..guard_pos].matches('\n').count();
    let record_line_idx = body[..record_pos].matches('\n').count();
    let lines: Vec<&str> = body.lines().collect();
    let guard_indent = lines[guard_line_idx].len() - lines[guard_line_idx].trim_start().len();
    for line in &lines[guard_line_idx + 1..record_line_idx] {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        assert!(
            !(trimmed.starts_with('}') && indent <= guard_indent),
            "the sampling guard closes before record_event(TIMER_TICK ...) is reached -- \
             the ring write has escaped the guard:\n{body}"
        );
    }
}

#[test]
fn tick_sample_constant_exists_and_is_named() {
    let source = read(IRQ_PROVIDER_SOURCE);
    assert!(
        source.contains("const TICK_SAMPLE: u64"),
        "trace_timer_tick's sampling divisor must be a named constant, not a magic number"
    );
}

#[test]
fn timer_tick_total_still_increments_unconditionally() {
    let source = read(IRQ_PROVIDER_SOURCE);
    let body = function_body(&source, "trace_timer_tick");
    let increment_line = body
        .lines()
        .find(|line| line.contains("TIMER_TICK_TOTAL.increment()"))
        .unwrap_or_else(|| {
            panic!("trace_timer_tick must still call TIMER_TICK_TOTAL.increment():\n{body}")
        });
    assert!(
        !increment_line.contains("TICK_SAMPLE"),
        "TIMER_TICK_TOTAL must increment unconditionally, not behind the sampling guard: {increment_line}"
    );
    // The increment must also precede the sampling guard's own `if`, not
    // follow it -- the soft-lockup detector and idle-tick accounting must
    // see it counted before this function does anything conditional.
    let increment_pos = body
        .find("TIMER_TICK_TOTAL.increment()")
        .expect("checked above");
    let guard_pos = body
        .find("% TICK_SAMPLE")
        .expect("checked by the sibling test in this module");
    assert!(
        increment_pos < guard_pos,
        "TIMER_TICK_TOTAL.increment() must run before the sampling guard is evaluated:\n{body}"
    );
}

#[test]
fn the_ring_event_is_recorded_only_inside_the_sampling_guard() {
    let source = read(IRQ_PROVIDER_SOURCE);
    let body = function_body(&source, "trace_timer_tick");
    assert_record_is_sampling_guarded(&body);
}

/// Anti-vacuity: proves `assert_record_is_sampling_guarded` can fail, against
/// a source string with the guard deleted (the #549/#551-style regression
/// this ratchet exists to catch) but otherwise identical to the real
/// function. Built from the real function body with one substring removed,
/// not hand-written, so it stays byte-for-byte the real shape minus the one
/// change under test.
#[test]
#[should_panic(expected = "gate its ring write")]
fn tick_sample_guard_deletion_would_be_caught() {
    let source = read(IRQ_PROVIDER_SOURCE);
    let body = function_body(&source, "trace_timer_tick");
    assert!(
        body.contains(" && tick_count % TICK_SAMPLE == 0"),
        "test fixture assumption broken -- the guard clause text this test \
         deletes no longer matches the real source:\n{body}"
    );
    let mutated = body.replace(" && tick_count % TICK_SAMPLE == 0", "");
    assert_record_is_sampling_guarded(&mutated);
}
