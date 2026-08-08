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

fn starts_char_literal(bytes: &[u8], apostrophe: usize) -> bool {
    let Some(&first) = bytes.get(apostrophe + 1) else {
        return false;
    };
    if first == b'\\' {
        return true;
    }
    let utf8_width = match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    bytes.get(apostrophe + 1 + utf8_width) == Some(&b'\'')
}

fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let marker = format!("fn {name}");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("missing function {name}"));
    let open = start + source[start..].find('{').expect("function open brace");
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;
    let mut cursor = open;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if in_string || in_char {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if in_string && byte == b'"' {
                in_string = false;
            } else if in_char && byte == b'\'' {
                in_char = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b'\'' && starts_char_literal(bytes, cursor) {
            in_char = true;
        } else if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            assert!(depth > 0, "unexpected closing brace in function {name}");
            depth -= 1;
            if depth == 0 {
                return &source[start..cursor + 1];
            }
        }
        cursor += 1;
    }
    panic!("unterminated function {name}")
}

fn require_contains(source_name: &str, source: &str, required: &str) -> Result<(), String> {
    if source.contains(required) {
        Ok(())
    } else {
        Err(format!(
            "{source_name} is missing required text: {required}"
        ))
    }
}

fn require_contains_ignoring_ascii_whitespace(
    source_name: &str,
    source: &str,
    required: &str,
) -> Result<(), String> {
    let compact_source: String = source
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    let compact_required: String = required
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    if compact_source.contains(&compact_required) {
        Ok(())
    } else {
        Err(format!(
            "{source_name} is missing required text (ignoring ASCII whitespace): {required}"
        ))
    }
}

#[test]
fn function_body_ignores_braces_in_quoted_literals() {
    let source = r###"
fn target() {
    let _normal = "}";
    let _escaped = "{{";
    let _char = '}';
    if true {}
}

fn next() {}
"###;
    let body = function_body(source, "target");
    assert!(body.contains("if true {}"));
    assert!(!body.contains("fn next"));
}

fn require_count(
    source_name: &str,
    source: &str,
    needle: &str,
    expected: usize,
) -> Result<(), String> {
    let actual = source.matches(needle).count();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{source_name} expected {expected} occurrences of {needle:?}, found {actual}"
        ))
    }
}

fn require_absent(source_name: &str, source: &str, forbidden: &str) -> Result<(), String> {
    if source.contains(forbidden) {
        Err(format!(
            "{source_name} still contains forbidden text: {forbidden}"
        ))
    } else {
        Ok(())
    }
}

fn validate_aarch64_liveness_bounds(
    provider: &str,
    main_aarch64: &str,
    smp: &str,
    scheduler: &str,
    kthread: &str,
) -> Result<(), String> {
    let gate = function_body(provider, "exit_kick_protocol_gate_test");
    for required in [
        "const NO_PROGRESS_WINDOW_MILLISECONDS: u64 = 3_000;",
        "const ABSOLUTE_WAIT_CEILING_MILLISECONDS: u64 = 10_000;",
        "const PROGRESS_SAMPLE_INTERVAL_MILLISECONDS: u64 = 1_000;",
        "const PROGRESS_SAMPLE_CAPACITY: usize = 3;",
        "const CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS: u64 = 100_000;",
        "const CNTVCT_STALL_MINIMUM_ADVANCE_TICKS: u64 = 16;",
        "const FIRST_RESCHED_REKICK_GRACE_MILLISECONDS: u64 = 500;",
        "const RESCHED_REKICK_INTERVAL_MILLISECONDS: u64 = 50;",
        "exit_kick_gate: CNTFRQ_EL0 unavailable; unresponsive watchdog cannot establish wall-clock bounds",
        "let wait_elapsed = now.wrapping_sub(wait_start);",
        "let no_progress_elapsed = now.wrapping_sub(last_advance);",
        "wait_elapsed >= watchdog.absolute_wait_ceiling_ticks",
        "no_progress_elapsed >= watchdog.no_progress_window_ticks",
        "if progress_value.advanced_since(last_progress)",
        "last_advance = now;",
        "stall_sample_iterations >= CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS",
        "now.wrapping_sub(last_counter_sample)",
        "< CNTVCT_STALL_MINIMUM_ADVANCE_TICKS",
        "last_counter_sample = now;",
        "now.wrapping_sub(last_kick) >= watchdog.rekick_interval_ticks",
        "WaitFailureKind::NoProgress",
        "WaitFailureKind::AbsoluteCeiling",
        "Self::NoProgress => \"no_progress\"",
        "Self::AbsoluteCeiling => \"absolute_ceiling\"",
        "no_progress_window_ms",
        "absolute_ceiling_ms",
        "rekick_sgis",
        "progress_rearms",
        "crate::arch_impl::aarch64::smp::cpus_online()",
        "elapsed_ms={}",
        "no_progress_window_ms={}",
        "absolute_ceiling_ms={}",
        "work_progress_start={}",
        "work_progress_final={}",
        "exit_stage_applicable={}",
        "exit_stage_progress_start={}",
        "exit_stage_progress_final={}",
        "progress_never_advanced={}",
        "progress_never_advanced: !progress_final.advanced_since(progress_start)",
        "last_advance_ms_ago={}",
        "progress_sample_count={}",
        "progress_sample_0_ms={}",
        "progress_sample_0_work={}",
        "progress_sample_0_exit_stage={}",
        "progress_sample_2_ms={}",
        "progress_sample_2_work={}",
        "progress_sample_2_exit_stage={}",
        "condition_name",
        "WaitFailureKind::CounterStall",
        "\"exit_kick_gate: CNTVCT stalled between watchdog samples; cannot bound wait; CPU may be unresponsive or counter frozen\";",
        "WaitFailureKind::CounterStall => TestResult::Fail(COUNTER_STALL_MESSAGE)",
        "late_true && matches!(failure.kind, WaitFailureKind::NoProgress)",
        "WaitFailureKind::NoProgress => TestResult::Fail(site.no_progress_message)",
        "WaitFailureKind::AbsoluteCeiling => TestResult::Fail(site.absolute_ceiling_message)",
        "late_true={}",
        "breadcrumb=1 elapsed_ms={}",
        "work_progress={}",
        "exit_stage_progress={}",
        "result={} cause={}",
        "result=pass gate_elapsed_ms={}",
        "slowest_wait_elapsed_ms={}",
        "struct WaitSite",
        "read_progress",
        "struct WaitProgress",
        "fn work_only(work: u64) -> Self",
        "fn with_exit_stage(work: u64, exit_stage: u64) -> Self",
        "boot_test_kthread_exit_stage(handle.tid())",
        "let publisher_a_progress = Arc::new(AtomicU64::new(0));",
        "let publisher_b_progress = Arc::new(AtomicU64::new(0));",
        "a_progress.store(1, Ordering::Release);",
        "a_progress.store(2, Ordering::Release);",
        "a_progress.store(3, Ordering::Release);",
        "b_progress.store(1, Ordering::Release);",
        "b_progress.store(2, Ordering::Release);",
        "b_progress.store(3, Ordering::Release);",
        "b_progress.store(4, Ordering::Release);",
        "while EXIT_KICK_TEST_HOOK_RESERVED.load(Ordering::Acquire) == 0",
        "worker_stages: AtomicU64",
        "accounting.worker_stages.fetch_add(1, Ordering::Release);",
        "publisher_a_attempts: AtomicU64",
        "publisher_b_attempts: AtomicU64",
        "observer_iterations: AtomicU64",
        "let observer_iterations = accounting.observer_iterations.load(Ordering::Acquire);",
        "if observer_iterations == 0",
        "exit-kick storm observer never executed its observation loop",
        "exit-kick reservation-loss publisher CPU is not online",
        "exit-kick storm requires four online CPUs",
        "name: \"publisher_a_cleanup_join\"",
        "name: \"reservation_loss_publisher_b_join\"",
        "name: \"publisher_a_join\"",
        "name: \"storm_workers_join\"",
        "exit_kick_gate: reservation-loss publisher B made no progress for 3 seconds; CPU 1 or CPU 2 is unresponsive",
        "exit_kick_gate: reservation-loss publisher B exceeded the 10-second absolute wait ceiling; CPU 1 or CPU 2 may be unresponsive",
        "exit_kick_gate: publisher A join exceeded the 10-second absolute wait ceiling; CPU 1 may be unresponsive",
        "exit_kick_gate: storm workers made no progress for 3 seconds; a worker CPU (1/2/3) is unresponsive",
        "exit_kick_gate: storm workers exceeded the 10-second absolute wait ceiling; a worker CPU (1/2/3) may be unresponsive",
        "failure_family=join_bookkeeping",
        "at most three normal-path waits",
        "3 * 10 seconds",
        "accounting.workers_ready.load(Ordering::Acquire) != 3",
    ] {
        require_contains("exit_kick_protocol_gate_test", gate, required)?;
    }
    for required in [
        "join_all_with_resched( &[&publisher_b], &[PUBLISHER_A_CPU, PUBLISHER_B_CPU], &wait_watchdog,",
        "join_all_with_resched( &[&publisher_a], &[PUBLISHER_A_CPU], &wait_watchdog,",
        "join_all_with_resched( &[&publisher_a, &publisher_b, &observer], &worker_cpus, &wait_watchdog,",
        "publisher_a_attempts.fetch_add(1, Ordering::Relaxed);",
        "publisher_b_attempts.fetch_add(1, Ordering::Relaxed);",
        "observer_iterations.fetch_add(1, Ordering::Relaxed);",
    ] {
        require_contains_ignoring_ascii_whitespace("exit_kick_protocol_gate_test", gate, required)?;
    }
    for (needle, expected) in [
        // Three normal-path scenario waits plus the mutually exclusive
        // publisher-A cleanup join after a publisher-B spawn failure.
        ("&wait_watchdog", 4),
        ("crate::task::kthread::kthread_join(", 1),
        ("join_all_with_resched(", 4),
        ("start_kthread_exit_watch_for_test(handle.tid())", 1),
        ("register_kthread_exit_watch_for_test(", 5),
    ] {
        require_count("exit_kick_protocol_gate_test", gate, needle, expected)?;
    }
    for forbidden in [
        "WHOLE_GATE_BUDGET_MILLISECONDS",
        "WholeGateDeadline",
        "gate_timeout_ticks",
        "watchdog.gate_start",
        "gate_budget_ms",
        "whole-gate deadline",
        "wait_elapsed == 0",
        "PER_WAIT_BUDGET_MILLISECONDS",
        "PerWaitDeadline",
        "per_wait_deadline",
        "per_wait_timeout_ticks",
        "wait_budget_ms",
        ".expect(\"kthread_join returned Err despite its infallible contract\")",
        "target_cpu_tick_progress",
        "TIMER_TICK_COUNT",
        "CNTVCT_FALLBACK_FREQUENCY_HZ",
        "name: \"publisher_a_reservation\"",
        "name: \"publisher_b_completion\"",
        "name: \"workers_ready\"",
        "await_kthread_exit_watch_for_test",
    ] {
        require_absent("exit_kick_protocol_gate_test", gate, forbidden)?;
    }

    let spin = function_body(provider, "spin_with_resched");
    require_count("spin_with_resched", spin, "if cond() {", 3)?;

    let join = function_body(provider, "join_all_with_resched");
    for required in [
        "handles: &[&crate::task::kthread::KthreadHandle]",
        "kick_cpus: &[usize]",
        "read_work_progress",
        "WaitProgress::with_exit_stage(",
        "boot_test_kthread_exit_stage(handle.tid())",
        "start_kthread_exit_watch_for_test(handle.tid())",
        "wait_with_resched(",
        "crate::task::kthread::kthread_join(handle).is_err()",
        "return Err(TestResult::Fail(site.watch_registration_message));",
        "return Err(TestResult::Fail(site.join_error_message));",
    ] {
        require_contains("join_all_with_resched", join, required)?;
    }

    for required in [
        "const SMP_ONLINE_TIMEOUT_SECONDS: u64 = 6;",
        "const SMP_ONLINE_PROGRESS_INTERVAL_SECONDS: u64 = 1;",
        "timer::CNTVCT_FALLBACK_FREQUENCY_HZ",
        "[smp] still waiting for CPUs ({} online, {} expected)",
        "reported_frequency_hz == 0",
    ] {
        require_contains("main_aarch64.rs", main_aarch64, required)?;
    }
    require_absent(
        "main_aarch64.rs",
        main_aarch64,
        "const CNTVCT_FALLBACK_FREQUENCY_HZ",
    )?;

    for required in [
        "const PSCI_CPU_ON_MAX_ATTEMPTS: usize = 4;",
        "const PSCI_CPU_ON_RETRY_BACKOFF_MICROSECONDS: u64 = 500;",
        "fn psci_cpu_on_result_is_retryable(ret: i64) -> bool",
        "fn psci_cpu_on_result_is_success(ret: i64) -> bool",
        "ret == PSCI_RETURN_INTERNAL_FAILURE",
        "0 | PSCI_RETURN_ALREADY_ON | PSCI_RETURN_ON_PENDING",
        "psci_cpu_on_result_is_retryable(hvc64_ret)",
        "psci_cpu_on_result_is_retryable(ret)",
        "HVC64 failed (ret={}), trying HVC32...",
        "hvc64_internal_failures",
        "hvc32_internal_failures",
        "PSCI CPU_ON accepted after {} attempts",
        "super::timer::CNTVCT_FALLBACK_FREQUENCY_HZ",
        "static LAST_PSCI_HVC64_RETURN_CODE:",
        "LAST_PSCI_HVC64_RETURN_CODE[cpu_id].store(hvc64_ret, Ordering::Relaxed);",
        "pub fn last_psci_hvc64_return_code(cpu_id: usize) -> Option<i64>",
    ] {
        require_contains("smp.rs", smp, required)?;
    }
    require_absent(
        "smp.rs",
        smp,
        "const CNTVCT_FALLBACK_FREQUENCY_HZ",
    )?;
    require_absent(
        "smp.rs",
        smp,
        "hvc64_ret == PSCI_RETURN_ON_PENDING || ret == PSCI_RETURN_ON_PENDING",
    )?;

    for required in [
        "const BOOT_TEST_KTHREAD_EXIT_STAGE_SLOTS: usize = 64;",
        "assert!(BOOT_TEST_KTHREAD_EXIT_STAGE_SLOTS.is_power_of_two())",
        "static BOOT_TEST_KTHREAD_EXIT_STAGES:",
        "static BOOT_TEST_KTHREAD_EXIT_STAGES_STATE:",
        "BOOT_TEST_KTHREAD_EXIT_STAGES_FINISHED",
        "fn reset_boot_test_kthread_exit_stages()",
        "let kthread_exit_stages_guard = reset_boot_test_kthread_exit_stages();",
        "core::mem::drop(kthread_exit_stages_guard);",
        "fn register_kthread_exit_watch_for_test(tid: u64) -> bool",
        "fn register_current_kthread_exit_watch_for_test",
        "pub(crate) fn record_kthread_exit_stage_for_test",
        "fn start_kthread_exit_watch_for_test(tid: u64) -> bool",
        "fn boot_test_kthread_exit_stage",
    ] {
        require_contains("teardown.rs", provider, required)?;
    }
    require_count(
        "exit_kick_protocol_gate_test",
        gate,
        "register_current_kthread_exit_watch_for_test();",
        4,
    )?;
    let clear_affinity = function_body(scheduler, "clear_cpu_affinity_for_test");
    require_contains(
        "clear_cpu_affinity_for_test",
        clear_affinity,
        "record_kthread_exit_stage_for_test(",
    )?;
    require_absent(
        "clear_cpu_affinity_for_test",
        clear_affinity,
        "await_kthread_exit_watch_for_test",
    )?;
    require_count(
        "clear_cpu_affinity_for_test",
        clear_affinity,
        "record_kthread_exit_stage_for_test(",
        2,
    )?;
    let kthread_exit = function_body(kthread, "kthread_exit");
    require_contains(
        "kthread_exit",
        kthread_exit,
        "scheduler::clear_cpu_affinity_for_test(handle.inner.tid);",
    )?;
    require_count(
        "kthread_exit",
        kthread_exit,
        "record_kthread_exit_stage_for_test(",
        4,
    )?;
    let registration = join
        .find("start_kthread_exit_watch_for_test(handle.tid())")
        .ok_or_else(|| "join_all_with_resched lost progress-watch validation".to_string())?;
    let wait = join
        .find("wait_with_resched(")
        .ok_or_else(|| "join_all_with_resched lost its bounded wait".to_string())?;
    if registration >= wait {
        return Err("join_all_with_resched validates progress watches too late".to_string());
    }

    Ok(())
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
    &[("kernel/src/tracing/providers/teardown.rs", 950)];
const BLOCKING_PRIMITIVES: &[(&str, usize)] = &[
    ("kernel/src/task/scheduler.rs", 1973),
    ("kernel/src/task/scheduler.rs", 2187),
    ("kernel/src/task/scheduler.rs", 2206),
    ("kernel/src/task/scheduler.rs", 2355),
    ("kernel/src/task/scheduler.rs", 2443),
    ("kernel/src/task/scheduler.rs", 2508),
    ("kernel/src/task/scheduler.rs", 2517),
    ("kernel/src/task/scheduler.rs", 2676),
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
fn exit_kick_waits_have_progress_rearmed_deadlines_and_bounded_rekicks() {
    let sources = rust_sources_below("kernel/src");
    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let main_aarch64 = source(&sources, "kernel/src/main_aarch64.rs");
    let smp = source(&sources, "kernel/src/arch_impl/aarch64/smp.rs");
    let timer = source(&sources, "kernel/src/arch_impl/aarch64/timer.rs");
    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    let kthread = source(&sources, "kernel/src/task/kthread.rs");
    validate_aarch64_liveness_bounds(provider, main_aarch64, smp, scheduler, kthread)
        .expect("exit-kick wait watchdog structure changed");
    assert!(timer.contains("pub const CNTVCT_FALLBACK_FREQUENCY_HZ: u64 = 1_000_000;"));
}

#[test]
fn boot_test_stage_completion_does_not_wait_for_worker_teardown() {
    let executor = repo_text("kernel/src/test_framework/executor.rs");
    let run_stage = function_body(&executor, "run_staged_tests");
    let run_subsystem = function_body(&executor, "run_subsystem_stage_tests");

    assert!(run_stage.contains("get_stage_progress(*id)[target_stage as usize]"));
    assert!(!run_stage.contains("kthread_join("));

    let completed = run_subsystem
        .find("increment_stage_completed(id, target_stage);")
        .expect("stage completion accounting must remain in the subsystem worker");
    let result_marker = run_subsystem
        .find("[TEST:{}:{}:PASS]")
        .expect("subsystem worker must emit the structured PASS marker");
    assert!(
        completed < result_marker,
        "a printed result must already be durable before a worker can be starved"
    );
}

#[test]
fn exit_kick_watch_progress_is_genuine_and_aggregate_is_bounded() {
    let provider = repo_text("kernel/src/tracing/providers/teardown.rs");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let gate = function_body(&provider, "exit_kick_protocol_gate_test");

    assert!(gate.contains("const ABSOLUTE_WAIT_CEILING_MILLISECONDS: u64 = 10_000;"));
    assert!(gate.contains("at most three normal-path waits"));
    assert!(!gate.contains("name: \"publisher_a_reservation\""));
    assert!(!gate.contains("name: \"publisher_b_completion\""));
    assert!(!gate.contains("name: \"workers_ready\""));
    assert!(gate.contains("accounting.workers_ready.load(Ordering::Acquire) != 3"));

    assert!(!provider.contains("await_kthread_exit_watch_for_test"));
    assert!(provider.contains("fn start_kthread_exit_watch_for_test(tid: u64) -> bool"));
    assert!(provider.contains(
        "assert!(BOOT_TEST_KTHREAD_EXIT_STAGE_SLOTS.is_power_of_two())"
    ));
    assert_eq!(
        function_body(&scheduler, "clear_cpu_affinity_for_test")
            .matches("record_kthread_exit_stage_for_test(")
            .count(),
        2
    );
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

    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let main_aarch64 = source(&sources, "kernel/src/main_aarch64.rs");
    let smp = source(&sources, "kernel/src/arch_impl/aarch64/smp.rs");
    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    let kthread = source(&sources, "kernel/src/task/kthread.rs");
    let unthrottled_rekick = provider.replacen(
        "now.wrapping_sub(last_kick) >= watchdog.rekick_interval_ticks",
        "true",
        1,
    );
    validate_aarch64_liveness_bounds(&unthrottled_rekick, main_aarch64, smp, scheduler, kthread)
        .expect_err("unthrottled re-kick variant passed the exit-kick watchdog ratchet");
    let uncoupled_storm_join = provider.replacen(
        "join_all_with_resched(\n        &[&publisher_a, &publisher_b, &observer],\n        &worker_cpus,\n        &wait_watchdog,",
        "join_all_with_resched(\n        &[&publisher_a, &publisher_b, &observer],\n        &[worker_cpus[0]],\n        &wait_watchdog,",
        1,
    );
    validate_aarch64_liveness_bounds(&uncoupled_storm_join, main_aarch64, smp, scheduler, kthread)
        .expect_err("uncoupled storm join passed the exit-kick watchdog ratchet");
    let stale_counter_baseline = provider.replacen(
        "if now.wrapping_sub(last_counter_sample)\n                    < CNTVCT_STALL_MINIMUM_ADVANCE_TICKS",
        "if wait_elapsed == 0",
        1,
    );
    validate_aarch64_liveness_bounds(
        &stale_counter_baseline,
        main_aarch64,
        smp,
        scheduler,
        kthread,
    )
    .expect_err("wait-start-based CNTVCT stall check passed the liveness ratchet");
    let fixed_no_progress_window = provider.replacen(
        "let no_progress_elapsed = now.wrapping_sub(last_advance);",
        "let no_progress_elapsed = wait_elapsed;",
        1,
    );
    validate_aarch64_liveness_bounds(
        &fixed_no_progress_window,
        main_aarch64,
        smp,
        scheduler,
        kthread,
    )
    .expect_err("fixed per-wait deadline passed the progress-rearm ratchet");
    let per_tid_join_progress = "boot_test_kthread_exit_stage(handle.tid())";
    assert!(provider.contains(per_tid_join_progress));
    let terminal_join_progress = provider.replacen(per_tid_join_progress, "0", 1);
    validate_aarch64_liveness_bounds(
        &terminal_join_progress,
        main_aarch64,
        smp,
        scheduler,
        kthread,
    )
    .expect_err("terminal-only join counter passed the per-TID exit-stage ratchet");
    let cpu_tick_only_join_progress = provider.replacen(
        per_tid_join_progress,
        "TIMER_TICK_COUNT[0].load(Ordering::Relaxed)",
        1,
    );
    validate_aarch64_liveness_bounds(
        &cpu_tick_only_join_progress,
        main_aarch64,
        smp,
        scheduler,
        kthread,
    )
    .expect_err("CPU-tick-only join progress passed the per-TID exit-stage ratchet");
    let unarmed_exit_stage = provider.replacen(
        "!start_kthread_exit_watch_for_test(handle.tid())",
        "false",
        1,
    );
    validate_aarch64_liveness_bounds(&unarmed_exit_stage, main_aarch64, smp, scheduler, kthread)
        .expect_err("unarmed per-TID exit-stage source passed the join-progress ratchet");
    let missing_absolute_ceiling = provider.replacen(
        "wait_elapsed >= watchdog.absolute_wait_ceiling_ticks",
        "false",
        1,
    );
    validate_aarch64_liveness_bounds(
        &missing_absolute_ceiling,
        main_aarch64,
        smp,
        scheduler,
        kthread,
    )
    .expect_err("missing absolute wait ceiling passed the liveness ratchet");
    let shared_gate_deadline = provider.replacen(
        "let wait_elapsed = now.wrapping_sub(wait_start);",
        "let wait_elapsed = now.wrapping_sub(wait_start); let gate_timeout_ticks = wait_elapsed;",
        1,
    );
    validate_aarch64_liveness_bounds(&shared_gate_deadline, main_aarch64, smp, scheduler, kthread)
        .expect_err("shared whole-gate deadline state passed the liveness ratchet");
    let discarded_join_result = provider.replacen(
        "if crate::task::kthread::kthread_join(handle).is_err() {",
        "if false {",
        1,
    );
    validate_aarch64_liveness_bounds(
        &discarded_join_result,
        main_aarch64,
        smp,
        scheduler,
        kthread,
    )
    .expect_err("discarded kthread_join result passed the exit-kick watchdog ratchet");
}
