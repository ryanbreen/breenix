//! Structural oracle for descriptor validation on degenerate transfers (#670).
//!
//! `read`, `write`, `pread64` and `pwrite64` answer a zero-length or
//! null-buffer request with `Ok(0)`. Linux looks the descriptor up first, so
//! the same request against a closed, negative or never-opened descriptor
//! fails with `EBADF`. Breenix used to return the descriptor-independent
//! `Ok(0)` before any lookup, so a caller could not tell a valid zero-length
//! operation from one on a descriptor it had already closed.
//!
//! The rule below is a shape rule over `kernel/src/syscall/handlers.rs`: every
//! degenerate-transfer guard anywhere in that file, present or future, must
//! validate the descriptor before it answers `Ok(0)`. It names no handler and
//! carries no allowlist, so a fifth handler written with the old shape is
//! caught when it is written. Its anti-vacuity companion fails if the guards
//! ever stop being found at all.

use std::fs;
use std::path::PathBuf;

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

fn identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric() || !byte.is_ascii()
}

fn identifier_offsets(source: &str, mask: &[bool], identifier: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    source
        .match_indices(identifier)
        .filter_map(|(offset, _)| {
            let end = offset + identifier.len();
            (mask.get(offset).copied().unwrap_or(false)
                && !offset
                    .checked_sub(1)
                    .and_then(|before| bytes.get(before))
                    .is_some_and(|byte| identifier_byte(*byte))
                && !bytes.get(end).is_some_and(|byte| identifier_byte(*byte)))
            .then_some(offset)
        })
        .collect()
}

fn braced_block<'a>(source: &'a str, mask: &[bool], start: usize) -> Option<&'a str> {
    let (open, close) = braced_block_span(source, mask, start)?;
    Some(&source[open..=close])
}

fn function_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    function_spans(source)
        .into_iter()
        .find(|span| span.name == name)
        .map(|span| &source[span.open..=span.close])
}

fn braced_block_span(source: &str, mask: &[bool], start: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let open = (start..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{')?;
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
                    return Some((open, index));
                }
            }
            _ => {}
        }
    }
    None
}

fn function_open_brace(source: &str, mask: &[bool], start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    for index in start..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'(' => parentheses += 1,
            b')' => parentheses = parentheses.checked_sub(1)?,
            b'[' => brackets += 1,
            b']' => brackets = brackets.checked_sub(1)?,
            b'{' if parentheses == 0 && brackets == 0 => return Some(index),
            b';' if parentheses == 0 && brackets == 0 => return None,
            _ => {}
        }
    }
    None
}

#[derive(Debug)]
struct FunctionSpan {
    name: String,
    open: usize,
    close: usize,
}

fn function_spans(source: &str) -> Vec<FunctionSpan> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    for function in identifier_offsets(source, &mask, "fn") {
        let mut cursor = function + 2;
        while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == name_start {
            continue;
        }
        let Some(brace) = function_open_brace(source, &mask, cursor) else {
            continue;
        };
        let Some((open, close)) = braced_block_span(source, &mask, brace) else {
            continue;
        };
        spans.push(FunctionSpan {
            name: source[name_start..cursor].to_string(),
            open,
            close,
        });
    }
    spans
}

fn normalized_code(fragment: &str) -> String {
    let mask = code_mask(fragment);
    let kept: Vec<u8> = fragment
        .bytes()
        .zip(mask)
        .filter_map(|(byte, code)| code.then_some(byte))
        .collect();
    String::from_utf8_lossy(&kept)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact_code(fragment: &str) -> String {
    normalized_code(fragment)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}


/// The degenerate-transfer guard, as it is written in every handler.
const DEGENERATE_GUARD: &str = "buf_ptr == 0 || count == 0";

/// The descriptor check the guard must perform first.
const VALIDATOR: &str = "validate_fd_for_degenerate_transfer(";

/// The descriptor-independent answer the guard gives once the check passes.
const DEGENERATE_ANSWER: &str = "SyscallResult::Ok(0)";

/// Every degenerate-transfer guard in `source`, as `(enclosing fn, guard block)`.
fn degenerate_guards(source: &str) -> Vec<(String, String)> {
    let mut guards = Vec::new();
    for span in function_spans(source) {
        let body = &source[span.open..=span.close];
        let mask = code_mask(body);
        let mut cursor = 0usize;
        while let Some(found) = body[cursor..].find(DEGENERATE_GUARD) {
            let offset = cursor + found;
            cursor = offset + DEGENERATE_GUARD.len();
            if !mask[offset..cursor].iter().all(|code| *code) {
                continue;
            }
            let Some(block) = braced_block(body, &mask, offset) else {
                continue;
            };
            guards.push((span.name.clone(), block.to_string()));
        }
    }
    guards
}

/// A guard is correct when it validates the descriptor and only then answers
/// `Ok(0)` - in that order, on the same path.
fn validate_guard(block: &str) -> Result<(), String> {
    let compact = compact_code(block);
    let Some(check) = compact.find(VALIDATOR) else {
        return Err(format!(
            "degenerate-transfer guard answers without validating the descriptor: {compact}"
        ));
    };
    let Some(answer) = compact.find(DEGENERATE_ANSWER) else {
        return Err(format!("degenerate-transfer guard has no Ok(0) answer: {compact}"));
    };
    if check > answer {
        return Err(format!(
            "degenerate-transfer guard answers Ok(0) before validating the descriptor: {compact}"
        ));
    }
    if !compact.contains("returnSyscallResult::Err(e);") {
        return Err(format!(
            "degenerate-transfer guard does not return the validator's error: {compact}"
        ));
    }
    Ok(())
}

#[test]
fn every_degenerate_transfer_guard_validates_the_descriptor_first() {
    let handlers = repo_text("kernel/src/syscall/handlers.rs");
    let failures: Vec<String> = degenerate_guards(&handlers)
        .into_iter()
        .filter_map(|(name, block)| validate_guard(&block).err().map(|e| format!("{name}: {e}")))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn the_degenerate_transfer_census_is_not_vacuous() {
    let handlers = repo_text("kernel/src/syscall/handlers.rs");
    let guards = degenerate_guards(&handlers);
    // read, write, pread64 and pwrite64 are the four #670 named. The rule is
    // the shape, not the names: a handler may be renamed or added, but the
    // census dropping below what the file has always carried means the oracle
    // stopped finding the guards it polices.
    assert!(
        guards.len() >= 4,
        "degenerate-transfer census collapsed to {} guards - the oracle would pass vacuously",
        guards.len()
    );
}

#[test]
fn the_validator_reports_ebadf_from_the_descriptor_table() {
    let handlers = repo_text("kernel/src/syscall/handlers.rs");
    let body = function_body(&handlers, "validate_fd_for_degenerate_transfer")
        .expect("missing validate_fd_for_degenerate_transfer");
    let compact = compact_code(body);
    assert!(
        compact.contains("process.fd_table.get(fd).is_some()"),
        "the validator does not consult the calling process's descriptor table"
    );
    assert!(
        compact.contains("Err(super::errno::EBADFasu64)"),
        "the validator does not report EBADF for an absent descriptor"
    );
    assert!(
        compact.contains("crate::arch_without_interrupts("),
        "the validator takes the process-manager lock without masking interrupts"
    );
}

#[test]
fn the_guard_validator_rejects_the_pre_fix_shape() {
    // Verbatim shape of sys_read before #670 was fixed.
    let pre_fix = r#"
        pub fn sys_read(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
            if buf_ptr == 0 || count == 0 {
                return SyscallResult::Ok(0);
            }
        }
    "#;
    let guards = degenerate_guards(pre_fix);
    assert_eq!(guards.len(), 1, "the census missed the pre-fix guard");
    assert!(validate_guard(&guards[0].1).is_err());

    // An answer that validates only after the fact is rejected too.
    let after_the_fact = r#"
        pub fn sys_read(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
            if buf_ptr == 0 || count == 0 {
                return SyscallResult::Ok(0);
                if let Err(e) = validate_fd_for_degenerate_transfer(fd as i32) {
                    return SyscallResult::Err(e);
                }
            }
        }
    "#;
    let guards = degenerate_guards(after_the_fact);
    assert_eq!(guards.len(), 1);
    assert!(validate_guard(&guards[0].1).is_err());

    // The shipped shape passes.
    let fixed = r#"
        pub fn sys_read(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
            if buf_ptr == 0 || count == 0 {
                if let Err(e) = validate_fd_for_degenerate_transfer(fd as i32) {
                    return SyscallResult::Err(e);
                }
                return SyscallResult::Ok(0);
            }
        }
    "#;
    let guards = degenerate_guards(fixed);
    assert_eq!(guards.len(), 1);
    assert_eq!(validate_guard(&guards[0].1), Ok(()));
}
