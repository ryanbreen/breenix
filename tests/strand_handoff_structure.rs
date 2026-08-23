use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const SCHEDULER_PATH: &str = "kernel/src/task/scheduler.rs";
const CONTEXT_SWITCH_PATH: &str = "kernel/src/arch_impl/aarch64/context_switch.rs";
const STRAND_ORACLE_PATH: &str = "kernel/src/task/strand_oracle.rs";
const RET_ZERO_PC_ORACLE_PATH: &str = "kernel/src/task/ret_zero_pc_oracle.rs";
const EXECUTOR_PATH: &str = "kernel/src/test_framework/executor.rs";
const TEST_REGISTRY_PATH: &str = "kernel/src/test_framework/registry.rs";
const X86_BOOT_GATE_PATH: &str = "docker/qemu/run-x86-boot-tests.sh";
const SERVICE_SEQUENCE_GATE_PATH: &str = "docker/qemu/run-aarch64-service-sequence-gate.sh";
const STRICT_GATE_PATH: &str = "docker/qemu/run-aarch64-boot-test-strict.sh";
const FULL_TEST_PATH: &str = "docker/qemu/run-aarch64-full-test.sh";
const MIN_REQUEUE_EARLY_RETURN_GUARDS: usize = 6;
/// New instruction-abort refusal reasons are welcome; losing one is not.
const MIN_INSTRUCTION_ABORT_REFUSAL_REASONS: usize = 3;
/// #576 was the sole named instruction-abort signature before #626 attribution.
const PRIOR_NAMED_INSTRUCTION_ABORT_ARMS: usize = 1;
/// Additional #609 clauses are welcome; dropping one is how a tolerated bucket starts absorbing unfiled failures.
const MIN_609_SIGNATURE_GUARDS: usize = 5;
/// GREEN alone. #635 was the second entry until its producer was repaired at
/// source on this branch and its tolerance was removed; the service-sequence
/// gate now has no non-failing bucket other than a healthy boot.
const EXPECTED_NON_FAILING_SERVICE_SEQUENCE_BUCKETS: usize = 1;
/// Each gate must reject both strand marker families; additional rejections are welcome.
const MIN_STRANDED_FORBIDDEN_REJECTIONS: usize = 2;
/// More discriminating markers are welcome; dropping one quietly disarms the profile guard.
const MIN_BOOT_TESTS_PROFILE_MARKERS: usize = 6;
/// The strict gate scores once while polling and again for the final verdict.
const MIN_STRICT_GATE_SCORE_SERIAL_CALLS: usize = 2;
/// Dormancy currently has two axes and ordinary reachability has five.
const MIN_CENSUS_DISPOSABILITY_REACHABILITY_DIMENSIONS: usize = 7;
/// Running/queued nonprogress and scheduler-silence fields are all independent
/// evidence. New axes are welcome, but this derived field census may not shrink.
const MIN_CENSUS_PROGRESS_AXES: usize = 6;
/// Complete-marker patterns plus direct classifier greps across both marker
/// families. This is a shape floor, not a script-name allowlist.
const MIN_ORACLE_GATE_PATTERNS: usize = 8;

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

fn struct_body<'a>(source: &'a str, name: &str) -> &'a str {
    let (masked, mask) = code_source(source);
    let name_offset = identifier_offsets(&masked, &mask, name)
        .into_iter()
        .find(|offset| {
            masked[..*offset]
                .rsplit_once("struct")
                .is_some_and(|(_, suffix)| suffix.trim().is_empty())
        })
        .unwrap_or_else(|| panic!("struct {name} not found"));
    let open = masked[name_offset + name.len()..]
        .find('{')
        .map(|offset| name_offset + name.len() + offset)
        .expect("struct opening brace");
    braced_body(source, &masked, open)
}

fn declared_public_fields<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    struct_body(source, name)
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub ")
                .and_then(|field| field.split_once(':').map(|(name, _)| name.trim()))
        })
        .collect()
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

fn preceding_nonempty_line(source: &str, offset: usize) -> &str {
    let line_start = source[..offset].rfind('\n').map_or(0, |index| index + 1);
    source[..line_start]
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .expect("source line before identifier")
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

fn rust_source_tree(relative: &str) -> String {
    fn visit(path: &std::path::Path, out: &mut String) {
        let mut entries: Vec<_> = fs::read_dir(path)
            .unwrap_or_else(|_| panic!("read source directory {}", path.display()))
            .map(|entry| entry.expect("source directory entry").path())
            .collect();
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                visit(&entry, out);
            } else if entry.extension().is_some_and(|extension| extension == "rs") {
                out.push_str(
                    &fs::read_to_string(&entry)
                        .unwrap_or_else(|_| panic!("read source file {}", entry.display())),
                );
                out.push('\n');
            }
        }
    }

    let mut source = String::new();
    visit(&repo_root().join(relative), &mut source);
    source
}

fn marker_format<'a>(function: &'a str, family: &str) -> &'a str {
    let anchor = format!("\"[{family}:");
    function
        .lines()
        .find_map(|line| {
            let start = line.find(&anchor)? + 1;
            let tail = &line[start..];
            let end = tail.find("\",")?;
            Some(&tail[..end])
        })
        .unwrap_or_else(|| panic!("{family} emitted format string"))
}

fn render_positional_format(format: &str, values: &[&str]) -> String {
    let mut rendered = String::new();
    let mut remainder = format;
    for value in values {
        let (before, after) = remainder
            .split_once("{}")
            .expect("format placeholder for structural marker sample");
        rendered.push_str(before);
        rendered.push_str(value);
        remainder = after;
    }
    assert!(
        !remainder.contains("{}"),
        "structural marker sample omitted format arguments"
    );
    rendered.push_str(remainder);
    rendered
}

fn shell_ere_matches(pattern: &str, marker: &str) -> bool {
    let mut child = Command::new("grep")
        .args(["-Eq", pattern])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn grep for gate-pattern structural check");
    child
        .stdin
        .take()
        .expect("grep stdin")
        .write_all(marker.as_bytes())
        .expect("write emitted marker sample");
    child.wait().expect("wait for grep").success()
}

fn quoted_segment_containing<'a>(line: &'a str, needle: &str) -> Option<&'a str> {
    for quote in ['\'', '"'] {
        if let Some(segment) = line
            .split(quote)
            .skip(1)
            .step_by(2)
            .find(|segment| segment.contains(needle))
        {
            return Some(segment);
        }
    }
    None
}

fn registered_test_location(source: &str, test_name: &str) -> (String, String, usize) {
    let registration = format!("name: \"{test_name}\"");
    let registrations: Vec<_> = source.match_indices(&registration).collect();
    assert_eq!(
        registrations.len(),
        1,
        "registered test {test_name} must have exactly one TestDef"
    );
    let registration_offset = registrations[0].0;
    let declaration_offset = source[..registration_offset]
        .rfind("static ")
        .unwrap_or_else(|| panic!("static test array for {test_name}"));
    let declaration = &source[declaration_offset + "static ".len()..];
    let array_name = declaration
        .split_once(':')
        .map(|(name, _)| name.trim())
        .unwrap_or_else(|| panic!("static test array name for {test_name}"));

    let subsystem_block = source
        .split("Subsystem {")
        .skip(1)
        .find(|block| block.contains(&format!("tests: {array_name},")))
        .unwrap_or_else(|| panic!("subsystem block for {array_name}"));
    let subsystem_id = subsystem_block
        .lines()
        .find_map(|line| line.trim().strip_prefix("id: SubsystemId::"))
        .and_then(|tail| tail.strip_suffix(','))
        .unwrap_or_else(|| panic!("subsystem id for {array_name}"));

    (
        array_name.to_string(),
        subsystem_id.to_string(),
        registration_offset,
    )
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
    let named_signature_prefix = r#"elif [ "$instruction_abort_signature" = ""#;
    let named_signature_suffix = r#"" ]; then"#;
    let named_signature_branches: Vec<_> = arm_lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            line.strip_prefix(named_signature_prefix)
                .and_then(|signature| signature.strip_suffix(named_signature_suffix))
                .map(|signature| (index, signature))
        })
        .collect();
    assert_eq!(
        named_signature_branches.len(),
        PRIOR_NAMED_INSTRUCTION_ABORT_ARMS + 1,
        "the instruction-abort classifier must add exactly one named field-signature arm"
    );
    let mut named_buckets = HashSet::new();
    for (branch_index, signature) in named_signature_branches {
        let fields: Vec<_> = signature.split_ascii_whitespace().collect();
        assert_eq!(
            fields.len(),
            3,
            "named instruction-abort signatures must carry FAR/ELR/ESR fields"
        );
        assert!(
            fields.iter().all(|field| {
                field.strip_prefix("0x").is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            }),
            "named instruction-abort signature fields must be exact hexadecimal values: {signature}"
        );
        let bucket = arm_lines
            .get(branch_index + 1)
            .and_then(|line| line.strip_prefix("CLASS_BUCKET=\"")?.strip_suffix('"'))
            .expect("field-exact instruction-abort arm must assign one bucket directly");
        assert_ne!(
            bucket, "UNATTRIBUTED",
            "field-exact instruction-abort arms must be named"
        );
        assert!(
            named_buckets.insert(bucket),
            "field-exact instruction-abort arms must name distinct buckets"
        );
    }
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
        "service-sequence gate must have exactly one named CLASS_BUCKET=\"576\" assignment"
    );
    assert_eq!(
        shell_exact_line_occurrences(&gate, r#"CLASS_BUCKET="626""#),
        1,
        "service-sequence gate must have exactly one named CLASS_BUCKET=\"626\" assignment"
    );
}

#[test]
fn service_sequence_pc_align_and_panic_classifiers_are_field_keyed_before_fallbacks() {
    let gate = repo_text(SERVICE_SEQUENCE_GATE_PATH);

    let pc_align_signatures = shell_function_body(&gate, "pc_align_signatures");
    assert!(
        pc_align_signatures.contains(r"\[PC_ALIGN\] ELR=0x")
            && pc_align_signatures.contains("FAR=0x")
            && pc_align_signatures.contains("from_el0=[01]")
            && pc_align_signatures.contains("} | sort -u")
            && !pc_align_signatures.contains("head -1"),
        "PC_ALIGN signatures must collect and deduplicate every readable ELR/FAR/from_el0 triple"
    );

    let panic_signatures = shell_function_body(&gate, "kernel_panic_signatures");
    assert!(
        panic_signatures.contains("panicked at ")
            && panic_signatures.contains("location")
            && panic_signatures.contains("message")
            && panic_signatures.contains("sort -u"),
        "kernel panic signatures must retain the verbatim panic location and message"
    );

    let classifier = shell_function_body(&gate, "classify_serial");
    let ctx596_offset = classifier
        .find(r#"if grep -qF "[CTX596_ORACLE:FAIL""#)
        .expect("#596 classifier arm");
    let last_existing_bucket_offset = classifier
        .find("[BOOT_TESTS:FAIL")
        .expect("last pre-existing named-bucket arm");
    let pc_align_offset = classifier
        .find(r#"if grep -qF "[PC_ALIGN]""#)
        .expect("PC_ALIGN classifier arm");
    let panic_offset = classifier
        .find(r#"if grep -qF "KERNEL PANIC""#)
        .expect("kernel panic classifier arm");
    let arm_609_offset = classifier
        .find(r#"is_609_early_boot_stage_stall "$serial_file""#)
        .expect("#609 classifier arm");
    let generic_offsets = [
        classifier
            .find(r#"if ! grep -qF "[BLOCK_EINTR_ORACLE:""#)
            .expect("generic oracle-marker arm"),
        classifier
            .find(r#"if ! grep -qF "[CTX596_ORACLE:ARMED""#)
            .expect("generic #596 anti-vacuity arm"),
        classifier
            .find(r#"last_line=$(grep -vF "[heartbeat]""#)
            .expect("generic last-line arm"),
    ];
    assert!(
        ctx596_offset < last_existing_bucket_offset
            && last_existing_bucket_offset < pc_align_offset
            && pc_align_offset < panic_offset
            && panic_offset < arm_609_offset
            && generic_offsets.iter().all(|offset| panic_offset < *offset),
        "PC_ALIGN and kernel panic field attribution must follow every existing named bucket, preserve PC_ALIGN precedence, and precede #609 plus every generic arm"
    );

    let pc_align_arm = &classifier[pc_align_offset..panic_offset];
    let pc_align_buckets: Vec<_> = pc_align_arm
        .lines()
        .filter(|line| line.trim().starts_with("CLASS_BUCKET="))
        .collect();
    let pc_align_reasons = pc_align_arm
        .lines()
        .filter(|line| line.trim().starts_with("CLASS_REASON="))
        .count();
    assert!(
        pc_align_arm.contains("pc_align_signatures \"$serial_file\"")
            && pc_align_arm.contains(r#"[ "$pc_align_variants" -gt 1 ]"#)
            && pc_align_arm.contains("#625")
            && pc_align_arm.contains("ELR=0x4b5 FAR=0x5 from_el0=1")
            && !pc_align_buckets.is_empty()
            && pc_align_buckets.len() == pc_align_reasons
            && pc_align_buckets
                .iter()
                .all(|line| line.trim() == r#"CLASS_BUCKET="UNATTRIBUTED""#),
        "PC_ALIGN must require one field signature, name filed #625 exactly, and remain hard-failing"
    );

    let panic_arm = &classifier[panic_offset..arm_609_offset];
    let panic_buckets: Vec<_> = panic_arm
        .lines()
        .filter(|line| line.trim().starts_with("CLASS_BUCKET="))
        .collect();
    let panic_reasons = panic_arm
        .lines()
        .filter(|line| line.trim().starts_with("CLASS_REASON="))
        .count();
    assert!(
        panic_arm.contains("kernel_panic_signatures \"$serial_file\"")
            && panic_arm.contains("$kernel_panic_signature")
            && panic_arm.contains("location=${kernel_panic_location:-<missing>}")
            && panic_arm.contains("message=${kernel_panic_message:-<missing>}")
            && panic_arm.contains("${kernel_panic_marker:-<missing>}")
            && !panic_buckets.is_empty()
            && panic_buckets.len() == panic_reasons
            && panic_buckets
                .iter()
                .all(|line| line.trim() == r#"CLASS_BUCKET="UNATTRIBUTED""#),
        "kernel panics must report their location/message fields (or the readable marker) and remain hard-failing"
    );
}

/// #609 is a hard-failing EarlyBoot-stage stall. Its detector is keyed to the
/// stage boundary so a wedge cannot evade attribution by stalling before one
/// arbitrarily selected subsystem marker.
#[test]
fn service_sequence_609_arm_is_field_keyed_and_untolerated() {
    let gate = repo_text(SERVICE_SEQUENCE_GATE_PATH);
    let signature = shell_function_body(&gate, "is_609_early_boot_stage_stall");
    assert!(
        !signature.trim().is_empty(),
        "is_609_early_boot_stage_stall body census"
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
        signature.contains("[STAGE:early:ADVANCE]"),
        "#609 signature must require the EarlyBoot stage boundary to advance"
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
    assert!(
        signature.contains("[TESTS_COMPLETE:"),
        "#609 signature must reject boots that completed their tests"
    );
    assert!(
        !signature.contains("SUBSYSTEM:memory:") && !signature.contains(":network:"),
        "#609 attribution must be stage-shaped, not subsystem-shaped"
    );

    assert_eq!(
        shell_exact_line_occurrences(&gate, r#"CLASS_BUCKET="609""#),
        1,
        "service-sequence gate must have exactly one named CLASS_BUCKET=\"609\" assignment"
    );

    let classifier = shell_function_body(&gate, "classify_serial");
    let arm_609_offset = classifier
        .find(r#"is_609_early_boot_stage_stall "$serial_file""#)
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
    let census_sum = run_profile
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("census_sum=$(("))
        .expect("run_profile bucket-census sum identity");
    for (_, counter) in &bucket_counters {
        assert!(
            census_sum.contains(*counter),
            "classifier counter {counter} must remain in the per-profile bucket-census sum identity"
        );
    }
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
    for required_failing_bucket in ["576", "626"] {
        let counter = bucket_counters
            .iter()
            .find_map(|(bucket, counter)| {
                (*bucket == required_failing_bucket).then_some(*counter)
            })
            .unwrap_or_else(|| {
                panic!(
                    "required instruction-abort bucket {required_failing_bucket} must map to a count_* counter"
                )
            });
        let failing_term = format!(r#"[ "${counter}" -ne 0 ]"#);
        assert!(
            fail_condition.contains(&failing_term),
            "instruction-abort bucket {required_failing_bucket} counter {counter} must remain in run_profile's per-profile FAIL condition"
        );
    }
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
        EXPECTED_NON_FAILING_SERVICE_SEQUENCE_BUCKETS,
        "service-sequence non-failing bucket census changed to {non_failing_buckets:?}; a new non-failing bucket is a new tolerance and needs a coordinator ruling"
    );
    assert!(
        non_failing_buckets.contains("GREEN"),
        "the healthy GREEN result must remain non-failing"
    );
    assert!(
        failing_buckets.contains("635"),
        "the 635 bucket keeps its field-keyed attribution but lost its non-failing exemption when its producer was repaired; it must gate like every other named bucket"
    );
    assert!(
        failing_buckets.contains(bucket_609),
        "#609 bucket {bucket_609} must be in the failing bucket census"
    );
    assert!(
        !non_failing_buckets.contains(bucket_609),
        "#609 bucket {bucket_609} must not be in the non-failing bucket census"
    );

    let boot_test_fail_offset = classifier
        .find("[BOOT_TESTS:FAIL")
        .expect("aggregate boot-test failure classifier arm");
    let boot_test_fail_tail = &classifier[boot_test_fail_offset..];
    let boot_test_fail_end = boot_test_fail_tail
        .find("\n    fi\n")
        .map(|offset| offset + "\n    fi\n".len())
        .expect("aggregate boot-test failure arm terminator");
    let boot_test_fail_arm = &boot_test_fail_tail[..boot_test_fail_end];
    assert!(
        boot_test_fail_arm.contains(r#"CLASS_BUCKET="BOOT_TEST_FAIL""#)
            && boot_test_fail_arm.contains("boot_test_fail_line"),
        "service-sequence boot-test failures must have a named bucket and carry the first failing test field signature"
    );
    let prior_oracle_failure = classifier
        .find(r"\[CENSUS_WIDEN_ORACLE:[^]]*:FAIL\]")
        .expect("census-oracle failure attribution arm");
    assert!(
        prior_oracle_failure < boot_test_fail_offset && boot_test_fail_offset < arm_609_offset,
        "BOOT_TEST_FAIL must follow crash/oracle attribution and precede later generic/stall classification"
    );
    assert!(
        failing_buckets.contains("BOOT_TEST_FAIL"),
        "BOOT_TEST_FAIL must remain a hard-failing per-profile bucket"
    );
}

#[test]
fn service_sequence_ret_dispatch_refusals_are_counted_and_reported_not_gated() {
    let gate = repo_text(SERVICE_SEQUENCE_GATE_PATH);
    let run_profile = shell_function_body(&gate, "run_profile");
    let refusal_count_lines: Vec<_> = run_profile
        .lines()
        .map(str::trim)
        .filter(|line| line.contains(r#"grep -cF "[RET_DISPATCH_REFUSED:""#))
        .collect();
    assert_eq!(
        refusal_count_lines.len(),
        1,
        "run_profile must count RET_DISPATCH_REFUSED marker lines exactly once per boot"
    );
    let boot_counter = refusal_count_lines[0]
        .split_once('=')
        .map(|(counter, _)| counter)
        .expect("per-boot refusal counter assignment");

    let profile_accumulators: Vec<_> = run_profile
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            let (counter, expression) = line.split_once("=$((")?;
            expression
                .contains(&format!("+ {boot_counter}"))
                .then_some(counter)
        })
        .collect();
    assert_eq!(
        profile_accumulators.len(),
        1,
        "the per-boot refusal count must feed exactly one per-profile line accumulator"
    );
    let profile_line_counter = profile_accumulators[0];

    let refusal_boot_conditions: Vec<_> = run_profile
        .lines()
        .map(str::trim)
        .enumerate()
        .filter(|(_, line)| {
            *line == format!(r#"if [ "${boot_counter}" -ne 0 ]; then"#)
        })
        .collect();
    assert_eq!(
        refusal_boot_conditions.len(),
        1,
        "the refusal census must count boots with one or more marker lines"
    );
    let profile_boot_counter = run_profile
        .lines()
        .map(str::trim)
        .nth(refusal_boot_conditions[0].0 + 1)
        .and_then(|line| {
            let (counter, expression) = line.split_once("=$((")?;
            expression
                .contains(&format!("{counter} + 1"))
                .then_some(counter)
        })
        .expect("per-profile refusal-boot accumulator");

    let boot_reports: Vec<_> = run_profile
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("ret_dispatch_refusals=$"))
        .collect();
    assert_eq!(
        boot_reports.len(),
        1,
        "the per-boot report must print the refusal-line count exactly once"
    );
    assert!(
        boot_reports[0].contains(&format!("${boot_counter}")),
        "the per-boot refusal report must print the derived marker counter"
    );

    let census_header = run_profile
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("printf 'boot\\t"))
        .expect("per-boot census header");
    assert!(
        census_header.contains("ret_dispatch_refusals"),
        "the per-boot census artifact must retain the refusal count"
    );

    let print_census = shell_function_body(&gate, "print_census");
    let profile_reports: Vec<_> = print_census
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("RET dispatch refused:"))
        .collect();
    assert_eq!(
        profile_reports.len(),
        1,
        "print_census must report the ret-dispatch refusal census exactly once"
    );
    assert!(
        profile_reports[0].contains(&format!("${profile_line_counter}"))
            && profile_reports[0].contains(&format!("${profile_boot_counter}/$count_boots"))
            && profile_reports[0].contains("marker line(s) across")
            && profile_reports[0].contains("reported, not gated"),
        "the profile refusal census must print the derived line accumulator with reported-only framing"
    );
    let census_echoes: Vec<_> = print_census
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("echo "))
        .collect();
    let divergence_report = census_echoes
        .iter()
        .position(|line| line.contains("CTX596 divergence:"))
        .expect("CTX596 divergence census output");
    let refusal_report = census_echoes
        .iter()
        .position(|line| line.contains("RET dispatch refused:"))
        .expect("ret-dispatch refusal census output");
    assert_eq!(
        refusal_report,
        divergence_report + 1,
        "ret-dispatch refusals must print immediately after CTX596 divergence"
    );

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
    // Exact `"$name"` terms, not substrings. The blanket `contains("refusal")`
    // catch-all this replaces cannot survive the resume-PC refusal becoming a
    // gate failure: `refusal_lines` is a substring of `resume_pc_refusal_lines`,
    // so a substring test can no longer tell the two families apart and would
    // read the intended tightening as a ret-dispatch regression. The three
    // counters below are still derived from the script by following the data
    // flow out of the `grep -cF "[RET_DISPATCH_REFUSED:"` line, so an alias is
    // followed rather than evaded, and the shell always writes these terms in
    // the quoted `[ "$name" -ne 0 ]` form the checks below require.
    for counter in [boot_counter, profile_line_counter, profile_boot_counter] {
        assert!(
            !fail_conditions[0].contains(&format!("\"${counter}\"")),
            "ret-dispatch refusal counter {counter} must appear in no per-profile FAIL condition"
        );
    }
    assert!(
        !fail_conditions[0].contains("RET_DISPATCH_REFUSED"),
        "ret-dispatch refusal observations must appear in no per-profile FAIL condition"
    );
}

/// The resume-PC refusal record is emitted only by production dispatch in this
/// gate's feature profile, so a non-zero count is a defect and must fail the
/// profile rather than be watched.
#[test]
fn service_sequence_resume_pc_refusals_fail_the_profile() {
    let gate = repo_text(SERVICE_SEQUENCE_GATE_PATH);
    let run_profile = shell_function_body(&gate, "run_profile");

    let refusal_count_line = run_profile
        .lines()
        .map(str::trim)
        .find(|line| line.contains(r#"grep -cF "[RESUME_PC_REFUSED:""#))
        .expect("per-boot resume-PC refusal count");
    let boot_counter = refusal_count_line
        .split_once('=')
        .map(|(counter, _)| counter)
        .expect("per-boot resume-PC refusal counter assignment");
    let profile_line_counter = run_profile
        .lines()
        .map(str::trim)
        .find_map(|line| {
            let (counter, expression) = line.split_once("=$((")?;
            expression
                .contains(&format!("+ {boot_counter}"))
                .then_some(counter)
        })
        .expect("per-profile resume-PC refusal line accumulator");

    let fail_conditions: Vec<_> = run_profile
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(r#"if [ "$count_"#) && line.ends_with("; then"))
        .collect();
    assert_eq!(
        fail_conditions.len(),
        1,
        "run_profile must retain exactly one per-profile FAIL condition"
    );
    assert!(
        fail_conditions[0].contains(&format!(r#"[ "${profile_line_counter}" -ne 0 ]"#)),
        "a non-zero production resume-PC refusal count must fail the profile"
    );

    let print_census = shell_function_body(&gate, "print_census");
    let resume_reports: Vec<_> = print_census
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("Resume PC refused:"))
        .collect();
    assert_eq!(
        resume_reports.len(),
        1,
        "print_census must report the resume-PC refusal census exactly once"
    );
    assert!(
        !resume_reports[0].contains("reported, not gated"),
        "the resume-PC refusal census must not describe itself as ungated once it gates"
    );
    assert!(
        resume_reports[0].contains(&format!("${profile_line_counter}")),
        "the resume-PC refusal census must print the derived line accumulator"
    );
}

/// #635 keeps its field-keyed classifier arm — attribution by FAR/ELR/ESR is
/// what stops the shape falling into UNATTRIBUTED — while gating like every
/// other named bucket. A catch-all arm would be a different thing entirely.
#[test]
fn service_sequence_635_arm_is_field_keyed_and_untolerated() {
    let gate = repo_text(SERVICE_SEQUENCE_GATE_PATH);
    let classifier = shell_function_body(&gate, "classify_serial");
    let bucket_offset = classifier
        .find(r#"CLASS_BUCKET="635""#)
        .expect("#635 classifier arm");
    let arm = &classifier[..bucket_offset];
    let guard = arm
        .rfind("elif ")
        .map(|offset| &arm[offset..])
        .expect("#635 arm guard");

    for term in [
        r#"[ "$instruction_abort_far" = "$instruction_abort_elr" ]"#,
        r#"[ "$instruction_abort_far" != "0x0" ]"#,
        r#"[ "$instruction_abort_esr" = "0x8600000e" ]"#,
        r#"^0xffff[0-9a-f]+$"#,
    ] {
        assert!(
            guard.contains(term),
            "#635 must stay keyed to its field signature; lost guard term {term}"
        );
    }

    let run_profile = shell_function_body(&gate, "run_profile");
    let fail_condition = run_profile
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(r#"if [ "$count_"#) && line.ends_with("; then"))
        .expect("per-profile FAIL condition");
    assert!(
        fail_condition.contains(r#"[ "$count_635" -ne 0 ]"#),
        "#635 must fail the profile it occurred in"
    );
    for stale in ["ATTRIBUTED, non-failing", "never gating", "non-failing]"] {
        assert!(
            !gate.contains(stale),
            "the service-sequence gate still describes a removed tolerance: {stale}"
        );
    }
}

#[test]
fn service_sequence_resume_pc_refusals_are_counted_exactly_once_per_boot() {
    let gate = repo_text(SERVICE_SEQUENCE_GATE_PATH);
    let run_profile = shell_function_body(&gate, "run_profile");
    let refusal_count_lines: Vec<_> = run_profile
        .lines()
        .map(str::trim)
        .filter(|line| line.contains(r#"grep -cF "[RESUME_PC_REFUSED:""#))
        .collect();
    assert_eq!(
        refusal_count_lines.len(),
        1,
        "run_profile must count RESUME_PC_REFUSED marker lines exactly once per boot"
    );
}

#[test]
fn strict_gate_rejects_aggregate_boot_test_failures_with_a_field_signature() {
    let gate = repo_text(STRICT_GATE_PATH);
    let score_serial = shell_function_body(&gate, "score_serial");
    let aggregate_fail = score_serial
        .find("[BOOT_TESTS:FAIL")
        .expect("strict scorer BOOT_TESTS failure arm");
    let first_presence_check = score_serial
        .find("if ! grep -qE \"(breenix>")
        .expect("strict scorer first presence check");
    assert!(
        aggregate_fail < first_presence_check
            && score_serial.contains(r"\[TESTS_COMPLETE:[^]]*:FAILED:[1-9][0-9]*\]")
            && score_serial.contains(r"\[TEST:[^]]*:FAIL:[^]]*\]")
            && score_serial.contains("Boot test failure: ${boot_test_fail_line"),
        "strict scoring must reject either aggregate failure marker before presence checks and report the first failing test field signature"
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
fn injection_stimulus_default_is_idle_gated_and_live_widening_is_cfg_only() {
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

    let default_cfg = "#[cfg(not(feature = \"strand_inject_live_outgoing\"))]";
    let widened_cfg = "#[cfg(feature = \"strand_inject_live_outgoing\")]";
    let default_arms: Vec<_> = trampoline.match_indices(default_cfg).map(|(at, _)| at).collect();
    let widened_arms: Vec<_> = trampoline.match_indices(widened_cfg).map(|(at, _)| at).collect();
    assert_eq!(
        default_arms.len() + widened_arms.len(),
        2,
        "the live-outgoing stimulus must have exactly two cfg-guarded shapes"
    );
    assert_eq!(
        default_arms.len(),
        1,
        "the feature-off idle-only stimulus shape must be unique"
    );
    assert_eq!(
        widened_arms.len(),
        1,
        "the feature-on live-outgoing stimulus shape must be unique"
    );
    assert!(
        default_arms[0] < widened_arms[0],
        "the unchanged default stimulus shape must precede its widened cfg peer"
    );

    let default_arm = &trampoline[default_arms[0]..widened_arms[0]];
    let widened_arm = &trampoline[widened_arms[0]..];
    let injection_guard = braced_block_after(default_arm, "if idle_id == old_id");
    assert!(
        !injection_guard.trim().is_empty(),
        "default idle-outgoing injection guard body census"
    );
    assert!(
        !code_occurrences(injection_guard, "inject_if_armed(").is_empty(),
        "the default inject_if_armed call must remain inside the idle-outgoing guard"
    );
    assert_eq!(
        code_occurrences(default_arm, "inject_if_armed(").len(),
        1,
        "the default cfg arm must contain exactly one injection call"
    );
    assert_eq!(
        code_occurrences(widened_arm, "inject_if_armed(").len(),
        1,
        "the live-outgoing cfg arm must contain exactly one injection call"
    );
    assert!(
        code_occurrences(widened_arm, "idle_id == old_id").is_empty(),
        "only the explicit live-outgoing cfg arm may omit the idle comparison"
    );
    assert_eq!(
        code_occurrences(widened_arm, "is_strand_live_driver(old_id)").len(),
        1,
        "the widened arm must drop only the dedicated no-wakeup driver"
    );
    let widened_fired = braced_block_after(widened_arm, "if live_outgoing_injected");
    let fired_notes = code_occurrences(widened_fired, "note_live_outgoing_fired(");
    let recovery_owner_clears = code_occurrences(
        widened_fired,
        "cpu_state[cpu_id].previous_thread = None",
    );
    assert_eq!(
        fired_notes.len(),
        1,
        "the widened arm must record its one live-outgoing injection"
    );
    assert_eq!(
        recovery_owner_clears.len(),
        1,
        "the widened arm must remove the independent exception-cleanup recovery owner"
    );
    assert!(
        fired_notes[0] < recovery_owner_clears[0],
        "the widened arm may clear the recovery owner only after its injection fired"
    );

    let oracle = repo_text(RET_ZERO_PC_ORACLE_PATH);
    let driver = function_body(&oracle, "strand_live_driver");
    let publish = code_occurrences(driver, "LIVE_DRIVER_TID.store(");
    let schedules = code_occurrences(driver, "scheduler::schedule()");
    let sleeps = code_occurrences(driver, "strand_oracle::sleep_sample_period()");
    assert_eq!(publish.len(), 1, "the driver must publish its own TID once");
    assert_eq!(schedules.len(), 1, "the driver must have one runnable handoff site");
    assert_eq!(sleeps.len(), 1, "the driver must have one post-resume throttle");
    assert!(
        publish[0] < schedules[0] && schedules[0] < sleeps[0],
        "the driver must publish, schedule while runnable, then install its next timer only after resuming"
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
    let x86_leg = braced_block_after(probe, "#[cfg(not(target_arch");
    for required in [
        "collect_strand_census(&mut candidates, &mut baseline_nonprogress)",
        "collect_strand_census(&mut candidates, &mut confirmation_nonprogress)",
        "arm=none",
        "reason=uniprocessor_no_dispatching_peer",
        "baseline_reported={}",
        "axes={}",
        "SKIP",
    ] {
        assert!(
            x86_leg.contains(required),
            "the x86 census-only leg must retain {required}"
        );
    }
    for forbidden in [
        "stale_peer_cpu_for_test",
        "kthread_run_on_cpu_for_test",
        "release_cpu_affine_thread_for_test",
        "kthread_join",
    ] {
        assert!(
            code_occurrences(x86_leg, forbidden).is_empty(),
            "the unarmed x86 census leg must not contain {forbidden}"
        );
    }
    assert!(
        !x86_leg.contains(":PASS"),
        "the x86 census-only marker must never claim PASS"
    );
    assert_eq!(
        code_occurrences(x86_leg, "collect_strand_census(").len(),
        2,
        "the x86 leg must compute both disarmed census passes"
    );
    assert!(
        probe.contains("x86 computes both disarmed census passes")
            && probe.contains("aarch64's real-thread arm does")
            && probe.contains("not a passing result"),
        "the oracle must disclose that x86 does not prove the widening"
    );
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
    let literal = gate
        .lines()
        .find(|line| line.starts_with("CENSUS_WIDEN_ORACLE_LITERAL="))
        .expect("x86 gate census-widening literal constant");
    assert_eq!(
        literal,
        "CENSUS_WIDEN_ORACLE_LITERAL='[CENSUS_WIDEN_ORACLE:x86:arm=none:reason=uniprocessor_no_dispatching_peer:baseline_reported=0:axes=6:SKIP]'",
        "the x86 gate must literally pin the disclosed non-PASS verdict"
    );
    assert!(
        gate.contains("&& grep -qF \"$CENSUS_WIDEN_ORACLE_LITERAL\""),
        "the x86 poll verdict must require the literal census SKIP marker"
    );
    let exact_count = gate
        .find("test \"$(grep -h -F -c \"$CENSUS_WIDEN_ORACLE_LITERAL\"")
        .expect("x86 gate exact census SKIP marker count");
    assert!(
        gate[exact_count..]
            .lines()
            .take(2)
            .any(|line| line.contains("-eq 1")),
        "the x86 final verdict must require exactly one disclosed census SKIP marker"
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
        report.contains(":worst_dwell_ms={}:overflow={}:worst_nonprogress_ms={}:nonprogress={}:queued_on_nondispatching_cpu={}:worst_queued_nondispatch_ms={}:worst_cpu_scheduler_silence_ms={}:worst_silence_cpu={}]"),
        "strand marker must append every census progress axis after the existing fields"
    );
}

#[test]
fn aarch64_census_widen_oracle_has_real_disarmed_and_armed_legs() {
    let scheduler = repo_text(SCHEDULER_PATH);
    let registry = repo_text(TEST_REGISTRY_PATH);
    let callable = function_body(&registry, "run_census_widen_oracle");
    let probe = braced_block_after(callable, "#[cfg(target_arch");
    let census_calls = code_occurrences(probe, "collect_strand_census(");
    let spawn_calls = code_occurrences(probe, "kthread_run_on_cpu_for_test(");
    let target_calls = code_occurrences(probe, "stale_peer_cpu_for_test()");
    let release_calls = code_occurrences(probe, "release_cpu_affine_thread_for_test(");
    assert!(
        census_calls.len() >= 2
            && spawn_calls.len() == 1
            && target_calls.len() == 1
            && release_calls.len() == 1
            && target_calls[0] < census_calls[0]
            && census_calls[0] < spawn_calls[0]
            && spawn_calls[0] < census_calls[1]
            && census_calls[1] < release_calls[0],
        "census widening oracle must retain target-selected baseline/spawn/armed/release ordering: \
         target={target_calls:?} census={census_calls:?} spawn={spawn_calls:?} release={release_calls:?}"
    );
    assert!(
        probe.contains("baseline_reported={}:armed_reported={}"),
        "census widening marker must report both anti-vacuity legs"
    );
    let spawn_end = probe[spawn_calls[0]..]
        .find("let mut tid")
        .map(|offset| spawn_calls[0] + offset)
        .expect("forced-placement call terminator");
    let spawn_call = &probe[spawn_calls[0]..spawn_end];
    assert!(
        code_occurrences(spawn_call, "arm_target").len() == 1,
        "forced placement must consume the scheduler-selected arm target"
    );
    assert!(
        probe.contains("baseline.checked == 0")
            && probe.contains("baseline.queued_on_nondispatching_cpu != 0")
            && probe.contains("baseline.worst_queued_nondispatch_ms != 0")
            && probe.contains("baseline_nonprogress[..baseline.nonprogress].contains(&tid)"),
        "the disarmed leg must fail closed on vacuity, dirty queued axes, or probe presence"
    );
    assert!(
        probe.contains("kthread_has_exited_for_test")
            && probe.contains("arch_halt()")
            && probe.contains("kthread_join"),
        "the real probe must dwell and join through bounded exit polling"
    );
    assert!(
        probe.contains("for _ in 0..CENSUS_WIDEN_RETIRE_ROUNDS")
            && probe.contains("reclaim_terminated_threads()")
            && probe.contains("nudge_retirement_grace_for_test()")
            && probe.contains("kernel_stack_pool_counters().slots_freed")
            && probe.contains(":joined={}:retired={}:{}]"),
        "the real probe must keep its bounded retirement attempt and report the observed evidence"
    );
    let passed_start = probe.find("let passed =").expect("oracle verdict expression");
    let passed_end = probe[passed_start..]
        .find("crate::serial_println!")
        .map(|offset| passed_start + offset)
        .expect("oracle marker after verdict expression");
    let passed_expression = &probe[passed_start..passed_end];
    assert!(
        passed_expression.contains("&& joined")
            && code_occurrences(passed_expression, "retired").is_empty(),
        "joined is controlled proof that the probe ran and exited; asynchronous retirement evidence must not gate PASS"
    );
    let retirement_nudge = function_body(&scheduler, "nudge_retirement_grace_for_test");
    assert!(
        retirement_nudge.contains("for cpu in 0..online.min(MAX_CPUS)")
            && retirement_nudge.contains("gic::send_sgi(SGI_RESCHEDULE as u8, cpu as u8)")
            && !retirement_nudge.contains("scheduler.send_resched_ipi()"),
        "the retirement grace nudge must reach every online peer directly; the idle-only scheduler wake helper can skip the probe's stale owner"
    );

    let kernel_sources = rust_source_tree("kernel/src");
    assert!(
        code_occurrences(&kernel_sources, "CENSUS_WIDEN_INJECT").is_empty()
            && code_occurrences(&kernel_sources, "arm_census_widen_injection").is_empty()
            && code_occurrences(&kernel_sources, "disarm_census_widen_injection").is_empty(),
        "no census view-injection state or API may exist anywhere in kernel/src"
    );

    let stale_target = function_body(&scheduler, "stale_peer_cpu_for_test");
    assert!(
        stale_target.contains("cpu != current_cpu")
            && stale_target.contains("scheduler.cpu_dispatch_stale(cpu)"),
        "the scheduler must choose a non-current stale CPU without a literal target"
    );
}

/// Both oracles observe global kernel-stack allocation/return counters. The census
/// probe must run last in their shared subsystem so its stack allocation starts
/// only after every earlier ownership-accounting window has closed.
#[test]
fn census_and_kernel_stack_ownership_oracles_share_a_subsystem_in_safe_order() {
    let registry = repo_text(TEST_REGISTRY_PATH);
    let (census_array, census_subsystem, census_offset) =
        registered_test_location(&registry, "census_widen_oracle");
    let (ownership_array, ownership_subsystem, ownership_offset) =
        registered_test_location(&registry, "kernel_stack_ownership_oracle");

    assert_eq!(
        census_subsystem, ownership_subsystem,
        "census and ownership oracles must derive to the same subsystem id"
    );
    assert_eq!(
        census_array, ownership_array,
        "one subsystem id must retain both oracle registrations in one sequential test array"
    );
    assert!(
        ownership_offset < census_offset,
        "kernel-stack ownership accounting must close before the census probe starts"
    );
    assert!(
        registry[census_offset..]
            .lines()
            .take(5)
            .any(|line| line.trim() == "arch: Arch::Aarch64,"),
        "the registered real-thread oracle must remain aarch64-only"
    );

    let array_declaration = format!("static {census_array}: &[TestDef] = &[");
    let array_start = registry
        .find(&array_declaration)
        .map(|offset| offset + array_declaration.len())
        .expect("derived shared test array declaration");
    let array_end = registry[array_start..]
        .find("\n];")
        .map(|offset| array_start + offset)
        .expect("derived shared test array terminator");
    let array_body = &registry[array_start..array_end];
    let registration_offsets: Vec<_> = array_body
        .match_indices("name: \"")
        .map(|(offset, _)| array_start + offset)
        .collect();
    assert_eq!(
        registration_offsets.last().copied(),
        Some(census_offset),
        "census oracle must remain the last registered test in its derived subsystem array"
    );
}

#[test]
fn census_real_thread_arm_and_forced_placement_helpers_are_aarch64_only() {
    const AARCH64_BOOT_TEST_CFG: &str =
        "#[cfg(all(target_arch = \"aarch64\", feature = \"boot_tests\"))]";

    let registry = repo_text(TEST_REGISTRY_PATH);
    let callable = function_body(&registry, "run_census_widen_oracle");
    let aarch64_leg = braced_block_after(callable, "#[cfg(target_arch");
    let x86_leg = braced_block_after(callable, "#[cfg(not(target_arch");
    for required in [
        "stale_peer_cpu_for_test()",
        "kthread_run_on_cpu_for_test(",
        "release_cpu_affine_thread_for_test(",
        "kthread_has_exited_for_test",
        "kthread_join",
        ":joined={}:retired={}:{}]",
    ] {
        assert!(
            aarch64_leg.contains(required),
            "the aarch64 real-thread arm must retain {required}"
        );
        assert!(
            code_occurrences(x86_leg, required).is_empty(),
            "the x86 census-only leg must not compile {required}"
        );
    }

    let scheduler = repo_text(SCHEDULER_PATH);
    for declaration in [
        "static BOOT_TEST_CPU_AFFINITY:",
        "fn retain_cpu_affine_test_thread(",
        "pub(crate) fn clear_cpu_affinity_for_test(",
        "fn add_thread_on_cpu_for_test(",
        "pub fn stale_peer_cpu_for_test(",
        "pub(crate) fn spawn_on_cpu_for_test(",
        "pub fn release_cpu_affine_thread_for_test(",
    ] {
        let offset = scheduler
            .find(declaration)
            .unwrap_or_else(|| panic!("forced-placement declaration {declaration}"));
        assert_eq!(
            preceding_nonempty_line(&scheduler, offset),
            AARCH64_BOOT_TEST_CFG,
            "{declaration} must be compiled only for aarch64 boot tests"
        );
    }
    for offset in code_occurrences(&scheduler, "retain_cpu_affine_test_thread(") {
        assert_eq!(
            preceding_nonempty_line(&scheduler, offset),
            AARCH64_BOOT_TEST_CFG,
            "every affinity-retention hook must disappear from x86 scheduler paths"
        );
    }

    let kthread = repo_text("kernel/src/task/kthread.rs");
    let run_on_cpu = kthread
        .find("pub(crate) fn kthread_run_on_cpu_for_test<")
        .expect("forced-placement kthread helper");
    assert_eq!(
        preceding_nonempty_line(&kthread, run_on_cpu),
        AARCH64_BOOT_TEST_CFG,
        "the forced-placement kthread helper must be aarch64-only"
    );
    let clear_affinity_call = kthread
        .find("scheduler::clear_cpu_affinity_for_test(")
        .expect("kthread affinity cleanup call");
    assert_eq!(
        preceding_nonempty_line(&kthread, clear_affinity_call),
        AARCH64_BOOT_TEST_CFG,
        "kthread exit must not compile affinity cleanup into x86"
    );
}

#[test]
fn cpu_affinity_zero_sentinel_is_never_a_thread_id() {
    let scheduler = repo_text(SCHEDULER_PATH);
    let retain = function_body(&scheduler, "retain_cpu_affine_test_thread");
    let refusal_offset = retain
        .find("if thread_id == 0")
        .expect("thread-zero affinity refusal");
    let target_lookup_offset = retain
        .find("let target_cpu = BOOT_TEST_CPU_AFFINITY")
        .expect("affinity-table lookup");
    let refusal = braced_block_after(retain, "if thread_id == 0");
    assert!(
        refusal.contains("return false;") && refusal_offset < target_lookup_offset,
        "tid 0 must be refused before scanning zero-sentinel affinity slots"
    );
}

#[test]
fn every_aarch64_census_gate_requires_joined_but_accepts_reported_retirement_evidence() {
    for gate_path in [SERVICE_SEQUENCE_GATE_PATH, STRICT_GATE_PATH] {
        let gate = repo_text(gate_path);
        let pattern = gate
            .lines()
            .find(|line| line.starts_with("CENSUS_WIDEN_ORACLE_PATTERN="))
            .unwrap_or_else(|| panic!("census-widening pattern in {gate_path}"));
        assert!(
            pattern.contains(":joined=1:retired=[01]:PASS\\]'")
                && !pattern.contains(":joined=[01]:"),
            "{gate_path} must require joined=1 and PASS without promising asynchronous retirement"
        );
    }
}

#[test]
fn strand_census_progress_axes_and_ordering_cannot_shrink() {
    let scheduler = repo_text(SCHEDULER_PATH);
    let fields = declared_public_fields(&scheduler, "StrandCensus");
    let progress_axes: Vec<_> = fields
        .iter()
        .copied()
        .filter(|field| {
            field.contains("nonprogress")
                || field.contains("nondispatch")
                || field.contains("silence")
        })
        .collect();
    assert!(
        progress_axes.len() >= MIN_CENSUS_PROGRESS_AXES,
        "declared StrandCensus progress-axis floor shrank: {progress_axes:?}"
    );
    let runtime_axis_count = scheduler
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("pub const STRAND_CENSUS_PROGRESS_AXES: usize = ")
                .and_then(|value| value.strip_suffix(';'))
                .and_then(|value| value.parse::<usize>().ok())
        })
        .expect("runtime strand-census progress-axis count");
    assert_eq!(
        runtime_axis_count,
        progress_axes.len(),
        "the x86 marker's runtime axis count must equal the declaration-derived census"
    );
    let x86_gate = repo_text(X86_BOOT_GATE_PATH);
    assert!(
        x86_gate.contains(&format!(":axes={runtime_axis_count}:SKIP]'")),
        "the literal x86 SKIP marker must expose the declaration-derived axis count"
    );

    let census = function_body(&scheduler, "collect_strand_census");
    assert_eq!(
        code_occurrences(census, "!injected &&").len(),
        0,
        "the census must contain zero injection short-circuits"
    );
    assert_eq!(
        code_occurrences(&scheduler, "CENSUS_WIDEN_INJECT").len(),
        0,
        "the scheduler must contain zero census-injection identifiers"
    );

    let online_bound = code_occurrences(census, "for cpu in 0..online_cpu_count");
    assert_eq!(
        online_bound.len(),
        1,
        "CPU-silence sampling must iterate exactly the derived online CPU count"
    );
    let silence_scan = braced_block_after(census, "for cpu in 0..online_cpu_count");
    assert!(
        silence_scan.contains("cpu_state[cpu].last_schedule_ticks"),
        "each online CPU must contribute its scheduler-silence timestamp"
    );

    let queued_scan = census
        .find("queued_on_nondispatching_cpu += 1")
        .expect("queued nondispatching thread scan");
    let reachability = census
        .find("let reachability_dimensions =")
        .expect("reachability continue anchor");
    assert!(
        queued_scan < reachability,
        "queued-thread nonprogress scan must precede the reachability continue"
    );
}

#[test]
fn every_oracle_gate_pattern_matches_the_emitted_format() {
    let scheduler_oracle = repo_text(STRAND_ORACLE_PATH);
    let scheduler_format = marker_format(
        function_body(&scheduler_oracle, "report_strand"),
        "SCHED_STRAND_ORACLE",
    );
    let registry = repo_text(TEST_REGISTRY_PATH);
    let census_format = marker_format(
        function_body(&registry, "run_census_widen_oracle"),
        "CENSUS_WIDEN_ORACLE",
    );

    let mut checked_patterns = 0usize;
    let qemu_dir = repo_root().join("docker/qemu");
    for entry in fs::read_dir(qemu_dir).expect("read docker/qemu") {
        let path = entry.expect("gate script entry").path();
        if path.extension().is_none_or(|extension| extension != "sh") {
            continue;
        }
        let gate = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("read gate script {}", path.display()));
        for line in gate.lines() {
            for (family, format) in [
                ("SCHED_STRAND_ORACLE", scheduler_format),
                ("CENSUS_WIDEN_ORACLE", census_format),
            ] {
                let assignment = line.starts_with(&format!("{family}_PATTERN="));
                if !assignment && !(line.contains("grep") && line.contains(family)) {
                    continue;
                }
                if !assignment
                    && (line.contains(&format!("${family}_PATTERN"))
                        || line.contains(&format!("${family}_LITERAL")))
                {
                    continue;
                }
                let pattern = quoted_segment_containing(line, family)
                    .unwrap_or_else(|| panic!("quoted {family} grep in {}", path.display()));
                if pattern.starts_with('$') {
                    continue;
                }
                let arch = if pattern.contains("x86") {
                    "x86"
                } else {
                    "aarch64"
                };
                let marker = if family == "SCHED_STRAND_ORACLE" {
                    let stranded = if pattern.contains("stranded=[1-9]") {
                        "1"
                    } else {
                        "0"
                    };
                    render_positional_format(
                        format,
                        &[
                            arch, "1000", "1", stranded, "1", "1", "1", "1", "1", "0", "1",
                            "1", "1", "1", "1", "1",
                        ],
                    )
                } else {
                    let verdict = if pattern.contains("FAIL") {
                        "FAIL"
                    } else {
                        "PASS"
                    };
                    render_positional_format(
                        format,
                        &[
                            arch, "1", "0", "1", "1", "1", "1", "1", "1", "1", verdict,
                        ],
                    )
                };
                let matches = if line.contains("grep") && line.contains("-qF") {
                    marker.contains(pattern)
                } else {
                    shell_ere_matches(pattern, &marker)
                };
                assert!(
                    matches,
                    "gate pattern in {} stopped matching emitted {family} format: {pattern}",
                    path.display()
                );
                checked_patterns += 1;
            }
        }
    }
    assert!(
        checked_patterns >= MIN_ORACLE_GATE_PATTERNS,
        "oracle gate-pattern census shrank: {checked_patterns} < {MIN_ORACLE_GATE_PATTERNS}"
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
