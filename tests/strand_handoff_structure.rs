use std::fs;
use std::path::PathBuf;

const SCHEDULER_PATH: &str = "kernel/src/task/scheduler.rs";
const CONTEXT_SWITCH_PATH: &str = "kernel/src/arch_impl/aarch64/context_switch.rs";
const MIN_REQUEUE_EARLY_RETURN_GUARDS: usize = 6;
const MIN_CONTEXT_PREVIOUS_RESOLVER_CALLS: usize = 3;

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
    let bare_previous_clears = code_occurrences(&context_switch, "previous_thread = None");
    assert!(
        bare_previous_clears.len() <= 1,
        "context-switch bare previous-thread clears grew beyond the paired-requeue allowance"
    );
    if bare_previous_clears.len() == 1 {
        let trampoline = function_body(&context_switch, "inline_schedule_trampoline");
        assert!(
            !code_occurrences(&trampoline, "requeue_thread_after_save(").is_empty(),
            "the remaining bare previous-thread clear must be paired with a requeue"
        );
    }
    assert!(
        code_occurrences(
            &context_switch,
            "resolve_exception_cleanup_previous_thread("
        )
        .len()
            >= MIN_CONTEXT_PREVIOUS_RESOLVER_CALLS,
        "context-switch previous-thread resolver call-site census shrank"
    );

    let resolver = function_body(&scheduler, "resolve_exception_cleanup_previous_thread");
    assert!(
        !code_occurrences(resolver, "push_back").is_empty(),
        "previous-thread resolver still requeues"
    );
    assert!(
        !code_occurrences(resolver, "previous_thread = None").is_empty(),
        "previous-thread resolver still clears its marker"
    );

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
