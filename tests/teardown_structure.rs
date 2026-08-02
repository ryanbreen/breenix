use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn source(path: &str) -> String {
    fs::read_to_string(root().join(path)).expect("source file must be readable")
}

fn git_diff(path: &str) -> String {
    let output = Command::new("git")
        .current_dir(root())
        .args(["diff", "main", "--", path])
        .output()
        .expect("git diff must run");
    assert!(output.status.success());
    String::from_utf8(output.stdout).expect("git diff must be UTF-8")
}

#[test]
fn exception_return_tails_do_not_drain_or_reclaim() {
    let diff = git_diff("kernel/src/arch_impl/aarch64/context_switch.rs");
    assert!(diff.contains("-    crate::task::process_task::drain_deferred_fault_sigsegv_exits();"));
    assert_eq!(
        source("kernel/src/arch_impl/aarch64/context_switch.rs")
            .matches("drain_deferred_fault_sigsegv_exits")
            .count(),
        0
    );
}

#[test]
fn assembly_publishes_installed_root_before_clearing_pending_lease() {
    let assembly = source("kernel/src/arch_impl/aarch64/syscall_entry.S");
    for sequence in [
        "msr ttbr0_el1, x1\n    isb\n    str x1, [x0, #80]           /* saved_process_cr3 = installed root */\n    str xzr, [x0, #64]          /* clear next_cr3 last */",
        "msr ttbr0_el1, x10\n    isb\n    str x10, [x9, #80]          /* saved_process_cr3 = installed root */\n    str xzr, [x9, #64]          /* clear next_cr3 last */",
    ] {
        assert!(assembly.contains(sequence));
    }
}

#[test]
fn no_new_rust_ttbr0_writer_escapes_the_reviewed_set() {
    let output = Command::new("rg")
        .current_dir(root())
        .args(["-l", "msr ttbr0_el1", "kernel/src", "--glob", "*.rs"])
        .output()
        .expect("ripgrep must run");
    assert!(output.status.success());
    let mut writers: Vec<_> = String::from_utf8(output.stdout)
        .expect("ripgrep output must be UTF-8")
        .lines()
        .map(str::to_owned)
        .collect();
    writers.sort();
    assert_eq!(
        writers,
        [
            "kernel/src/arch_impl/aarch64/paging.rs",
            "kernel/src/arch_impl/aarch64/ttbr0.rs",
            "kernel/src/main_aarch64.rs",
            "kernel/src/memory/arch_stub.rs",
            "kernel/src/syscall/graphics.rs",
            "kernel/src/syscall/handlers.rs",
            "kernel/src/syscall/time.rs",
            "kernel/src/syscall/wait.rs",
        ]
    );
}

#[test]
fn reclaim_capability_has_only_the_reviewed_mint_sites() {
    let output = Command::new("rg")
        .current_dir(root())
        .args([
            "-n",
            "ReclaimContext::assert_preemptible\\(\\)",
            "kernel/src",
        ])
        .output()
        .expect("ripgrep must run");
    assert!(output.status.success());
    let matches = String::from_utf8(output.stdout).expect("ripgrep output must be UTF-8");
    assert_eq!(
        matches.lines().count(),
        3,
        "unexpected capability mint:\n{matches}"
    );
    assert_eq!(
        source("kernel/src/arch_impl/aarch64/syscall_entry.rs")
            .matches("ReclaimContext::assert_preemptible()")
            .count(),
        1
    );
    assert_eq!(
        source("kernel/src/task/reclaim.rs")
            .matches("ReclaimContext::assert_preemptible()")
            .count(),
        2
    );

    let process = source("kernel/src/process/process.rs");
    let memory = source("kernel/src/memory/process_memory.rs");
    let reclaim = source("kernel/src/task/reclaim.rs");
    assert!(process
        .contains("cleanup_cow_frames(&mut self, context: &crate::task::reclaim::ReclaimContext)"));
    assert!(process.contains("cleanup_cow_page_table(\n    page_table: &ProcessPageTable,\n    _context: &crate::task::reclaim::ReclaimContext,"));
    assert!(
        memory.contains("cleanup_for_exec(self, _context: &crate::task::reclaim::ReclaimContext)")
    );
    assert!(reclaim.contains("fn release_stack(&mut self, _context: &ReclaimContext)"));
}

#[test]
fn unconstrained_ttbr0_teardown_helpers_stay_deleted() {
    let ttbr0 = source("kernel/src/arch_impl/aarch64/ttbr0.rs");
    for removed in [
        "switch_ttbr0_to_kernel",
        "quiesce_ttbr0_for_exit",
        "current_cpu_retains_ttbr0_root",
    ] {
        assert!(
            !ttbr0.contains(removed),
            "unconstrained helper returned: {removed}"
        );
    }
}

#[test]
fn every_new_teardown_counter_has_an_in_tree_reader() {
    let reclaim = source("kernel/src/task/reclaim.rs");
    for counter in [
        "GRAVES_QUEUED",
        "GRAVES_RECLAIMED",
        "GRAVES_BLOCKED",
        "FAULT_EXIT_INTENT_DROPPED",
        "FRAME_DECREF_UNDERFLOW",
        "FRAME_DECREF_UNTRACKED",
    ] {
        assert!(
            reclaim.contains(counter),
            "counter {counter} must be read by dump_reclaim_state"
        );
    }
}

#[test]
fn frozen_regions_remain_outside_the_branch_diff() {
    let context_diff = git_diff("kernel/src/arch_impl/aarch64/context_switch.rs");
    for frozen in [
        "idle_loop_arm64",
        "dispatch_thread_locked",
        "aarch64_enter_exception_frame",
    ] {
        assert!(
            !context_diff.contains(frozen),
            "frozen context-switch region changed: {frozen}"
        );
    }
    assert!(git_diff("kernel/src/arch_impl/aarch64/gic.rs").is_empty());
    assert!(git_diff("kernel/src/arch_impl/aarch64/timer_interrupt.rs").is_empty());
}

#[test]
fn only_the_reviewed_assembly_file_changed() {
    let output = Command::new("git")
        .current_dir(root())
        .args(["diff", "--name-only", "main", "--", "*.S"])
        .output()
        .expect("git diff must run");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("git diff must be UTF-8"),
        "kernel/src/arch_impl/aarch64/syscall_entry.S\n"
    );
}
