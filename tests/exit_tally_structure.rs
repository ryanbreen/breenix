//! Structural ratchets for the exactly-once process-exit tally seam.

use std::fs;
use std::path::{Path, PathBuf};

const TERMINATED_STATE: &str = "ProcessState::Terminated(";
const EXACT_RECORD_EXIT_CALL: &str = "crate::task::exit_tally::record_exit(";
const RECORD_EXIT_CALL: &str = "exit_tally::record_exit(";
const TERMINATED_GUARD: &str = "ifmatches!(self.state,ProcessState::Terminated(_)){return;}";
const FAILURE_TABLE_LOCK: &str = "USERSPACE_FAILURES.lock()";
const FAILURE_TABLE_INTERRUPT_GUARD: &str = "with_failure_table_interrupts_disabled(||{";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|error| panic!("read repository file {relative}: {error}"))
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
            mask[index] = false;
            if byte == b'\n' {
                line_comment = false;
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

fn compact_code(source: &str) -> String {
    let mask = code_mask(source);
    source
        .bytes()
        .zip(mask)
        .filter_map(|(byte, is_code)| {
            (is_code && !byte.is_ascii_whitespace()).then_some(byte as char)
        })
        .collect()
}

fn identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric() || !byte.is_ascii()
}

fn braced_block<'a>(source: &'a str, mask: &[bool], open: usize) -> Option<&'a str> {
    let bytes = source.as_bytes();
    if bytes.get(open) != Some(&b'{') || !mask.get(open).copied().unwrap_or(false) {
        return None;
    }

    let mut depth = 0usize;
    for index in open..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&source[open..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn matching_delimiter(
    source: &str,
    open: usize,
    opening_delimiter: u8,
    closing_delimiter: u8,
) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open) != Some(&opening_delimiter) {
        return None;
    }

    let mut depth = 0usize;
    for (index, byte) in bytes.iter().enumerate().skip(open) {
        if *byte == opening_delimiter {
            depth += 1;
        } else if *byte == closing_delimiter {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn function_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();

    for (fn_offset, _) in source.match_indices("fn") {
        if !mask[fn_offset]
            || fn_offset
                .checked_sub(1)
                .and_then(|before| bytes.get(before))
                .is_some_and(|byte| identifier_byte(*byte))
            || bytes
                .get(fn_offset + 2)
                .is_some_and(|byte| identifier_byte(*byte))
        {
            continue;
        }

        let mut cursor = fn_offset + 2;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            cursor += 1;
        }
        if !source[cursor..].starts_with(name)
            || bytes
                .get(cursor + name.len())
                .is_some_and(|byte| identifier_byte(*byte))
        {
            continue;
        }

        let open = (cursor + name.len()..bytes.len())
            .find(|index| mask[*index] && bytes[*index] == b'{')?;
        let semicolon =
            (cursor + name.len()..open).find(|index| mask[*index] && bytes[*index] == b';');
        if semicolon.is_none() {
            return braced_block(source, &mask, open);
        }
    }
    None
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

fn terminated_assignment_offsets(code: &str) -> Vec<usize> {
    code.match_indices(TERMINATED_STATE)
        .filter_map(|(offset, _)| {
            let bytes = code.as_bytes();
            let mut path_start = offset;
            while bytes.get(path_start.saturating_sub(2)..path_start) == Some(b"::") {
                path_start -= 2;
                while path_start != 0 && identifier_byte(bytes[path_start - 1]) {
                    path_start -= 1;
                }
            }

            let prefix = &code[..path_start];
            let assignment = prefix.ends_with('=')
                && !prefix.ends_with("==")
                && !prefix.ends_with("!=")
                && !prefix.ends_with(">=")
                && !prefix.ends_with("<=");
            let field_initializer = prefix.ends_with(':') && !prefix.ends_with("::");
            (assignment || field_initializer).then_some(offset)
        })
        .collect()
}

#[test]
fn terminated_assignment_detector_handles_paths_without_counting_patterns() {
    for assignment in [
        "self.state=ProcessState::Terminated(code);",
        "self.state=crate::process::ProcessState::Terminated(code);",
        "Process{state:ProcessState::Terminated(code)}",
        "Process{state:crate::process::ProcessState::Terminated(code)}",
    ] {
        assert_eq!(
            terminated_assignment_offsets(assignment).len(),
            1,
            "must detect terminated-state construction in {assignment}"
        );
    }

    for reader in [
        "matches!(self.state,ProcessState::Terminated(_))",
        "ifletcrate::process::ProcessState::Terminated(code)=state{}",
        "matchstate{ProcessState::Terminated(code)=>code}",
        "state==crate::process::ProcessState::Terminated(code)",
        "state!=crate::process::ProcessState::Terminated(code)",
        "state>=crate::process::ProcessState::Terminated(code)",
        "state<=crate::process::ProcessState::Terminated(code)",
    ] {
        assert_eq!(
            terminated_assignment_offsets(reader).len(),
            0,
            "must not classify terminated-state reader as assignment in {reader}"
        );
    }
}

#[test]
fn failure_table_lock_is_only_taken_with_interrupts_disabled() {
    let source = repo_text("kernel/src/task/exit_tally.rs");
    let code = compact_code(&source);
    let lock_offsets: Vec<_> = code
        .match_indices(FAILURE_TABLE_LOCK)
        .map(|(offset, _)| offset)
        .collect();
    assert_eq!(
        lock_offsets.len(),
        2,
        "exit_tally.rs must contain exactly the recording and snapshot failure-table locks"
    );

    let guarded_closures: Vec<_> = code
        .match_indices(FAILURE_TABLE_INTERRUPT_GUARD)
        .map(|(call_start, _)| {
            let call_open = call_start + "with_failure_table_interrupts_disabled".len();
            let call_close = matching_delimiter(&code, call_open, b'(', b')')
                .expect("interrupt-disabling helper call must have balanced parentheses");
            let closure_open = call_start + FAILURE_TABLE_INTERRUPT_GUARD.len() - 1;
            let closure_close = matching_delimiter(&code, closure_open, b'{', b'}')
                .expect("interrupt-disabling helper closure must have balanced braces");
            assert_eq!(
                closure_close + 1,
                call_close,
                "interrupt-disabling helper must receive exactly the matched closure"
            );
            (closure_open, closure_close)
        })
        .collect();

    for lock_offset in lock_offsets {
        assert!(
            guarded_closures
                .iter()
                .any(|(start, end)| *start < lock_offset && lock_offset < *end),
            "USERSPACE_FAILURES.lock() at compacted byte {lock_offset} must be lexically inside the interrupt-disabling helper closure"
        );
    }
}

fn kernel_source_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap_or_else(|error| {
        panic!(
            "read kernel source directory {}: {error}",
            directory.display()
        )
    }) {
        let path = entry.expect("read kernel source directory entry").path();
        if path.is_dir() {
            kernel_source_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

#[test]
fn terminated_state_assignments_are_confined_to_the_two_termination_functions() {
    let process_source = repo_text("kernel/src/process/process.rs");
    let process_code = compact_code(&process_source);
    assert_eq!(
        terminated_assignment_offsets(&process_code).len(),
        2,
        "process.rs must contain exactly two ProcessState::Terminated assignments"
    );

    let mut files = Vec::new();
    kernel_source_files(&repo_root().join("kernel/src"), &mut files);
    for path in files {
        if path.ends_with("process/process.rs") {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read kernel source {}: {error}", path.display()));
        assert_eq!(
            terminated_assignment_offsets(&compact_code(&source)).len(),
            0,
            "ProcessState::Terminated assignment escaped the universal seam into {}",
            path.display()
        );
    }
}

#[test]
fn termination_functions_guard_assign_and_record_in_order_once_each() {
    let source = repo_text("kernel/src/process/process.rs");

    for name in ["terminate", "terminate_minimal"] {
        let body = function_body(&source, name)
            .unwrap_or_else(|| panic!("missing brace-matched Process::{name} body"));
        let code = compact_code(body);
        assert!(
            code.starts_with(&format!("{{{TERMINATED_GUARD}")),
            "Process::{name} must begin with the already-terminated early-return guard"
        );
        assert_eq!(
            count(&code, TERMINATED_GUARD),
            1,
            "Process::{name} must contain exactly one already-terminated guard"
        );
        assert_eq!(
            terminated_assignment_offsets(&code).len(),
            1,
            "Process::{name} must contain exactly one terminated-state assignment"
        );
        assert_eq!(
            count(&code, EXACT_RECORD_EXIT_CALL),
            1,
            "Process::{name} must contain exactly one exit-tally recording call"
        );

        let guard = code.find(TERMINATED_GUARD).expect("guard checked above");
        let assignment = terminated_assignment_offsets(&code)[0];
        let record = code
            .find(EXACT_RECORD_EXIT_CALL)
            .expect("record checked above");
        assert!(
            guard < assignment && assignment < record,
            "Process::{name} must guard, assign Terminated, then record the exit"
        );
    }
}

#[test]
fn record_exit_calls_exist_only_at_the_two_termination_transitions() {
    let mut files = Vec::new();
    kernel_source_files(&repo_root().join("kernel/src"), &mut files);

    let sites: Vec<_> = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read kernel source {}: {error}", path.display()));
            let calls = count(&compact_code(&source), RECORD_EXIT_CALL);
            (calls != 0).then_some((path, calls))
        })
        .collect();

    assert_eq!(
        sites.len(),
        1,
        "exit_tally::record_exit must be called from process.rs only; found {sites:?}"
    );
    assert!(sites[0].0.ends_with("process/process.rs"));
    assert_eq!(
        sites[0].1, 2,
        "process.rs must contain exactly the two universal record_exit calls"
    );
}

#[test]
fn process_task_has_no_record_exit_call() {
    let source = repo_text("kernel/src/task/process_task.rs");
    assert_eq!(
        count(&compact_code(&source), "record_exit("),
        0,
        "process_task.rs must not restore a second exit-tally recording seam"
    );
}
