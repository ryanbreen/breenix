//! PR-1 of the critical-path logging drain: what replaced the sixteen deleted
//! `log::*!` calls in `kernel/src/interrupts/context_switch.rs`.
//!
//! The census ratchet next door
//! (`tests/critical_path_logging_census_structure.rs`) says the sixteen calls
//! are GONE. It does not say whether the facts they carried are still
//! published. This suite is the other half: for each of the sixteen sites, the
//! arm now carries either a `trace_dispatch_abandon(DispatchAbandonSite::…)`
//! that already counted it, or exactly one
//! `note_fact(DispatchLogFact::…)` in the new sibling family -- and the ten
//! new facts each reach the `[DISPATCH_STRAND_CENSUS:...]` line under their
//! own name.
//!
//! # Why this is the binding, and the boot oracle is not
//!
//! `run_x86_dispatch_fact_oracle` drives the ten counters directly and shows
//! each field of the census line moving by exactly one. That measures the
//! counter-to-line plumbing on real bytes. It cannot measure the
//! site-to-counter binding, because seven of the ten arms are defensive arms
//! this tree cannot reach on a running kernel -- a userspace thread with no
//! process row, an idle thread with no kernel stack, a `ProcessManager` that
//! is `None` after `process::init` -- and reaching them would mean injecting
//! a fault into a Tier-2 dispatch path. So the binding is pinned HERE, at
//! source, per site.
//! claim-lint:ok: the 16/6/10 split and the 7 unreachable arms are the
//! per-site table in
//! docs/planning/green-program/gates/CRITICAL-PATH-DEBT-PR1-2026-09-06.md.
//!
//! # Shape-anchored, not line-pinned
//!
//! The assertions below name each function by its signature and each
//! publication by its spelling. 0 line numbers appear, per this repository's
//! standing lesson from the census ratchets of #549 and #551.

use std::fs;
use std::path::PathBuf;

fn read(path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

const CONTEXT_SWITCH: &str = "kernel/src/interrupts/context_switch.rs";
const CENSUS: &str = "kernel/src/task/dispatch_strand_census.rs";
const REGISTRY: &str = "kernel/src/test_framework/registry.rs";
const GATE: &str = "docker/qemu/run-x86-boot-tests.sh";
const STRAND_TOOL: &str = "scripts/x86-strand-census.sh";

/// The body of the first `fn <name>(` in `source`, up to the closing brace in
/// the same column as the line the signature starts on -- the same extractor
/// `tests/dispatch_path_lock_free_structure.rs` uses, and exact for the same
/// reason: each function it is called on below is written at the one
/// indentation level its terminator assumes.
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

/// Lines of `body` that are not whole-line `//` comments. The comments in this
/// tree NAME the spellings being counted, which is what makes the hazard
/// legible, so a counter that counted its own documentation would be wrong.
fn code(body: &str) -> String {
    body.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<&str>>()
        .join("\n")
}

/// The ten new facts, as `(variant, census-line field name)`.
const FACTS: [(&str, &str); 10] = [
    ("SaveNoMainThread", "save_no_thread"),
    ("SaveProcessNotFound", "save_no_proc"),
    ("SaveManagerNone", "save_no_pm"),
    ("SignalPendingBlocked", "sig_pending_blocked"),
    ("SignalContextBlocked", "sig_ctx_blocked"),
    ("SignalDeliveredBlocked", "sig_delivered_blocked"),
    ("IdleStackMissing", "idle_no_stack"),
    ("KernelThreadInfoMissing", "kthread_no_info"),
    ("UserKernelStackMissing", "user_no_kstack"),
    ("SignalDeliverableUser", "sig_deliverable_user"),
];

/// Each of the sixteen deleted sites, as
/// `(enclosing fn, publication spelling, how many that fn carries)`.
///
/// Six rows name a `DispatchAbandonSite` that was ALREADY beside the deleted
/// print and already counted that arm; ten name a new `DispatchLogFact`. The
/// count column is what makes a copy-paste of the wrong variant fail: a
/// function that publishes the same fact twice, or drops one of two, moves it.
const SITE_PUBLICATIONS: [(&str, &str, usize); 16] = [
    // Already counted before this PR (the print was redundant).
    (
        "check_need_resched_and_switch",
        "DispatchAbandonSite::RollbackSaveFailed",
        1,
    ),
    ("switch_to_thread", "DispatchAbandonSite::RollbackTls", 1),
    (
        "switch_to_thread",
        "DispatchAbandonSite::IdleSignalTerminatedBlocked",
        1,
    ),
    (
        "switch_to_thread",
        "DispatchAbandonSite::RollbackKernelContextLock",
        1,
    ),
    (
        "restore_userspace_thread_context",
        "DispatchAbandonSite::IdleRestoreError",
        1,
    ),
    // The KernelFrame arm's own split, which `IdleRestoreError` does not
    // carry: a lock-free raw-serial marker that was already there beside the
    // deleted `log::error!`.
    ("restore_userspace_thread_context", "<KFRAME>", 1),
    // New this PR.
    (
        "save_current_thread_context_with_guard",
        "DispatchLogFact::SaveNoMainThread",
        1,
    ),
    (
        "save_current_thread_context_with_guard",
        "DispatchLogFact::SaveProcessNotFound",
        1,
    ),
    (
        "save_current_thread_context_with_guard",
        "DispatchLogFact::SaveManagerNone",
        1,
    ),
    (
        "switch_to_thread",
        "DispatchLogFact::SignalPendingBlocked",
        1,
    ),
    ("switch_to_thread", "DispatchLogFact::SignalContextBlocked", 1),
    (
        "switch_to_thread",
        "DispatchLogFact::SignalDeliveredBlocked",
        1,
    ),
    ("setup_idle_return", "DispatchLogFact::IdleStackMissing", 1),
    (
        "setup_kernel_thread_return",
        "DispatchLogFact::KernelThreadInfoMissing",
        1,
    ),
    (
        "restore_userspace_thread_context",
        "DispatchLogFact::UserKernelStackMissing",
        1,
    ),
    (
        "restore_userspace_thread_context",
        "DispatchLogFact::SignalDeliverableUser",
        1,
    ),
];

#[test]
fn every_deleted_site_still_publishes_its_fact() {
    let source = read(CONTEXT_SWITCH);
    for (function, publication, expected) in SITE_PUBLICATIONS {
        let body = code(&function_body(&source, function));
        let found = body.matches(publication).count();
        assert_eq!(
            found, expected,
            "{function} carries {found} `{publication}` publications, expected {expected}"
        );
    }
}

/// The three functions PR-1 emptied carry no EMITTING logging macro any more.
///
/// `log::trace!` is deliberately NOT in this denylist, and two of the three
/// functions still carry one. Trace records are dropped by
/// `CombinedLogger::log` before any lock is taken (the `Level::Trace` early
/// return in `kernel/src/logger.rs`), so they emit 0 bytes and acquire 0
/// locks today; the drain plan classifies them H3 and hands them to its
/// PR-11, not to this PR. Asserting their absence here would be this PR
/// claiming a scope it did not do. What their presence still costs -- format
/// arguments evaluated on every dispatch, and one logger change away from
/// being a live acquisition -- is stated in the round doc under what is NOT
/// claimed.
/// claim-lint:ok: the Trace early return is quoted from kernel/src/logger.rs
/// by docs/planning/green-program/gates/CRITICAL-PATH-DEBT-2026-09-06.md §2.
#[test]
fn the_touched_functions_carry_no_emitting_logging_macro() {
    let source = read(CONTEXT_SWITCH);
    for function in [
        "save_current_thread_context_with_guard",
        "setup_idle_return",
        "setup_kernel_thread_return",
    ] {
        let body = code(&function_body(&source, function));
        for spelling in [
            "log::error!",
            "log::warn!",
            "log::info!",
            "log::debug!",
            "serial_println!",
            "log_serial_println!",
            "format!",
        ] {
            assert!(
                !body.contains(spelling),
                "{function} still carries `{spelling}` on the dispatch path"
            );
        }
    }
    // `setup_kernel_thread_return` is the one of the three PR-1 took to 0
    // logging calls of any level, which is why its census anchor row is gone
    // rather than reduced.
    let kthread_return = code(&function_body(&source, "setup_kernel_thread_return"));
    assert!(
        !kthread_return.contains("log::"),
        "setup_kernel_thread_return is the anchor row PR-1 removed; it must carry no log call at all"
    );
}

/// The publication the dispatch path performs is a relaxed atomic add and no
/// other operation: no lock, no allocation, no formatting, no I/O.
#[test]
fn note_fact_is_one_relaxed_atomic_add() {
    let census = read(CENSUS);
    let body = code(&function_body(&census, "note_fact"));
    assert!(
        body.contains("DISPATCH_LOG_FACTS[fact as usize].fetch_add(1, Ordering::Relaxed)"),
        "note_fact is no longer a single relaxed increment:\n{body}"
    );
    for forbidden in [
        ".lock()",
        "log::",
        "serial_print",
        "format!",
        "write!",
        "alloc::",
        "Vec<",
        "Box<",
        "String",
    ] {
        assert!(
            !body.contains(forbidden),
            "note_fact runs with IF=0 on the dispatch path; found `{forbidden}`:\n{body}"
        );
    }
    assert!(
        census.contains("#[inline(always)]\npub(crate) fn note_fact(fact: DispatchLogFact)"),
        "note_fact must stay inlineable into the dispatch path"
    );
}

/// The ten facts, their ten field names and the array they index are one
/// list, in one order, of one length.
#[test]
fn the_fact_family_is_one_list_in_one_order() {
    let census = read(CENSUS);
    assert!(
        census.contains("pub(crate) const DISPATCH_LOG_FACT_COUNT: usize = 10;"),
        "DISPATCH_LOG_FACT_COUNT is no longer 10"
    );
    assert!(
        census.contains(
            "static DISPATCH_LOG_FACTS: [AtomicU64; DISPATCH_LOG_FACT_COUNT] =\n    [const { AtomicU64::new(0) }; DISPATCH_LOG_FACT_COUNT];"
        ),
        "the fact counters are no longer a fixed relaxed array sized by the count"
    );

    let enum_body = census
        .split("pub(crate) enum DispatchLogFact {")
        .nth(1)
        .expect("the DispatchLogFact enum")
        .split("\n}\n")
        .next()
        .expect("a closing brace for DispatchLogFact");
    let names_body = census
        .split("const FACT_FIELD_NAMES: [&str; DISPATCH_LOG_FACT_COUNT] = [")
        .nth(1)
        .expect("the FACT_FIELD_NAMES table")
        .split("];")
        .next()
        .expect("a closing bracket for FACT_FIELD_NAMES");

    let mut previous_variant = 0usize;
    let mut previous_name = 0usize;
    for (index, (variant, field)) in FACTS.iter().enumerate() {
        let at_variant = enum_body
            .find(&format!("{variant} = {index},"))
            .unwrap_or_else(|| panic!("`{variant} = {index},` is not in DispatchLogFact"));
        let at_name = names_body
            .find(&format!("\"{field}\","))
            .unwrap_or_else(|| panic!("`{field}` is not in FACT_FIELD_NAMES"));
        if index > 0 {
            assert!(
                at_variant > previous_variant,
                "DispatchLogFact::{variant} is declared out of discriminant order"
            );
            assert!(
                at_name > previous_name,
                "the `{field}` census field is out of discriminant order"
            );
        }
        previous_variant = at_variant;
        previous_name = at_name;
    }
    assert_eq!(
        names_body.matches('"').count(),
        FACTS.len() * 2,
        "FACT_FIELD_NAMES carries a name that is not one of the ten facts"
    );
}

/// The ten totals reach the census line, appended after the eight fields the
/// strand verdict reads, and the strand tool accepts both shapes.
#[test]
fn the_ten_facts_reach_the_census_line() {
    let census = read(CENSUS);
    assert!(
        census.contains(
            "\"[DISPATCH_STRAND_CENSUS:seq={}:tick={}:ms={}:saved={}:stranded={}:tids={}:tid_overflow={}:ledger_overflow={}{}]\""
        ),
        "the census line no longer appends the fact fields after ledger_overflow"
    );
    assert!(
        census.contains("FactFields(fact_counts()),"),
        "the census line no longer renders the fact totals"
    );

    // The rendered field is `:<name>=<value>`, which is exactly the shape the
    // strand tool's widened tail accepts.
    let display = code(&function_body(&census, "fmt"));
    assert!(
        display.contains(r#"write!(formatter, ":{name}={value}")"#),
        "the fact fields are no longer rendered as `:name=value`:\n{display}"
    );

    let tool = read(STRAND_TOOL);
    assert!(
        tool.contains(":ledger_overflow=[0-9]+(:[a-z_]+=[0-9]+)*"),
        "scripts/x86-strand-census.sh no longer accepts the appended fact fields"
    );
    assert!(
        tool.contains(":ledger_overflow=[0-9]+(:"),
        "the appended fields must be OPTIONAL so the committed #775 captures still replay"
    );
}

/// The boot oracle exists, is x86-and-boot_tests only, drives exactly one leg
/// per fact, and brackets them with two forced snapshots.
#[test]
fn the_boot_oracle_drives_one_leg_per_fact() {
    let registry = read(REGISTRY);
    assert!(
        registry.contains(
            "#[cfg(all(target_arch = \"x86_64\", feature = \"boot_tests\"))]\npub fn run_x86_dispatch_fact_oracle()"
        ),
        "the dispatch-fact oracle is no longer gated to x86 boot_tests"
    );
    let body = code(&function_body(&registry, "run_x86_dispatch_fact_oracle"));
    for (variant, _) in FACTS {
        assert_eq!(
            body.matches(&format!("DispatchLogFact::{variant}")).count(),
            1,
            "the oracle drives DispatchLogFact::{variant} other than exactly once"
        );
    }
    assert_eq!(
        body.matches("force_snapshot();").count(),
        2,
        "the oracle must bracket its legs with a before and an after census line"
    );
    assert!(
        body.contains("after[index] == before[index] + 1"),
        "the oracle no longer requires each counter to move by exactly one leg"
    );

    let main = read("kernel/src/main.rs");
    assert_eq!(
        main.matches("run_x86_dispatch_fact_oracle()").count(),
        1,
        "the oracle must be dispatched exactly once from the boot_tests gate block"
    );
}

/// The gate pins the oracle's PASS line, and pins it as an exact literal so a
/// FAIL cannot pass by being present.
#[test]
fn the_gate_pins_the_oracle_verdict() {
    let gate = read(GATE);
    assert!(
        gate.contains(
            "DISPATCH_FACT_ORACLE_LITERAL='[DISPATCH_FACT_ORACLE:x86:facts=10:legs=10:moved_by_one=10:moved_wrong=0:irqs_enabled_before=1:PASS]'"
        ),
        "docker/qemu/run-x86-boot-tests.sh no longer pins the dispatch-fact oracle verdict"
    );
    assert!(
        gate.contains("grep -qF \"$DISPATCH_FACT_ORACLE_LITERAL\""),
        "the oracle literal is defined but never checked in the pass condition"
    );
    assert!(
        gate.contains("grep -h -F -c \"$DISPATCH_FACT_ORACLE_LITERAL\""),
        "the oracle literal is not pinned to an exact emission count"
    );
}
