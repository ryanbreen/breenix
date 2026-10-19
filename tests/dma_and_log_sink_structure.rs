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
            } else {
                index += 1;
            }
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
                    return Some(&source[start..=index]);
                }
            }
            _ => {}
        }
    }
    None
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

fn function_body<'a>(scope: &'a str, name: &str) -> Option<&'a str> {
    let mask = code_mask(scope);
    let bytes = scope.as_bytes();
    for function in identifier_offsets(scope, &mask, "fn") {
        let mut cursor = function + 2;
        while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        if &scope[name_start..cursor] != name {
            continue;
        }
        let brace = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{')?;
        let semicolon = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b';');
        if semicolon.is_some_and(|semicolon| semicolon < brace) {
            continue;
        }
        return braced_block(scope, &mask, brace);
    }
    None
}

fn impl_body<'a>(source: &'a str, expected_header: &str) -> Option<&'a str> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    for implementation in identifier_offsets(source, &mask, "impl") {
        let brace =
            (implementation..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{')?;
        if normalized_code(&source[implementation..brace]) == expected_header {
            return braced_block(source, &mask, implementation);
        }
    }
    None
}

fn call_arguments<'a>(source: &'a str, callee: &str) -> Vec<Vec<&'a str>> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut calls = Vec::new();
    for offset in identifier_offsets(source, &mask, callee) {
        let mut open = offset + callee.len();
        while open < bytes.len() && (!mask[open] || bytes[open].is_ascii_whitespace()) {
            open += 1;
        }
        if bytes.get(open) != Some(&b'(') {
            continue;
        }

        let mut arguments = Vec::new();
        let mut argument_start = open + 1;
        let mut parens = 1usize;
        let mut braces = 0usize;
        let mut brackets = 0usize;
        for index in open + 1..bytes.len() {
            if !mask[index] {
                continue;
            }
            match bytes[index] {
                b'(' => parens += 1,
                b')' => {
                    parens -= 1;
                    if parens == 0 {
                        arguments.push(&source[argument_start..index]);
                        calls.push(arguments);
                        break;
                    }
                }
                b'{' => braces += 1,
                b'}' => braces = braces.saturating_sub(1),
                b'[' => brackets += 1,
                b']' => brackets = brackets.saturating_sub(1),
                b',' if parens == 1 && braces == 0 && brackets == 0 => {
                    arguments.push(&source[argument_start..index]);
                    argument_start = index + 1;
                }
                _ => {}
            }
        }
    }
    calls
}

fn has_whole_framebuffer_clear(body: &str) -> bool {
    let fills_whole_slice = call_arguments(body, "fill").iter().any(|arguments| {
        arguments
            .first()
            .is_some_and(|argument| normalized_code(argument) == "0")
    });
    let writes_buffer_len = call_arguments(body, "write_bytes").iter().any(|arguments| {
        arguments.get(2).is_some_and(|length| {
            let mask = code_mask(length);
            !identifier_offsets(length, &mask, "buffer_len").is_empty()
        })
    });
    fills_whole_slice || writes_buffer_len
}

fn validate_virtqueue_source(source: &str) -> Result<(), &'static str> {
    let body = function_body(source, "new").ok_or("missing Virtqueue::new")?;
    let mask = code_mask(body);
    if identifier_offsets(body, &mask, "allocate_contiguous_frames").is_empty() {
        return Err("virtqueue ring is not allocated through allocate_contiguous_frames");
    }
    if !body.contains("return Err(\"VirtIO queue: allocator returned non-contiguous ring frames\")")
    {
        return Err("virtqueue non-contiguity arm does not return the required error");
    }
    if !identifier_offsets(body, &mask, "error").is_empty() || body.contains("Continue anyway") {
        return Err("virtqueue non-contiguity still logs and continues");
    }
    Ok(())
}

/// The kernel log's screen sink (`kernel/src/fbcon`) takes records from
/// interrupt handlers as well as threads, so it must not block or be
/// strandable: its grid lock is only tried, with interrupts masked, and the
/// record path writes text into the grid and does not touch the framebuffer,
/// which the render thread draws.
fn validate_logger_source(logger: &str, fbcon: &str, records: &str) -> Result<(), &'static str> {
    let logger_mask = code_mask(logger);
    if !identifier_offsets(logger, &logger_mask, "LockedLogger").is_empty() {
        return Err("logger still uses the upstream blocking LockedLogger");
    }

    let grid_writer = function_body(fbcon, "with_grid").ok_or("missing with_grid")?;
    let interrupt_masked = call_arguments(grid_writer, "arch_without_interrupts")
        .into_iter()
        .any(|arguments| {
            arguments.first().is_some_and(|closure| {
                let mask = code_mask(closure);
                !identifier_offsets(closure, &mask, "try_lock").is_empty()
                    && identifier_offsets(closure, &mask, "lock").is_empty()
            })
        });
    if !interrupt_masked {
        return Err("console grid try_lock is not inside arch_without_interrupts");
    }

    let combined_log_impl = impl_body(logger, "impl Log for CombinedLogger")
        .ok_or("missing CombinedLogger Log implementation")?;
    let log_body = function_body(combined_log_impl, "log").ok_or("missing CombinedLogger::log")?;
    let log_mask = code_mask(log_body);
    if identifier_offsets(log_body, &log_mask, "log_record").is_empty() {
        return Err("CombinedLogger::log does not use the nonblocking console sink");
    }
    let record_writer = function_body(records, "log_record").ok_or("missing log_record")?;
    let record_mask = code_mask(record_writer);
    if identifier_offsets(record_writer, &record_mask, "with_grid").is_empty() {
        return Err("log_record does not write through with_grid");
    }
    if has_whole_framebuffer_clear(log_body) {
        return Err("logger hot path clears the whole framebuffer");
    }
    for (source, writer) in [(fbcon, "with_grid"), (records, "log_record"), (records, "write_tty")] {
        let body = function_body(source, writer).ok_or("missing console writer")?;
        if has_whole_framebuffer_clear(body) {
            return Err("logger hot path clears the whole framebuffer");
        }
        let mask = code_mask(body);
        if !identifier_offsets(body, &mask, "DISPLAY").is_empty()
            || !identifier_offsets(body, &mask, "copy_lines").is_empty()
        {
            return Err("a console writer touches the framebuffer");
        }
    }
    Ok(())
}

#[test]
fn virtqueue_dma_ring_allocation_fails_closed() {
    let source = repo_text("kernel/src/drivers/virtio/queue.rs");
    assert_eq!(validate_virtqueue_source(&source), Ok(()));
}

#[test]
fn virtqueue_validator_rejects_log_and_continue_shape() {
    let synthetic = r#"
        impl Virtqueue {
            fn new() -> Result<(), &'static str> {
                frame_allocator::allocate_contiguous_frames(2, &mut frames);
                if frames_are_not_contiguous {
                    log::error!("non-contiguous ring");
                    // Continue anyway
                }
                if allocator_contract_is_broken {
                    return Err("VirtIO queue: allocator returned non-contiguous ring frames");
                }
                Ok(())
            }
        }
    "#;
    assert!(validate_virtqueue_source(synthetic).is_err());
}

#[test]
fn framebuffer_log_sink_is_nonblocking_and_bounded() {
    let logger = repo_text("kernel/src/logger.rs");
    let fbcon = repo_text("kernel/src/fbcon/mod.rs");
    let records = repo_text("kernel/src/fbcon/records.rs");
    assert_eq!(validate_logger_source(&logger, &fbcon, &records), Ok(()));
}

#[test]
fn logger_validator_rejects_blocking_framebuffer_lock() {
    let logger = r#"
        struct CombinedLogger;
        impl Log for CombinedLogger {
            fn log(&self, record: &Record) {
                crate::fbcon::log_record(record.level(), record.args());
            }
        }
    "#;
    let blocking = r#"
        fn with_grid<R>(f: impl FnOnce(&mut Grid) -> R) -> Option<R> {
            crate::arch_without_interrupts(|| Some(f(&mut GRID.lock())))
        }
        pub fn log_record(level: Level, args: &Arguments) {
            with_grid(|grid| grid.write(b"x"));
        }
        pub fn write_tty(bytes: &[u8]) {
            with_grid(|grid| grid.write(bytes));
        }
    "#;
    assert!(validate_logger_source(logger, blocking, blocking).is_err());
    let drawing = r#"
        fn with_grid<R>(f: impl FnOnce(&mut Grid) -> R) -> Option<R> {
            crate::arch_without_interrupts(|| GRID.try_lock().map(|mut grid| f(&mut grid)))
        }
        pub fn log_record(level: Level, args: &Arguments) {
            with_grid(|grid| grid.write(b"x"));
            let display = DISPLAY.get();
        }
        pub fn write_tty(bytes: &[u8]) {
            with_grid(|grid| grid.write(bytes));
        }
    "#;
    assert!(validate_logger_source(logger, drawing, drawing).is_err());
}

#[test]
fn logger_validator_rejects_framebuffer_clear_in_shared_grid_writer() {
    let logger = repo_text("kernel/src/logger.rs");
    let fbcon = repo_text("kernel/src/fbcon/mod.rs");
    let records = repo_text("kernel/src/fbcon/records.rs");
    let anchor = "fn with_grid<R>(f: impl FnOnce(&mut Grid) -> R) -> Option<R> {";
    for clear in ["framebuffer.fill(0);", "core::ptr::write_bytes(buffer, 0, buffer_len);"] {
        let mutated = fbcon.replacen(anchor, &format!("{anchor}\n    {clear}"), 1);
        assert_ne!(mutated, fbcon, "mutation must reach with_grid");
        assert_eq!(validate_logger_source(&logger, &mutated, &records),
            Err("logger hot path clears the whole framebuffer"));
    }
}

#[test]
fn code_mask_raw_string_close_preserves_next_byte() {
    // Check scanner correctness with a consistent live-code token.
    // Hash counts 0, 1, 2, and 3 are exercised directly here, not just 0 and 1.
    for fixture in [
        r##"r"x"serial_println!"##,
        r##"r#"x"#serial_println!"##,
        r####"r##"x"##serial_println!"####,
        r#####"r###"x"###serial_println!"#####,
    ] {
        let mask = code_mask(fixture);
        let offset = fixture.find("serial_println!").unwrap();
        assert!(mask[offset], "raw-string close swallowed the next byte");
    }
    // A skipped ordinary identifier byte stays true in the default mask.
    // A skipped raw opener instead changes lexical state: the embedded quote
    // closes an ordinary string, hiding the real token after the raw close.
    // Exercised at a 0-then-1 hash boundary and again at a 1-then-2 hash
    // boundary, so the compound-skip fix is checked past the smallest counts too.
    for fixture in [
        r###"r"x"r#"a"b"#serial_println!"###,
        r######"r#"x"#r##"a"b"##serial_println!"######,
    ] {
        let mask = code_mask(fixture);
        let offset = fixture.find("serial_println!").unwrap();
        assert!(mask[offset], "raw-string close swallowed the next byte");
    }
}
