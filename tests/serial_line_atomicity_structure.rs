use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn kernel_sources() -> Vec<(String, String)> {
    let root = repo_root();
    rust_sources_under(&root.join("kernel/src"))
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .expect("kernel source below repository root")
                .to_string_lossy()
                .replace('\\', "/");
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("read repository file {}", path.display()));
            (relative, source)
        })
        .collect()
}

fn with_synthetic_source(
    sources: &[(String, String)],
    path: &str,
    synthetic_source: &str,
) -> Vec<(String, String)> {
    let mut perturbed = sources.to_vec();
    perturbed.push((path.to_owned(), synthetic_source.to_owned()));
    perturbed.sort_by(|left, right| left.0.cmp(&right.0));
    perturbed
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

fn code_offsets(source: &str, mask: &[bool], needle: &str) -> Vec<usize> {
    source
        .match_indices(needle)
        .filter_map(|(offset, _)| mask.get(offset).copied().unwrap_or(false).then_some(offset))
        .collect()
}

fn all_identifiers<'a>(source: &'a str, mask: &[bool]) -> Vec<(usize, &'a str)> {
    let bytes = source.as_bytes();
    let mut identifiers = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !mask[cursor]
            || !identifier_byte(bytes[cursor])
            || cursor
                .checked_sub(1)
                .is_some_and(|before| mask[before] && identifier_byte(bytes[before]))
        {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        identifiers.push((start, &source[start..cursor]));
    }
    identifiers
}

fn next_code(source: &str, mask: &[bool], from: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    (from..bytes.len()).find(|index| mask[*index] && !bytes[*index].is_ascii_whitespace())
}

fn previous_code(source: &str, mask: &[bool], before: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    (0..before)
        .rev()
        .find(|index| mask[*index] && !bytes[*index].is_ascii_whitespace())
}

fn preceded_by_keyword(source: &str, mask: &[bool], offset: usize, keyword: &str) -> bool {
    let bytes = source.as_bytes();
    let Some(end) = previous_code(source, mask, offset) else {
        return false;
    };
    if !identifier_byte(bytes[end]) {
        return false;
    }
    let mut start = end;
    while start > 0 && mask[start - 1] && identifier_byte(bytes[start - 1]) {
        start -= 1;
    }
    &source[start..end + 1] == keyword
}

fn matching_brace(source: &str, mask: &[bool], open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
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
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
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

type Anchor = (String, String);
type Census = BTreeMap<Anchor, usize>;

fn header_cfg(header: &str, mask: &[bool], keyword: usize) -> String {
    let bytes = header.as_bytes();
    let mut attributes = Vec::new();
    for offset in code_offsets(header, mask, "#[cfg") {
        if offset >= keyword {
            break;
        }
        let Some(paren) = next_code(header, mask, offset + "#[cfg".len()) else {
            continue;
        };
        if bytes[paren] != b'(' {
            continue;
        }
        let mut depth = 0usize;
        for close in offset..bytes.len() {
            match bytes[close] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        let compact: String = header[offset..close + 1]
                            .chars()
                            .filter(|character| !character.is_whitespace() && *character != '"')
                            .collect();
                        attributes.push(compact);
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    if attributes.is_empty() {
        String::new()
    } else {
        format!("{} ", attributes.join(" "))
    }
}

fn impl_segment(header: &str, mask: &[bool], keyword: usize) -> String {
    let kept: Vec<u8> = header.as_bytes()[keyword..]
        .iter()
        .zip(&mask[keyword..])
        .filter_map(|(byte, code)| code.then_some(*byte))
        .collect();
    let text = String::from_utf8_lossy(&kept)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match text.find(" where ") {
        Some(clause) => text[..clause].to_owned(),
        None => text,
    }
}

fn item_segment(header: &str, mask: &[bool]) -> Option<String> {
    let bytes = header.as_bytes();
    let named = |keyword: usize, length: usize| -> Option<String> {
        let mut cursor = keyword + length;
        while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        (cursor > start).then(|| header[start..cursor].to_owned())
    };

    let declaration = identifier_offsets(header, mask, "fn")
        .into_iter()
        .filter_map(|offset| named(offset, "fn".len()).map(|name| (offset, format!("fn {name}"))))
        .next_back()
        .or_else(|| {
            ["impl", "mod", "trait", "struct"]
                .into_iter()
                .flat_map(|keyword| {
                    identifier_offsets(header, mask, keyword)
                        .into_iter()
                        .map(move |offset| (keyword, offset))
                })
                .max_by_key(|(_, offset)| *offset)
                .and_then(|(keyword, offset)| match keyword {
                    "impl" => Some((offset, impl_segment(header, mask, offset))),
                    _ => named(offset, keyword.len())
                        .map(|name| (offset, format!("{keyword} {name}"))),
                })
        });
    let (keyword, segment) = declaration?;
    Some(format!("{}{segment}", header_cfg(header, mask, keyword)))
}

fn item_spans(source: &str, mask: &[bool]) -> Vec<(usize, usize, String)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut header = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for index in 0..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b'{' => {
                stack.push((index, header));
                header = index + 1;
            }
            b'}' => {
                if let Some((open, start)) = stack.pop() {
                    if let Some(segment) = item_segment(&source[start..open], &mask[start..open]) {
                        spans.push((open, index, segment));
                    }
                }
                header = index + 1;
            }
            b';' if paren_depth == 0 && bracket_depth == 0 => header = index + 1,
            _ => {}
        }
    }
    spans
}

fn rendered_item_spans(spans: &[(usize, usize, String)]) -> Vec<(usize, usize, String)> {
    let mut ordered = spans.to_vec();
    ordered.sort_by_key(|(open, _, _)| *open);

    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut rendered = Vec::with_capacity(ordered.len());
    for (open, close, segment) in ordered {
        while stack
            .last()
            .is_some_and(|(ancestor_close, _)| *ancestor_close < open)
        {
            stack.pop();
        }
        let path = match stack.last() {
            Some((_, parent)) => format!("{parent}::{segment}"),
            None => segment,
        };
        stack.push((close, path.clone()));
        rendered.push((open, close, path));
    }

    let mut path_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, _, path) in &rendered {
        *path_counts.entry(path.clone()).or_default() += 1;
    }
    for (_, _, path) in &mut rendered {
        if path_counts.get(path).is_some_and(|count| *count > 1) {
            path.push_str(" [duplicate item path]");
        }
    }
    rendered
}

fn item_path_at(spans: &[(usize, usize, String)], offset: usize) -> String {
    spans
        .iter()
        .filter(|(open, close, _)| *open <= offset && offset <= *close)
        .max_by_key(|(open, _, _)| *open)
        .map(|(_, _, path)| path.clone())
        .unwrap_or_default()
}

fn census<F>(sources: &[(String, String)], mut matcher: F) -> Census
where
    F: FnMut(&str, &[bool]) -> Vec<usize>,
{
    let mut census = Census::new();
    for (path, source) in sources {
        let mask = code_mask(source);
        let matches = matcher(source, &mask);
        if matches.is_empty() {
            continue;
        }
        let spans = rendered_item_spans(&item_spans(source, &mask));
        for offset in matches {
            *census
                .entry((path.clone(), item_path_at(&spans, offset)))
                .or_default() += 1;
        }
    }
    census
}

fn expected_census(anchors: &[(&str, &str, usize)]) -> Census {
    let mut census = Census::new();
    for (path, item, count) in anchors {
        let anchor = ((*path).to_owned(), (*item).to_owned());
        assert!(
            census.insert(anchor, *count).is_none(),
            "duplicate census anchor {path} :: {item}"
        );
    }
    census
}

fn census_diff(actual: &Census, anchors: &[(&str, &str, usize)]) -> Vec<String> {
    let expected = expected_census(anchors);
    let mut diff = Vec::new();
    for (anchor, count) in actual {
        match expected.get(anchor) {
            None => diff.push(format!(
                "+ {} :: {}  ({count} occurrences, expected none)",
                anchor.0, anchor.1
            )),
            Some(want) if want != count => diff.push(format!(
                "~ {} :: {}  (expected {want}, found {count})",
                anchor.0, anchor.1
            )),
            Some(_) => {}
        }
    }
    for (anchor, count) in &expected {
        if !actual.contains_key(anchor) {
            diff.push(format!(
                "- {} :: {}  (expected {count}, found none)",
                anchor.0, anchor.1
            ));
        }
    }
    diff
}

fn validate_census(actual: &Census, anchors: &[(&str, &str, usize)]) -> Result<(), Vec<String>> {
    let diff = census_diff(actual, anchors);
    diff.is_empty().then_some(()).ok_or(diff)
}

fn construct_body_span(
    source: &str,
    mask: &[bool],
    keyword_offset: usize,
    keyword: &str,
) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut saw_in = keyword != "for";
    let start = keyword_offset + keyword.len();
    for index in start..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.checked_sub(1)?,
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.checked_sub(1)?,
            b'{' if paren_depth == 0 && bracket_depth == 0 && saw_in => {
                return matching_brace(source, mask, index).map(|close| (index, close));
            }
            b';' if paren_depth == 0 && bracket_depth == 0 => return None,
            _ if paren_depth == 0 && bracket_depth == 0 && keyword == "for" => {
                saw_in |= bytes.get(index..index + 2) == Some(b"in")
                    && mask.get(index + 1).copied().unwrap_or(false)
                    && !index
                        .checked_sub(1)
                        .and_then(|before| bytes.get(before))
                        .is_some_and(|byte| identifier_byte(*byte))
                    && !bytes
                        .get(index + 2)
                        .is_some_and(|byte| identifier_byte(*byte));
            }
            _ => {}
        }
    }
    None
}

fn loop_body_spans(source: &str, mask: &[bool]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for keyword in ["for", "while", "loop"] {
        for offset in identifier_offsets(source, mask, keyword) {
            if let Some(span) = construct_body_span(source, mask, offset, keyword) {
                spans.push(span);
            }
        }
    }
    spans
}

fn identifier_is_call(source: &str, mask: &[bool], offset: usize, name: &str) -> bool {
    let bytes = source.as_bytes();
    next_code(source, mask, offset + name.len()).is_some_and(|open| bytes[open] == b'(')
        && !preceded_by_keyword(source, mask, offset, "fn")
}

fn unlocked_multi_byte_write_calls<'a>(source: &'a str, mask: &[bool]) -> Vec<(usize, &'a str)> {
    let loops = loop_body_spans(source, mask);
    all_identifiers(source, mask)
        .into_iter()
        .filter(|(offset, name)| {
            if !name.starts_with("raw_") || !identifier_is_call(source, mask, *offset, name) {
                return false;
            }
            if name.contains("str") || name.contains("bytes") {
                return true;
            }
            name.contains("char")
                && loops
                    .iter()
                    .any(|(open, close)| *open < *offset && *offset < *close)
        })
        .collect()
}

fn unlocked_multi_byte_write_census(sources: &[(String, String)]) -> Census {
    census(sources, |source, mask| {
        unlocked_multi_byte_write_calls(source, mask)
            .into_iter()
            .map(|(offset, _)| offset)
            .collect()
    })
}

fn function_spans(source: &str, mask: &[bool]) -> Vec<(usize, usize)> {
    item_spans(source, mask)
        .into_iter()
        .filter_map(|(open, close, item)| {
            (item.starts_with("fn ") || item.contains("] fn ")).then_some((open, close))
        })
        .collect()
}

fn enclosing_function_span(spans: &[(usize, usize)], offset: usize) -> Option<(usize, usize)> {
    spans
        .iter()
        .filter(|(open, close)| *open <= offset && offset <= *close)
        .max_by_key(|(open, _)| *open)
        .copied()
}

fn compact_code(source: &str, mask: &[bool], start: usize, end: usize) -> String {
    source.as_bytes()[start..end]
        .iter()
        .zip(&mask[start..end])
        .filter_map(|(byte, code)| {
            (*code && !byte.is_ascii_whitespace()).then_some(char::from(*byte))
        })
        .collect()
}

fn hex_literal_offsets(source: &str, mask: &[bool], value: u64) -> Vec<usize> {
    let bytes = source.as_bytes();
    let mut offsets = Vec::new();
    let mut cursor = 0usize;
    while cursor + 2 < bytes.len() {
        let starts_hex = mask[cursor]
            && bytes[cursor] == b'0'
            && matches!(bytes[cursor + 1], b'x' | b'X')
            && mask[cursor + 1]
            && !cursor
                .checked_sub(1)
                .and_then(|before| bytes.get(before))
                .is_some_and(|byte| identifier_byte(*byte));
        if !starts_hex {
            cursor += 1;
            continue;
        }

        let start = cursor;
        cursor += 2;
        let digits = cursor;
        while cursor < bytes.len()
            && mask[cursor]
            && (bytes[cursor].is_ascii_hexdigit() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        let normalized: String = source[digits..cursor]
            .chars()
            .filter(|character| *character != '_')
            .collect();
        if !normalized.is_empty()
            && u64::from_str_radix(&normalized, 16).is_ok_and(|literal| literal == value)
        {
            offsets.push(start);
        }
    }
    offsets
}

fn span_has_member_write_call(source: &str, mask: &[bool], (open, close): (usize, usize)) -> bool {
    identifier_offsets(source, mask, "write")
        .into_iter()
        .filter(|offset| open < *offset && *offset < close)
        .any(|offset| {
            identifier_is_call(source, mask, offset, "write")
                && previous_code(source, mask, offset)
                    .is_some_and(|dot| source.as_bytes()[dot] == b'.')
        })
}

fn raw_serial_primitive_write_offsets(source: &str, mask: &[bool]) -> Vec<usize> {
    let functions = function_spans(source, mask);
    let uart_helpers = identifier_offsets(source, mask, "uart_virt")
        .into_iter()
        .filter(|offset| identifier_is_call(source, mask, *offset, "uart_virt"))
        .collect::<Vec<_>>();
    let mut writes = identifier_offsets(source, mask, "write_volatile")
        .into_iter()
        .filter(|offset| identifier_is_call(source, mask, *offset, "write_volatile"))
        .filter(|offset| {
            let Some((open, close)) = enclosing_function_span(&functions, *offset) else {
                return false;
            };
            uart_helpers
                .iter()
                .any(|helper| open < *helper && *helper < close)
        })
        .collect::<Vec<_>>();

    for port in hex_literal_offsets(source, mask, 0x3f8) {
        let Some(function) = enclosing_function_span(&functions, port) else {
            continue;
        };
        let prefix = compact_code(source, mask, function.0, port);
        let constructs_port =
            prefix.ends_with("Port::new(") || prefix.ends_with("Port::<u8>::new(");
        if constructs_port && span_has_member_write_call(source, mask, function) {
            writes.push(port);
        }
    }

    writes.sort_unstable();
    writes
}

fn raw_serial_primitive_census(sources: &[(String, String)]) -> Census {
    census(sources, raw_serial_primitive_write_offsets)
}

const RAW_SERIAL_PRIMITIVE_ANCHORS: &[(&str, &str, usize)] = &[
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn raw_uart_char",
        1,
    ),
    ("kernel/src/arch_impl/aarch64/smp.rs", "fn raw_uart_char", 1),
    (
        "kernel/src/arch_impl/aarch64/syscall_entry.rs",
        "fn emit_el0_syscall_marker",
        1,
    ),
    (
        "kernel/src/graphics/particles.rs",
        "fn animation_thread_entry::fn raw_char",
        1,
    ),
    (
        "kernel/src/interrupts/context_switch.rs",
        "fn raw_serial_char",
        1,
    ),
    (
        "kernel/src/interrupts/context_switch.rs",
        "fn raw_serial_str",
        1,
    ),
    ("kernel/src/per_cpu.rs", "fn can_schedule", 1),
    (
        "kernel/src/serial.rs",
        "fn emergency_print::impl fmt::Write for EmergencySerial::fn write_str",
        1,
    ),
    ("kernel/src/serial_aarch64.rs", "fn raw_serial_char", 2),
    ("kernel/src/serial_aarch64.rs", "fn raw_serial_str", 2),
    (
        "kernel/src/syscall/handler.rs",
        "fn raw_serial_str_local",
        1,
    ),
    ("kernel/src/tracing/output.rs", "fn raw_serial_char", 2),
];

fn validate_raw_serial_primitive_census(sources: &[(String, String)]) -> Result<(), Vec<String>> {
    validate_census(
        &raw_serial_primitive_census(sources),
        RAW_SERIAL_PRIMITIVE_ANCHORS,
    )
}

const UNLOCKED_MULTI_BYTE_WRITE_ANCHORS: &[(&str, &str, usize)] = &[
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn check_inline_eret_resume_pc",
        5,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn check_inline_save_resume_point",
        7,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn check_need_resched_and_switch_arm64",
        4,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dispatch_thread_locked",
        13,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dump_all_dispatch_mismatch_snapshots",
        7,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dump_all_eret_frame_anomaly_snapshots",
        9,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dump_all_eret_guard_records",
        9,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dump_all_idle_redirect_histories",
        13,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dump_all_inline_save_skew_snapshots",
        7,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dump_all_last_dispatched_tids",
        4,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dump_all_save_skew_snapshots",
        11,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dump_dispatch_trace",
        12,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn dump_stack_pivot_alias_history",
        9,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn emit_el0_entry_marker",
        2,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn emit_schedule_boot_marker",
        1,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn log_bad_thread_sp",
        11,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn log_idle_thread_context",
        11,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn log_last_defer_requeue_snapshot",
        9,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn raw_uart_dec",
        1,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn raw_uart_hex",
        2,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn raw_uart_str",
        1,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn record_inline_elr_divergence",
        6,
    ),
    // `record_resume_pc_refusal` emits from refusal paths that hold the
    // scheduler lock, where the locked writer is unavailable. It is capped at
    // 16 emissions per boot and is never periodic; a torn record can only
    // under-count the report-only census, never flip a verdict.
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn record_resume_pc_refusal",
        11,
    ),
    // `emit_resume_pc_census` is the fatal-postmortem form and therefore must
    // remain lock-free by construction.
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn emit_resume_pc_census",
        9,
    ),
    // `drain_asm_resume_pc_refusals`, `record_resume_pc_refusal_locked`, and
    // `emit_resume_pc_census_locked` use the locked writer and intentionally
    // do not appear here. If they enter this raw-writer census, restore the
    // locked write in production instead of admitting them to the test.
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn restore_kernel_context_inline",
        13,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn save_kernel_context_inline",
        22,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn save_userspace_context_inline",
        11,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn set_next_ttbr0_for_thread",
        6,
    ),
    // `take_inline_ret_dispatch_info` and the four ret-dispatch oracle injectors
    // emit dispatch-path markers while the scheduler lock is held, so the
    // locked writer is unavailable. All five are one-shot or emission-capped,
    // never periodic. `[RET_DISPATCH_REFUSED:` is a service-sequence census
    // only, never a gate condition: a torn line can only under-count the
    // reported number, not flip a verdict.
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn take_inline_ret_dispatch_info",
        10,
    ),
    // Saved-LR custody, PR-B round 3. `set_saved_lr` reports an EL1 saved link
    // register that is not a kernel PC and `record_ret_stage_refusal` reports a
    // ret-dispatch staging copy that disagreed with what was admitted; both run
    // inside a dispatch with the scheduler lock held, so the locked writer is
    // unavailable, and both are emission-capped at 8 per boot rather than
    // periodic. `[LR_NONTEXT:` is census only. `[RET_STAGE_REFUSED:` IS a gate
    // condition, and a torn line can only under-count it, never invent one.
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn set_saved_lr",
        6,
    ),
    (
        "kernel/src/arch_impl/aarch64/context_switch.rs",
        "fn record_ret_stage_refusal",
        6,
    ),
    (
        "kernel/src/arch_impl/aarch64/exception.rs",
        "fn defer_current_user_thread_sigsegv_exit",
        6,
    ),
    (
        "kernel/src/arch_impl/aarch64/exception.rs",
        "fn dump_el1_fatal_frame_and_dispatch_trace",
        13,
    ),
    (
        "kernel/src/arch_impl/aarch64/exception.rs",
        "fn dump_el1_first_fault",
        23,
    ),
    (
        "kernel/src/arch_impl/aarch64/exception.rs",
        "fn dump_fatal_postmortem_once",
        18,
    ),
    (
        "kernel/src/arch_impl/aarch64/exception.rs",
        "fn dump_fatal_postmortem_section",
        1,
    ),
    (
        "kernel/src/arch_impl/aarch64/exception.rs",
        "fn dump_stack_classification",
        6,
    ),
    (
        "kernel/src/arch_impl/aarch64/exception.rs",
        "fn handle_sync_exception",
        196,
    ),
    (
        "kernel/src/arch_impl/aarch64/exception.rs",
        "fn raw_uart_hex_u32",
        2,
    ),
    // The per-CPU stack-top ownership refusal record, in the one function both
    // custody sides funnel through. It runs on the dispatch path, so it cannot
    // take the serial lock; it is bounded to 16 emissions for the whole boot.
    (
        "kernel/src/arch_impl/aarch64/percpu.rs",
        "fn record_percpu_stack_alien",
        9,
    ),
    // The CPU-identity split record: a carried CPU index that disagreed with
    // the hardware identity where the decision was made. Same constraints as
    // the alien record above — dispatch path, no lock, bounded to 16 emissions
    // for the whole boot — and deliberately its own literal so the shape can
    // never again be absorbed by the alien record.
    (
        "kernel/src/arch_impl/aarch64/percpu.rs",
        "fn record_cpu_identity_split",
        6,
    ),
    (
        "kernel/src/arch_impl/aarch64/timer_interrupt.rs",
        "fn dump_lockup_state",
        60,
    ),
    (
        "kernel/src/arch_impl/aarch64/timer_interrupt.rs",
        "fn dump_trace_counters",
        10,
    ),
    (
        "kernel/src/arch_impl/aarch64/timer_interrupt.rs",
        "fn print_hex_u64",
        1,
    ),
    (
        "kernel/src/arch_impl/aarch64/timer_interrupt.rs",
        "fn print_timer_count_decimal",
        1,
    ),
    (
        "kernel/src/arch_impl/aarch64/timer_interrupt.rs",
        "fn raw_serial_str",
        1,
    ),
    (
        "kernel/src/arch_impl/aarch64/timer_interrupt.rs",
        "fn timer_interrupt_handler",
        6,
    ),
    (
        "kernel/src/interrupts/context_switch.rs",
        "fn check_need_resched_and_switch",
        4,
    ),
    (
        "kernel/src/interrupts/context_switch.rs",
        "fn note_dispatch_guard_unavailable",
        5,
    ),
    (
        "kernel/src/interrupts/context_switch.rs",
        "fn raw_serial_u64",
        1,
    ),
    (
        "kernel/src/interrupts/context_switch.rs",
        "fn refuse_unpublished_dispatch",
        3,
    ),
    (
        "kernel/src/interrupts/context_switch.rs",
        "fn restore_userspace_thread_context",
        10,
    ),
    (
        "kernel/src/interrupts/context_switch.rs",
        "fn switch_to_thread",
        10,
    ),
    // #608 F4: the timed-futex failure record. It fires only when a timed
    // wait arbitrates to something other than ETIMEDOUT, is budgeted to 32
    // lines a boot, and must stay lock-free because the futex wait reaches it
    // with preemption disabled.
    (
        "kernel/src/syscall/futex_timeout_record.rs",
        "fn record",
        12,
    ),
    (
        "kernel/src/syscall/handler.rs",
        "fn emit_ring3_syscall_marker",
        2,
    ),
    (
        "kernel/src/task/ret_zero_pc_oracle.rs",
        "#[cfg(all(target_arch=aarch64,feature=ret_zero_pc_oracle_exec))] fn inject_exec_commit_if_armed",
        2,
    ),
    (
        "kernel/src/task/ret_zero_pc_oracle.rs",
        "#[cfg(all(target_arch=aarch64,feature=ret_zero_pc_oracle))] fn inject_ret_zero_pc_if_armed",
        3,
    ),
    (
        "kernel/src/task/ret_zero_pc_oracle.rs",
        "#[cfg(all(target_arch=aarch64,feature=lr_poison_oracle))] fn inject_saved_lr_if_armed",
        3,
    ),
    (
        "kernel/src/task/ret_zero_pc_oracle.rs",
        "#[cfg(all(target_arch=aarch64,feature=ret_stack_pc_oracle))] fn inject_ret_stack_pc_if_armed",
        4,
    ),
    (
        "kernel/src/task/ret_zero_pc_oracle.rs",
        "#[cfg(all(target_arch=aarch64,feature=ret_floor_oracle))] fn inject_ret_floor_if_armed",
        3,
    ),
    // The resume-PC oracle injectors emit one-shot markers while the scheduler
    // lock is held, matching the existing ret-dispatch injector exception.
    (
        "kernel/src/task/ret_zero_pc_oracle.rs",
        "#[cfg(all(target_arch=aarch64,any(feature=resume_pc_el0_kernel_oracle,feature=resume_pc_el0_tid_oracle),not(feature=resume_pc_el0_frame_oracle)))] fn inject_el0_resume_pc_if_armed",
        6,
    ),
    (
        "kernel/src/task/ret_zero_pc_oracle.rs",
        "#[cfg(all(target_arch=aarch64,any(feature=resume_pc_el1_oracle,feature=eret_zero_pc_oracle,all(feature=resume_pc_el0_frame_oracle,any(feature=resume_pc_el0_kernel_oracle,feature=resume_pc_el0_tid_oracle)))))] fn inject_el1_frame_resume_pc_if_armed",
        6,
    ),
    (
        "kernel/src/task/ret_zero_pc_oracle.rs",
        "#[cfg(all(target_arch=aarch64,feature=resume_pc_el0_frame_oracle,any(feature=resume_pc_el0_kernel_oracle,feature=resume_pc_el0_tid_oracle)))] fn inject_el0_frame_resume_pc_if_armed",
        6,
    ),
    (
        "kernel/src/task/scheduler.rs",
        "#[cfg(target_arch=aarch64)] fn dump_cpu_state_history",
        9,
    ),
    // The pinned-wake hold emits a one-shot marker from inside the scheduler
    // lock with interrupts masked, where the logger's own lock would deadlock:
    // the same exception the injectors above already carry. It fires at most
    // once per boot, and on a healthy boot zero times.
    // claim-lint:ok: 0 of 3 strict boots and 0 of 3 production boots at this
    // head printed it --
    // docs/planning/green-program/aarch64-testing/serials/slice3d/01-strict-x3.txt
    // and 02-prod-boot1.txt with its 2 siblings
    (
        "kernel/src/task/scheduler.rs",
        "impl Scheduler::fn hold_pinned_wake_for_home",
        5,
    ),
    (
        "kernel/src/test_framework/registry.rs",
        "fn test_serial_output",
        1,
    ),
    ("kernel/src/tracing/output.rs", "fn dump_all_buffers", 6),
    ("kernel/src/tracing/output.rs", "fn dump_buffer", 7),
    ("kernel/src/tracing/output.rs", "fn dump_counters", 11),
    ("kernel/src/tracing/output.rs", "fn dump_event_summary", 6),
    ("kernel/src/tracing/output.rs", "fn dump_latest_events", 3),
    ("kernel/src/tracing/output.rs", "fn dump_on_panic", 2),
    ("kernel/src/tracing/output.rs", "fn dump_providers", 7),
    (
        "kernel/src/tracing/output.rs",
        "fn format_event_to_serial",
        7,
    ),
    ("kernel/src/tracing/output.rs", "fn raw_serial_dec", 1),
    ("kernel/src/tracing/output.rs", "fn raw_serial_hex", 2),
    ("kernel/src/tracing/output.rs", "fn raw_serial_hex16", 1),
    ("kernel/src/tracing/output.rs", "fn raw_serial_str", 1),
    (
        "kernel/src/tty/driver.rs",
        "impl TtyDevice::fn send_signal_to_foreground_nonblock",
        1,
    ),
    (
        "kernel/src/tty/driver.rs",
        "impl TtyDevice::fn send_signal_to_process_nonblock",
        1,
    ),
];

fn validate_unlocked_multi_byte_write_census(
    sources: &[(String, String)],
) -> Result<(), Vec<String>> {
    validate_census(
        &unlocked_multi_byte_write_census(sources),
        UNLOCKED_MULTI_BYTE_WRITE_ANCHORS,
    )
}

fn condition_has_modulo_integer_literal(
    source: &str,
    mask: &[bool],
    start: usize,
    end: usize,
) -> bool {
    let bytes = source.as_bytes();
    (start..end).any(|percent| {
        if !mask[percent] || bytes[percent] != b'%' {
            return false;
        }
        let Some(mut literal) = next_code(source, mask, percent + 1) else {
            return false;
        };
        while literal < end && bytes[literal] == b'(' {
            let Some(next) = next_code(source, mask, literal + 1) else {
                return false;
            };
            literal = next;
        }
        literal < end && bytes[literal].is_ascii_digit()
    })
}

fn periodic_guard_body_spans(source: &str, mask: &[bool]) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    for offset in identifier_offsets(source, mask, "if") {
        let condition_start = offset + "if".len();
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        for open in condition_start..bytes.len() {
            if !mask[open] {
                continue;
            }
            match bytes[open] {
                b'(' => paren_depth += 1,
                b')' => paren_depth = paren_depth.saturating_sub(1),
                b'[' => bracket_depth += 1,
                b']' => bracket_depth = bracket_depth.saturating_sub(1),
                b'{' if paren_depth == 0 && bracket_depth == 0 => {
                    if condition_has_modulo_integer_literal(source, mask, condition_start, open) {
                        if let Some(close) = matching_brace(source, mask, open) {
                            spans.push((open, close));
                        }
                    }
                    break;
                }
                b';' if paren_depth == 0 && bracket_depth == 0 => break,
                _ => {}
            }
        }
    }
    spans
}

fn validate_no_periodic_unlocked_multi_byte_writes(
    sources: &[(String, String)],
) -> Result<(), Vec<String>> {
    let mut violations: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for (path, source) in sources {
        let mask = code_mask(source);
        let guards = periodic_guard_body_spans(source, &mask);
        if guards.is_empty() {
            continue;
        }
        let spans = rendered_item_spans(&item_spans(source, &mask));
        for (offset, writer) in unlocked_multi_byte_write_calls(source, &mask) {
            if guards
                .iter()
                .any(|(open, close)| *open < offset && offset < *close)
            {
                *violations
                    .entry((
                        path.clone(),
                        item_path_at(&spans, offset),
                        writer.to_owned(),
                    ))
                    .or_default() += 1;
            }
        }
    }
    let errors: Vec<String> = violations
        .into_iter()
        .map(|((path, item, writer), count)| {
            format!(
                "{path} :: {item}: {count} call(s) to {writer} under a modulo-integer periodic guard"
            )
        })
        .collect();
    errors.is_empty().then_some(()).ok_or(errors)
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

#[test]
fn unlocked_multi_byte_serial_write_census_is_pinned() {
    assert_eq!(
        validate_unlocked_multi_byte_write_census(&kernel_sources()),
        Ok(())
    );
}

#[test]
fn raw_serial_primitive_census_is_pinned() {
    assert_eq!(
        validate_raw_serial_primitive_census(&kernel_sources()),
        Ok(())
    );
}

#[test]
fn census_validator_rejects_a_synthetic_unlocked_multi_byte_writer() {
    let sources = with_synthetic_source(
        &kernel_sources(),
        "kernel/src/synthetic_unlocked_writer.rs",
        r#"
            fn synthetic_unlocked_writer(message: &[u8]) {
                raw_uart_bytes(message);
            }
        "#,
    );
    assert_eq!(
        validate_unlocked_multi_byte_write_census(&sources),
        Err(vec![
            "+ kernel/src/synthetic_unlocked_writer.rs :: fn synthetic_unlocked_writer  (1 occurrences, expected none)".to_owned()
        ])
    );
}

#[test]
fn primitive_census_validator_rejects_a_differently_named_uart_writer() {
    let sources = with_synthetic_source(
        &kernel_sources(),
        "kernel/src/synthetic_uart_primitive.rs",
        r#"
            fn uart_emit_line(byte: u8) {
                let uart = crate::platform_config::uart_virt();
                unsafe {
                    core::ptr::write_volatile(uart as *mut u8, byte);
                }
            }
        "#,
    );
    assert_eq!(
        validate_raw_serial_primitive_census(&sources),
        Err(vec![
            "+ kernel/src/synthetic_uart_primitive.rs :: fn uart_emit_line  (1 occurrences, expected none)".to_owned()
        ])
    );
}

#[test]
fn unlocked_multi_byte_serial_writes_are_never_periodic() {
    assert_eq!(
        validate_no_periodic_unlocked_multi_byte_writes(&kernel_sources()),
        Ok(())
    );
}

#[test]
fn periodic_guard_validator_rejects_a_synthetic_unlocked_writer() {
    let sources = vec![(
        "kernel/src/synthetic_periodic_writer.rs".to_owned(),
        r#"
            fn synthetic_periodic_writer(tick: u64) {
                if (tick % (5_000u64)) == 0 {
                    raw_uart_bytes(b"tick");
                }
            }
        "#
        .to_owned(),
    )];
    assert_eq!(
        validate_no_periodic_unlocked_multi_byte_writes(&sources),
        Err(vec![
            "kernel/src/synthetic_periodic_writer.rs :: fn synthetic_periodic_writer: 1 call(s) to raw_uart_bytes under a modulo-integer periodic guard".to_owned()
        ])
    );
}
