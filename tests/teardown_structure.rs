use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

type Site = (String, usize);

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn rust_sources_below(relative: &str) -> Vec<(String, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).expect("read source directory") {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(root)
                    .expect("source below repository root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((
                    relative,
                    fs::read_to_string(path).expect("read Rust source"),
                ));
            }
        }
    }

    let root = repo_root();
    let mut sources = Vec::new();
    visit(&root, &root.join(relative), &mut sources);
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

fn is_code(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.starts_with("//") && !trimmed.starts_with("/*") && !trimmed.starts_with('*')
}

fn sites_matching<F>(sources: &[(String, String)], mut predicate: F) -> BTreeSet<Site>
where
    F: FnMut(&str) -> bool,
{
    let mut sites = BTreeSet::new();
    for (path, source) in sources {
        for (index, line) in source.lines().enumerate() {
            if is_code(line) && predicate(line) {
                sites.insert((path.clone(), index + 1));
            }
        }
    }
    sites
}

fn expected(sites: &[(&str, usize)]) -> BTreeSet<Site> {
    sites
        .iter()
        .map(|(path, line)| ((*path).to_owned(), *line))
        .collect()
}

fn assert_exact(actual: BTreeSet<Site>, expected_sites: &[(&str, usize)], label: &str) {
    assert_eq!(actual, expected(expected_sites), "{label} changed");
}

fn validate_exact(actual: &BTreeSet<Site>, expected_sites: &[(&str, usize)]) -> Result<(), ()> {
    (*actual == expected(expected_sites))
        .then_some(())
        .ok_or(())
}

fn with_synthetic_source(
    sources: &[(String, String)],
    path: &str,
    synthetic_source: &str,
) -> Vec<(String, String)> {
    let mut perturbed = sources.to_vec();
    perturbed.push((path.to_owned(), synthetic_source.to_owned()));
    perturbed.sort_by(|left, right| left.0.cmp(&right.0));
    perturbed
}

fn with_replaced_source(
    sources: &[(String, String)],
    path: &str,
    replacement: String,
) -> Vec<(String, String)> {
    sources
        .iter()
        .map(|(candidate, contents)| {
            if candidate == path {
                (candidate.clone(), replacement.clone())
            } else {
                (candidate.clone(), contents.clone())
            }
        })
        .collect()
}

fn source<'a>(sources: &'a [(String, String)], path: &str) -> &'a str {
    &sources
        .iter()
        .find(|(candidate, _)| candidate == path)
        .unwrap_or_else(|| panic!("missing source {path}"))
        .1
}

fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let marker = format!("fn {name}");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("missing function {name}"));
    let open = start + source[start..].find('{').expect("function open brace");
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..open + offset + 1];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated function {name}")
}

const TERMINATE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/interrupts/context_switch.rs", 1017),
    ("kernel/src/process/manager.rs", 1170),
    ("kernel/src/signal/delivery.rs", 225),
    ("kernel/src/signal/delivery.rs", 260),
];
const TERMINATE_MINIMAL_CALLS: &[(&str, usize)] = &[("kernel/src/task/process_task.rs", 496)];
const PRODUCTION_INIT_PID_SITES: &[(&str, usize)] = &[
    ("kernel/src/process/manager.rs", 1187),
    ("kernel/src/task/process_task.rs", 458),
    ("kernel/src/task/process_task.rs", 528),
];
const TEST_INIT_PID_SITES: &[(&str, usize)] = &[
    ("kernel/src/test_userspace.rs", 84),
    ("kernel/src/test_userspace.rs", 203),
    ("kernel/src/test_userspace.rs", 292),
];
const QUARANTINE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/exception.rs", 815),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1177),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1271),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1369),
    ("kernel/src/syscall/signal.rs", 163),
];
const KERNEL_STACK_MUTATIONS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/syscall_entry.rs", 961),
    ("kernel/src/process/manager.rs", 1856),
    ("kernel/src/syscall/clone.rs", 252),
];
const RECLAIM_ENQUEUE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/process/mod.rs", 53),
    ("kernel/src/process/mod.rs", 280),
    ("kernel/src/task/process_task.rs", 570),
];
const EXIT_PROCESS_AND_RETIRE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/exception.rs", 824),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1187),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1273),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1371),
    ("kernel/src/interrupts.rs", 1440),
    ("kernel/src/interrupts.rs", 1751),
    ("kernel/src/process/mod.rs", 413),
    ("kernel/src/syscall/signal.rs", 172),
];
const EXIT_PROCESS_LOCKED_CALLS: &[(&str, usize)] = &[("kernel/src/process/mod.rs", 265)];
const EXIT_PROCESS_BY_PID_CALLS: &[(&str, usize)] = &[
    ("kernel/src/process/mod.rs", 406),
    ("kernel/src/process/mod.rs", 418),
];
const EXIT_PROCESS_FOR_TEARDOWN_TEST_CALLS: &[(&str, usize)] =
    &[("kernel/src/tracing/providers/teardown.rs", 1047)];
const BLOCKING_PRIMITIVES: &[(&str, usize)] = &[
    ("kernel/src/task/scheduler.rs", 1975),
    ("kernel/src/task/scheduler.rs", 2189),
    ("kernel/src/task/scheduler.rs", 2208),
    ("kernel/src/task/scheduler.rs", 2357),
    ("kernel/src/task/scheduler.rs", 2445),
    ("kernel/src/task/scheduler.rs", 2510),
    ("kernel/src/task/scheduler.rs", 2519),
    ("kernel/src/task/scheduler.rs", 2678),
    ("kernel/src/task/waitqueue.rs", 52),
];
const RAW_SCHEDULER_LOCK_SITES: &[(&str, usize)] = &[
    ("kernel/src/task/scheduler.rs", 281),
    ("kernel/src/task/scheduler.rs", 288),
];

const BLOCKING_NAMES: &[&str] = &[
    "block_current(",
    "block_current_for_signal(",
    "block_current_for_signal_with_context(",
    "block_current_for_child_exit(",
    "block_current_for_timer(",
    "block_current_for_io(",
    "block_current_for_io_with_timeout(",
    "block_current_for_compositor(",
    "prepare_to_wait(",
];

fn validate_reclaim_enqueue_callers(sources: &[(String, String)]) -> Result<(), ()> {
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("enqueue_process_reclaim(")
                && !line.contains("fn enqueue_process_reclaim")
        }),
        RECLAIM_ENQUEUE_CALLS,
    )
}

fn validate_exit_process_entry_points(sources: &[(String, String)]) -> Result<(), ()> {
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("exit_process_and_retire(")
                && !line.contains("fn exit_process_and_retire")
        }),
        EXIT_PROCESS_AND_RETIRE_CALLS,
    )?;
    validate_exact(
        &sites_matching(sources, |line| line.contains(".exit_process_locked(")),
        EXIT_PROCESS_LOCKED_CALLS,
    )?;
    if !sites_matching(sources, |line| line.contains(".exit_process(")).is_empty() {
        return Err(());
    }
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("exit_process_by_pid(") && !line.contains("fn exit_process_by_pid")
        }),
        EXIT_PROCESS_BY_PID_CALLS,
    )?;
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("exit_process_for_teardown_test(")
                && !line.contains("fn exit_process_for_teardown_test")
        }),
        EXIT_PROCESS_FOR_TEARDOWN_TEST_CALLS,
    )
}

fn validate_blocking_primitives(sources: &[(String, String)]) -> Result<(), ()> {
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("pub fn ") && BLOCKING_NAMES.iter().any(|name| line.contains(name))
        }),
        BLOCKING_PRIMITIVES,
    )
}

fn validate_group_writes(sources: &[(String, String)]) -> Result<(), ()> {
    validate_exact(
        &sites_matching(sources, |line| line.contains("thread_group_id = Some(")),
        &[("kernel/src/syscall/clone.rs", 210)],
    )
}

fn validate_exit_sgi_is_teardown_only(sources: &[(String, String)]) -> Result<(), ()> {
    let scheduler = source(sources, "kernel/src/task/scheduler.rs");
    (function_body(scheduler, "send_exit_expedite_sgi").contains("EXIT_SGI_SENT")
        && !function_body(scheduler, "send_resched_ipi").contains("EXIT_SGI_SENT")
        && !function_body(scheduler, "send_resched_ipi_to_cpu").contains("EXIT_SGI_SENT"))
    .then_some(())
    .ok_or(())
}

#[test]
fn current_teardown_bypass_surface_is_exact() {
    let sources = rust_sources_below("kernel/src");

    assert_exact(
        sites_matching(&sources, |line| line.contains(".terminate(")),
        TERMINATE_CALLS,
        "Process::terminate callers",
    );
    assert_exact(
        sites_matching(&sources, |line| line.contains(".terminate_minimal(")),
        TERMINATE_MINIMAL_CALLS,
        "Process::terminate_minimal callers",
    );

    let init_sites = sites_matching(&sources, |line| line.contains("ProcessId::new(1)"));
    let test_sites: BTreeSet<_> = init_sites
        .iter()
        .filter(|(path, _)| path == "kernel/src/test_userspace.rs")
        .cloned()
        .collect();
    let production_sites: BTreeSet<_> = init_sites.difference(&test_sites).cloned().collect();
    assert_exact(
        production_sites,
        PRODUCTION_INIT_PID_SITES,
        "production PID-1 literals",
    );
    assert_exact(
        test_sites,
        TEST_INIT_PID_SITES,
        "test_minimal_userspace PID-1 allowlist",
    );
    let test_userspace = source(&sources, "kernel/src/test_userspace.rs");
    assert_eq!(
        test_userspace
            .matches("pub fn test_minimal_userspace()")
            .count(),
        1,
        "test_minimal_userspace must remain uniquely nameable"
    );
    assert_eq!(
        function_body(test_userspace, "test_minimal_userspace")
            .matches("ProcessId::new(1)")
            .count(),
        3,
        "the three test PID-1 sites must remain in test_minimal_userspace"
    );

    assert_exact(
        sites_matching(&sources, |line| {
            line.contains(".terminate_process_threads(")
        }),
        QUARANTINE_CALLS,
        "terminate_process_threads callers",
    );
    assert_exact(
        sites_matching(&sources, |line| line.contains(".kernel_stack_allocation =")),
        KERNEL_STACK_MUTATIONS,
        "kernel_stack_allocation ownership mutations",
    );
    validate_reclaim_enqueue_callers(&sources)
        .expect("enqueue_process_reclaim caller ratchet changed");
}

#[test]
fn v3_structural_closures_are_exact() {
    let sources = rust_sources_below("kernel/src");
    validate_exit_process_entry_points(&sources).expect("process-exit entry-point ratchet changed");
    validate_blocking_primitives(&sources).expect("the nine P0 blocking primitives changed");
    validate_group_writes(&sources).expect("thread_group_id production writers changed");
    assert_exact(
        sites_matching(&sources, |line| {
            line.contains("SCHEDULER.lock()") || line.contains("SCHEDULER.try_lock()")
        }),
        RAW_SCHEDULER_LOCK_SITES,
        "raw scheduler-lock acquisitions outside the instrumented wrappers",
    );
    assert_exact(
        sites_matching(&sources, |line| line.contains("btrt::on_process_exit(")),
        &[("kernel/src/task/process_task.rs", 597)],
        "btrt::on_process_exit callers",
    );

    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    assert_eq!(provider.matches("counter!(EXIT_SGI_SENT,").count(), 1);
    assert_eq!(provider.matches("counter!(EXIT_KICK_PUBLISHED,").count(), 1);
    validate_exit_sgi_is_teardown_only(&sources)
        .expect("EXIT_SGI_SENT escaped the teardown-only producer");
    let expedite = function_body(scheduler, "send_exit_expedite_sgi");
    assert_eq!(expedite.matches("EXIT_SGI_SENT").count(), 1);
    assert_eq!(
        expedite
            .matches("trace_count!(EXIT_KICK_PUBLISHED)")
            .count(),
        1
    );
    assert!(expedite.find("slot.publish(").unwrap() < expedite.find("gic::send_sgi(").unwrap());
    assert!(!expedite.contains("current_thread"));
    assert_eq!(scheduler.matches("send_exit_expedite_sgi(").count(), 1);
    assert!(provider.contains("struct KickSlot"));
    assert!(provider.contains("pub(crate) pid: AtomicU64"));
    assert!(provider.contains("pub(crate) at: AtomicU64"));
    assert!(provider.contains("pub(crate) state: AtomicU64"));
    assert!(!provider.contains("trace_count!(EXIT_SGI_SENT"));
    assert!(!provider.contains("trace_count!(EXIT_KICK_PUBLISHED"));

    let process_mod = source(&sources, "kernel/src/process/mod.rs");
    assert!(process_mod.contains("pub(crate) struct RetirementReceipt"));
    assert!(!process_mod.contains("pub struct RetirementReceipt"));
    assert!(!process_mod.contains("pub fn from_reclaim"));
    assert!(function_body(process_mod, "drop").contains("enqueue_process_reclaim("));

    let process = source(&sources, "kernel/src/process/process.rs");
    for state in ["Absent", "Pending", "Claimed", "Completed"] {
        assert!(process.contains(state));
    }
    assert!(!process.contains("report_marker"));
    assert!(!process.contains("claim_exit_slot"));
    assert!(!process.contains("record_exit"));
}

#[test]
fn phase_one_retirement_fence_and_lock_domains_are_structural() {
    let sources = rust_sources_below("kernel/src");
    let process = source(&sources, "kernel/src/task/process_task.rs");
    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    let manager = source(&sources, "kernel/src/process/manager.rs");
    let ttbr0 = source(&sources, "kernel/src/arch_impl/aarch64/ttbr0.rs");
    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");

    assert_eq!(
        process.matches("static PENDING_PROCESS_RECLAIMS:").count(),
        1
    );
    assert_eq!(
        process.matches("static PARKED_PROCESS_RECLAIMS:").count(),
        1
    );
    assert!(process.contains("last_pass: u32"));
    assert!(process.contains("proof_failures: u8"));
    assert!(process.contains("parked: Option<ParkRecord>"));
    assert!(process.contains("fence_at_park: scheduler::RetirementFence"));
    assert!(process.contains("row_epoch_at_park: u64"));
    assert!(process.contains("age_epoch_sum_at_park: u64"));
    assert!(process.contains("const PARK_AGE_BACKSTOP_EPOCHS: u64 = 64"));

    let drain = function_body(process, "reclaim_deferred_process_resources");
    assert_eq!(drain.matches("RECLAIM_PASS_ID").count(), 1);
    assert!(drain.contains(".fetch_add(1, Ordering::Relaxed)"));
    let cycle = function_body(process, "reclaim_deferred_process_resources_for_pass");
    assert!(
        cycle.find("unpark_sweep();").unwrap() < cycle.find("PENDING_PROCESS_RECLAIMS").unwrap()
    );
    assert!(cycle.contains("reclaim.last_pass = my_pass"));
    assert!(cycle.contains("pending.swap_remove(index)"));
    assert!(cycle.contains("reclaim.cached_root_is_live()"));
    assert!(cycle.contains("reclaim.live_row_names_root()"));

    let lock_free = function_body(process, "lock_free_root_proof");
    assert!(lock_free.contains("fence_elapsed(&self.after_epoch)"));
    assert!(lock_free.contains("local_ttbr0_root()"));
    assert!(lock_free.contains("is_ttbr0_root_live_in_mask"));
    assert!(!lock_free.contains("with_scheduler"));
    assert!(!lock_free.contains("process::manager"));

    let park = function_body(process, "park_reclaim");
    assert!(park.contains("let snapshot_at_park = scheduler::RetirementSnapshot::capture();"));
    assert!(park.contains("let fence_at_park = snapshot_at_park.as_fence();"));
    assert!(!park.contains("reclaim.after_epoch"));
    let unpark = function_body(process, "unpark_sweep_with_snapshot");
    assert!(
        unpark.find("PARKED_PROCESS_RECLAIMS.lock()").unwrap()
            < unpark.find("PENDING_PROCESS_RECLAIMS.lock()").unwrap()
    );

    assert!(scheduler.contains("pub(crate) struct RetirementFence"));
    assert!(scheduler.contains("pub(crate) struct RetirementSnapshot"));
    let capture = function_body(scheduler, "capture");
    assert!(capture.contains("core::sync::atomic::fence(Ordering::Acquire)"));
    let elapsed = function_body(scheduler, "fence_elapsed");
    assert!(
        elapsed.find("fence.online_mask == 0").unwrap()
            < elapsed.find("(0..MAX_CPUS).all").unwrap()
    );
    let stack_reclaim = function_body(scheduler, "reclaim_terminated_threads");
    assert!(
        stack_reclaim.find("retirement_grace_elapsed").unwrap()
            < stack_reclaim.find("is_kernel_stack_slot_live").unwrap()
    );

    assert_exact(
        sites_matching(&sources, |line| {
            line.contains("note_process_row_removed()")
                && !line.contains("fn note_process_row_removed")
        }),
        &[("kernel/src/process/manager.rs", 1104)],
        "ROW_REMOVAL_EPOCH bump sites",
    );
    assert!(function_body(manager, "remove_process").contains("self.processes.remove(&pid)"));
    assert!(ttbr0.contains("core::arch::asm!(\"mrs {}, ttbr0_el1\""));

    for counter in [
        "ROOT_PROOF_BLOCKED_EPOCH",
        "ROOT_PROOF_BLOCKED_HW",
        "ROOT_PROOF_BLOCKED_SHADOW",
        "ROOT_PROOF_BLOCKED_CACHED",
        "ROOT_PROOF_BLOCKED_LIVE_ROW",
        "RETIRE_EMPTY_ONLINE_MASK",
    ] {
        assert!(provider.contains(counter));
    }
    let declaration_only = provider
        .split("// Declaration-only until the phase named in PLAN.md.")
        .nth(1)
        .expect("declaration-only counter boundary")
        .split("pub const COUNTER_COUNT")
        .next()
        .expect("declaration-only counter terminator");
    assert!(!declaration_only.contains("RECLAIM_PASS_SKIPPED"));
    assert!(!declaration_only.contains("RETIRE_EMPTY_ONLINE_MASK"));

    let registry = source(&sources, "kernel/src/test_framework/registry.rs");
    assert!(registry.contains("name: \"retirement_fence_gate\""));
    assert!(registry.contains("name: \"reclaim_progress_gate\""));
}

#[test]
fn all_phase_zero_counters_have_registered_readers_and_honest_runtime_gates() {
    let sources = rust_sources_below("kernel/src");
    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let declarations: BTreeSet<_> = provider
        .split("counter!(")
        .skip(1)
        .filter_map(|rest| {
            rest.trim_start()
                .split_once(',')
                .map(|(name, _)| name.trim().to_owned())
        })
        .filter(|name| name != "$name")
        .collect();
    let inventory = provider
        .split("pub static COUNTERS")
        .nth(1)
        .expect("COUNTERS inventory")
        .split("];")
        .next()
        .expect("COUNTERS terminator");
    let readers: BTreeSet<_> = inventory
        .lines()
        .filter_map(|line| line.trim().strip_prefix('&'))
        .filter_map(|rest| rest.strip_suffix(','))
        .map(str::to_owned)
        .collect();
    assert_eq!(declarations.len(), 47);
    assert_eq!(
        readers, declarations,
        "every counter must have an inventory reader"
    );
    assert!(provider.contains("core::array::from_fn(|index| COUNTERS[index].aggregate())"));
    assert!(provider.contains("for iteration in 0..64"));
    assert!(provider.contains("reset_boot_test_pid_counts();"));
    assert!(provider.contains("for pid in pairing_child_pids"));
    for exact_failure in [
        "adapted-site per-PID defer proof was absent",
        "adapted-site per-PID defer proof was duplicated",
        "adapted-site per-PID reclaim proof was absent",
        "adapted-site per-PID reclaim proof was duplicated",
    ] {
        assert!(provider.contains(exact_failure));
    }
    assert!(!provider.contains("deferred_delta != reclaimed_delta || reclaimed_delta < 64"));
    assert!(!provider.contains("TeardownPairingEvidence"));
    assert!(!provider.contains("defer_reclaim_events_are_paired("));
    assert!(!provider.contains("deferred_pids"));
    assert!(!provider.contains("iter_events()"));
    assert!(!provider.contains("TRACE_BUFFERS"));
    assert!(!provider.contains("super::disable_all()"));
    assert!(!provider.contains("crate::tracing::enable()"));
    assert!(!provider.contains("crate::tracing::disable()"));
    assert!(!provider.contains("TEARDOWN_PROVIDER.enable_all()"));
    assert!(!provider.contains("TEARDOWN_PROVIDER.disable_all()"));
    assert!(source(&sources, "kernel/src/task/process_task.rs").contains("for tid in 1..=17"));
    assert_eq!(
        function_body(provider, "exit_kick_protocol_gate_test")
            .matches("EXIT_SGI_SENT.aggregate()")
            .count(),
        5
    );
    assert!(!provider.contains("TEARDOWN_ENTRY_GROUP.aggregate()"));

    let declaration_only = provider
        .split("// Declaration-only until the phase named in PLAN.md.")
        .nth(1)
        .expect("declaration-only counter boundary");
    assert!(declaration_only.contains("counter!(TEARDOWN_ENTRY_GROUP,"));
    assert!(declaration_only.contains("counter!(EXIT_REQUEST_OBSERVED,"));
    assert!(!declaration_only.contains("counter!(EXIT_SGI_SENT,"));
    assert!(!declaration_only.contains("counter!(EXIT_KICK_PUBLISHED,"));
    assert!(!declaration_only.contains("counter!(RECEIPT_DROPPED_UNRETIRED,"));

    let registry = source(&sources, "kernel/src/test_framework/registry.rs");
    assert!(registry.contains("name: \"fork_exit_defer_reclaim_pairing_test\""));
    assert!(registry.contains("name: \"deferred_fault_ring_overflow_injection\""));
    assert!(registry.contains("name: \"exit_kick_protocol_gate\""));
    assert!(registry.contains("name: \"retirement_receipt_drop_gate\""));

    let plan = repo_text("docs/planning/teardown-unification/PLAN.md");
    assert!(plan.contains("./docker/qemu/run-aarch64-full-test.sh --rebuild --boot-tests-only"));
    let aarch64_gate = repo_text("docker/qemu/run-aarch64-full-test.sh");
    assert!(aarch64_gate.contains("cargo build --release --features boot_tests"));
    assert!(aarch64_gate.contains("--boot-tests-only"));
    assert!(aarch64_gate.contains("grep -q \"\\[BOOT_TESTS:PASS\\]\""));
}

#[test]
fn aarch64_exit_kick_waits_are_progress_bounded() {
    let provider = repo_text("kernel/src/tracing/providers/teardown.rs");
    let gate = function_body(&provider, "exit_kick_protocol_gate_test");
    for required in [
        "const NO_PROGRESS_WINDOW_MILLISECONDS: u64 = 3_000;",
        "const ABSOLUTE_WAIT_CEILING_MILLISECONDS: u64 = 30_000;",
        "const RESCHED_REKICK_INTERVAL_MILLISECONDS: u64 = 50;",
        "const CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS: u64 = 100_000;",
        "if counter_frequency_hz == 0",
        "let counter_delta = now.wrapping_sub(last_counter_sample);",
        "if counter_delta == 0",
        "progress_current.advanced_from(last_progress)",
        "now.wrapping_sub(last_advance) >= no_progress_ticks",
        "elapsed >= absolute_ceiling_ticks",
        "now.wrapping_sub(last_re_kick) >= re_kick_ticks",
        "let late_condition = condition_value();",
        "cause={} elapsed_ms={} window_budget_ms={}",
        "last_advance_ms_ago={}",
        "breadcrumb=1 elapsed_ms={}",
        "WaitProgress::work(publisher_a_progress.load(Ordering::Acquire))",
        "WaitProgress::work(publisher_b_progress.load(Ordering::Acquire))",
        "WaitProgress::work(accounting.workers_ready.load(Ordering::Acquire))",
        "exit: kthread_exit_progress_for_test(tid)",
    ] {
        assert!(
            gate.contains(required),
            "missing exit-kick bound: {required}"
        );
    }
    assert!(!gate.contains("HANDSHAKE_SPIN_CAP"));
    assert!(!gate.contains("TIMER_TICK_COUNT"));
    assert!(!gate.contains("WHOLE_GATE_BUDGET"));
    assert_eq!(
        gate.matches("EXIT_KICK_TEST_HOOK_RESERVED.load").count(),
        1,
        "the reservation wait must remain coordinator-owned"
    );

    let kthread = repo_text("kernel/src/task/kthread.rs");
    let kthread_exit = function_body(&kthread, "kthread_exit");
    assert_eq!(
        kthread_exit
            .matches("record_kthread_exit_stage_for_test")
            .count(),
        4
    );
    assert!(
        kthread_exit
            .rfind("record_kthread_exit_stage_for_test")
            .expect("terminal exit progress bump")
            > kthread_exit
                .find("handle.inner.exited.store(true, Ordering::SeqCst);")
                .expect("exited store"),
        "terminal exit progress must follow the exited store"
    );

    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    assert_eq!(
        function_body(&scheduler, "clear_cpu_affinity_for_test")
            .matches("record_kthread_exit_stage_for_test")
            .count(),
        2
    );

    let main = repo_text("kernel/src/main_aarch64.rs");
    assert!(main.contains("let timeout_ticks = counter_frequency_hz.saturating_mul(6);"));
    assert!(main.contains("[smp] still waiting, {} online"));
    assert!(main.contains("last_psci_return_code(cpu)"));

    let smp = repo_text("kernel/src/arch_impl/aarch64/smp.rs");
    assert!(smp.contains("const PSCI_CPU_ON_MAX_ATTEMPTS: usize = 4;"));
    assert!(smp.contains("ret != PSCI_RETURN_INTERNAL_FAILURE"));
    assert!(smp.contains("PSCI_CPU_ON_BACKOFF_ITERATION_CAP"));
    assert!(smp.contains("LAST_PSCI_RETURN_CODE[cpu_id].store(ret, Ordering::Release);"));
}

#[test]
fn deliberately_broken_variants_fail_the_ratchet() {
    let sources = rust_sources_below("kernel/src");

    let broken_exit = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_exit.rs",
        "fn rogue_exit(pm: &mut ProcessManager, pid: ProcessId) { pm.exit_process(pid, 0); }",
    );
    assert!(validate_exit_process_entry_points(&broken_exit).is_err());

    let broken_by_pid = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_by_pid.rs",
        "fn rogue_exit(pid: ProcessId) { exit_process_by_pid(pid, 0); }",
    );
    assert!(validate_exit_process_entry_points(&broken_by_pid).is_err());

    let broken_test_helper = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_test_helper.rs",
        "fn rogue_test_exit(pid: ProcessId) { exit_process_for_teardown_test(pid, 0); }",
    );
    assert!(validate_exit_process_entry_points(&broken_test_helper).is_err());

    let broken_enqueue = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_enqueue.rs",
        "fn rogue_enqueue(reclaim: DeferredProcessReclaim) { enqueue_process_reclaim(reclaim); }",
    );
    assert!(validate_reclaim_enqueue_callers(&broken_enqueue).is_err());

    let broken_blocking = with_synthetic_source(
        &sources,
        "kernel/src/task/synthetic_blocking.rs",
        "pub fn block_current() {}",
    );
    assert!(validate_blocking_primitives(&broken_blocking).is_err());

    let broken_group_write = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_group.rs",
        "fn rogue_group(thread: &mut Thread) { thread.thread_group_id = Some(1); }",
    );
    assert!(validate_group_writes(&broken_group_write).is_err());

    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    let broken_scheduler = scheduler.replacen(
        "fn send_resched_ipi(&self) {",
        "fn send_resched_ipi(&self) { crate::trace_count!(EXIT_SGI_SENT);",
        1,
    );
    let broken_sgi =
        with_replaced_source(&sources, "kernel/src/task/scheduler.rs", broken_scheduler);
    assert!(validate_exit_sgi_is_teardown_only(&broken_sgi).is_err());
}
