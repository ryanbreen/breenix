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
                    return Some(&source[open..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn function_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    for function in identifier_offsets(source, &mask, "fn") {
        let mut cursor = function + 2;
        while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        if &source[name_start..cursor] != name {
            continue;
        }
        let brace = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{')?;
        let semicolon = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b';');
        if semicolon.is_some_and(|semicolon| semicolon < brace) {
            continue;
        }
        return braced_block(source, &mask, brace);
    }
    None
}

fn calls_identifier(source: &str, identifier: &str) -> bool {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    identifier_offsets(source, &mask, identifier)
        .into_iter()
        .any(|offset| {
            let mut cursor = offset + identifier.len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            bytes.get(cursor) == Some(&b'(')
        })
}

fn validate_eintr_call_site(source: &str) -> Result<(), &'static str> {
    let body = function_body(source, "check_signals_for_eintr")
        .ok_or("missing check_signals_for_eintr")?;
    if !calls_identifier(body, "has_interrupting_signals") {
        return Err("EINTR check does not call has_interrupting_signals");
    }
    if calls_identifier(body, "has_deliverable_signals") {
        return Err("EINTR check still calls has_deliverable_signals");
    }
    Ok(())
}

fn validate_interrupting_predicate(source: &str) -> Result<(), &'static str> {
    let body = function_body(source, "has_interrupting_signals")
        .ok_or("missing SignalState::has_interrupting_signals")?;
    let mask = code_mask(body);

    for field in ["pending", "blocked"] {
        if identifier_offsets(body, &mask, field).is_empty() {
            return Err("interrupting predicate does not filter pending and blocked signals");
        }
    }
    for helper in [
        "get_handler",
        "is_default",
        "is_ignore",
        "is_handler",
        "default_action",
    ] {
        if !calls_identifier(body, helper) {
            return Err("interrupting predicate is missing a disposition helper call");
        }
    }
    for disposition in ["SignalDefaultAction", "Ignore"] {
        if identifier_offsets(body, &mask, disposition).is_empty() {
            return Err("interrupting predicate is missing the default-ignore check");
        }
    }
    Ok(())
}

fn validate_delivery_wrapper(source: &str) -> Result<(), &'static str> {
    let body = function_body(source, "has_interrupting_signals")
        .ok_or("missing delivery::has_interrupting_signals")?;
    if !calls_identifier(body, "has_interrupting_signals") {
        return Err("delivery wrapper does not call the SignalState predicate");
    }
    Ok(())
}

#[test]
fn syscall_eintr_uses_disposition_aware_signal_predicate() {
    let syscall = repo_text("kernel/src/syscall/mod.rs");
    let signal_types = repo_text("kernel/src/signal/types.rs");
    let delivery = repo_text("kernel/src/signal/delivery.rs");

    assert_eq!(validate_eintr_call_site(&syscall), Ok(()));
    assert_eq!(validate_interrupting_predicate(&signal_types), Ok(()));
    assert_eq!(validate_delivery_wrapper(&delivery), Ok(()));
}

#[test]
fn eintr_validator_rejects_deliverable_signal_call() {
    let synthetic = r#"
        fn check_signals_for_eintr() -> Option<i32> {
            if crate::signal::delivery::has_deliverable_signals(process) {
                return Some(errno::EINTR);
            }
            None
        }
    "#;

    assert!(validate_eintr_call_site(synthetic).is_err());
}
