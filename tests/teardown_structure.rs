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
    ("kernel/src/process/manager.rs", 1162),
    ("kernel/src/signal/delivery.rs", 225),
    ("kernel/src/signal/delivery.rs", 260),
    ("kernel/src/syscall/signal.rs", 163),
];
const TERMINATE_MINIMAL_CALLS: &[(&str, usize)] = &[("kernel/src/task/process_task.rs", 274)];
const PRODUCTION_INIT_PID_SITES: &[(&str, usize)] = &[
    ("kernel/src/process/manager.rs", 1179),
    ("kernel/src/task/process_task.rs", 244),
    ("kernel/src/task/process_task.rs", 303),
];
const TEST_INIT_PID_SITES: &[(&str, usize)] = &[
    ("kernel/src/test_userspace.rs", 84),
    ("kernel/src/test_userspace.rs", 203),
    ("kernel/src/test_userspace.rs", 292),
];
const QUARANTINE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/exception.rs", 815),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1179),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1275),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1375),
];
const KERNEL_STACK_MUTATIONS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/syscall_entry.rs", 961),
    ("kernel/src/process/manager.rs", 1839),
    ("kernel/src/syscall/clone.rs", 252),
];
const RECLAIM_ENQUEUE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/process/manager.rs", 1153),
    ("kernel/src/task/process_task.rs", 262),
];
const EXIT_PROCESS_CALLS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/exception.rs", 825),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1190),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1278),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1378),
    ("kernel/src/interrupts.rs", 1429),
    ("kernel/src/interrupts.rs", 1735),
    ("kernel/src/process/mod.rs", 325),
];
const EXIT_PROCESS_BY_PID_CALLS: &[(&str, usize)] = &[
    ("kernel/src/process/mod.rs", 317),
    ("kernel/src/process/mod.rs", 333),
];
const EXIT_PROCESS_FOR_TEARDOWN_TEST_CALLS: &[(&str, usize)] =
    &[("kernel/src/tracing/providers/teardown.rs", 517)];
const BLOCKING_PRIMITIVES: &[(&str, usize)] = &[
    ("kernel/src/task/scheduler.rs", 1740),
    ("kernel/src/task/scheduler.rs", 1911),
    ("kernel/src/task/scheduler.rs", 1930),
    ("kernel/src/task/scheduler.rs", 2079),
    ("kernel/src/task/scheduler.rs", 2167),
    ("kernel/src/task/scheduler.rs", 2232),
    ("kernel/src/task/scheduler.rs", 2241),
    ("kernel/src/task/scheduler.rs", 2400),
    ("kernel/src/task/waitqueue.rs", 52),
];
const RAW_SCHEDULER_LOCK_SITES: &[(&str, usize)] = &[
    ("kernel/src/task/scheduler.rs", 239),
    ("kernel/src/task/scheduler.rs", 246),
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
        &sites_matching(sources, |line| line.contains(".exit_process(")),
        EXIT_PROCESS_CALLS,
    )?;
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
    (!scheduler.contains("EXIT_SGI_SENT")
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
        &[("kernel/src/task/process_task.rs", 285)],
        "btrt::on_process_exit callers",
    );

    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    assert_eq!(provider.matches("counter!(EXIT_SGI_SENT,").count(), 1);
    assert_eq!(provider.matches("counter!(EXIT_KICK_PUBLISHED,").count(), 1);
    validate_exit_sgi_is_teardown_only(&sources)
        .expect("EXIT_SGI_SENT escaped the teardown-only producer");
    assert!(!scheduler.contains("EXIT_KICK_PUBLISHED"));
    assert!(!scheduler.contains("send_exit_expedite_sgi"));
    assert!(!provider.contains("struct KickSlot"));
    assert!(!provider.contains("trace_count!(EXIT_SGI_SENT"));
    assert!(!provider.contains("trace_count!(EXIT_KICK_PUBLISHED"));
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
    assert_eq!(declarations.len(), 39);
    assert_eq!(
        readers, declarations,
        "every counter must have an inventory reader"
    );
    assert!(provider.contains("core::array::from_fn(|index| COUNTERS[index].aggregate())"));
    assert!(provider.contains("for _ in 0..64"));
    assert!(provider.contains("reset_boot_test_pid_counts();"));
    assert!(provider.contains("for pid in pairing_child_pids"));
    assert!(provider.contains("defer_count == 0 || defer_count != reclaim_count"));
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
    assert!(!provider.contains("EXIT_SGI_SENT.aggregate()"));
    assert!(!provider.contains("TEARDOWN_ENTRY_GROUP.aggregate()"));

    let declaration_only = provider
        .split("// Declaration-only until the phase named in PLAN.md.")
        .nth(1)
        .expect("declaration-only counter boundary");
    assert!(declaration_only.contains("counter!(TEARDOWN_ENTRY_GROUP,"));
    assert!(declaration_only.contains("counter!(EXIT_SGI_SENT,"));

    let registry = source(&sources, "kernel/src/test_framework/registry.rs");
    assert!(registry.contains("name: \"fork_exit_defer_reclaim_pairing_test\""));
    assert!(registry.contains("name: \"deferred_fault_ring_overflow_injection\""));

    let plan = repo_text("docs/planning/teardown-unification/PLAN.md");
    assert!(plan.contains("./docker/qemu/run-aarch64-full-test.sh --rebuild --boot-tests-only"));
    let aarch64_gate = repo_text("docker/qemu/run-aarch64-full-test.sh");
    assert!(aarch64_gate.contains("cargo build --release --features boot_tests"));
    assert!(aarch64_gate.contains("--boot-tests-only"));
    assert!(aarch64_gate.contains("grep -q \"\\[BOOT_TESTS:PASS\\]\""));
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
