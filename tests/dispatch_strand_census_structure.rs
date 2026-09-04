use std::fs;

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
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
fn replacement_census_is_wired_to_save_restore_exit_and_completion() {
    let context_switch = read("kernel/src/interrupts/context_switch.rs");
    let process_task = read("kernel/src/task/process_task.rs");
    let handlers = read("kernel/src/syscall/handlers.rs");
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
            .matches("dispatch_strand_census::report_once()")
            .count(),
        1
    );
    assert!(census.contains("[DISPATCH_STRAND_CENSUS:threads_saved_blocked="));
    assert!(census.contains("static LEDGER: [AtomicU8; LEDGER_CAPACITY]"));
}

#[test]
fn host_consumers_have_no_removed_record_dependency() {
    let strand_gate = read("scripts/x86-strand-census.sh");
    let dispatch_census = read("scripts/772-dispatch-census.py");

    assert!(strand_gate.contains("DISPATCH_STRAND_CENSUS"));
    assert!(strand_gate.contains("[[ \"$stranded\" -eq 0 ]]"));
    assert!(strand_gate.contains("[[ \"$overflow\" -ne 0 ]]"));
    assert!(!strand_gate.contains("/Saved kernel context for blocked thread"));
    assert!(!strand_gate.contains("/Restored kernel context for thread"));

    assert!(dispatch_census.contains("#775 retired"));
    assert!(!dispatch_census.contains("RESTORE_RE = re.compile"));
    assert!(!dispatch_census.contains("SAVE_RE = re.compile"));
    assert!(!dispatch_census.contains("\"turns\":"));
}
