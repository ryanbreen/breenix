use std::fs;
use std::path::{Path, PathBuf};

/// Adding a driver completion wait is expected to force explicit review of this
/// number rather than allowing a new interruptible wait to land silently.
const DRIVER_COMPLETION_WAIT_POPULATION: usize = 8;
const INTERRUPTIBLE_WAIT: &str = ".wait_timeout(";
const UNINTERRUPTIBLE_WAIT: &str = ".wait_timeout_uninterruptible(";
const BLOCK_EINTR_ORACLE_PREFIX: &str = "[BLOCK_EINTR_ORACLE:";
const BOOT_TEST_FAIL_PREFIX: &str = "[BOOT_TESTS:FAIL";

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
            index += 1;
            continue;
        }
        if byte == b'\'' {
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

fn code_occurrences(source: &str, needle: &str) -> Vec<usize> {
    let mask = code_mask(source);
    source
        .match_indices(needle)
        .filter_map(|(offset, _)| {
            mask[offset..offset + needle.len()]
                .iter()
                .all(|is_code| *is_code)
                .then_some(offset)
        })
        .collect()
}

fn line_number(source: &str, offset: usize) -> usize {
    source[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn discover_files(root: &Path, extension: &str) -> Vec<PathBuf> {
    fn visit(directory: &Path, extension: &str, files: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(directory)
            .unwrap_or_else(|_| panic!("read repository directory {}", directory.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|_| panic!("read entry in {}", directory.display()))
                .path();
            if path.is_dir() {
                visit(&path, extension, files);
            } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, extension, &mut files);
    files.sort();
    files
}

#[derive(Default)]
struct CompletionWaitCensus {
    interruptible: usize,
    uninterruptible: usize,
    interruptible_sites: Vec<String>,
}

impl CompletionWaitCensus {
    fn total(&self) -> usize {
        self.interruptible + self.uninterruptible
    }
}

fn census_source(label: &str, source: &str) -> CompletionWaitCensus {
    let interruptible_offsets = code_occurrences(source, INTERRUPTIBLE_WAIT);
    CompletionWaitCensus {
        interruptible: interruptible_offsets.len(),
        uninterruptible: code_occurrences(source, UNINTERRUPTIBLE_WAIT).len(),
        interruptible_sites: interruptible_offsets
            .into_iter()
            .map(|offset| format!("{label}:{}", line_number(source, offset)))
            .collect(),
    }
}

fn driver_completion_wait_census() -> (CompletionWaitCensus, usize) {
    let drivers_root = repo_root().join("kernel/src/drivers");
    let files = discover_files(&drivers_root, "rs");
    let mut census = CompletionWaitCensus::default();

    for path in &files {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("read driver source {}", path.display()));
        let relative = path
            .strip_prefix(repo_root())
            .unwrap_or(path)
            .display()
            .to_string();
        let file_census = census_source(&relative, &source);
        census.interruptible += file_census.interruptible;
        census.uninterruptible += file_census.uninterruptible;
        census
            .interruptible_sites
            .extend(file_census.interruptible_sites);
    }

    (census, files.len())
}

fn validate_no_interruptible_waits(label: &str, source: &str) -> Result<(), String> {
    let census = census_source(label, source);
    if census.interruptible == 0 {
        Ok(())
    } else {
        Err(format!(
            "found {} interruptible completion wait(s): {}",
            census.interruptible,
            census.interruptible_sites.join(", ")
        ))
    }
}

fn discover_aarch64_oracle_gates() -> Vec<PathBuf> {
    let qemu_root = repo_root().join("docker/qemu");
    discover_files(&qemu_root, "sh")
        .into_iter()
        .filter(|path| {
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if !file_name.starts_with("run-aarch64-") {
                return false;
            }
            let source = fs::read_to_string(path)
                .unwrap_or_else(|_| panic!("read aarch64 gate {}", path.display()));
            let builds_boot_tests = source.contains("--features boot_tests");
            let consumes_full_test_kernel = file_name.contains("boot-test")
                && source.contains("target/aarch64-breenix-kernel/release/kernel-aarch64");
            builds_boot_tests || consumes_full_test_kernel
        })
        .collect()
}

#[test]
fn drivers_have_no_interruptible_completion_waits() {
    let (census, discovered_files) = driver_completion_wait_census();
    assert_eq!(
        census.interruptible,
        0,
        "discovered {discovered_files} driver Rust files with interruptible completion waits at: {}",
        census.interruptible_sites.join(", ")
    );
}

#[test]
fn driver_completion_wait_population_is_pinned() {
    let (census, discovered_files) = driver_completion_wait_census();
    assert_eq!(
        census.total(),
        DRIVER_COMPLETION_WAIT_POPULATION,
        "completion-wait census changed across {discovered_files} discovered driver Rust files: {} interruptible + {} uninterruptible",
        census.interruptible,
        census.uninterruptible
    );
}

#[test]
fn completion_wait_validator_rejects_interruptible_synthetic_source() {
    let synthetic = "self.completion.wait_timeout(token, TIMEOUT);";
    let error = validate_no_interruptible_waits("synthetic.rs", synthetic)
        .expect_err("synthetic interruptible wait must be rejected");
    assert!(
        error.contains("synthetic.rs:1") && error.contains("1 interruptible completion wait"),
        "validator returned the wrong diagnostic: {error}"
    );
}

#[test]
fn completion_wait_census_ignores_comments_and_string_literals() {
    let synthetic = r###"
        // self.completion.wait_timeout(token, TIMEOUT);
        /* device.wait_timeout(token, TIMEOUT); */
        const MESSAGE: &str = ".wait_timeout(";
        const RAW_MESSAGE: &str = r#".wait_timeout("#;
    "###;
    let census = census_source("synthetic.rs", synthetic);
    assert_eq!(
        census.total(),
        0,
        "comment/string-only waits were counted: {} interruptible + {} uninterruptible",
        census.interruptible,
        census.uninterruptible
    );
}

#[test]
fn block_eintr_oracle_marker_is_pinned_in_the_gates() {
    let oracle = repo_text("userspace/programs/src/block_eintr_oracle.rs");
    assert!(
        oracle.contains(BLOCK_EINTR_ORACLE_PREFIX),
        "userspace block EINTR oracle does not emit {BLOCK_EINTR_ORACLE_PREFIX}"
    );

    let gates = discover_aarch64_oracle_gates();
    let discovered_count = gates.len();
    assert!(
        discovered_count != 0,
        "discovered 0 aarch64 boot_tests/full-kernel gate scripts"
    );
    let missing: Vec<String> = gates
        .iter()
        .filter_map(|path| {
            let source = fs::read_to_string(path)
                .unwrap_or_else(|_| panic!("read aarch64 gate {}", path.display()));
            (!source.contains(BLOCK_EINTR_ORACLE_PREFIX)).then(|| {
                path.strip_prefix(repo_root())
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
        })
        .collect();
    assert!(
        missing.is_empty(),
        "oracle marker missing from {} of {discovered_count} discovered aarch64 gate scripts: {}",
        missing.len(),
        missing.join(", ")
    );
}

#[test]
fn every_discovered_aarch64_boot_tests_gate_rejects_boot_test_failures() {
    let gates = discover_aarch64_oracle_gates();
    let discovered_count = gates.len();
    assert!(
        discovered_count != 0,
        "discovered 0 aarch64 boot_tests/full-kernel gate scripts"
    );

    let missing: Vec<String> = gates
        .iter()
        .filter_map(|path| {
            let source = fs::read_to_string(path)
                .unwrap_or_else(|_| panic!("read aarch64 gate {}", path.display()));
            let lines: Vec<_> = source.lines().collect();
            let rejects = lines.iter().enumerate().any(|(index, line)| {
                let normalized = line.replace("\\[", "[");
                if line.trim_start().starts_with('#')
                    || !line.contains("grep")
                    || !normalized.contains(BOOT_TEST_FAIL_PREFIX)
                {
                    return false;
                }
                lines[index..lines.len().min(index + 8)]
                    .iter()
                    .map(|line| line.trim())
                    .any(|line| {
                        line == "return 1"
                            || line == "exit 1"
                            || line.starts_with("FAIL_REASON=")
                            || line.starts_with("CLASS_BUCKET=\"BOOT_TEST_FAIL\"")
                            || line.starts_with("CLASS=\"BOOT_TEST_FAIL\"")
                            || line.starts_with("CLASS=\"ORACLE_FAIL\"")
                    })
            });
            (!rejects).then(|| {
                path.strip_prefix(repo_root())
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
        })
        .collect();

    assert!(
        missing.is_empty(),
        "{} of {discovered_count} discovered aarch64 boot_tests gates do not explicitly reject {BOOT_TEST_FAIL_PREFIX}: {}",
        missing.len(),
        missing.join(", ")
    );
}

const KEEP_LOCKED_IDENTIFIER: &str = "keep_locked";

/// Adding a guard `wedge()` call site must force explicit review of every
/// abandoned-request path rather than silently expanding the quarantine set.
const DRIVER_GUARD_WEDGE_CALL_POPULATION: usize = 9;

/// Every `release_on_drop = false` assignment marks an arm that intentionally keeps a driver
/// gate locked past guard drop. Each such arm MUST also latch `wedged` in the same method body
/// (via a `.wedged.store(` call) so a future locker is refused rather than hanging silently.
/// Pinning this population forces explicit review of any new such arm, regardless of what the
/// enclosing method is named.
const DRIVER_RELEASE_ON_DROP_FALSE_POPULATION: usize = 4;
const RELEASE_ON_DROP_FALSE_ASSIGNMENT: &str = "release_on_drop = false";
const WEDGED_STORE_CALL: &str = ".wedged.store(";

fn is_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn code_identifier_occurrences(source: &str, identifier: &str) -> Vec<usize> {
    code_occurrences(source, identifier)
        .into_iter()
        .filter(|offset| {
            let before = offset
                .checked_sub(1)
                .and_then(|index| source.as_bytes().get(index))
                .copied();
            let after = source.as_bytes().get(offset + identifier.len()).copied();
            before.is_none_or(|byte| !is_identifier_byte(byte))
                && after.is_none_or(|byte| !is_identifier_byte(byte))
        })
        .collect()
}

fn code_method_call_occurrences(source: &str, method: &str) -> Vec<usize> {
    code_identifier_occurrences(source, method)
        .into_iter()
        .filter(|offset| {
            let bytes = source.as_bytes();
            let mut before = *offset;
            while before != 0 && bytes[before - 1].is_ascii_whitespace() {
                before -= 1;
            }
            let mut after = offset + method.len();
            while bytes
                .get(after)
                .is_some_and(|byte| byte.is_ascii_whitespace())
            {
                after += 1;
            }
            before != 0 && bytes[before - 1] == b'.' && bytes.get(after) == Some(&b'(')
        })
        .collect()
}

fn masked_code(source: &str) -> String {
    let mask = code_mask(source);
    let bytes: Vec<u8> = source
        .as_bytes()
        .iter()
        .zip(mask)
        .map(|(byte, is_code)| if is_code { *byte } else { b' ' })
        .collect();
    String::from_utf8(bytes).expect("masked Rust source remains UTF-8")
}

fn compact_code(source: &str) -> String {
    masked_code(source)
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .map(char::from)
        .collect()
}

fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (relative, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + relative);
                }
            }
            _ => {}
        }
    }
    None
}

fn skip_ascii_whitespace(source: &str, mut offset: usize) -> usize {
    while source
        .as_bytes()
        .get(offset)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        offset += 1;
    }
    offset
}

fn identifier_at(source: &str, offset: usize) -> Option<(&str, usize)> {
    let bytes = source.as_bytes();
    if !bytes
        .get(offset)
        .is_some_and(|byte| *byte == b'_' || byte.is_ascii_alphabetic())
    {
        return None;
    }
    let mut end = offset + 1;
    while bytes.get(end).is_some_and(|byte| is_identifier_byte(*byte)) {
        end += 1;
    }
    Some((&source[offset..end], end))
}

fn enclosing_fn_body(source: &str, offset: usize) -> Option<(usize, usize)> {
    let mut innermost = None;

    for fn_offset in code_identifier_occurrences(source, "fn") {
        let Some(open_relative) = source[fn_offset + "fn".len()..].find('{') else {
            continue;
        };
        let open = fn_offset + "fn".len() + open_relative;
        let Some(close) = matching_brace(source, open) else {
            continue;
        };
        if !(open < offset && offset < close) {
            continue;
        }

        let candidate = (open + 1, close);
        if innermost.is_none_or(|(start, end)| candidate.1 - candidate.0 < end - start) {
            innermost = Some(candidate);
        }
    }

    innermost
}

fn fn_name_for_body(source: &str, body_start: usize) -> Option<&str> {
    let open = body_start.checked_sub(1)?;
    code_identifier_occurrences(source, "fn")
        .into_iter()
        .find_map(|fn_offset| {
            let name_start = skip_ascii_whitespace(source, fn_offset + "fn".len());
            let (name, name_end) = identifier_at(source, name_start)?;
            let open_relative = source[name_end..].find('{')?;
            (name_end + open_relative == open).then_some(name)
        })
}

fn struct_definitions(source: &str) -> Vec<(&str, usize, usize)> {
    let mut definitions = Vec::new();
    for offset in code_identifier_occurrences(source, "struct") {
        let name_start = skip_ascii_whitespace(source, offset + "struct".len());
        let Some((name, _name_end)) = identifier_at(source, name_start) else {
            continue;
        };
        let Some(open_relative) = source[name_start..].find('{') else {
            continue;
        };
        let open = name_start + open_relative;
        let Some(close) = matching_brace(source, open) else {
            continue;
        };
        definitions.push((name, open + 1, close));
    }
    definitions
}

fn guard_gate_type<'a>(source: &'a str, body_start: usize, body_end: usize) -> Option<&'a str> {
    let body = &source[body_start..body_end];
    for gate_offset in code_identifier_occurrences(body, "gate") {
        let mut cursor = skip_ascii_whitespace(body, gate_offset + "gate".len());
        if body.as_bytes().get(cursor) != Some(&b':') {
            continue;
        }
        cursor = skip_ascii_whitespace(body, cursor + 1);
        if body.as_bytes().get(cursor) != Some(&b'&') {
            continue;
        }
        cursor = skip_ascii_whitespace(body, cursor + 1);
        if body.as_bytes().get(cursor) == Some(&b'\'') {
            let (_, lifetime_end) = identifier_at(body, cursor + 1)?;
            cursor = skip_ascii_whitespace(body, lifetime_end);
        }
        if let Some((gate_type, _)) = identifier_at(body, cursor) {
            return Some(gate_type);
        }
    }
    None
}

fn impl_bodies_for_type(source: &str, type_name: &str) -> Vec<(usize, usize)> {
    let mut bodies = Vec::new();
    for offset in code_identifier_occurrences(source, "impl") {
        let Some(open_relative) = source[offset..].find('{') else {
            continue;
        };
        let open = offset + open_relative;
        if code_identifier_occurrences(&source[offset..open], type_name).is_empty() {
            continue;
        }
        if let Some(close) = matching_brace(source, open) {
            bodies.push((open + 1, close));
        }
    }
    bodies
}

fn previous_identifier(source: &str, offset: usize) -> Option<&str> {
    let bytes = source.as_bytes();
    let mut end = offset;
    while end != 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start != 0 && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }
    (start != end).then_some(&source[start..end])
}

fn method_bodies(
    source: &str,
    impl_start: usize,
    impl_end: usize,
    method_name: &str,
) -> Vec<(usize, usize)> {
    let mut bodies = Vec::new();
    let impl_body = &source[impl_start..impl_end];
    for relative in code_identifier_occurrences(impl_body, method_name) {
        let name_offset = impl_start + relative;
        if previous_identifier(source, name_offset) != Some("fn") {
            continue;
        }
        let Some(open_relative) = source[name_offset..impl_end].find('{') else {
            continue;
        };
        let open = name_offset + open_relative;
        if let Some(close) = matching_brace(source, open) {
            bodies.push((open + 1, close));
        }
    }
    bodies
}

fn wedge_gate_types(source: &str) -> std::collections::BTreeSet<String> {
    let masked = masked_code(source);
    let mut gate_types = std::collections::BTreeSet::new();

    for (guard_type, body_start, body_end) in struct_definitions(&masked) {
        let Some(gate_type) = guard_gate_type(&masked, body_start, body_end) else {
            continue;
        };
        let has_real_wedge = impl_bodies_for_type(&masked, guard_type)
            .into_iter()
            .flat_map(|(start, end)| method_bodies(&masked, start, end, "wedge"))
            .any(|(start, end)| {
                compact_code(&masked[start..end]).contains("self.gate.wedged.store(")
            });
        if has_real_wedge {
            gate_types.insert(gate_type.to_string());
        }
    }

    gate_types
}

fn validate_gate_poison_source(label: &str, source: &str) -> Result<(), String> {
    let keep_locked = code_identifier_occurrences(source, KEEP_LOCKED_IDENTIFIER);
    if !keep_locked.is_empty() {
        return Err(format!(
            "{label} contains {} keep_locked identifier(s)",
            keep_locked.len()
        ));
    }

    let masked = masked_code(source);
    for gate_type in wedge_gate_types(source) {
        let lock_reads_wedged = impl_bodies_for_type(&masked, &gate_type)
            .into_iter()
            .flat_map(|(start, end)| method_bodies(&masked, start, end, "lock"))
            .any(|(start, end)| compact_code(&masked[start..end]).contains("self.wedged.load("));
        if !lock_reads_wedged {
            return Err(format!(
                "{label}: gate {gate_type} has a wedge path but lock() never reads wedged"
            ));
        }
    }

    Ok(())
}

fn validate_release_on_drop_pairing(label: &str, source: &str) -> Result<(), String> {
    let masked = masked_code(source);
    for offset in code_occurrences(&masked, RELEASE_ON_DROP_FALSE_ASSIGNMENT) {
        let line = line_number(source, offset);
        let Some((body_start, body_end)) = enclosing_fn_body(&masked, offset) else {
            return Err(format!(
                "{label}:{line}: `{RELEASE_ON_DROP_FALSE_ASSIGNMENT}` assignment has no enclosing method body"
            ));
        };
        if compact_code(&masked[body_start..body_end]).contains(WEDGED_STORE_CALL) {
            continue;
        }

        let method = fn_name_for_body(&masked, body_start)
            .map(|name| format!("method `{name}`"))
            .unwrap_or_else(|| "enclosing method".to_string());
        return Err(format!(
            "{label}:{line}: {method} contains unpaired `{RELEASE_ON_DROP_FALSE_ASSIGNMENT}` assignment without a `{WEDGED_STORE_CALL}` call"
        ));
    }

    Ok(())
}

fn driver_rust_sources() -> Vec<(String, String)> {
    discover_files(&repo_root().join("kernel/src/drivers"), "rs")
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("read driver source {}", path.display()));
            let label = path
                .strip_prefix(repo_root())
                .unwrap_or(&path)
                .display()
                .to_string();
            (label, source)
        })
        .collect()
}

#[test]
fn drivers_have_no_unpaired_keep_locked_escape() {
    let sources = driver_rust_sources();
    let sites: Vec<String> = sources
        .iter()
        .flat_map(|(label, source)| {
            code_identifier_occurrences(source, KEEP_LOCKED_IDENTIFIER)
                .into_iter()
                .map(move |offset| format!("{label}:{}", line_number(source, offset)))
        })
        .collect();
    assert!(
        sites.is_empty(),
        "keep_locked is forbidden under discovered driver sources: {}",
        sites.join(", ")
    );
}

#[test]
fn driver_guard_wedge_call_population_is_pinned() {
    let sources = driver_rust_sources();
    let wedge_calls: usize = sources
        .iter()
        .map(|(_, source)| code_method_call_occurrences(source, "wedge").len())
        .sum();
    assert_eq!(
        wedge_calls,
        DRIVER_GUARD_WEDGE_CALL_POPULATION,
        "guard wedge-call census changed across {} discovered driver Rust files",
        sources.len()
    );
}

#[test]
fn driver_release_on_drop_false_population_is_pinned() {
    let sources = driver_rust_sources();
    let assignments: usize = sources
        .iter()
        .map(|(_, source)| code_occurrences(source, RELEASE_ON_DROP_FALSE_ASSIGNMENT).len())
        .sum();
    assert_eq!(
        assignments,
        DRIVER_RELEASE_ON_DROP_FALSE_POPULATION,
        "release_on_drop-false census changed across {} discovered driver Rust files",
        sources.len()
    );
}

#[test]
fn every_wedged_driver_gate_refuses_inside_lock() {
    let sources = driver_rust_sources();
    let violations: Vec<String> = sources
        .iter()
        .filter_map(|(label, source)| validate_gate_poison_source(label, source).err())
        .collect();
    assert!(
        violations.is_empty(),
        "driver gate-poison validation failed: {}",
        violations.join("; ")
    );
}

#[test]
fn every_release_on_drop_false_arm_latches_wedged() {
    let sources = driver_rust_sources();
    let violations: Vec<String> = sources
        .iter()
        .filter_map(|(label, source)| validate_release_on_drop_pairing(label, source).err())
        .collect();
    assert!(
        violations.is_empty(),
        "driver release_on_drop pairing validation failed: {}",
        violations.join("; ")
    );
}

#[test]
fn gate_poison_validator_rejects_synthetic_regressions() {
    let missing_lock_check = r#"
        struct SyntheticRequestGate { locked: AtomicBool, wedged: AtomicBool }
        struct SyntheticRequestGuard<'a> { gate: &'a SyntheticRequestGate }
        impl SyntheticRequestGate {
            fn lock(&self) { self.locked.load(Ordering::Acquire); }
        }
        impl SyntheticRequestGuard<'_> {
            fn wedge(&mut self) {
                self.gate.wedged.store(true, Ordering::Release);
            }
        }
    "#;
    let missing_error = validate_gate_poison_source("missing.rs", missing_lock_check)
        .expect_err("a wedged gate whose lock ignores wedged must be rejected");
    assert!(
        missing_error.contains("SyntheticRequestGate")
            && missing_error.contains("lock() never reads wedged"),
        "validator returned the wrong missing-lock diagnostic: {missing_error}"
    );

    let keep_locked_source = "impl Guard { fn keep_locked(&mut self) {} }";
    let keep_error = validate_gate_poison_source("keep.rs", keep_locked_source)
        .expect_err("keep_locked must be rejected by construction");
    assert!(
        keep_error.contains("keep_locked"),
        "validator returned the wrong keep_locked diagnostic: {keep_error}"
    );

    let unpaired_retain = r#"
        struct SyntheticRetainGuard { gate: SyntheticRetainGate, release_on_drop: bool }
        struct SyntheticRetainGate { wedged: AtomicBool }
        impl SyntheticRetainGuard {
            fn retain(mut self) {
                self.release_on_drop = false;
            }
        }
    "#;
    let retain_error = validate_release_on_drop_pairing("retain.rs", unpaired_retain)
        .expect_err("an unpaired release_on_drop assignment must be rejected");
    assert!(
        retain_error.contains("retain.rs:")
            && retain_error.contains("method `retain`")
            && retain_error.contains(RELEASE_ON_DROP_FALSE_ASSIGNMENT),
        "validator returned the wrong unpaired-assignment diagnostic: {retain_error}"
    );

    let paired_retain = r#"
        struct SyntheticRetainGuard { gate: SyntheticRetainGate, release_on_drop: bool }
        struct SyntheticRetainGate { wedged: AtomicBool }
        impl SyntheticRetainGuard {
            fn retain(mut self) {
                self.release_on_drop = false;
                self.gate.wedged.store(true, Ordering::Release);
            }
        }
    "#;
    validate_release_on_drop_pairing("paired.rs", paired_retain)
        .expect("a release_on_drop assignment paired with wedged.store must be accepted");
}
