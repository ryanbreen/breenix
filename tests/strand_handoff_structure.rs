use std::fs;
use std::path::PathBuf;

const SCHEDULER_PATH: &str = "kernel/src/task/scheduler.rs";
const CONTEXT_SWITCH_PATH: &str = "kernel/src/arch_impl/aarch64/context_switch.rs";
const STRAND_ORACLE_PATH: &str = "kernel/src/task/strand_oracle.rs";
const EXECUTOR_PATH: &str = "kernel/src/test_framework/executor.rs";
const MIN_REQUEUE_EARLY_RETURN_GUARDS: usize = 6;

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

fn u64_constant(source: &str, name: &str) -> u64 {
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
    masked[value_start..value_end]
        .trim()
        .replace('_', "")
        .parse()
        .unwrap_or_else(|_| panic!("u64 literal for {name}"))
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
