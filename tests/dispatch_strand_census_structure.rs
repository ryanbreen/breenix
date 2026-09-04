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
    // level histogram. Adding a record, or removing one, reddens this and forces
    // the equivalence document's surviving-record table to be updated with it.
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
    assert!(!census.contains("kthread_run"));
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
    assert!(strand_gate.contains("exit (stranded > 0) ? 1 : 0"));
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
