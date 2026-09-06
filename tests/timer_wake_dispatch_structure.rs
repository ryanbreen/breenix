//! Structural ratchet for #766: a late timer wake is dispatched at the HEAD of
//! its ready queue, not at the tail.
//!
//! `Scheduler::wake_expired_timers` (`kernel/src/task/scheduler.rs`) pops
//! entries only while `wake_time <= now`, so a thread it enqueues is one whose
//! sleep deadline has already passed. It used to enqueue those at the
//! TAIL of the target ready queue. On x86 `MAX_CPUS` is 1, so that is the one
//! queue the runnable threads share, and the woken thread then waited for each
//! thread ahead of it to exhaust a full quantum -- measured in #766 as a
//! wake-to-dispatch overrun of p90 2592 ms and max 10318 ms over 324 trials.
//!
//! What is pinned here is the SHAPE, not a line number and not a diff:
//!
//! * inside `wake_expired_timers`, the ready-queue enqueue is `push_front`, and
//!   no `push_back` survives in that function -- so a future edit that reverts
//!   to a tail enqueue, or adds a second enqueue at the tail, reddens this;
//! * the `QUANTUM_TICKS` the #766 oracle prints its `quantum_ms` from equals the
//!   `TIME_QUANTUM` literal in BOTH timer interrupt handlers, so the oracle's
//!   reported bound cannot silently drift from the quantum the scheduler
//!   actually runs (the two `TIME_QUANTUM` declarations are private, and the
//!   x86 one lives in a Tier-1 file this change has no reason to edit);
//! * the oracle is `boot_tests`-only and is called from both architectures'
//!   boot paths, so the two arches run the same leg;
//! * the x86 boot-test gate asserts the marker, the x86 and aarch64 production
//!   gates assert its ABSENCE, and the aarch64 strict gate asserts it -- a leg
//!   no gate reads is a leg that cannot fail.
//!
//! The `#[should_panic]` legs are the anti-vacuity proof: they run the same
//! assertion bodies against in-memory sources carrying the pre-#766 shape and
//! assert that the assertions fail. The mutation applied to the real file on
//! disk, and its exit status, is recorded in
//! `docs/planning/green-program/timekeeping/766-TIMER-WAKE-DISPATCH-2026-09-06.md`.

use std::fs;
use std::path::PathBuf;

const SCHEDULER_SOURCE: &str = "kernel/src/task/scheduler.rs";
const ORACLE_SOURCE: &str = "kernel/src/task/timer_wake_oracle.rs";
const TASK_MOD_SOURCE: &str = "kernel/src/task/mod.rs";
const X86_MAIN_SOURCE: &str = "kernel/src/main.rs";
const AARCH64_MAIN_SOURCE: &str = "kernel/src/main_aarch64.rs";
const X86_TIMER_SOURCE: &str = "kernel/src/interrupts/timer.rs";
const AARCH64_TIMER_SOURCE: &str = "kernel/src/arch_impl/aarch64/timer_interrupt.rs";
const X86_BOOT_TESTS_GATE: &str = "docker/qemu/run-x86-boot-tests.sh";
const X86_PROD_GATE: &str = "docker/qemu/run-x86-prod-profile-boot-test.sh";
const AARCH64_STRICT_GATE: &str = "docker/qemu/run-aarch64-boot-test-strict.sh";
const AARCH64_PROD_GATE: &str = "docker/qemu/run-aarch64-prod-profile-boot-test.sh";

/// The marker's fixed prefix. Everything after it is per-boot numbers.
const MARKER_PREFIX: &str = "[TIMER_WAKE_LATENCY_ORACLE:";

fn read(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

/// The text of the first item whose opening line contains `header`, up to the
/// closing brace at that line's own indentation. Same helper shape
/// `tests/ring_span_report_site_structure.rs` uses: it finds the item by what
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

/// The assertion body shared by the real-source test and its mutation leg.
fn assert_late_wakes_go_to_the_head(body: &str) {
    let tail = code_lines_mentioning(body, "push_back(");
    assert!(
        tail.is_empty(),
        "`wake_expired_timers` must not enqueue at the tail of a ready queue: every entry it \
         pops has a deadline that already passed, and a tail enqueue makes the woken thread \
         wait a full round robin behind threads that are not late at all (#766). \
         Offending line(s): {tail:?}"
    );
    let head = code_lines_mentioning(body, "push_front(");
    assert!(
        !head.is_empty(),
        "`wake_expired_timers` must enqueue a late wake at the HEAD of its target ready queue \
         (#766); no `push_front(` call was found in it, so either the enqueue moved out of this \
         function or it stopped happening at all"
    );
}

/// Value of a `const <name>: <ty> = <literal>;` declaration, by name.
fn const_literal(source: &str, name: &str) -> u64 {
    let needle = format!("const {name}");
    let line = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .find(|line| line.contains(&needle) && line.contains('='))
        .unwrap_or_else(|| panic!("no `const {name}` declaration in the source under test"));
    let value = line
        .split('=')
        .nth(1)
        .unwrap_or_else(|| panic!("no value on `{line}`"));
    let digits: String = value
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit() || *c == '_')
        .filter(|c| *c != '_')
        .collect();
    digits
        .parse()
        .unwrap_or_else(|error| panic!("`{line}` does not carry an integer literal: {error}"))
}

/// The assertion body shared by the real-source test and its mutation leg.
fn assert_quantum_agrees(oracle: &str, x86_timer: &str, aarch64_timer: &str) {
    let declared = const_literal(oracle, "QUANTUM_TICKS");
    let x86 = const_literal(x86_timer, "TIME_QUANTUM");
    let aarch64 = const_literal(aarch64_timer, "TIME_QUANTUM");
    assert_eq!(
        declared, x86,
        "the #766 oracle prints its `quantum_ms` (and therefore the bound it is read against) \
         from QUANTUM_TICKS={declared}, but the x86 timer interrupt handler runs TIME_QUANTUM={x86} \
         ticks. Update the oracle's copy, or the number on the marker is not the quantum the \
         scheduler enforces."
    );
    assert_eq!(
        declared, aarch64,
        "the #766 oracle prints its `quantum_ms` from QUANTUM_TICKS={declared}, but the aarch64 \
         timer interrupt handler runs TIME_QUANTUM={aarch64} ticks."
    );
}

#[test]
fn a_late_timer_wake_is_enqueued_at_the_head() {
    let source = read(SCHEDULER_SOURCE);
    assert_late_wakes_go_to_the_head(&item_body(&source, "pub fn wake_expired_timers("));
}

#[test]
fn the_oracle_reports_the_quantum_the_scheduler_enforces() {
    assert_quantum_agrees(
        &read(ORACLE_SOURCE),
        &read(X86_TIMER_SOURCE),
        &read(AARCH64_TIMER_SOURCE),
    );
}

#[test]
fn the_oracle_is_boot_tests_only_and_runs_on_both_architectures() {
    let task_mod = read(TASK_MOD_SOURCE);
    let module_line = task_mod
        .lines()
        .position(|line| line.contains("pub mod timer_wake_oracle;"))
        .expect("kernel/src/task/mod.rs must register the #766 oracle module");
    let guard = task_mod
        .lines()
        .nth(module_line - 1)
        .expect("a module declaration cannot be the first line of the file");
    assert!(
        guard.contains(r#"cfg(feature = "boot_tests")"#),
        "the #766 oracle makes 8 threads CPU-bound on purpose and must stay out of the shipped \
         kernel, so its module declaration must sit under \
         `#[cfg(feature = \"boot_tests\")]`; found `{guard}`"
    );

    for (path, source) in [
        (X86_MAIN_SOURCE, read(X86_MAIN_SOURCE)),
        (AARCH64_MAIN_SOURCE, read(AARCH64_MAIN_SOURCE)),
    ] {
        let call = source
            .lines()
            .position(|line| line.contains("timer_wake_oracle::run()"))
            .unwrap_or_else(|| panic!("{path} must call the #766 oracle: the two architectures \
                 run the same leg, and an arm that is never called cannot fail"));
        let guard = source
            .lines()
            .nth(call - 1)
            .unwrap_or_else(|| panic!("{path}: the call cannot be the first line of the file"));
        assert!(
            guard.contains(r#"feature = "boot_tests""#),
            "{path} must call the #766 oracle under `feature = \"boot_tests\"`; found `{guard}`"
        );
    }
}

#[test]
fn the_gates_read_the_marker() {
    for path in [X86_BOOT_TESTS_GATE, AARCH64_STRICT_GATE] {
        assert!(
            read(path).contains(MARKER_PREFIX),
            "{path} must assert `{MARKER_PREFIX}...`: a leg no gate reads is a leg that cannot \
             fail (#766)"
        );
    }
    for path in [X86_PROD_GATE, AARCH64_PROD_GATE] {
        assert!(
            read(path).contains(MARKER_PREFIX),
            "{path} must assert `{MARKER_PREFIX}...` is ABSENT: the oracle is boot_tests-only, \
             and a count of 0 on the shipped profile is a reading where a silent absence is an \
             assumption (#766)"
        );
    }
}

// ---------------------------------------------------------------------------
// Anti-vacuity. Each leg feeds the assertion body above the pre-#766 shape and
// asserts that the assertion rejects it.
// ---------------------------------------------------------------------------

const TAIL_ENQUEUE_BODY: &str = r#"    pub fn wake_expired_timers(&mut self) {
        while let Some(&Reverse((wake_time, tid))) = self.timer_heap.peek() {
            if let Some(target) = self.find_target_cpu_for_wakeup(tid) {
                self.per_cpu_queues[target].push_back(tid);
            }
        }
    }"#;

const NO_ENQUEUE_BODY: &str = r#"    pub fn wake_expired_timers(&mut self) {
        while let Some(&Reverse((wake_time, tid))) = self.timer_heap.peek() {
            let _ = tid;
        }
    }"#;

#[test]
#[should_panic(expected = "must not enqueue at the tail")]
fn the_head_check_rejects_a_tail_enqueue() {
    assert_late_wakes_go_to_the_head(TAIL_ENQUEUE_BODY);
}

#[test]
#[should_panic(expected = "must enqueue a late wake at the HEAD")]
fn the_head_check_rejects_a_body_with_no_enqueue_at_all() {
    assert_late_wakes_go_to_the_head(NO_ENQUEUE_BODY);
}

#[test]
#[should_panic(expected = "x86 timer interrupt handler runs TIME_QUANTUM")]
fn the_quantum_check_rejects_a_drifted_x86_quantum() {
    assert_quantum_agrees(
        "const QUANTUM_TICKS: u64 = 10;",
        "const TIME_QUANTUM: u32 = 4;",
        "const TIME_QUANTUM: u32 = 10;",
    );
}

#[test]
#[should_panic(expected = "aarch64 timer interrupt handler runs TIME_QUANTUM")]
fn the_quantum_check_rejects_a_drifted_aarch64_quantum() {
    assert_quantum_agrees(
        "const QUANTUM_TICKS: u64 = 10;",
        "const TIME_QUANTUM: u32 = 10;",
        "const TIME_QUANTUM: u32 = 7;",
    );
}
