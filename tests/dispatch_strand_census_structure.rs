use std::fs;
use std::path::PathBuf;

fn read(path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

/// Each `log::<level>!` invocation in a file, as (line number, level). Matches
/// one per physical line, which is the shape context_switch.rs uses.
fn log_records(source: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let Some(start) = line.find("log::") else {
            continue;
        };
        let rest = &line[start + "log::".len()..];
        let Some(bang) = rest.find('!') else {
            continue;
        };
        let level = &rest[..bang];
        if matches!(level, "trace" | "debug" | "info" | "warn" | "error") {
            found.push((index + 1, level.to_string()));
        }
    }
    found
}

#[test]
fn dispatch_records_and_diagnostic_feature_are_gone() {
    let workspace_manifest = read("Cargo.toml");
    let kernel_manifest = read("kernel/Cargo.toml");
    let context_switch = read("kernel/src/interrupts/context_switch.rs");

    assert!(!workspace_manifest.contains("quiet_dispatch_log"));
    assert!(!kernel_manifest.contains("quiet_dispatch_log"));
    for removed in [
        // The three records #775 round 1 removed.
        "Saved kernel context for blocked thread",
        "Restored kernel context for thread",
        "Switched to process CR3 {:#x} for blocked-in-syscall kernel return",
        // The three finding F15 named in round 2, removed in round 3.
        "Set CR3 to {:#x} for thread {} (pid {})",
        "Switched to process CR3 {:#x} for signal delivery (blocked-in-syscall path)",
        "Switched to process CR3 {:#x} for signal delivery",
    ] {
        assert!(
            !context_switch.contains(removed),
            "interrupt-path dispatch record returned: {removed}"
        );
    }
}

#[test]
fn the_surviving_record_census_in_the_dispatch_path_is_pinned() {
    // #775 round 3, F15. Finding F15 named three records and this round removed
    // exactly those three. Removing the WHOLE non-error class from this file was
    // built and measured first and is NOT shipped: it reddened the x86
    // production-profile gate on 4 of 9 boots with prompt-count signatures that
    // 8 baseline boots and 5 boots of the narrow removal never produced. The
    // measurement is the table in 775-CENSUS-EQUIVALENCE-2026-09-04.md.
    //
    // What is pinned here is therefore a CENSUS, not a name list and not a rule
    // this branch cannot honour: the number of records in the file and their
    // level histogram. Adding a record, or removing one, reddens this. Round 4,
    // finding R3-9: this comment used to promise that a redness here "forces the
    // equivalence document's surviving-record table to be updated", and no such
    // table existed. What the document carries is the same two numbers plus a
    // per-function breakdown of where the survivors sit; nothing mechanically
    // enforces that breakdown, and no per-record listing is claimed by this test.
    // claim-lint:ok: the per-function breakdown is the table in
    // docs/planning/green-program/sockets/775-CENSUS-EQUIVALENCE-2026-09-04.md.
    let context_switch = read("kernel/src/interrupts/context_switch.rs");
    let records = log_records(&context_switch);

    let mut histogram: Vec<(String, usize)> = Vec::new();
    for (_, level) in &records {
        match histogram.iter_mut().find(|(name, _)| name == level) {
            Some((_, count)) => *count += 1,
            None => histogram.push((level.clone(), 1)),
        }
    }
    histogram.sort();

    assert_eq!(
        records.len(),
        30,
        "context_switch.rs record census moved: {histogram:?}"
    );
    assert_eq!(
        histogram,
        vec![
            ("debug".to_string(), 2),
            ("error".to_string(), 11),
            ("info".to_string(), 8),
            ("trace".to_string(), 9),
        ],
        "context_switch.rs record level histogram moved"
    );
}

#[test]
fn replacement_census_is_wired_to_save_restore_exit_heartbeat_and_completion() {
    let context_switch = read("kernel/src/interrupts/context_switch.rs");
    let process_task = read("kernel/src/task/process_task.rs");
    let handlers = read("kernel/src/syscall/handlers.rs");
    let main = read("kernel/src/main.rs");
    let loopback_pump = read("kernel/src/net/loopback_pump.rs");
    let task_mod = read("kernel/src/task/mod.rs");
    let census = read("kernel/src/task/dispatch_strand_census.rs");

    assert_eq!(
        context_switch
            .matches("dispatch_strand_census::note_save(thread_id)")
            .count(),
        1
    );
    assert_eq!(
        context_switch
            .matches("dispatch_strand_census::note_restore(thread_id)")
            .count(),
        1
    );
    assert_eq!(
        process_task
            .matches("dispatch_strand_census::note_exit(thread_id)")
            .count(),
        1
    );
    assert_eq!(
        handlers
            .matches("dispatch_strand_census::report_snapshot()")
            .count(),
        1
    );

    // #775 round 3, N1: the heartbeat lives in the idle loop x86 actually runs.
    // `main.rs`'s idle_thread_fn is the idle task's stored entry point and is
    // never dispatched, so a call there would be certified-but-dead wiring.
    // claim-lint:ok: #775 round 3 finding N1, measured in
    // docs/planning/green-program/sockets/775-CENSUS-EQUIVALENCE-2026-09-04.md.
    let idle_loop_start = context_switch
        .find("pub fn idle_loop()")
        .expect("context_switch.rs must still define idle_loop()");
    assert_eq!(
        context_switch[idle_loop_start..]
            .matches("crate::task::report_dispatch_strand_census_heartbeat()")
            .count(),
        1,
        "the census heartbeat is not called from idle_loop()"
    );
    assert_eq!(
        context_switch
            .matches("crate::task::report_dispatch_strand_census_heartbeat()")
            .count(),
        1
    );
    assert_eq!(
        main.matches("task::report_dispatch_strand_census_heartbeat()")
            .count(),
        0,
        "the census heartbeat is wired to main.rs's undispatched idle entry point again"
    );
    assert!(main.contains("THIS BODY IS NEVER DISPATCHED"));
    assert_eq!(
        loopback_pump
            .matches("crate::task::report_dispatch_strand_census_heartbeat()")
            .count(),
        1
    );

    assert!(task_mod
        .contains("#[cfg(target_arch = \"x86_64\")]\npub(crate) mod dispatch_strand_census;"));
    assert!(task_mod.contains("pub fn report_dispatch_strand_census_heartbeat()"));
    assert!(census.contains("[DISPATCH_STRAND_CENSUS:seq={}:tick={}:ms={}:saved="));
    assert!(census.contains("const STRANDED_TID_CAPACITY: usize = 16;"));
    assert!(census.contains("if !crate::arch_interrupts_enabled()"));
    assert!(census.contains("pub(crate) fn report_heartbeat_if_due()"));

    // #775 round 4, R3-5/N14: kstrandd, the third cadence context. It is
    // spawned beside kloopbackd on main.rs's unconditional init path, so it
    // exists in the zero-feature production profile, and it sleeps on the
    // scheduler's timer-block primitive rather than on anything the rest of the
    // kernel has to do for it.
    // claim-lint:ok: 6 zero-feature production boots are recorded in
    // docs/planning/green-program/sockets/serials/775/round4/production/.
    assert!(census.contains("kthread_run(census_thread_fn, \"kstrandd\")"));
    assert!(census.contains("scheduler.block_current_for_timer(wake_at)"));
    assert_eq!(
        census.matches("report_heartbeat_if_due();").count(),
        1,
        "kstrandd must emit through the shared rate limiter, not report_snapshot"
    );
    assert!(
        !census.contains("wake_expired_timers"),
        "the round-2 kthread called wake_expired_timers from thread context and \
         page-faulted on every boot"
    );
    assert!(task_mod.contains("pub fn start_dispatch_strand_census_kthread()"));
    assert_eq!(
        main.matches("task::start_dispatch_strand_census_kthread()").count(),
        1
    );
    let pump_start = main
        .find("crate::net::init_loopback_pump();")
        .expect("main.rs must still spawn kloopbackd");
    let census_start = main
        .find("task::start_dispatch_strand_census_kthread()")
        .expect("main.rs must spawn kstrandd");
    assert!(
        census_start > pump_start && census_start - pump_start < 400,
        "kstrandd is no longer spawned beside kloopbackd"
    );
    assert!(census.contains("static LEDGER: [AtomicU8; LEDGER_CAPACITY]"));
}

#[test]
fn the_snapshot_is_emitted_on_the_kernel_log_channel() {
    // #775 round 3, N8. The three removed records were log::info!/log::debug!,
    // i.e. COM2. COM1 is the interactive user console (kernel/src/serial.rs), so
    // the replacement must not move the diagnostic onto it.
    let census = read("kernel/src/task/dispatch_strand_census.rs");
    assert!(
        census.contains("crate::log_serial_println!"),
        "the census snapshot is not emitted on the kernel-log channel"
    );
    assert!(
        !census.contains("crate::serial_println!"),
        "the census snapshot writes to the user console (COM1)"
    );
}

#[test]
fn host_consumers_have_no_removed_record_dependency() {
    let strand_gate = read("scripts/x86-strand-census.sh");
    let verdict_gate = read("scripts/x86-gate-verdict.sh");
    let dispatch_census = read("scripts/772-dispatch-census.py");

    assert!(strand_gate.contains("DISPATCH_STRAND_CENSUS"));
    assert!(strand_gate.contains("if (seq > best_seq) { best_seq = seq; best = marker }"));
    assert!(strand_gate.contains("names[tid] = rest"));
    assert!(strand_gate.contains("if (stranded > 0) {"));
    assert!(strand_gate.contains("if (age_measured && age_ms > stale_limit_ms) {"));
    // The bound EXISTS, is printed with the age, and carries its derivation.
    // Its VALUE is deliberately not pinned here. Round 5 finding R4-2 made it a
    // derivation from #766's measured wake-to-dispatch distribution that
    // TIGHTENS when #766 lands, so a literal here would be a second place to
    // update and a false ratchet the day it is missed -- this repository's
    // binding lesson from the census ratchets of #549 and #551. What holds the
    // value is tests/x86_gate_verdict_test.rs, which asserts the printed bound
    // on a committed capture and on a synthesised stale one.
    assert!(strand_gate.contains("stale_limit_ms = "));
    assert!(strand_gate.contains("bound %d ms)"));
    assert!(strand_gate.contains("693-RCA-2026-09-02.md"));
    assert!(!strand_gate.contains("/Saved kernel context for blocked thread"));
    assert!(!strand_gate.contains("/Restored kernel context for thread"));

    // #775 round 3, N4/F21: the printed sentence must not overclaim, and an
    // overflowed ledger must leave the census with no verdict to report.
    assert!(strand_gate.contains("saved blocked and not restored as of the latest snapshot"));
    assert!(!strand_gate.contains("saved blocked and never restored"));
    assert!(strand_gate.contains("carries no verdict\\n\", ledger_overflow"));
    assert!(strand_gate.contains("exit 3"));
    assert!(verdict_gate.contains("2) echo \"x86 userspace gate: census unavailable;"));
    assert!(verdict_gate.contains("3) echo \"x86 userspace gate: STRAND CENSUS INCOMPLETE"));
    assert!(verdict_gate.contains("4) fail \"the strand census read stranded=0"));

    assert!(dispatch_census.contains("#775 retired"));
    assert!(!dispatch_census.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("RESTORE_RE") || line.starts_with("SAVE_RE")
    }));
    assert!(!dispatch_census.contains("\"turns\":"));
    // #775 round 3, F12: the retired equality has a stated replacement.
    assert!(dispatch_census.contains("census_saved_tids"));
    assert!(dispatch_census.contains("kernel_blocked_saves_ge_census_saved_tids"));
}

#[test]
fn every_in_repo_caller_hands_the_census_the_kernel_serial() {
    // #775 round 3, N8: the snapshots are on COM2, so a caller that passed only
    // the COM1 capture would silently get "census unavailable" forever.
    for (path, needle) in [
        ("docker/qemu/run-x86-gate.sh", "serial_kernel.log"),
        ("docker/qemu/run-boot-parallel.sh", "serial_kernel.txt"),
        ("docker/qemu/run-x86-boot-tests.sh", "serial_*.txt"),
    ] {
        let script = read(path);
        let lines: Vec<&str> = script.lines().collect();
        // A call site, not a mention: the line invokes the script and is not a
        // comment. The 3 in-repo call sites must each be handed the kernel
        // capture, because the snapshots are on COM2.
        let call_sites: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                line.contains("x86-gate-verdict.sh") && !line.trim_start().starts_with('#')
            })
            .map(|(index, _)| index)
            .collect();
        assert!(
            !call_sites.is_empty(),
            "{path} no longer calls the verdict script"
        );
        for index in call_sites {
            let end = (index + 4).min(lines.len());
            let window = lines[index..end].join("\n");
            assert!(
                window.contains(needle),
                "{path} line {} does not hand the kernel serial to the verdict script",
                index + 1
            );
        }
    }
}
