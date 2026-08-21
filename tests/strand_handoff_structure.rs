use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

const SCHEDULER_PATH: &str = "kernel/src/task/scheduler.rs";
const CONTEXT_SWITCH_PATH: &str = "kernel/src/arch_impl/aarch64/context_switch.rs";
const STRAND_ORACLE_PATH: &str = "kernel/src/task/strand_oracle.rs";
const EXECUTOR_PATH: &str = "kernel/src/test_framework/executor.rs";
const TEST_REGISTRY_PATH: &str = "kernel/src/test_framework/registry.rs";
const X86_BOOT_GATE_PATH: &str = "docker/qemu/run-x86-boot-tests.sh";
const SERVICE_SEQUENCE_GATE_PATH: &str = "docker/qemu/run-aarch64-service-sequence-gate.sh";
const STRICT_GATE_PATH: &str = "docker/qemu/run-aarch64-boot-test-strict.sh";
const FULL_TEST_PATH: &str = "docker/qemu/run-aarch64-full-test.sh";
const MIN_REQUEUE_EARLY_RETURN_GUARDS: usize = 6;
/// New instruction-abort refusal reasons are welcome; losing one is not.
const MIN_INSTRUCTION_ABORT_REFUSAL_REASONS: usize = 3;
/// Additional #609 clauses are welcome; dropping one is how a tolerated bucket starts absorbing unfiled failures.
const MIN_609_SIGNATURE_GUARDS: usize = 5;
const MAX_NON_FAILING_SERVICE_SEQUENCE_BUCKETS: usize = 2;
/// Each gate must reject both strand marker families; additional rejections are welcome.
const MIN_STRANDED_FORBIDDEN_REJECTIONS: usize = 2;
/// More discriminating markers are welcome; dropping one quietly disarms the profile guard.
const MIN_BOOT_TESTS_PROFILE_MARKERS: usize = 6;
/// The strict gate scores once while polling and again for the final verdict.
const MIN_STRICT_GATE_SCORE_SERIAL_CALLS: usize = 2;
/// Dormancy currently has two axes and ordinary reachability has five.
const MIN_CENSUS_DISPOSABILITY_REACHABILITY_DIMENSIONS: usize = 7;

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

fn bracketed_block_after<'a>(source: &'a str, needle: &str) -> &'a str {
    let (masked, mask) = code_source(source);
    let offset = masked
        .match_indices(needle)
        .filter(|(offset, _)| mask[*offset..*offset + needle.len()].iter().all(|v| *v))
        .last()
        .map(|(offset, _)| offset)
        .unwrap_or_else(|| panic!("missing source anchor {needle}"));
    let open = masked[offset + needle.len()..]
        .find('[')
        .map(|relative| offset + needle.len() + relative)
        .expect("array opening bracket");
    let bytes = masked.as_bytes();
    let mut depth = 0usize;
    for index in open..bytes.len() {
        match bytes[index] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..index];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed source array")
}

fn comma_separated_shape_count(source: &str) -> usize {
    let (masked, _) = code_source(source);
    masked
        .split(',')
        .filter(|dimension| !dimension.trim().is_empty())
        .count()
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

fn function_bodies<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let (masked, mask) = code_source(source);
    identifier_offsets(&masked, &mask, name)
        .into_iter()
        .filter_map(|name_offset| {
            masked[..name_offset]
                .rsplit_once("fn")
                .is_some_and(|(_, suffix)| suffix.trim().is_empty())
                .then(|| {
                    let open = masked[name_offset + name.len()..]
                        .find('{')
                        .map(|offset| name_offset + name.len() + offset)
                        .expect("function opening brace");
                    braced_body(source, &masked, open)
                })
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

/// Coordinator ruling R33 retired #609's rate pre-adjudication after its forced arm
/// falsified the RCA mechanism: stimulus armed 20/20 and dispatched 0/20, while 290
/// non-forcing boots on main produced zero occurrences. Any recurrence must now red
/// the gate and yield a fresh serial.
#[test]
fn service_sequence_609_arm_is_field_keyed_and_untolerated() {
    let gate = repo_text(SERVICE_SEQUENCE_GATE_PATH);
    let signature = shell_function_body(&gate, "is_609_network_early_stall");
    assert!(
        !signature.trim().is_empty(),
        "is_609_network_early_stall body census"
    );

    let signature_guards = signature
        .lines()
        .filter(|line| line.contains("grep "))
        .count();
    assert!(
        signature_guards >= MIN_609_SIGNATURE_GUARDS,
        "#609 signature-guard census shrank: {} < {}",
        signature_guards,
        MIN_609_SIGNATURE_GUARDS
    );
    assert!(
        signature.contains("[SUBSYSTEM:memory:early:COMPLETE:"),
        "#609 signature must require memory:early completion"
    );
    assert!(
        signature.contains("(TEST|SUBSYSTEM):network:"),
        "#609 signature must require complete network:early silence"
    );
    assert!(
        signature.contains("[STAGE:early:COMPLETE"),
        "#609 signature must require an unfinished early stage"
    );
    assert!(
        signature.contains(r"(DATA|INSTRUCTION)_ABORT\]|KERNEL PANIC|panic!"),
        "#609 signature must reject abort and panic evidence"
    );
    assert!(
        signature.contains("stranded=0"),
        "#609 signature must require a clean live strand census"
    );

    assert_eq!(
        shell_exact_line_occurrences(&gate, r#"CLASS_BUCKET="609""#),
        1,
        "service-sequence gate must have exactly one tolerated CLASS_BUCKET=\"609\" assignment"
    );

    let classifier = shell_function_body(&gate, "classify_serial");
    let arm_609_offset = classifier
        .find(r#"is_609_network_early_stall "$serial_file""#)
        .expect("#609 classifier arm");
    let arm_609_tail = &classifier[arm_609_offset..];
    let arm_609_end = arm_609_tail
        .find("\n    fi\n")
        .map(|offset| offset + "\n    fi\n".len())
        .expect("#609 classifier arm terminator");
    let arm_609 = &arm_609_tail[..arm_609_end];
    let bucket_609_assignments: Vec<_> = arm_609
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix("CLASS_BUCKET=\"")
                .and_then(|bucket| bucket.strip_suffix('"'))
        })
        .collect();
    assert_eq!(
        bucket_609_assignments.len(),
        1,
        "#609 classifier arm must assign exactly one bucket"
    );
    let bucket_609 = bucket_609_assignments[0];
    // Field-key this ratchet: #589 was closed and its bucket renamed, and a name-pinned
    // ratchet would go green if a rename dropped both strand-attribution arms entirely.
    let strand_fields = [
        (
            "scheduler strand census",
            r"\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:",
        ),
        (
            "strand injection oracle",
            r"\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]",
        ),
    ];
    let strand_arms: Vec<_> = strand_fields
        .iter()
        .map(|(field, predicate)| {
            let predicate_offset = classifier
                .find(predicate)
                .unwrap_or_else(|| panic!("classify_serial must retain its nonzero {field} arm"));
            let arm_tail = &classifier[predicate_offset..];
            let arm_end = arm_tail
                .find("\n    fi\n")
                .map(|offset| offset + "\n    fi\n".len())
                .unwrap_or_else(|| panic!("{field} classifier arm terminator"));
            let arm = &arm_tail[..arm_end];
            let bucket_assignments: Vec<_> = arm
                .lines()
                .map(str::trim)
                .filter_map(|line| {
                    line.strip_prefix("CLASS_BUCKET=\"")
                        .and_then(|bucket| bucket.strip_suffix('"'))
                })
                .collect();
            assert_eq!(
                bucket_assignments.len(),
                1,
                "{field} classifier arm must assign exactly one bucket"
            );
            (
                field,
                predicate_offset + arm_end,
                bucket_assignments[0],
            )
        })
        .collect();
    assert!(
        strand_arms
            .iter()
            .all(|(_, arm_end, _)| arm_609_offset > *arm_end),
        "#609 classification must remain strictly after both field-keyed strand-attribution arms"
    );

    let run_profile = shell_function_body(&gate, "run_profile");
    let classifier_dispatch = run_profile
        .split_once(r#"case "$CLASS_BUCKET" in"#)
        .and_then(|(_, tail)| tail.split_once("esac").map(|(dispatch, _)| dispatch))
        .expect("run_profile CLASS_BUCKET dispatch");
    let bucket_counters: Vec<_> = classifier_dispatch
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (bucket, update) = line.split_once(')')?;
            let bucket = bucket.trim();
            let update = update.trim();
            if bucket.is_empty()
                || bucket == "*"
                || bucket.bytes().any(|byte| byte.is_ascii_whitespace())
                || !update.ends_with(";;")
            {
                return None;
            }
            let counter = update.split_once('=').map(|(counter, _)| counter.trim())?;
            counter
                .strip_prefix("count_")
                .filter(|suffix| {
                    !suffix.is_empty()
                        && suffix
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                })
                .map(|_| (bucket, counter))
        })
        .collect();
    let fail_conditions: Vec<_> = run_profile
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(r#"if [ "$count_"#) && line.ends_with("; then"))
        .collect();
    assert_eq!(
        fail_conditions.len(),
        1,
        "run_profile must retain exactly one per-profile count_* FAIL condition"
    );
    let fail_condition = fail_conditions[0];
    for (field, _, bucket) in strand_arms {
        let counter = bucket_counters
            .iter()
            .find_map(|(dispatch_bucket, counter)| (*dispatch_bucket == bucket).then_some(*counter))
            .unwrap_or_else(|| {
                panic!("{field} bucket {bucket} must map to a count_* counter in run_profile")
            });
        let failing_term = format!(r#"[ "${counter}" -ne 0 ]"#);
        assert!(
            fail_condition.contains(&failing_term),
            "{field} bucket {bucket} counter {counter} must remain in run_profile's per-profile FAIL condition"
        );
    }

    let counter_609 = bucket_counters
        .iter()
        .find_map(|(bucket, counter)| (*bucket == bucket_609).then_some(*counter))
        .unwrap_or_else(|| {
            panic!("#609 bucket {bucket_609} must map to a count_* counter in run_profile")
        });
    let failing_609_term = format!(r#"[ "${counter_609}" -ne 0 ]"#);
    assert!(
        fail_condition.contains(&failing_609_term),
        "#609 bucket {bucket_609} counter {counter_609} must remain in run_profile's per-profile FAIL condition"
    );

    assert!(
        !gate.contains("TOTAL_609_CEILING"),
        "#609's retired run-wide rate-ceiling token must not survive anywhere in the gate"
    );
    assert!(
        !gate.contains("_CEILING="),
        "service-sequence gate must not contain any per-class _CEILING= assignment"
    );

    let classifier_buckets: HashSet<_> = classifier
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix("CLASS_BUCKET=\"")
                .and_then(|bucket| bucket.strip_suffix('"'))
        })
        .collect();
    assert!(
        !classifier_buckets.is_empty(),
        "classify_serial bucket census"
    );
    let (failing_buckets, non_failing_buckets): (HashSet<_>, HashSet<_>) =
        classifier_buckets.iter().copied().partition(|bucket| {
            let counter = bucket_counters
                .iter()
                .find_map(|(dispatch_bucket, counter)| {
                    (*dispatch_bucket == *bucket).then_some(*counter)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "classify_serial bucket {} must map to a count_* counter in run_profile",
                        *bucket
                    )
                });
            let failing_term = format!(r#"[ "${counter}" -ne 0 ]"#);
            fail_condition.contains(&failing_term)
        });
    assert_eq!(
        non_failing_buckets.len(),
        MAX_NON_FAILING_SERVICE_SEQUENCE_BUCKETS,
        "service-sequence non-failing bucket census changed to {non_failing_buckets:?}; a new non-failing bucket is a new tolerance and needs a coordinator ruling"
    );
    assert_eq!(
        non_failing_buckets,
        HashSet::from(["GREEN", "576"]),
        "service-sequence gate may pass only the healthy GREEN bucket and open pre-adjudicated bucket 576"
    );
    assert!(
        failing_buckets.contains(bucket_609),
        "#609 bucket {bucket_609} must be in the failing bucket census"
    );
    assert!(
        !non_failing_buckets.contains(bucket_609),
        "#609 bucket {bucket_609} must not be in the non-failing bucket census"
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
fn boot_tests_gates_refuse_a_wrong_profile_kernel() {
    for (gate_name, gate_path) in [
        ("service-sequence gate", SERVICE_SEQUENCE_GATE_PATH),
        ("strict gate", STRICT_GATE_PATH),
        ("full test", FULL_TEST_PATH),
    ] {
        let gate = repo_text(gate_path);
        let guard = shell_function_body(&gate, "require_boot_tests_kernel");
        assert!(
            !guard.trim().is_empty(),
            "{gate_name} boot-tests profile guard body census"
        );

        let marker_line = guard
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("for marker in ") && line.ends_with("; do"))
            .unwrap_or_else(|| panic!("{gate_name} boot-tests profile marker census"));
        let markers: Vec<_> = marker_line.split('\'').skip(1).step_by(2).collect();
        assert!(
            markers.len() >= MIN_BOOT_TESTS_PROFILE_MARKERS,
            "{gate_name} boot-tests profile marker census shrank: {} < {}",
            markers.len(),
            MIN_BOOT_TESTS_PROFILE_MARKERS
        );
        assert!(
            markers
                .iter()
                .all(|marker| marker.starts_with('[') && marker.ends_with(':')),
            "{gate_name} boot-tests profile marker census must contain only bracketed, colon-terminated kernel marker prefixes"
        );

        let marker_loop = guard[guard
            .find(marker_line)
            .unwrap_or_else(|| panic!("{gate_name} boot-tests profile marker loop"))..]
            .split_once("\n    done")
            .map(|(body, _)| body)
            .unwrap_or_else(|| panic!("{gate_name} boot-tests profile marker loop terminator"));
        assert!(
            marker_loop.lines().map(str::trim).any(|line| {
                line.starts_with(r#"if ! grep -aqF "$marker" "$kernel""#)
            }),
            "{gate_name} boot-tests profile guard must inspect the kernel with binary-safe fixed-string grep"
        );
        assert_eq!(
            shell_exact_line_occurrences(marker_loop, r#"missing="$missing $marker""#),
            1,
            "{gate_name} boot-tests profile guard must record every missing marker"
        );
        let missing_arm = guard
            .split_once(r#"if [ -n "$missing" ]; then"#)
            .and_then(|(_, tail)| tail.split_once("\n    fi"))
            .map(|(arm, _)| arm)
            .unwrap_or_else(|| panic!("{gate_name} missing-marker refusal arm"));
        assert_eq!(
            shell_exact_line_occurrences(missing_arm, "exit 1"),
            1,
            "{gate_name} must exit non-zero when a boot-tests profile marker is missing"
        );

        let guard_invocation = r#"require_boot_tests_kernel "$KERNEL""#;
        assert_eq!(
            shell_exact_line_occurrences(&gate, guard_invocation),
            1,
            "{gate_name} must invoke the boot-tests profile guard exactly once at top level"
        );
        assert!(
            gate.lines().any(|line| line == guard_invocation),
            "{gate_name} boot-tests profile guard invocation must remain top level"
        );

        let no_neon_invocation = r#""$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL""#;
        let no_neon_offset = gate
            .find(no_neon_invocation)
            .unwrap_or_else(|| panic!("{gate_name} no-NEON preflight"));
        let profile_guard_offset = gate
            .find(guard_invocation)
            .unwrap_or_else(|| panic!("{gate_name} boot-tests profile preflight"));
        assert!(
            profile_guard_offset > no_neon_offset,
            "{gate_name} must run the no-NEON preflight before the boot-tests profile guard"
        );
    }
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
fn x86_census_widen_oracle_is_one_shot_in_the_live_verdict_path() {
    let registry = repo_text(TEST_REGISTRY_PATH);
    assert!(
        registry.contains("pub fn run_census_widen_oracle() -> bool"),
        "the census-widening probe must expose its boolean verdict to the x86 driver"
    );
    let probe = function_body(&registry, "run_census_widen_oracle");
    for required in [
        "disarm_census_widen_injection()",
        "collect_strand_census(&mut candidates)",
        "arm_census_widen_injection(tid)",
        "[CENSUS_WIDEN_ORACLE:",
        "passed",
    ] {
        assert!(
            probe.contains(required),
            "the callable census-widening probe must retain {required}"
        );
    }
    let test_def = function_body(&registry, "test_census_widen_oracle");
    assert!(
        !code_occurrences(test_def, "run_census_widen_oracle()").is_empty(),
        "the registered aarch64 test must keep calling the factored probe"
    );

    let executor = repo_text(EXECUTOR_PATH);
    let driver = function_body(&executor, "run_census_widen_oracle_x86_once");
    assert!(
        !code_occurrences(driver, "compare_exchange(false, true").is_empty(),
        "the x86 census-widening driver must use an atomic one-shot"
    );
    assert_eq!(
        code_occurrences(driver, "registry::run_census_widen_oracle()").len(),
        1,
        "the one-shot driver must have exactly one probe call site"
    );

    let marker_only = function_body(&executor, "advance_stage_marker_only");
    let cfg_anchor = "#[cfg(not(target_arch = \"aarch64\"))]";
    let cfg_offset = marker_only
        .rfind(cfg_anchor)
        .expect("x86 verdict cfg block");
    let (masked_marker_only, _) = code_source(marker_only);
    let cfg_open = masked_marker_only[cfg_offset + cfg_anchor.len()..]
        .find('{')
        .map(|relative| cfg_offset + cfg_anchor.len() + relative)
        .expect("x86 verdict cfg opening brace");
    let x86_verdict = braced_body(marker_only, &masked_marker_only, cfg_open);
    let sample = code_occurrences(x86_verdict, "strand_oracle::sample_now()");
    let census = code_occurrences(x86_verdict, "run_census_widen_oracle_x86_once()");
    let report = code_occurrences(x86_verdict, "strand_oracle::report_x86_once()");
    assert_eq!(sample.len(), 1, "x86 verdict block final sample census");
    assert_eq!(census.len(), 1, "x86 verdict block census-oracle driver census");
    assert_eq!(report.len(), 1, "x86 verdict block strand report census");
    assert!(
        sample[0] < census[0] && census[0] < report[0],
        "the census-widening probe must run after the final sample and before the strand report"
    );

    let gate = repo_text(X86_BOOT_GATE_PATH);
    let pattern = gate
        .lines()
        .find(|line| line.starts_with("CENSUS_WIDEN_ORACLE_PATTERN="))
        .expect("x86 gate census-widening pattern constant");
    assert!(
        pattern.contains("CENSUS_WIDEN_ORACLE:x86:") && pattern.ends_with(":PASS\\]'"),
        "the x86 gate pattern must accept only the passing x86 census-widening marker"
    );
    assert!(
        gate.contains("&& grep -qE \"$CENSUS_WIDEN_ORACLE_PATTERN\""),
        "the x86 poll verdict must require the census-widening marker"
    );
    let exact_count = gate
        .find("test \"$(grep -h -E -c \"$CENSUS_WIDEN_ORACLE_PATTERN\"")
        .expect("x86 gate exact census-widening marker count");
    assert!(
        gate[exact_count..]
            .lines()
            .take(2)
            .any(|line| line.contains("-eq 1")),
        "the x86 final verdict must require exactly one passing census-widening marker"
    );
}

#[test]
fn strand_census_disposability_and_reachability_dimensions_cannot_shrink() {
    let scheduler = repo_text(SCHEDULER_PATH);
    let census = function_body(&scheduler, "collect_strand_census");
    let dormancy = bracketed_block_after(census, "let dormancy_dimensions =");
    let reachability = bracketed_block_after(census, "let reachability_dimensions =");
    let dimensions =
        comma_separated_shape_count(dormancy) + comma_separated_shape_count(reachability);
    assert!(
        dimensions >= MIN_CENSUS_DISPOSABILITY_REACHABILITY_DIMENSIONS,
        "strand-census disposability/reachability dimension floor shrank: {dimensions} < \
         {MIN_CENSUS_DISPOSABILITY_REACHABILITY_DIMENSIONS}"
    );
}

#[test]
fn idle_disposability_is_parked_state_not_bare_tid_identity() {
    let scheduler = repo_text(SCHEDULER_PATH);
    let census = function_body(&scheduler, "collect_strand_census");
    let dormant_guard = braced_block_after(census, "let dormant_idle =");
    assert!(
        !code_occurrences(dormant_guard, "is_cpu_idle(").is_empty(),
        "idle disposability must consult the scheduler's parked-state predicate"
    );
    assert!(
        !code_occurrences(dormant_guard, "current_thread").is_empty()
            && !code_occurrences(dormant_guard, "current_tid != tid").is_empty(),
        "idle disposability must recognize a dormant idle thread while another thread runs"
    );
    assert!(
        code_occurrences(census, ".any(|cpu| cpu.idle_thread == tid)").is_empty(),
        "the census must not restore the bare idle-TID identity skip"
    );
}

#[test]
fn strand_marker_reports_dwell_and_nonprogress_axes() {
    let oracle = repo_text(STRAND_ORACLE_PATH);
    let report = function_body(&oracle, "report_strand");
    assert!(
        report.contains(":worst_dwell_ms={}:overflow={}:worst_nonprogress_ms={}]"),
        "strand marker must carry both dwell and nonprogress axes"
    );
}

#[test]
fn census_widen_injection_has_two_legs_and_is_boot_tests_only() {
    let scheduler = repo_text(SCHEDULER_PATH);
    let registry = repo_text(TEST_REGISTRY_PATH);
    let probe = function_body(&registry, "run_census_widen_oracle");
    let census_calls = code_occurrences(probe, "collect_strand_census(");
    let arm_calls = code_occurrences(probe, "let armed_once = arm_census_widen_injection(");
    let disarm_calls = code_occurrences(probe, "disarm_census_widen_injection(");
    assert!(
        census_calls.len() >= 2
            && !arm_calls.is_empty()
            && disarm_calls.len() >= 2
            && census_calls[0] < arm_calls[0]
            && arm_calls[0] < census_calls[1]
            && census_calls[1] < disarm_calls[disarm_calls.len() - 1],
        "census widening oracle must retain disarmed-baseline and armed census legs: \
         census={census_calls:?} arm={arm_calls:?} disarm={disarm_calls:?}"
    );
    assert!(
        probe.contains("baseline_reported={}:armed_reported={}"),
        "census widening marker must report both anti-vacuity legs"
    );
    assert!(
        scheduler.contains("#[cfg(feature = \"boot_tests\")]\nstatic CENSUS_WIDEN_INJECT_TID")
            && scheduler
                .contains("#[cfg(feature = \"boot_tests\")]\npub fn arm_census_widen_injection"),
        "census widening arming state and entry point must compile out of production builds"
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

#[test]
fn wakeup_placement_requires_local_dispatchability() {
    let scheduler = repo_text(SCHEDULER_PATH);
    let context_switch = repo_text(CONTEXT_SWITCH_PATH);

    let accepts = function_body(&scheduler, "cpu_accepts_wakeups");
    let compact_accepts: String = code_source(accepts)
        .0
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(
        compact_accepts.contains(
            "ifcpu==Self::current_cpu_id()&&arch_can_dispatch_here(){returntrue;}"
        ),
        "the current CPU fast path must be conditional on architecture dispatchability"
    );
    assert_eq!(
        code_occurrences(accepts, "arch_can_dispatch_here").len(),
        1,
        "cpu_accepts_wakeups must consult dispatchability exactly once"
    );
    assert_eq!(
        code_occurrences(accepts, "return true").len(),
        1,
        "cpu_accepts_wakeups must not regain an unconditional current-CPU return"
    );
    assert_eq!(
        code_occurrences(accepts, "cpu_dispatch_stale").len(),
        1,
        "a non-dispatchable current CPU must fall through to peer-style staleness"
    );

    let stale = function_body(&scheduler, "cpu_dispatch_stale");
    assert!(
        code_occurrences(stale, "current_cpu_id").is_empty(),
        "the outside-CPU staleness predicate must not identify the observer CPU"
    );
    assert_eq!(
        code_occurrences(stale, "self.cpu_state[cpu].last_schedule_ticks").len(),
        1,
        "cpu_dispatch_stale must read the target CPU's scheduling timestamp"
    );
    assert_eq!(
        code_occurrences(stale, "wrapping_sub(last_schedule_ticks)").len(),
        1,
        "cpu_dispatch_stale must compare elapsed ticks with wrapping arithmetic"
    );

    let dispatch_bodies = function_bodies(&scheduler, "arch_can_dispatch_here");
    assert_eq!(
        dispatch_bodies.len(),
        2,
        "arch_can_dispatch_here must have aarch64 and non-aarch64 definitions"
    );
    let arm_body = dispatch_bodies
        .iter()
        .find(|body| !code_occurrences(body, "context_switch::can_dispatch_here").is_empty())
        .expect("aarch64 dispatchability delegates to context_switch");
    assert!(
        code_occurrences(arm_body, "PREEMPT_GUARD_MASK").is_empty()
            && !arm_body.contains("0x"),
        "the scheduler wrapper must not duplicate the aarch64 preemption mask"
    );
    let non_arm_bodies = dispatch_bodies
        .iter()
        .filter(|body| {
            let (code, _) = code_source(body);
            code.split_whitespace().collect::<String>() == "true"
        })
        .count();
    assert_eq!(
        non_arm_bodies, 1,
        "the non-aarch64 dispatchability predicate must preserve true"
    );

    let compact_scheduler: String = scheduler
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(
        compact_scheduler.contains(
            "#[cfg(not(target_arch=\"aarch64\"))]#[inline(always)]fnarch_can_dispatch_here()->bool{true}"
        ),
        "the true arch_can_dispatch_here body must be the non-aarch64 arm"
    );

    let context_predicate = function_body(&context_switch, "can_dispatch_here");
    assert_eq!(
        code_occurrences(context_predicate, "PREEMPT_GUARD_MASK").len(),
        1,
        "aarch64 dispatchability must derive from the shared guard mask"
    );
    assert_eq!(
        code_occurrences(context_predicate, "preempt_count").len(),
        1,
        "aarch64 dispatchability must be a pure per-CPU preemption-count read"
    );
    let compact_context: String = context_switch
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(
        compact_context.contains("#[inline(always)]pubfncan_dispatch_here()->bool"),
        "the shared aarch64 predicate must remain public and always-inline"
    );
}
