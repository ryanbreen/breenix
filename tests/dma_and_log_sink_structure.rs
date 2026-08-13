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

fn validate_logger_source(source: &str) -> Result<(), &'static str> {
    let source_mask = code_mask(source);
    if !identifier_offsets(source, &source_mask, "LockedLogger").is_empty() {
        return Err("logger still uses the upstream blocking LockedLogger");
    }

    let record_writer = function_body(source, "write_framebuffer_record")
        .ok_or("missing write_framebuffer_record")?;
    let interrupt_masked = call_arguments(record_writer, "arch_without_interrupts")
        .into_iter()
        .any(|arguments| {
            arguments.first().is_some_and(|closure| {
                let mask = code_mask(closure);
                !identifier_offsets(closure, &mask, "try_lock").is_empty()
                    && identifier_offsets(closure, &mask, "lock").is_empty()
            })
        });
    if !interrupt_masked {
        return Err("framebuffer try_lock is not inside arch_without_interrupts");
    }

    let combined_log_impl = impl_body(source, "impl Log for CombinedLogger")
        .ok_or("missing CombinedLogger Log implementation")?;
    let log_body = function_body(combined_log_impl, "log").ok_or("missing CombinedLogger::log")?;
    let log_mask = code_mask(log_body);
    if identifier_offsets(log_body, &log_mask, "write_framebuffer_record").is_empty() {
        return Err("CombinedLogger::log does not use the nonblocking framebuffer helper");
    }
    if has_whole_framebuffer_clear(log_body) || has_whole_framebuffer_clear(record_writer) {
        return Err("logger hot path clears the whole framebuffer");
    }

    let framebuffer_impl =
        impl_body(source, "impl LogFrameBuffer").ok_or("missing LogFrameBuffer implementation")?;
    let framebuffer_mask = code_mask(framebuffer_impl);
    for function in identifier_offsets(framebuffer_impl, &framebuffer_mask, "fn") {
        let mut cursor = function + 2;
        while cursor < framebuffer_impl.len()
            && (!framebuffer_mask[cursor]
                || framebuffer_impl.as_bytes()[cursor].is_ascii_whitespace())
        {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < framebuffer_impl.len()
            && framebuffer_mask[cursor]
            && identifier_byte(framebuffer_impl.as_bytes()[cursor])
        {
            cursor += 1;
        }
        let name = &framebuffer_impl[name_start..cursor];
        if name == "new" {
            continue;
        }
        let Some(body) = function_body(&framebuffer_impl[function..], name) else {
            continue;
        };
        if has_whole_framebuffer_clear(body) {
            return Err("LogFrameBuffer clears the whole framebuffer after initialization");
        }
    }

    let framebuffer_write_impl = impl_body(source, "impl fmt::Write for LogFrameBuffer")
        .ok_or("missing LogFrameBuffer fmt::Write implementation")?;
    if has_whole_framebuffer_clear(framebuffer_write_impl) {
        return Err("LogFrameBuffer fmt::Write path clears the whole framebuffer");
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
    let source = repo_text("kernel/src/logger.rs");
    assert_eq!(validate_logger_source(&source), Ok(()));
}

#[test]
fn logger_validator_rejects_blocking_framebuffer_lock() {
    let synthetic = r#"
        struct LogFrameBuffer;
        impl LogFrameBuffer {
            fn new() -> Self { Self }
            fn write_record(&mut self) {}
        }

        fn write_framebuffer_record(record: &Record) {
            let Some(framebuffer) = LOG_FRAMEBUFFER.get() else { return; };
            crate::arch_without_interrupts(|| {
                let mut framebuffer = framebuffer.lock();
                framebuffer.write_record(record);
            });
        }

        struct CombinedLogger;
        impl Log for CombinedLogger {
            fn log(&self, record: &Record) {
                write_framebuffer_record(record);
            }
        }
    "#;
    assert!(validate_logger_source(synthetic).is_err());
}
