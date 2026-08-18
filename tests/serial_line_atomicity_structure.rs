use std::fs;
use std::path::{Path, PathBuf};

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

fn rust_sources_under(directory: &Path) -> Vec<PathBuf> {
    fn collect(directory: &Path, sources: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(directory)
            .unwrap_or_else(|_| panic!("read source directory {}", directory.display()));
        for entry in entries {
            let path = entry.expect("read source directory entry").path();
            if path.is_dir() {
                collect(&path, sources);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }

    let mut sources = Vec::new();
    collect(directory, &mut sources);
    sources.sort();
    sources
}

fn serial_module_sources() -> Vec<(PathBuf, String)> {
    rust_sources_under(&repo_root().join("kernel/src"))
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("read repository file {}", path.display()));
            let mask = code_mask(&source);
            let defines_serial_port = identifier_offsets(&source, &mask, "SERIAL1")
                .into_iter()
                .any(|offset| {
                    let start = offset.saturating_sub(32);
                    let prefix = &source[start..offset];
                    let prefix_mask = code_mask(prefix);
                    !identifier_offsets(prefix, &prefix_mask, "static").is_empty()
                });
            defines_serial_port.then_some((path, source))
        })
        .collect()
}

fn validate_tty_source(source: &str) -> Result<(), &'static str> {
    let write_bytes =
        function_body(source, "write_bytes").ok_or("missing TtyDevice::write_bytes")?;
    let write_mask = code_mask(write_bytes);
    if !identifier_offsets(write_bytes, &write_mask, "write_byte").is_empty() {
        return Err("TtyDevice::write_bytes calls write_byte");
    }
    if identifier_offsets(write_bytes, &write_mask, "write_bytes_atomic").is_empty() {
        return Err("TtyDevice::write_bytes does not call write_bytes_atomic");
    }
    if identifier_offsets(write_bytes, &write_mask, "for_each_line_segment").is_empty() {
        return Err("TtyDevice::write_bytes does not use the line segmentation helper");
    }

    let segmenter = function_body(source, "for_each_line_segment")
        .ok_or("missing for_each_line_segment helper")?;
    let segmenter_mask = code_mask(segmenter);
    if identifier_offsets(segmenter, &segmenter_mask, "split_inclusive").is_empty()
        || !segmenter.contains("b'\\n'")
    {
        return Err("line segmentation helper does not split inclusively on newline");
    }
    Ok(())
}

fn validate_serial_module(source: &str) -> Result<(), &'static str> {
    if !source.contains("pub fn write_bytes_atomic") {
        return Err("serial module does not expose write_bytes_atomic");
    }
    let writer = function_body(source, "write_bytes_atomic")
        .ok_or("serial module is missing write_bytes_atomic body")?;
    let writer_mask = code_mask(writer);
    if identifier_offsets(writer, &writer_mask, "SERIAL1").len() != 1 {
        return Err("write_bytes_atomic must use the SERIAL1 port exactly once");
    }
    let lock_count = identifier_offsets(writer, &writer_mask, "lock").len();
    if lock_count != 1 {
        return Err("write_bytes_atomic must acquire the serial lock exactly once");
    }
    Ok(())
}

#[test]
fn tty_writes_line_segments_through_atomic_serial_path() {
    let source = repo_text("kernel/src/tty/driver.rs");
    assert_eq!(validate_tty_source(&source), Ok(()));
}

#[test]
fn every_serial_module_has_a_single_lock_batched_writer() {
    let modules = serial_module_sources();
    assert_eq!(
        modules.len(),
        2,
        "expected to discover both architecture serial modules"
    );
    for (path, source) in modules {
        assert_eq!(
            validate_serial_module(&source),
            Ok(()),
            "serial module {}",
            path.display()
        );
    }
}

#[test]
fn tty_validator_rejects_per_byte_serial_writes() {
    let synthetic = r#"
        fn for_each_line_segment(buf: &[u8], mut write: impl FnMut(&[u8])) {
            for segment in buf.split_inclusive(|byte| *byte == b'\n') {
                write(segment);
            }
        }

        impl TtyDevice {
            pub fn write_bytes(&self, buf: &[u8]) {
                for byte in buf {
                    crate::serial::write_byte(*byte);
                }
                crate::serial::write_bytes_atomic(buf);
                for_each_line_segment(buf, |_| {});
            }
        }
    "#;
    assert_eq!(
        validate_tty_source(synthetic),
        Err("TtyDevice::write_bytes calls write_byte")
    );
}
