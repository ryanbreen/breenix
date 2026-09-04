use std::fs;
use std::path::PathBuf;

fn read(path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

#[test]
fn dispatch_records_and_diagnostic_feature_are_gone() {
    let workspace_manifest = read("Cargo.toml");
    let kernel_manifest = read("kernel/Cargo.toml");
    let context_switch = read("kernel/src/interrupts/context_switch.rs");

    assert!(!workspace_manifest.contains("quiet_dispatch_log"));
    assert!(!kernel_manifest.contains("quiet_dispatch_log"));
    for removed in [
        "Saved kernel context for blocked thread",
        "Restored kernel context for thread",
        "Switched to process CR3 {:#x} for blocked-in-syscall kernel return",
    ] {
        assert!(
            !context_switch.contains(removed),
            "interrupt-path dispatch record returned: {removed}"
        );
    }
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
    assert_eq!(
        main.matches("task::report_dispatch_strand_census_heartbeat()")
            .count(),
        1
    );
    assert_eq!(
        loopback_pump
            .matches("crate::task::report_dispatch_strand_census_heartbeat()")
            .count(),
        1
    );
    assert!(task_mod
        .contains("#[cfg(target_arch = \"x86_64\")]\npub(crate) mod dispatch_strand_census;"));
    assert!(task_mod.contains("pub fn report_dispatch_strand_census_heartbeat()"));
    assert!(census.contains("[DISPATCH_STRAND_CENSUS:saved="));
    assert!(census.contains("const STRANDED_TID_CAPACITY: usize = 16;"));
    assert!(census.contains("if !crate::arch_interrupts_enabled()"));
    assert!(census.contains("pub(crate) fn report_heartbeat_if_due()"));
    assert!(!census.contains("kthread_run"));
    assert!(census.contains("static LEDGER: [AtomicU8; LEDGER_CAPACITY]"));
}

#[test]
fn host_consumers_have_no_removed_record_dependency() {
    let strand_gate = read("scripts/x86-strand-census.sh");
    let verdict_gate = read("scripts/x86-gate-verdict.sh");
    let dispatch_census = read("scripts/772-dispatch-census.py");

    assert!(strand_gate.contains("DISPATCH_STRAND_CENSUS"));
    assert!(strand_gate.contains("latest = substr(rest, RSTART, RLENGTH)"));
    assert!(strand_gate.contains("names[tid] = rest"));
    assert!(strand_gate.contains("exit (value[\"stranded\"] > 0) ? 1 : 0"));
    assert!(!strand_gate.contains("/Saved kernel context for blocked thread"));
    assert!(!strand_gate.contains("/Restored kernel context for thread"));
    assert!(verdict_gate.contains("2) echo \"x86 userspace gate: census unavailable;"));

    assert!(dispatch_census.contains("#775 retired"));
    assert!(!dispatch_census.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("RESTORE_RE") || line.starts_with("SAVE_RE")
    }));
    assert!(!dispatch_census.contains("\"turns\":"));
}
