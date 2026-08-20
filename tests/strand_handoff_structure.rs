use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

const SCHEDULER_PATH: &str = "kernel/src/task/scheduler.rs";
const CONTEXT_SWITCH_PATH: &str = "kernel/src/arch_impl/aarch64/context_switch.rs";
const STRAND_ORACLE_PATH: &str = "kernel/src/task/strand_oracle.rs";
const EXECUTOR_PATH: &str = "kernel/src/test_framework/executor.rs";
const SERVICE_SEQUENCE_GATE_PATH: &str = "docker/qemu/run-aarch64-service-sequence-gate.sh";
const STRICT_GATE_PATH: &str = "docker/qemu/run-aarch64-boot-test-strict.sh";
const FULL_TEST_PATH: &str = "docker/qemu/run-aarch64-full-test.sh";
const MIN_REQUEUE_EARLY_RETURN_GUARDS: usize = 6;
/// New instruction-abort refusal reasons are welcome; losing one is not.
const MIN_INSTRUCTION_ABORT_REFUSAL_REASONS: usize = 3;
/// Each gate must reject both strand marker families; additional rejections are welcome.
const MIN_STRANDED_FORBIDDEN_REJECTIONS: usize = 2;
/// The strict gate scores once while polling and again for the final verdict.
const MIN_STRICT_GATE_SCORE_SERIAL_CALLS: usize = 2;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn code_mask(source: &str) -> Vec<bool> {
    let bytes = source.as_bytes();
    let mut mask = vec![true; bytes.len()];
    let mut line_comment = false;
    let mut block_comment_depth = 0usize;
    let mut string = false;
    let mut character = false;
    let mut raw_string_hashes = None;
    let mut escaped = false;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            } else {
                mask[index] = false;
            }
            index += 1;
            continue;
        }
        if block_comment_depth != 0 {
            mask[index] = false;
            if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
                mask[index + 1] = false;
                block_comment_depth += 1;
                index += 2;
            } else if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                mask[index + 1] = false;
                block_comment_depth -= 1;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(hashes) = raw_string_hashes {
            mask[index] = false;
            if byte == b'"'
                && bytes
                    .get(index + 1..index + 1 + hashes)
                    .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
            {
                mask[index + 1..=index + hashes].fill(false);
                raw_string_hashes = None;
                index += hashes + 1;
            }
            index += 1;
            continue;
        }
        if string || character {
            mask[index] = false;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if (string && byte == b'"') || (character && byte == b'\'') {
                string = false;
                character = false;
            }
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            mask[index] = false;
            mask[index + 1] = false;
            line_comment = true;
            index += 2;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            mask[index] = false;
            mask[index + 1] = false;
            block_comment_depth = 1;
            index += 2;
            continue;
        }
        if byte == b'r' {
            let mut quote = index + 1;
            while bytes.get(quote) == Some(&b'#') {
                quote += 1;
            }
            if bytes.get(quote) == Some(&b'"') {
                mask[index..=quote].fill(false);
                raw_string_hashes = Some(quote - index - 1);
                index = quote + 1;
                continue;
            }
        }
        if byte == b'"' {
            mask[index] = false;
            string = true;
        } else if byte == b'\'' {
            let plain_char = bytes.get(index + 2) == Some(&b'\'');
            let escaped_char =
                bytes.get(index + 1) == Some(&b'\\') && bytes.get(index + 3) == Some(&b'\'');
            if plain_char || escaped_char {
                mask[index] = false;
                character = true;
            }
        }
        index += 1;
    }

    mask
}

fn code_source(source: &str) -> (String, Vec<bool>) {
    let mask = code_mask(source);
    let mut masked = source.as_bytes().to_vec();
    for (byte, is_code) in masked.iter_mut().zip(mask.iter()) {
        if !is_code {
            *byte = b' ';
        }
    }
    (
        String::from_utf8(masked).expect("repository source is UTF-8"),
        mask,
    )
}

fn identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric() || !byte.is_ascii()
}

fn identifier_offsets(source: &str, mask: &[bool], identifier: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    source
        .match_indices(identifier)
        .filter_map(|(offset, _)| {
            let end = offset + identifier.len();
            (mask.get(offset..end).is_some_and(|range| range.iter().all(|v| *v))
                && !offset
                    .checked_sub(1)
                    .and_then(|before| bytes.get(before))
                    .is_some_and(|byte| identifier_byte(*byte))
                && !bytes.get(end).is_some_and(|byte| identifier_byte(*byte)))
            .then_some(offset)
        })
        .collect()
}

fn braced_body<'a>(source: &'a str, masked: &str, open: usize) -> &'a str {
    let bytes = masked.as_bytes();
    assert_eq!(bytes.get(open), Some(&b'{'), "opening brace anchor");
    let mut depth = 0usize;
    for index in open..bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..index];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed source block")
}

fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let (masked, mask) = code_source(source);
    let name_offset = identifier_offsets(&masked, &mask, name)
        .into_iter()
        .find(|offset| {
            masked[..*offset]
                .rsplit_once("fn")
                .is_some_and(|(_, suffix)| suffix.trim().is_empty())
        })
        .unwrap_or_else(|| panic!("function {name} not found"));
    let open = masked[name_offset + name.len()..]
        .find('{')
        .map(|offset| name_offset + name.len() + offset)
        .expect("function opening brace");
    braced_body(source, &masked, open)
}

fn braced_block_after<'a>(source: &'a str, needle: &str) -> &'a str {
    let (masked, mask) = code_source(source);
    let offset = masked
        .match_indices(needle)
        .filter(|(offset, _)| mask[*offset..*offset + needle.len()].iter().all(|v| *v))
        .last()
        .map(|(offset, _)| offset)
        .unwrap_or_else(|| panic!("missing source anchor {needle}"));
    let open = masked[offset + needle.len()..]
        .find('{')
        .map(|relative| offset + needle.len() + relative)
        .expect("branch opening brace");
    braced_body(source, &masked, open)
}

fn code_occurrences(source: &str, needle: &str) -> Vec<usize> {
    let (masked, mask) = code_source(source);
    masked
        .match_indices(needle)
        .filter_map(|(offset, _)| {
            mask[offset..offset + needle.len()]
                .iter()
                .all(|is_code| *is_code)
                .then_some(offset)
        })
        .collect()
}

fn shell_function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let declaration = format!("\n{name}() {{\n");
    let body_start = source
        .find(&declaration)
        .map(|offset| offset + declaration.len())
        .unwrap_or_else(|| panic!("shell function {name} not found"));
    let body_end = source[body_start..]
        .find("\n}\n")
        .map(|offset| body_start + offset)
        .unwrap_or_else(|| panic!("shell function {name} closing brace not found"));
    &source[body_start..body_end]
}

fn shell_exact_line_occurrences(source: &str, needle: &str) -> usize {
    source
        .lines()
        .filter(|line| line.trim() == needle)
        .count()
}

fn stranded_rejection_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("if grep -qE ") && line.contains("stranded=[1-9]"))
        .collect()
}

fn u64_constant_initializer(source: &str, name: &str) -> String {
    let (masked, mask) = code_source(source);
    assert!(
        mask.iter().any(|is_code| *is_code),
        "constant source census must inspect code"
    );
    let declaration = format!("const {name}: u64 =");
    let anchors = code_occurrences(source, &declaration);
    assert_eq!(anchors.len(), 1, "constant declaration anchor for {name}");
    let value_start = anchors[0] + declaration.len();
    let value_end = masked[value_start..]
        .find(';')
        .map(|relative| value_start + relative)
        .unwrap_or_else(|| panic!("constant terminator for {name}"));
    let initializer = masked[value_start..value_end].trim();
    assert!(!initializer.is_empty(), "constant initializer for {name}");
    initializer.to_owned()
}

fn u64_expression(source: &str, expression: &str) -> u64 {
    let expression = expression.trim();
    if let Some((left, right)) = expression.split_once('+') {
        return u64_expression(source, left).saturating_add(u64_expression(source, right));
    }
    if let Some((left, right)) = expression.split_once('*') {
        return u64_expression(source, left).saturating_mul(u64_expression(source, right));
    }
    if let Ok(value) = expression.replace('_', "").parse() {
        return value;
    }
    u64_constant(source, expression)
}

fn u64_constant(source: &str, name: &str) -> u64 {
    let initializer = u64_constant_initializer(source, name);
    u64_expression(source, &initializer)
}

#[test]
fn service_sequence_instruction_abort_signatures_are_set_shaped() {
    let gate = repo_text(SERVICE_SEQUENCE_GATE_PATH);
    let signatures = shell_function_body(&gate, "instruction_abort_signatures");
    assert!(
        !signatures.trim().is_empty(),
        "instruction_abort_signatures body census"
    );

    let record_greps: Vec<_> = signatures
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("grep ") && line.contains("INSTRUCTION_ABORT"))
        .collect();
    assert!(
        record_greps
            .iter()
            .any(|line| line.contains(r"\[INSTRUCTION_ABORT\] FAR=")),
        "instruction_abort_signatures must consult the abort header record"
    );
    assert!(
        record_greps
            .iter()
            .any(|line| line.contains("label=INSTRUCTION_ABORT")),
        "instruction_abort_signatures must consult the FATAL_REGS record"
    );
    assert_eq!(
        shell_exact_line_occurrences(signatures, "} | sort -u"),
        1,
        "instruction_abort_signatures must deduplicate the record union with sort -u"
    );
    assert!(
        !signatures.contains("head -1"),
        "instruction_abort_signatures must not prefer the first abort record with head -1"
    );
}

#[test]
fn service_sequence_instruction_abort_classifier_is_single_signature() {
    let gate = repo_text(SERVICE_SEQUENCE_GATE_PATH);
    let classifier = shell_function_body(&gate, "classify_serial");
    assert!(
        !classifier.trim().is_empty(),
        "classify_serial body census"
    );

    let arm_start = classifier
        .find(r#"    if grep -qF "[INSTRUCTION_ABORT]""#)
        .expect("instruction-abort classifier arm");
    let arm_tail = &classifier[arm_start..];
    let arm_end = arm_tail
        .find("\n    fi\n")
        .map(|offset| offset + "\n    fi\n".len())
        .expect("instruction-abort classifier arm terminator");
    let abort_arm = &arm_tail[..arm_end];
    assert!(
        !abort_arm.trim().is_empty(),
        "instruction-abort classifier arm census"
    );
    assert!(
        abort_arm.contains("instruction_abort_signatures \"$serial_file\""),
        "instruction-abort classifier must call instruction_abort_signatures"
    );
    assert!(
        abort_arm.contains(r#"[ "$instruction_abort_variants" -gt 1 ]"#),
        "instruction-abort classifier must refuse a multi-element signature set"
    );

    let arm_lines: Vec<_> = abort_arm.lines().map(str::trim).collect();
    let refusal_reasons: HashSet<_> = arm_lines
        .windows(2)
        .filter(|pair| pair[0] == r#"CLASS_BUCKET="UNATTRIBUTED""#)
        .filter_map(|pair| pair[1].strip_prefix("CLASS_REASON="))
        .collect();
    assert!(
        refusal_reasons.len() >= MIN_INSTRUCTION_ABORT_REFUSAL_REASONS,
        "instruction-abort refusal-reason census shrank: {} < {}",
        refusal_reasons.len(),
        MIN_INSTRUCTION_ABORT_REFUSAL_REASONS
    );
    assert_eq!(
        shell_exact_line_occurrences(&gate, r#"CLASS_BUCKET="576""#),
        1,
        "service-sequence gate must have exactly one tolerated CLASS_BUCKET=\"576\" assignment"
    );
}

#[test]
fn strict_gate_rejects_stranded_markers_from_finished_serial() {
    let gate = repo_text(STRICT_GATE_PATH);
    let score_serial = shell_function_body(&gate, "score_serial");
    assert!(!score_serial.trim().is_empty(), "score_serial body census");

    let rejections = stranded_rejection_lines(score_serial);
    assert!(
        rejections.len() >= MIN_STRANDED_FORBIDDEN_REJECTIONS,
        "strict-gate stranded rejection census shrank: {} < {}",
        rejections.len(),
        MIN_STRANDED_FORBIDDEN_REJECTIONS
    );
    assert!(
        rejections
            .iter()
            .any(|line| line.contains("SCHED_STRAND_ORACLE")),
        "strict gate must reject a stranded scheduler-census marker"
    );
    assert!(
        rejections
            .iter()
            .any(|line| line.contains("STRAND_INJECT_ORACLE")),
        "strict gate must reject a stranded injection-oracle marker"
    );
}

#[test]
fn strict_gate_poll_loop_stops_only_on_crash_or_complete_score() {
    let gate = repo_text(STRICT_GATE_PATH);
    let run_single_test = shell_function_body(&gate, "run_single_test");
    assert!(
        !run_single_test.trim().is_empty(),
        "run_single_test body census"
    );

    let poll_tail = run_single_test
        .split_once("for POLL in $(seq 1 12); do\n")
        .map(|(_, tail)| tail)
        .expect("strict-gate poll-loop anchor");
    let (poll_loop, after_poll_loop) = poll_tail
        .split_once("\n    done\n")
        .expect("strict-gate poll-loop terminator");
    assert!(!poll_loop.trim().is_empty(), "strict-gate poll-loop census");

    let poll_lines: Vec<_> = poll_loop.lines().map(str::trim).collect();
    let break_count = poll_lines.iter().filter(|line| **line == "break").count();
    assert!(break_count > 0, "strict-gate poll-loop break census");
    let approved_break_count = poll_lines
        .windows(2)
        .filter(|pair| pair[1] == "break")
        .filter(|pair| {
            pair[0].starts_with("if ")
                && (pair[0].contains("check_crash_markers")
                    || pair[0].contains("score_serial"))
        })
        .count();
    assert!(
        approved_break_count == break_count,
        "strict-gate poll-loop break census found {break_count} breaks but only \
         {approved_break_count} are guarded by check_crash_markers or score_serial"
    );

    let poll_score_calls = poll_lines
        .iter()
        .filter(|line| line.starts_with("if ") && line.contains("score_serial \""))
        .count();
    assert!(
        poll_score_calls > 0,
        "strict-gate poll loop must break on score_serial"
    );
    assert!(
        after_poll_loop
            .lines()
            .map(str::trim)
            .any(|line| line.starts_with("if ") && line.contains("score_serial \"")),
        "strict gate must score serial again after the poll loop for the verdict"
    );

    let score_serial_calls = gate
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && line.contains("score_serial \""))
        .count();
    assert!(
        score_serial_calls >= MIN_STRICT_GATE_SCORE_SERIAL_CALLS,
        "strict-gate score_serial call census shrank: {} < {}",
        score_serial_calls,
        MIN_STRICT_GATE_SCORE_SERIAL_CALLS
    );
}

#[test]
fn full_test_rejects_post_run_stranded_markers() {
    let gate = repo_text(FULL_TEST_PATH);
    let cleanup = "wait $QEMU_PID 2>/dev/null || true\nunset QEMU_PID\n";
    let post_run = gate
        .split_once(cleanup)
        .map(|(_, tail)| tail)
        .expect("full-test QEMU exit cleanup anchor");
    assert!(!post_run.trim().is_empty(), "full-test post-run census");

    let rejections = stranded_rejection_lines(post_run);
    assert!(
        rejections.len() >= MIN_STRANDED_FORBIDDEN_REJECTIONS,
        "full-test post-run stranded rejection census shrank: {} < {}",
        rejections.len(),
        MIN_STRANDED_FORBIDDEN_REJECTIONS
    );
    assert!(
        rejections
            .iter()
            .any(|line| line.contains("SCHED_STRAND_ORACLE")),
        "full test must reject a post-run stranded scheduler-census marker"
    );
    assert!(
        rejections
            .iter()
            .any(|line| line.contains("STRAND_INJECT_ORACLE")),
        "full test must reject a post-run stranded injection-oracle marker"
    );
}

#[test]
fn strict_gate_build_hint_enables_boot_tests() {
    let gate = repo_text(STRICT_GATE_PATH);
    let build_hints: Vec<_> = gate
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("echo ") && line.contains("cargo build"))
        .collect();
    assert!(!build_hints.is_empty(), "strict-gate build-hint census");
    assert!(
        build_hints
            .iter()
            .any(|line| line.contains("--features boot_tests")),
        "strict gate build hint must enable --features boot_tests"
    );
}

#[test]
fn strand_handoff_structure_is_pinned_without_line_numbers() {
    let scheduler = repo_text(SCHEDULER_PATH);
    let (masked_scheduler, scheduler_mask) = code_source(&scheduler);

    let initializers: Vec<_> = masked_scheduler
        .match_indices("CpuSchedulerState {")
        .filter_map(|(offset, _)| {
            let prefix = masked_scheduler[..offset].trim_end();
            (!prefix.ends_with("struct")).then_some(offset)
        })
        .collect();
    assert!(!initializers.is_empty(), "CpuSchedulerState initializer census");
    let mentioning_pending_next = initializers
        .iter()
        .filter(|offset| {
            let open = masked_scheduler[**offset..]
                .find('{')
                .map(|relative| **offset + relative)
                .expect("CpuSchedulerState initializer opening brace");
            let body = braced_body(&scheduler, &masked_scheduler, open);
            code_occurrences(body, "pending_next").len() >= 1
        })
        .count();
    assert_eq!(
        mentioning_pending_next,
        initializers.len(),
        "every CpuSchedulerState initializer must initialize pending_next"
    );

    let schedule = function_body(&scheduler, "schedule_deferred_requeue");
    let publications = code_occurrences(schedule, "pending_next = Some(");
    let resolve_calls = code_occurrences(schedule, "resolve_pending_next_locked(");
    assert!(!publications.is_empty(), "scheduler publishes pending_next");
    assert!(!resolve_calls.is_empty(), "scheduler resolves pending_next");
    let publication = publications[0];
    for return_offset in code_occurrences(schedule, "Some((") {
        assert!(
            publication < return_offset,
            "every Some(( scheduler return must follow pending_next publication"
        );
    }

    let commit = function_body(&scheduler, "commit_cpu_state_after_save");
    assert!(
        !code_occurrences(commit, "pending_next = None").is_empty(),
        "commit_cpu_state_after_save clears pending_next"
    );

    let requeue = function_body(&scheduler, "requeue_thread_after_save");
    let (masked_requeue, _) = code_source(requeue);
    let enqueue = masked_requeue
        .find("push_back")
        .expect("requeue enqueue anchor");
    let early_returns = identifier_offsets(
        &masked_requeue[..enqueue],
        &code_mask(&masked_requeue[..enqueue]),
        "return",
    )
    .len();
    assert!(
        early_returns >= MIN_REQUEUE_EARLY_RETURN_GUARDS,
        "requeue refusal census shrank: {early_returns} < {MIN_REQUEUE_EARLY_RETURN_GUARDS}"
    );

    let context_switch = repo_text(CONTEXT_SWITCH_PATH);
    let trampoline = function_body(&context_switch, "inline_schedule_trampoline");
    let null_branch = braced_block_after(&trampoline, "if sched_ptr.is_null()");
    assert!(
        !code_occurrences(null_branch, "resolve_pending_next_locked(").is_empty(),
        "null scheduler_ptr branch resolves the released pending handoff"
    );

    assert!(
        scheduler_mask.iter().any(|is_code| *is_code),
        "scheduler source census must inspect code, not an empty mask"
    );
}

#[test]
fn injection_stimulus_is_gated_by_the_idle_outgoing_thread() {
    let context_switch = repo_text(CONTEXT_SWITCH_PATH);
    let trampoline = function_body(&context_switch, "inline_schedule_trampoline");
    assert!(
        !trampoline.trim().is_empty(),
        "inline_schedule_trampoline body census"
    );
    let (_, trampoline_mask) = code_source(trampoline);
    assert!(
        trampoline_mask.iter().any(|is_code| *is_code),
        "inline_schedule_trampoline census must inspect code"
    );

    let idle_loads = code_occurrences(trampoline, "cpu_state[cpu_id].idle_thread");
    assert!(
        !idle_loads.is_empty(),
        "injection gate reads this CPU's idle_thread"
    );
    let idle_comparisons = code_occurrences(trampoline, "idle_id == old_id");
    assert!(
        !idle_comparisons.is_empty(),
        "injection gate compares idle_thread identity with the outgoing id"
    );
    assert!(
        idle_loads[0] < idle_comparisons[0],
        "idle_thread load must introduce the outgoing-id comparison"
    );

    let injection_guard = braced_block_after(trampoline, "if idle_id == old_id");
    assert!(
        !injection_guard.trim().is_empty(),
        "idle outgoing injection guard body census"
    );
    assert!(
        !code_occurrences(injection_guard, "inject_if_armed(").is_empty(),
        "inject_if_armed must be inside the idle outgoing guard"
    );

    let trampoline_start = context_switch
        .find(trampoline)
        .expect("inline_schedule_trampoline body position");
    let comparison_start = trampoline_start + idle_comparisons[0];
    let injection_calls = code_occurrences(&context_switch, "inject_if_armed(");
    assert!(!injection_calls.is_empty(), "inject_if_armed call census");
    assert!(
        injection_calls
            .iter()
            .all(|call_offset| *call_offset > comparison_start),
        "every inject_if_armed call must follow the idle/outgoing comparison"
    );
}

#[test]
fn x86_strand_oracle_is_synchronous_and_sampled_by_the_executor() {
    let oracle = repo_text(STRAND_ORACLE_PATH);
    let sample = function_body(&oracle, "sample_now");
    let sample_once = function_body(&oracle, "sample_once");
    assert!(
        !code_occurrences(sample, "sample_once").is_empty(),
        "sample_now must use the shared sampling implementation"
    );
    assert!(
        !code_occurrences(sample_once, "collect_strand_census").is_empty(),
        "sample_now must collect one scheduler census"
    );
    assert!(
        !code_occurrences(sample_once, "update_dwell").is_empty(),
        "sample_now must share the kthread dwell bookkeeping"
    );
    for forbidden in [
        "block_current_for_timer",
        "yield_current",
        "arch_halt_with_interrupts",
        "serial_println",
    ] {
        assert!(
            code_occurrences(sample, forbidden).is_empty(),
            "sample_now must not contain {forbidden}"
        );
    }

    let start = function_body(&oracle, "start");
    assert!(
        start.contains("#[cfg(target_arch = \"aarch64\")]\n    {")
            && start.contains("kthread_run(strand_oracle_thread"),
        "the oracle kthread must be inside the aarch64-only start block"
    );

    let executor = repo_text(EXECUTOR_PATH);
    assert!(
        code_occurrences(&executor, "strand_oracle::sample_now()").len() >= 2,
        "the executor must sample at stage/completion boundaries and before verdict"
    );
    assert!(
        !code_occurrences(&executor, "strand_oracle::report_x86_once()").is_empty(),
        "the executor must emit the x86 oracle once from the verdict path"
    );
}

#[test]
fn pending_next_is_taken_only_after_rollback_refusals() {
    let scheduler = repo_text(SCHEDULER_PATH);
    let resolver = function_body(&scheduler, "resolve_pending_next_locked");
    assert!(
        !resolver.trim().is_empty(),
        "resolve_pending_next_locked body census"
    );
    let (_, resolver_mask) = code_source(resolver);
    assert!(
        resolver_mask.iter().any(|is_code| *is_code),
        "resolve_pending_next_locked census must inspect code"
    );

    let refusal_returns = code_occurrences(resolver, "return;");
    assert!(
        !refusal_returns.is_empty(),
        "resolve_pending_next_locked refusal return census"
    );
    let pending_takes = code_occurrences(resolver, "pending_next.take()");
    assert!(
        !pending_takes.is_empty(),
        "resolve_pending_next_locked pending_next take census"
    );
    let last_refusal_return = *refusal_returns.last().expect("last refusal return");
    assert!(
        pending_takes
            .iter()
            .all(|take_offset| *take_offset > last_refusal_return),
        "pending_next must be taken only after every refusal return"
    );
}

#[test]
fn first_strand_census_precedes_the_steady_state_cadence() {
    let oracle = repo_text(STRAND_ORACLE_PATH);
    let first_report_ms = u64_constant(&oracle, "STRAND_FIRST_REPORT_MS");
    let report_period_ms = u64_constant(&oracle, "STRAND_REPORT_PERIOD_MS");
    assert!(
        first_report_ms < report_period_ms,
        "first strand census must precede the steady-state cadence"
    );
}

#[test]
fn strand_victim_parks_on_the_sample_timer() {
    let oracle = repo_text(STRAND_ORACLE_PATH);
    let victim = function_body(&oracle, "strand_victim");
    assert!(!victim.trim().is_empty(), "strand_victim body census");
    let (_, victim_mask) = code_source(victim);
    assert!(
        victim_mask.iter().any(|is_code| *is_code),
        "strand_victim census must inspect code"
    );
    assert!(
        code_occurrences(victim, "schedule_from_kernel").is_empty(),
        "strand_victim must not drive the inline schedule path"
    );
    assert!(
        code_occurrences(victim, "yield_current").is_empty(),
        "strand_victim must not yield directly"
    );
    assert!(
        !code_occurrences(victim, "sleep_sample_period()").is_empty(),
        "strand_victim must park on the sample timer"
    );
}

#[test]
fn injection_report_cap_preserves_in_flight_scoring_windows() {
    let oracle = repo_text(STRAND_ORACLE_PATH);
    let report_cap_initializer = u64_constant_initializer(&oracle, "INJECT_REPORT_CAP_MS");
    assert!(
        !code_occurrences(&report_cap_initializer, "INJECT_DEADLINE_MS").is_empty(),
        "report cap initializer must include the firing deadline"
    );
    assert!(
        !code_occurrences(&report_cap_initializer, "INJECT_SCORE_WAIT_MS").is_empty(),
        "report cap initializer must include the scoring window"
    );

    let report_cap_ms = u64_constant(&oracle, "INJECT_REPORT_CAP_MS");
    let deadline_ms = u64_constant(&oracle, "INJECT_DEADLINE_MS");
    let score_wait_ms = u64_constant(&oracle, "INJECT_SCORE_WAIT_MS");
    assert!(
        report_cap_ms >= deadline_ms.saturating_add(2 * score_wait_ms),
        "report cap must leave two full scoring windows after the firing deadline"
    );

    let marker_ready = function_body(&oracle, "injection_marker_ready");
    assert!(
        !marker_ready.trim().is_empty(),
        "injection_marker_ready body census"
    );
    let (_, marker_ready_mask) = code_source(marker_ready);
    assert!(
        marker_ready_mask.iter().any(|is_code| *is_code),
        "injection_marker_ready census must inspect code"
    );
    assert!(
        !code_occurrences(marker_ready, "INJECT_A_FIRED").is_empty(),
        "marker readiness must consult leg A's mid-scoring state"
    );
    assert!(
        !code_occurrences(marker_ready, "INJECT_B_FIRED").is_empty(),
        "marker readiness must consult leg B's mid-scoring state"
    );
}
