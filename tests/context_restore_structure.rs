use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn rust_sources_below(relative: &str) -> Vec<(PathBuf, String)> {
    fn visit(path: &std::path::Path, sources: &mut Vec<(PathBuf, String)>) {
        if path.is_dir() {
            for entry in fs::read_dir(path).expect("read source directory") {
                visit(&entry.expect("read source entry").path(), sources);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push((
                path.to_path_buf(),
                fs::read_to_string(path).expect("read Rust source"),
            ));
        }
    }

    let mut sources = Vec::new();
    visit(&repo_root().join(relative), &mut sources);
    sources
}

fn text_sources_below(relative: &str) -> Vec<(String, String)> {
    fn visit(root: &std::path::Path, path: &std::path::Path, sources: &mut Vec<(String, String)>) {
        if path.is_dir() {
            for entry in fs::read_dir(path).expect("read source directory") {
                visit(root, &entry.expect("read source entry").path(), sources);
            }
        } else if let Ok(source) = fs::read_to_string(path) {
            let relative = path
                .strip_prefix(root)
                .expect("source below repository root")
                .to_string_lossy()
                .into_owned();
            sources.push((relative, source));
        }
    }

    let root = repo_root();
    let mut sources = Vec::new();
    visit(&root, &root.join(relative), &mut sources);
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
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

fn code_offsets(source: &str, mask: &[bool], needle: &str) -> Vec<usize> {
    source
        .match_indices(needle)
        .filter_map(|(offset, _)| mask.get(offset).copied().unwrap_or(false).then_some(offset))
        .collect()
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

fn call_offsets(source: &str, mask: &[bool], name: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    identifier_offsets(source, mask, name)
        .into_iter()
        .filter(|offset| {
            let mut cursor = *offset + name.len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            bytes.get(cursor) == Some(&b'(')
        })
        .collect()
}

fn binding_offsets(source: &str, mask: &[bool], name: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    identifier_offsets(source, mask, "let")
        .into_iter()
        .filter(|let_offset| {
            let mut cursor = *let_offset + "let".len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            if bytes
                .get(cursor..cursor + "mut".len())
                .is_some_and(|candidate| candidate == b"mut")
                && !bytes
                    .get(cursor + "mut".len())
                    .is_some_and(|byte| identifier_byte(*byte))
            {
                cursor += "mut".len();
                while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace())
                {
                    cursor += 1;
                }
            }

            let name_start = cursor;
            while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
                cursor += 1;
            }
            &source[name_start..cursor] == name
        })
        .collect()
}

fn assigned_value_offsets(source: &str, mask: &[bool], value: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    code_offsets(source, mask, value)
        .into_iter()
        .filter(|offset| {
            let mut cursor = *offset;
            while cursor > 0 && (!mask[cursor - 1] || bytes[cursor - 1].is_ascii_whitespace()) {
                cursor -= 1;
            }
            if cursor == 0 || bytes[cursor - 1] != b'=' {
                return false;
            }
            !cursor
                .checked_sub(2)
                .and_then(|before| bytes.get(before))
                .is_some_and(|byte| matches!(byte, b'=' | b'!' | b'<' | b'>'))
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

fn normalized_predicate(fragment: &str) -> String {
    let mut predicate = normalized_code(fragment);
    loop {
        let without_semicolon = predicate.trim_end_matches(';').trim().to_string();
        if without_semicolon.starts_with('(') && without_semicolon.ends_with(')') {
            let mut depth = 0usize;
            let mut wraps_entire_predicate = true;
            for (offset, byte) in without_semicolon.bytes().enumerate() {
                match byte {
                    b'(' => depth += 1,
                    b')' => {
                        let Some(new_depth) = depth.checked_sub(1) else {
                            wraps_entire_predicate = false;
                            break;
                        };
                        depth = new_depth;
                        if depth == 0 && offset + 1 != without_semicolon.len() {
                            wraps_entire_predicate = false;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if wraps_entire_predicate && depth == 0 {
                predicate = without_semicolon[1..without_semicolon.len() - 1]
                    .trim()
                    .to_string();
                continue;
            }
        }
        return without_semicolon;
    }
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

/// Every `fn NAME(` definition in a module, keyed by name. Names may repeat
/// (`cfg`-split, or same-named inherent methods on different types); every body
/// is kept so a check over a name covers all of them.
fn module_function_bodies(source: &str) -> BTreeMap<String, Vec<&str>> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut bodies: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for offset in identifier_offsets(source, &mask, "fn") {
        let mut cursor = offset + "fn".len();
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
        let name = &source[name_start..cursor];
        // A signature terminated by `;` (trait requirement, extern block) has no
        // body; taking the next brace would attribute a foreign body to it.
        let brace = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{');
        let semicolon = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b';');
        let Some(brace) = brace else { continue };
        if semicolon.is_some_and(|semicolon| semicolon < brace) {
            continue;
        }
        let Some(body) = braced_block(source, &mask, brace) else {
            continue;
        };
        bodies.entry(name.to_owned()).or_default().push(body);
    }
    bodies
}

// Structural ratchets below use `(repo-relative path, canonical item path)`
// plus occurrence count. Reflowing or moving code inside an item is free;
// adding, removing, relocating, or feature-gating a site changes the census.
type Anchor = (String, String);
type Census = BTreeMap<Anchor, usize>;

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
        return String::new();
    }
    format!("{} ", attributes.join(" "))
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

fn census_tagged<F>(sources: &[(String, String)], mut matcher: F) -> Census
where
    F: FnMut(&str, &[bool]) -> Vec<(usize, String)>,
{
    let mut census = Census::new();
    for (path, source) in sources {
        let mask = code_mask(source);
        let matches = matcher(source, &mask);
        if matches.is_empty() {
            continue;
        }
        let spans = rendered_item_spans(&item_spans(source, &mask));
        for (offset, tag) in matches {
            let mut item = item_path_at(&spans, offset);
            if !tag.is_empty() {
                item = format!("{item} => {tag}");
            }
            *census.entry((path.clone(), item)).or_default() += 1;
        }
    }
    census
}

fn census<F>(sources: &[(String, String)], mut matcher: F) -> Census
where
    F: FnMut(&str, &[bool]) -> Vec<usize>,
{
    census_tagged(sources, |source, mask| {
        matcher(source, mask)
            .into_iter()
            .map(|offset| (offset, String::new()))
            .collect()
    })
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

fn definition_span(
    source: &str,
    mask: &[bool],
    offset: usize,
    end: usize,
) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let keyword = previous_code(source, mask, offset)?;
    if !preceded_by_keyword(source, mask, offset, "fn") {
        return None;
    }
    let keyword = keyword + 1 - "fn".len();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for index in end..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.checked_sub(1)?,
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.checked_sub(1)?,
            b'{' if paren_depth == 0 && bracket_depth == 0 => {
                return Some((keyword, index));
            }
            b';' if paren_depth == 0 && bracket_depth == 0 => return None,
            _ => {}
        }
    }
    None
}

fn definition_offsets(source: &str, mask: &[bool], name: &str) -> Vec<(usize, usize)> {
    identifier_offsets(source, mask, name)
        .into_iter()
        .filter_map(|offset| definition_span(source, mask, offset, offset + name.len()))
        .collect()
}

fn assignment_to_false(body: &str, field: &str) -> bool {
    let mask = code_mask(body);
    let bytes = body.as_bytes();

    identifier_offsets(body, &mask, field)
        .into_iter()
        .any(|offset| {
            let mut cursor = offset + field.len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            if bytes.get(cursor) != Some(&b'=') || bytes.get(cursor + 1) == Some(&b'=') {
                return false;
            }
            cursor += 1;
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            identifier_offsets(&body[cursor..], &mask[cursor..], "false")
                .first()
                .is_some_and(|false_offset| *false_offset == 0)
        })
}

fn else_if_condition_has_disjunction(body: &str, left: &str, right: &str) -> bool {
    let mask = code_mask(body);
    let bytes = body.as_bytes();

    identifier_offsets(body, &mask, "else")
        .into_iter()
        .any(|else_offset| {
            let mut cursor = else_offset + "else".len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            if bytes.get(cursor..cursor + 2) != Some(b"if")
                || bytes
                    .get(cursor + 2)
                    .is_some_and(|byte| identifier_byte(*byte))
            {
                return false;
            }
            cursor += 2;
            let Some(open) =
                (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{')
            else {
                return false;
            };
            let condition = &body[cursor..open];
            let condition_mask = code_mask(condition);
            let Some(left_offset) = identifier_offsets(condition, &condition_mask, left)
                .first()
                .copied()
            else {
                return false;
            };
            let Some(right_offset) = identifier_offsets(condition, &condition_mask, right)
                .first()
                .copied()
            else {
                return false;
            };
            let (first, second) = if left_offset < right_offset {
                (left_offset + left.len(), right_offset)
            } else {
                (right_offset + right.len(), left_offset)
            };
            normalized_code(&condition[first..second]).contains("||")
        })
}

fn validate_unblock_source(source: &str) -> Result<(), String> {
    let body = function_body(source, "unblock").ok_or_else(|| "missing fn unblock".to_string())?;
    if assignment_to_false(body, "blocked_in_syscall") {
        return Err("unblock clears blocked_in_syscall from a foreign context".to_string());
    }
    Ok(())
}

fn validate_switch_source(source: &str) -> Result<(), String> {
    let switch_body = function_body(source, "switch_to_thread")
        .ok_or_else(|| "missing fn switch_to_thread".to_string())?;
    if !else_if_condition_has_disjunction(
        switch_body,
        "blocked_in_syscall",
        "saved_context_is_kernel_frame",
    ) {
        return Err(
            "switch_to_thread does not combine blocked_in_syscall with the saved-frame term"
                .to_string(),
        );
    }

    let helper_body = function_body(source, "saved_context_is_kernel_frame")
        .ok_or_else(|| "missing fn saved_context_is_kernel_frame".to_string())?;
    let helper_mask = code_mask(helper_body);
    if identifier_offsets(helper_body, &helper_mask, "is_kernel_code_selector").is_empty() {
        return Err("saved-frame helper does not consult is_kernel_code_selector".to_string());
    }
    Ok(())
}

fn validate_restore_source(source: &str) -> Result<(), String> {
    let body = function_body(source, "restore_userspace_context")
        .ok_or_else(|| "missing fn restore_userspace_context".to_string())?;
    let mask = code_mask(body);
    let first_write = identifier_offsets(body, &mask, "saved_regs")
        .into_iter()
        .chain(identifier_offsets(body, &mask, "interrupt_frame"))
        .min()
        .ok_or_else(|| "restore_userspace_context has no context writes".to_string())?;

    let guarded_check = identifier_offsets(body, &mask, "if")
        .into_iter()
        .find_map(|if_offset| {
            let block = braced_block(body, &mask, if_offset)?;
            let open = block.find('{')?;
            let condition = &block[..open];
            let condition_mask = code_mask(condition);
            if identifier_offsets(condition, &condition_mask, "is_kernel_code_selector").is_empty()
            {
                return None;
            }
            let normalized_block = normalized_code(block);
            normalized_block
                .contains("return Err(RestoreError::KernelFrame)")
                .then_some((if_offset, block))
        })
        .ok_or_else(|| {
            "restore_userspace_context lacks a guarded RestoreError::KernelFrame return".to_string()
        })?;

    let block_mask = code_mask(guarded_check.1);
    let kernel_frame_offset = identifier_offsets(guarded_check.1, &block_mask, "KernelFrame")
        .first()
        .copied()
        .ok_or_else(|| "missing RestoreError::KernelFrame".to_string())?;
    if guarded_check.0 >= first_write || guarded_check.0 + kernel_frame_offset >= first_write {
        return Err("kernel-frame guard appears after context writes".to_string());
    }
    Ok(())
}

fn validate_routing_matches_enforcement(
    routing_source: &str,
    enforcement_source: &str,
) -> Result<(), String> {
    fn closure_body<'a>(body: &'a str, call: &str) -> Result<&'a str, String> {
        let mask = code_mask(body);
        let bytes = body.as_bytes();
        for call_offset in identifier_offsets(body, &mask, call) {
            let mut open = call_offset + call.len();
            while open < bytes.len() && (!mask[open] || bytes[open].is_ascii_whitespace()) {
                open += 1;
            }
            if bytes.get(open) != Some(&b'(') {
                continue;
            }

            let mut depth = 0usize;
            let mut close = None;
            for offset in open..bytes.len() {
                if !mask[offset] {
                    continue;
                }
                match bytes[offset] {
                    b'(' => depth += 1,
                    b')' => {
                        depth = depth
                            .checked_sub(1)
                            .ok_or_else(|| format!("unbalanced {call} call"))?;
                        if depth == 0 {
                            close = Some(offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let close = close.ok_or_else(|| format!("unterminated {call} call"))?;
            let pipes: Vec<_> = (open + 1..close)
                .filter(|offset| mask[*offset] && bytes[*offset] == b'|')
                .take(2)
                .collect();
            if pipes.len() != 2 || normalized_code(&body[pipes[0] + 1..pipes[1]]) != "thread" {
                continue;
            }

            let mut expression_start = pipes[1] + 1;
            while expression_start < close
                && (!mask[expression_start] || bytes[expression_start].is_ascii_whitespace())
            {
                expression_start += 1;
            }
            if bytes.get(expression_start) == Some(&b'{') {
                let block = braced_block(body, &mask, expression_start)
                    .ok_or_else(|| format!("{call} closure has an unterminated body"))?;
                let block_end = expression_start + block.len();
                if block_end > close || !normalized_code(&body[block_end..close]).is_empty() {
                    return Err(format!("{call} closure has tokens after its braced body"));
                }
                return Ok(&block[1..block.len() - 1]);
            }
            if expression_start == close {
                return Err(format!("{call} closure has no body"));
            }
            return Ok(&body[expression_start..close]);
        }
        Err(format!("missing {call} closure with |thread| parameter"))
    }

    let routing = function_body(routing_source, "saved_context_is_kernel_frame")
        .ok_or_else(|| "missing fn saved_context_is_kernel_frame".to_string())?;
    let routing_predicate = normalized_predicate(closure_body(routing, "is_some_and")?);

    let enforcement = function_body(enforcement_source, "restore_userspace_context")
        .ok_or_else(|| "missing fn restore_userspace_context".to_string())?;
    let enforcement_mask = code_mask(enforcement);
    let enforcement_predicate = identifier_offsets(enforcement, &enforcement_mask, "if")
        .into_iter()
        .find_map(|if_offset| {
            let block = braced_block(enforcement, &enforcement_mask, if_offset)?;
            let open = block.find('{')?;
            let condition = &block["if".len()..open];
            let condition_mask = code_mask(condition);
            if identifier_offsets(condition, &condition_mask, "is_kernel_code_selector").is_empty()
                || !normalized_code(block).contains("return Err(RestoreError::KernelFrame)")
            {
                return None;
            }
            Some(normalized_predicate(condition))
        })
        .ok_or_else(|| {
            "restore_userspace_context lacks the guarded RestoreError::KernelFrame predicate"
                .to_string()
        })?;

    if routing_predicate != enforcement_predicate {
        return Err(format!(
            "routing predicate `{routing_predicate}` does not match enforcement predicate `{enforcement_predicate}`"
        ));
    }
    Ok(())
}

fn validate_dispatch_guard_precheck(source: &str) -> Result<(), String> {
    fn binding_initializer<'a>(body: &'a str, name: &str) -> Result<(usize, &'a str), String> {
        let mask = code_mask(body);
        let bytes = body.as_bytes();
        let mut bindings = Vec::new();

        for let_offset in identifier_offsets(body, &mask, "let") {
            let mut cursor = let_offset + "let".len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            if bytes
                .get(cursor..cursor + "mut".len())
                .is_some_and(|candidate| candidate == b"mut")
                && !bytes
                    .get(cursor + "mut".len())
                    .is_some_and(|byte| identifier_byte(*byte))
            {
                cursor += "mut".len();
                while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace())
                {
                    cursor += 1;
                }
            }

            let name_start = cursor;
            while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
                cursor += 1;
            }
            if &body[name_start..cursor] != name {
                continue;
            }
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            if bytes.get(cursor) != Some(&b'=') {
                return Err(format!("{name} binding has no initializer"));
            }

            let initializer_start = cursor + 1;
            let mut parentheses = 0usize;
            let mut braces = 0usize;
            let mut brackets = 0usize;
            let initializer_end = (initializer_start..bytes.len())
                .find(|offset| {
                    if !mask[*offset] {
                        return false;
                    }
                    match bytes[*offset] {
                        b'(' => parentheses += 1,
                        b')' => parentheses = parentheses.saturating_sub(1),
                        b'{' => braces += 1,
                        b'}' => braces = braces.saturating_sub(1),
                        b'[' => brackets += 1,
                        b']' => brackets = brackets.saturating_sub(1),
                        b';' if parentheses == 0 && braces == 0 && brackets == 0 => return true,
                        _ => {}
                    }
                    false
                })
                .ok_or_else(|| format!("unterminated {name} binding"))?;
            bindings.push((let_offset, &body[initializer_start..initializer_end]));
        }

        if bindings.len() != 1 {
            return Err(format!(
                "expected exactly one {name} binding, found {}",
                bindings.len()
            ));
        }
        Ok(bindings[0])
    }

    fn qualified_zero_arg_call_offsets(scope: &str, qualifier: &str, call: &str) -> Vec<usize> {
        let mask = code_mask(scope);
        let bytes = scope.as_bytes();
        identifier_offsets(scope, &mask, call)
            .into_iter()
            .filter(|call_offset| {
                let previous_code = |before: usize| {
                    (0..before)
                        .rev()
                        .find(|offset| mask[*offset] && !bytes[*offset].is_ascii_whitespace())
                };
                let Some(second_colon) = previous_code(*call_offset) else {
                    return false;
                };
                let Some(first_colon) = previous_code(second_colon) else {
                    return false;
                };
                if bytes[second_colon] != b':' || bytes[first_colon] != b':' {
                    return false;
                }
                let Some(qualifier_end) = previous_code(first_colon) else {
                    return false;
                };
                let mut qualifier_start = qualifier_end;
                while qualifier_start > 0
                    && mask[qualifier_start - 1]
                    && identifier_byte(bytes[qualifier_start - 1])
                {
                    qualifier_start -= 1;
                }
                if &scope[qualifier_start..=qualifier_end] != qualifier {
                    return false;
                }

                let mut open = *call_offset + call.len();
                while open < bytes.len() && (!mask[open] || bytes[open].is_ascii_whitespace()) {
                    open += 1;
                }
                if bytes.get(open) != Some(&b'(') {
                    return false;
                }
                let mut close = open + 1;
                while close < bytes.len() && (!mask[close] || bytes[close].is_ascii_whitespace()) {
                    close += 1;
                }
                bytes.get(close) == Some(&b')')
            })
            .collect()
    }

    let body = function_body(source, "check_need_resched_and_switch")
        .ok_or_else(|| "missing fn check_need_resched_and_switch".to_string())?;
    let (binding_offset, initializer) = binding_initializer(body, "process_manager_guard")?;
    let compact_initializer = normalized_code(initializer).replace(' ', "");
    if compact_initializer
        .matches("crate::process::try_manager()")
        .count()
        != 1
    {
        return Err(
            "process_manager_guard is not bound exactly once from crate::process::try_manager()"
                .to_string(),
        );
    }

    let initializer_mask = code_mask(initializer);
    if !identifier_offsets(initializer, &initializer_mask, "from_userspace").is_empty() {
        return Err(
            "process-manager guard acquisition is conditional on from_userspace".to_string(),
        );
    }

    let schedule_offsets = qualified_zero_arg_call_offsets(body, "scheduler", "schedule");
    let schedule_offset = schedule_offsets
        .first()
        .copied()
        .ok_or_else(|| "missing scheduler::schedule() call".to_string())?;
    if binding_offset >= schedule_offset {
        return Err("process-manager guard is acquired after scheduler::schedule()".to_string());
    }

    let none_blocks: Vec<_> = identifier_offsets(initializer, &initializer_mask, "None")
        .into_iter()
        .filter_map(|offset| braced_block(initializer, &initializer_mask, offset))
        .collect();
    if none_blocks.len() != 1 {
        return Err("guard acquisition does not have exactly one unavailable arm".to_string());
    }
    let unavailable = none_blocks[0];
    let unavailable_mask = code_mask(unavailable);
    let compact_unavailable = normalized_code(unavailable).replace(' ', "");
    if !compact_unavailable.contains("scheduler::set_need_resched();")
        || identifier_offsets(unavailable, &unavailable_mask, "return").is_empty()
    {
        return Err("guard-unavailable arm does not re-arm rescheduling and return".to_string());
    }
    if !identifier_offsets(unavailable, &unavailable_mask, "schedule").is_empty()
        || !identifier_offsets(unavailable, &unavailable_mask, "abort_dispatch_and_resume")
            .is_empty()
    {
        return Err("guard-unavailable arm commits or rolls back a dispatch".to_string());
    }

    Ok(())
}

fn validate_first_run_dispatch_has_no_logging(source: &str) -> Result<(), String> {
    fn log_path_offsets(scope: &str) -> Vec<usize> {
        let mask = code_mask(scope);
        let bytes = scope.as_bytes();
        identifier_offsets(scope, &mask, "log")
            .into_iter()
            .filter(|offset| {
                let mut cursor = *offset + "log".len();
                while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace())
                {
                    cursor += 1;
                }
                bytes.get(cursor..cursor + 2) == Some(b"::")
            })
            .collect()
    }

    let restore = function_body(source, "restore_userspace_thread_context")
        .ok_or_else(|| "missing fn restore_userspace_thread_context".to_string())?;
    let restore_mask = code_mask(restore);
    let first_run = identifier_offsets(restore, &restore_mask, "if")
        .into_iter()
        .find_map(|if_offset| {
            let block = braced_block(restore, &restore_mask, if_offset)?;
            let open = block.find('{')?;
            (normalized_predicate(&block["if".len()..open]) == "!has_started").then_some(block)
        })
        .ok_or_else(|| "restore has no if !has_started block".to_string())?;
    let first_run_mask = code_mask(first_run);
    let installed_offset = identifier_offsets(first_run, &first_run_mask, "Installed")
        .first()
        .copied()
        .ok_or_else(|| "first-run block has no Installed arm".to_string())?;
    let installed_arm = braced_block(first_run, &first_run_mask, installed_offset)
        .ok_or_else(|| "Installed arm has no block".to_string())?;
    let installed_end = installed_offset + installed_arm.len();
    if log_path_offsets(first_run)
        .into_iter()
        .any(|offset| offset < installed_offset || offset >= installed_end)
    {
        return Err("first-run dispatch logs outside the Installed arm".to_string());
    }

    let setup = function_body(source, "setup_first_userspace_entry")
        .ok_or_else(|| "missing fn setup_first_userspace_entry".to_string())?;
    let setup_mask = code_mask(setup);
    let ring3_commit = identifier_offsets(setup, &setup_mask, "user_code_selector")
        .first()
        .copied()
        .ok_or_else(|| "first userspace entry has no ring-3 commit".to_string())?;
    if log_path_offsets(setup)
        .into_iter()
        .any(|offset| offset < ring3_commit)
    {
        return Err("first userspace setup logs before the ring-3 commit".to_string());
    }

    Ok(())
}

fn validate_first_userspace_entry_source(source: &str) -> Result<(), String> {
    let setup = function_body(source, "setup_first_userspace_entry")
        .ok_or_else(|| "missing fn setup_first_userspace_entry".to_string())?;
    let setup_mask = code_mask(setup);
    let ring3_commit = identifier_offsets(setup, &setup_mask, "user_code_selector")
        .first()
        .copied()
        .ok_or_else(|| "first userspace entry has no ring-3 commit".to_string())?;
    for install in ["set_next_cr3", "update_tss_rsp0"] {
        let install_offset = identifier_offsets(setup, &setup_mask, install)
            .first()
            .copied()
            .ok_or_else(|| format!("first userspace entry does not call {install}"))?;
        if install_offset > ring3_commit {
            return Err(format!(
                "first userspace entry calls {install} after the ring-3 commit"
            ));
        }
    }
    if identifier_offsets(setup, &setup_mask, "Aborted")
        .into_iter()
        .any(|offset| offset > ring3_commit)
    {
        return Err("first userspace entry can abort after the ring-3 commit".to_string());
    }

    let restore = function_body(source, "restore_userspace_thread_context")
        .ok_or_else(|| "missing fn restore_userspace_thread_context".to_string())?;
    let restore_mask = code_mask(restore);
    let installed_offset = identifier_offsets(restore, &restore_mask, "Installed")
        .first()
        .copied()
        .ok_or_else(|| "first-entry restore has no Installed arm".to_string())?;
    let installed_arm = braced_block(restore, &restore_mask, installed_offset)
        .ok_or_else(|| "Installed arm has no block".to_string())?;
    let normalized_restore = normalized_code(restore);
    if normalized_restore
        .matches("thread.has_started = true;")
        .count()
        != 1
        || !normalized_code(installed_arm).contains("thread.has_started = true;")
    {
        return Err("has_started is not committed only in the Installed arm".to_string());
    }

    let aborted_offset = identifier_offsets(restore, &restore_mask, "Aborted")
        .first()
        .copied()
        .ok_or_else(|| "first-entry restore has no Aborted arm".to_string())?;
    let aborted_arm = braced_block(restore, &restore_mask, aborted_offset)
        .ok_or_else(|| "Aborted arm has no block".to_string())?;
    let normalized_abort = normalized_code(aborted_arm);
    for required in [
        "scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);",
        "scheduler::set_need_resched();",
    ] {
        if !normalized_abort.contains(required) {
            return Err(format!("Aborted arm is missing {required}"));
        }
    }
    let aborted_mask = code_mask(aborted_arm);
    if !identifier_offsets(aborted_arm, &aborted_mask, "setup_idle_return").is_empty() {
        return Err("Aborted arm parks the CPU in idle".to_string());
    }
    if !identifier_offsets(aborted_arm, &aborted_mask, "interrupt_frame").is_empty() {
        return Err("Aborted arm touches the interrupt return frame".to_string());
    }
    Ok(())
}

fn validate_blocked_syscall_dispatch_resolves_cr3(source: &str) -> Result<(), String> {
    fn qualified_zero_arg_call_offsets(scope: &str, qualifier: &str, call: &str) -> Vec<usize> {
        let mask = code_mask(scope);
        let bytes = scope.as_bytes();
        identifier_offsets(scope, &mask, call)
            .into_iter()
            .filter(|call_offset| {
                let mut cursor = *call_offset;
                while cursor > 0 && (!mask[cursor - 1] || bytes[cursor - 1].is_ascii_whitespace()) {
                    cursor -= 1;
                }
                if cursor == 0 || bytes[cursor - 1] != b'.' {
                    return false;
                }
                cursor -= 1;
                while cursor > 0 && (!mask[cursor - 1] || bytes[cursor - 1].is_ascii_whitespace()) {
                    cursor -= 1;
                }
                let qualifier_end = cursor;
                while cursor > 0 && mask[cursor - 1] && identifier_byte(bytes[cursor - 1]) {
                    cursor -= 1;
                }
                if &scope[cursor..qualifier_end] != qualifier {
                    return false;
                }

                let mut open = *call_offset + call.len();
                while open < bytes.len() && (!mask[open] || bytes[open].is_ascii_whitespace()) {
                    open += 1;
                }
                if bytes.get(open) != Some(&b'(') {
                    return false;
                }
                let mut close = open + 1;
                while close < bytes.len() && (!mask[close] || bytes[close].is_ascii_whitespace()) {
                    close += 1;
                }
                bytes.get(close) == Some(&b')')
            })
            .collect()
    }

    fn field_assignment_offsets(scope: &str, field: &str) -> Vec<usize> {
        let mask = code_mask(scope);
        let bytes = scope.as_bytes();
        identifier_offsets(scope, &mask, field)
            .into_iter()
            .filter(|field_offset| {
                let mut cursor = *field_offset + field.len();
                while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace())
                {
                    cursor += 1;
                }
                if bytes.get(cursor) != Some(&b'.') {
                    return false;
                }
                let statement_end = (*field_offset..bytes.len())
                    .find(|offset| mask[*offset] && bytes[*offset] == b';')
                    .unwrap_or(bytes.len());
                (cursor..statement_end).any(|offset| {
                    bytes[offset] == b'='
                        && bytes.get(offset + 1) != Some(&b'=')
                        && (offset == 0 || bytes[offset - 1] != b'!')
                })
            })
            .collect()
    }

    let switch = function_body(source, "switch_to_thread")
        .ok_or_else(|| "missing fn switch_to_thread".to_string())?;
    let switch_mask = code_mask(switch);
    let blocked_branch = identifier_offsets(switch, &switch_mask, "if")
        .into_iter()
        .find_map(|if_offset| {
            let block = braced_block(switch, &switch_mask, if_offset)?;
            let open = block.find('{')?;
            let predicate = normalized_predicate(&block["if".len()..open]);
            (predicate.contains("blocked_in_syscall")
                && predicate.contains("saved_context_is_kernel_frame"))
            .then_some(block)
        })
        .ok_or_else(|| "switch_to_thread has no blocked-in-syscall dispatch branch".to_string())?;

    let blocked_mask = code_mask(blocked_branch);
    let lookup_offset =
        identifier_offsets(blocked_branch, &blocked_mask, "find_process_by_thread_mut")
            .first()
            .copied()
            .ok_or_else(|| "blocked-in-syscall branch has no process lookup".to_string())?;
    let process_block = identifier_offsets(blocked_branch, &blocked_mask, "if")
        .into_iter()
        .filter_map(|if_offset| {
            let block = braced_block(blocked_branch, &blocked_mask, if_offset)?;
            let block_end = if_offset + block.len();
            (lookup_offset >= if_offset && lookup_offset < block_end).then_some(block)
        })
        .min_by_key(|block| block.len())
        .ok_or_else(|| "blocked-in-syscall process lookup has no body".to_string())?;
    let process_mask = code_mask(process_block);
    let compact_process = normalized_code(process_block).replace(' ', "");
    let cr3_calls = qualified_zero_arg_call_offsets(process_block, "process", "cr3_value");
    if cr3_calls.len() != 1 || compact_process.matches("process.cr3_value()").count() != 1 {
        return Err(format!(
            "blocked-in-syscall dispatch must resolve process.cr3_value() exactly once, found {}",
            cr3_calls.len()
        ));
    }
    if !compact_process.contains("letprocess_cr3=matchprocess.cr3_value()") {
        return Err(
            "blocked-in-syscall dispatch does not bind the resolved CR3 up front".to_string(),
        );
    }
    if compact_process.matches("Cr3::write(").count() != 2
        || compact_process.matches("set_next_cr3(process_cr3)").count() != 1
        || compact_process
            .matches("PhysAddr::new(process_cr3)")
            .count()
            != 2
    {
        return Err(
            "blocked-in-syscall CR3 uses do not all consume the resolved value".to_string(),
        );
    }
    let cr3_resolution = cr3_calls[0];
    let unpublished_refusals =
        identifier_offsets(process_block, &process_mask, "refuse_unpublished_dispatch");
    if unpublished_refusals.len() != 1 || unpublished_refusals[0] >= cr3_resolution {
        return Err(
            "blocked-in-syscall dispatch does not refuse unpublished rows before CR3 resolution"
                .to_string(),
        );
    }
    let unpublished_arm = identifier_offsets(process_block, &process_mask, "if")
        .into_iter()
        .filter_map(|if_offset| {
            let block = braced_block(process_block, &process_mask, if_offset)?;
            let end = if_offset + block.len();
            (unpublished_refusals[0] >= if_offset && unpublished_refusals[0] < end).then_some(block)
        })
        .min_by_key(|block| block.len())
        .ok_or_else(|| "unpublished-row refusal call has no recovery arm".to_string())?;
    let compact_unpublished_arm = normalized_code(unpublished_arm).replace(' ', "");
    if identifier_offsets(
        unpublished_arm,
        &code_mask(unpublished_arm),
        "set_terminated",
    )
    .len()
        != 0
        || !compact_unpublished_arm.contains("scheduler::set_need_resched();")
        || !compact_unpublished_arm.contains("setup_idle_return(interrupt_frame);")
        || !compact_unpublished_arm.contains("scheduler::switch_to_idle();")
        || !compact_unpublished_arm.contains("scheduler::requeue_refused_dispatch(")
        || !compact_unpublished_arm.contains("process_memory::switch_to_kernel_page_table();")
        || !compact_unpublished_arm.contains("return;")
    {
        return Err("blocked-in-syscall unpublished-row recovery is not retry-only".to_string());
    }
    let switch_idle = compact_unpublished_arm
        .find("scheduler::switch_to_idle();")
        .ok_or_else(|| "unpublished-row recovery has no switch_to_idle".to_string())?;
    let requeue = compact_unpublished_arm
        .find("scheduler::requeue_refused_dispatch(")
        .ok_or_else(|| "unpublished-row recovery has no refused-thread requeue".to_string())?;
    let early_return = compact_unpublished_arm
        .find("return;")
        .ok_or_else(|| "unpublished-row recovery has no early return".to_string())?;
    if !(switch_idle < requeue && requeue < early_return) {
        return Err("blocked-in-syscall unpublished-row requeue is out of order".to_string());
    }
    for publisher in ["set_next_cr3", "Cr3"] {
        if identifier_offsets(process_block, &process_mask, publisher)
            .into_iter()
            .any(|offset| offset < unpublished_refusals[0])
        {
            return Err("unpublished-row refusal occurs after a CR3 publish".to_string());
        }
    }

    let first_frame_mutation =
        qualified_zero_arg_call_offsets(process_block, "interrupt_frame", "as_mut")
            .first()
            .copied()
            .ok_or_else(|| {
                "blocked-in-syscall branch has no interrupt-frame mutation".to_string()
            })?;
    let first_saved_regs_assignment = field_assignment_offsets(process_block, "saved_regs")
        .first()
        .copied()
        .ok_or_else(|| "blocked-in-syscall branch has no saved_regs assignment".to_string())?;
    if cr3_resolution > first_frame_mutation || cr3_resolution > first_saved_regs_assignment {
        return Err(
            "process CR3 resolution occurs after a return-frame or saved-register mutation"
                .to_string(),
        );
    }

    for if_offset in identifier_offsets(process_block, &process_mask, "if") {
        let block = braced_block(process_block, &process_mask, if_offset)
            .ok_or_else(|| "blocked-in-syscall if has no body".to_string())?;
        let open = block
            .find('{')
            .ok_or_else(|| "blocked-in-syscall if has no opening brace".to_string())?;
        let predicate = normalized_predicate(&block["if".len()..open]);
        let normalized_block = normalized_code(block).replace(' ', "");
        if predicate.contains("cr3_value")
            && (normalized_block.contains("Cr3::write(")
                || normalized_block.contains("set_next_cr3("))
        {
            return Err("a CR3 publish is still guarded by an if on cr3_value".to_string());
        }
    }

    let no_cr3_arm = identifier_offsets(process_block, &process_mask, "None")
        .into_iter()
        .filter_map(|none_offset| braced_block(process_block, &process_mask, none_offset))
        .find(|arm| normalized_code(arm).contains("USERSPACE_DISPATCH_NO_CR3_REFUSED"))
        .ok_or_else(|| "process CR3 resolution has no unavailable arm".to_string())?;
    let no_cr3_mask = code_mask(no_cr3_arm);
    let normalized_arm = normalized_code(no_cr3_arm);
    let compact_arm = normalized_arm.replace(' ', "");
    if !compact_arm.contains("USERSPACE_DISPATCH_NO_CR3_REFUSED.fetch_add")
        || !compact_arm.contains("USERSPACE_DISPATCH_NO_CR3_LOGGED.swap")
        || identifier_offsets(no_cr3_arm, &no_cr3_mask, "raw_serial_str").is_empty()
        || identifier_offsets(no_cr3_arm, &no_cr3_mask, "raw_serial_u64").len() < 2
    {
        return Err("unavailable CR3 arm lacks the guarded raw breadcrumb".to_string());
    }

    let with_thread_mut_source = identifier_offsets(no_cr3_arm, &no_cr3_mask, "with_thread_mut")
        .first()
        .copied()
        .ok_or_else(|| "unavailable CR3 arm has no scheduler synchronization".to_string())?;
    let with_thread_mut = normalized_arm
        .find("scheduler::with_thread_mut")
        .ok_or_else(|| "unavailable CR3 arm has no scheduler with_thread_mut call".to_string())?;
    if !normalized_arm.contains("process.main_thread") {
        return Err("unavailable CR3 arm does not mark the process-owned thread".to_string());
    }
    identifier_offsets(no_cr3_arm, &no_cr3_mask, "set_terminated")
        .into_iter()
        .find(|offset| *offset < with_thread_mut_source)
        .ok_or_else(|| {
            "unavailable CR3 arm does not terminate the process-owned thread".to_string()
        })?;
    let process_terminated = normalized_arm
        .find("set_terminated();")
        .ok_or_else(|| "unavailable CR3 arm has no process-owned termination call".to_string())?;
    let closure = braced_block(no_cr3_arm, &no_cr3_mask, with_thread_mut_source)
        .ok_or_else(|| "scheduler with_thread_mut has no closure body".to_string())?;
    if identifier_offsets(closure, &code_mask(closure), "set_terminated").is_empty() {
        return Err("unavailable CR3 arm does not terminate the scheduler thread".to_string());
    }
    let closure_terminated = normalized_arm
        .get(with_thread_mut..)
        .and_then(|suffix| suffix.find("set_terminated();"))
        .map(|offset| with_thread_mut + offset)
        .ok_or_else(|| "scheduler closure lacks set_terminated".to_string())?;
    let set_need_resched = normalized_arm
        .find("scheduler::set_need_resched();")
        .ok_or_else(|| "unavailable CR3 arm has no set_need_resched".to_string())?;
    let setup_idle = normalized_arm
        .find("setup_idle_return(interrupt_frame);")
        .ok_or_else(|| "unavailable CR3 arm has no setup_idle_return".to_string())?;
    let switch_idle = normalized_arm
        .find("scheduler::switch_to_idle();")
        .ok_or_else(|| "unavailable CR3 arm has no switch_to_idle".to_string())?;
    let switch_kernel_page_table = normalized_arm
        .find("crate::memory::process_memory::switch_to_kernel_page_table();")
        .ok_or_else(|| "unavailable CR3 arm has no kernel page-table recovery".to_string())?;
    let early_return = normalized_arm
        .find("return;")
        .ok_or_else(|| "unavailable CR3 arm has no early return".to_string())?;
    if !(process_terminated < with_thread_mut
        && with_thread_mut < closure_terminated
        && closure_terminated < set_need_resched
        && set_need_resched < setup_idle
        && setup_idle < switch_idle
        && switch_idle < switch_kernel_page_table
        && switch_kernel_page_table < early_return)
    {
        return Err("unavailable CR3 recovery sequence is out of order".to_string());
    }

    Ok(())
}

fn validate_no_cr3_dispatch_fails_closed(source: &str) -> Result<(), String> {
    let restore = function_body(source, "restore_userspace_thread_context")
        .ok_or_else(|| "missing fn restore_userspace_thread_context".to_string())?;
    let restore_mask = code_mask(restore);
    let cr3_writes = identifier_offsets(restore, &restore_mask, "set_next_cr3");
    if cr3_writes.len() != 1 {
        return Err(format!(
            "restore must publish exactly one next CR3, found {}",
            cr3_writes.len()
        ));
    }
    let cr3_resolutions = identifier_offsets(restore, &restore_mask, "cr3_value");
    let unpublished_refusals =
        identifier_offsets(restore, &restore_mask, "refuse_unpublished_dispatch");
    if cr3_resolutions.is_empty()
        || unpublished_refusals.len() != 1
        || unpublished_refusals[0] >= cr3_resolutions[0]
        || cr3_writes
            .iter()
            .any(|write| *write < unpublished_refusals[0])
        || identifier_offsets(restore, &restore_mask, "Cr3")
            .into_iter()
            .any(|write| write < unpublished_refusals[0])
    {
        return Err(
            "normal userspace restore does not refuse unpublished rows before CR3 resolution and publication"
                .to_string(),
        );
    }
    let unpublished_arm = identifier_offsets(restore, &restore_mask, "if")
        .into_iter()
        .filter_map(|if_offset| {
            let block = braced_block(restore, &restore_mask, if_offset)?;
            let end = if_offset + block.len();
            (unpublished_refusals[0] >= if_offset && unpublished_refusals[0] < end).then_some(block)
        })
        .min_by_key(|block| block.len())
        .ok_or_else(|| "normal-restore unpublished refusal has no recovery arm".to_string())?;
    let compact_unpublished_arm = normalized_code(unpublished_arm).replace(' ', "");
    if !identifier_offsets(
        unpublished_arm,
        &code_mask(unpublished_arm),
        "set_terminated",
    )
    .is_empty()
        || !compact_unpublished_arm.contains("scheduler::set_need_resched();")
        || !compact_unpublished_arm.contains("setup_idle_return(interrupt_frame);")
        || !compact_unpublished_arm.contains("scheduler::switch_to_idle();")
        || !compact_unpublished_arm.contains("scheduler::requeue_refused_dispatch(")
        || !compact_unpublished_arm.contains("return;")
    {
        return Err("normal-restore unpublished-row recovery is not retry-only".to_string());
    }
    let switch_idle = compact_unpublished_arm
        .find("scheduler::switch_to_idle();")
        .ok_or_else(|| "normal-restore refusal has no switch_to_idle".to_string())?;
    let requeue = compact_unpublished_arm
        .find("scheduler::requeue_refused_dispatch(")
        .ok_or_else(|| "normal-restore refusal has no refused-thread requeue".to_string())?;
    let early_return = compact_unpublished_arm
        .find("return;")
        .ok_or_else(|| "normal-restore refusal has no early return".to_string())?;
    if !(switch_idle < requeue && requeue < early_return) {
        return Err("normal-restore unpublished-row requeue is out of order".to_string());
    }

    let (cr3_if_offset, cr3_if_block) = identifier_offsets(restore, &restore_mask, "if")
        .into_iter()
        .find_map(|if_offset| {
            let block = braced_block(restore, &restore_mask, if_offset)?;
            let open = block.find('{')?;
            let predicate = normalized_predicate(&block[..open]);
            let block_start = if_offset;
            let block_end = block_start + block.len();
            (predicate == "if let Some(cr3_value) = process_cr3"
                && cr3_writes[0] >= block_start
                && cr3_writes[0] < block_end)
                .then_some((if_offset, block))
        })
        .ok_or_else(|| {
            "next CR3 publish is not guarded by if let Some(cr3_value) = process_cr3".to_string()
        })?;

    let if_end = cr3_if_offset + cr3_if_block.len();
    let else_offset = identifier_offsets(restore, &restore_mask, "else")
        .into_iter()
        .find(|offset| *offset >= if_end)
        .ok_or_else(|| "CR3 guard has no else arm".to_string())?;
    let no_cr3_arm = braced_block(restore, &restore_mask, else_offset)
        .ok_or_else(|| "CR3 guard else arm has no block".to_string())?;
    let normalized_arm = normalized_code(no_cr3_arm);
    let arm_mask = code_mask(no_cr3_arm);
    let arm_bytes = no_cr3_arm.as_bytes();
    if identifier_offsets(no_cr3_arm, &arm_mask, "log")
        .into_iter()
        .any(|offset| {
            let mut cursor = offset + "log".len();
            while cursor < arm_bytes.len()
                && (!arm_mask[cursor] || arm_bytes[cursor].is_ascii_whitespace())
            {
                cursor += 1;
            }
            arm_bytes.get(cursor..cursor + 2) == Some(b"::")
        })
    {
        return Err("no-CR3 arm still calls log".to_string());
    }

    let process_terminated = normalized_arm
        .find("thread.set_terminated();")
        .ok_or_else(|| "no-CR3 arm does not terminate the process-owned thread".to_string())?;
    let with_thread_mut_source = identifier_offsets(no_cr3_arm, &arm_mask, "with_thread_mut")
        .into_iter()
        .next()
        .ok_or_else(|| "no-CR3 arm has no scheduler with_thread_mut closure".to_string())?;
    let with_thread_mut = normalized_arm
        .find("scheduler::with_thread_mut")
        .ok_or_else(|| "no-CR3 arm has no scheduler with_thread_mut call".to_string())?;
    if process_terminated >= with_thread_mut {
        return Err("process-owned termination must precede scheduler synchronization".to_string());
    }

    let closure = braced_block(no_cr3_arm, &arm_mask, with_thread_mut_source)
        .ok_or_else(|| "with_thread_mut has no closure block".to_string())?;
    let closure_mask = code_mask(closure);
    if identifier_offsets(closure, &closure_mask, "set_terminated").is_empty() {
        return Err("with_thread_mut closure does not terminate its scheduler thread".to_string());
    }

    let closure_terminated = normalized_arm
        .get(with_thread_mut..)
        .and_then(|suffix| suffix.find("set_terminated();"))
        .map(|offset| with_thread_mut + offset)
        .ok_or_else(|| "with_thread_mut closure lacks scheduler set_terminated".to_string())?;
    let set_need_resched = normalized_arm
        .find("scheduler::set_need_resched();")
        .ok_or_else(|| "no-CR3 arm has no set_need_resched".to_string())?;
    let setup_idle = normalized_arm
        .find("setup_idle_return(interrupt_frame);")
        .ok_or_else(|| "no-CR3 arm has no setup_idle_return".to_string())?;
    let switch_idle = normalized_arm
        .find("scheduler::switch_to_idle();")
        .ok_or_else(|| "no-CR3 arm has no switch_to_idle".to_string())?;
    let early_return = normalized_arm
        .find("return;")
        .ok_or_else(|| "no-CR3 arm has no early return".to_string())?;
    if !(with_thread_mut < closure_terminated
        && closure_terminated < set_need_resched
        && set_need_resched < setup_idle
        && setup_idle < switch_idle
        && switch_idle < early_return)
    {
        return Err("no-CR3 recovery sequence is out of order".to_string());
    }

    Ok(())
}

fn validate_fault_idle_return_source(source: &str) -> Result<(), String> {
    fn inline_rsp_outputs(body: &str) -> Vec<String> {
        let mask = code_mask(body);
        let mut outputs = Vec::new();
        for (literal_offset, literal) in body.match_indices("\"mov {}, rsp\"") {
            let after_literal = literal_offset + literal.len();
            let tail = &body[after_literal..];
            let tail_mask = &mask[after_literal..];
            let Some(out_offset) = identifier_offsets(tail, tail_mask, "out").first().copied()
            else {
                continue;
            };
            let after_out = after_literal + out_offset + "out".len();
            let reg_tail = &body[after_out..];
            let reg_mask = &mask[after_out..];
            let Some(reg_offset) = identifier_offsets(reg_tail, reg_mask, "reg")
                .first()
                .copied()
            else {
                continue;
            };
            let mut cursor = after_out + reg_offset + "reg".len();
            while cursor < body.len()
                && (!mask[cursor] || !identifier_byte(body.as_bytes()[cursor]))
            {
                cursor += 1;
            }
            let start = cursor;
            while cursor < body.len() && mask[cursor] && identifier_byte(body.as_bytes()[cursor]) {
                cursor += 1;
            }
            if cursor > start {
                outputs.push(body[start..cursor].to_string());
            }
        }
        outputs
    }

    for handler in ["page_fault_handler", "general_protection_fault_handler"] {
        let body = function_body(source, handler).ok_or_else(|| format!("missing fn {handler}"))?;
        let mask = code_mask(body);
        if identifier_offsets(body, &mask, "setup_idle_return").len() != 1 {
            return Err(format!(
                "{handler} does not use the shared idle-return helper"
            ));
        }
        if !identifier_offsets(body, &mask, "kernel_stack_top").is_empty() {
            return Err(format!(
                "{handler} computes an idle stack from kernel_stack_top"
            ));
        }
        for output in inline_rsp_outputs(body) {
            for stack_pointer in identifier_offsets(body, &mask, "stack_pointer") {
                let end = (stack_pointer..body.len())
                    .find(|offset| mask[*offset] && body.as_bytes()[*offset] == b';')
                    .unwrap_or(body.len());
                let statement = &body[stack_pointer..end];
                let statement_mask = code_mask(statement);
                if statement.contains('=')
                    && !identifier_offsets(statement, &statement_mask, &output).is_empty()
                {
                    return Err(format!(
                        "{handler} derives its idle stack from the exception RSP"
                    ));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn unblock_preserves_blocked_syscall_context_ownership() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    assert_eq!(validate_unblock_source(&source), Ok(()));
}

#[test]
fn unblock_validator_rejects_foreign_flag_clear() {
    let synthetic = r#"
        fn unblock(&mut self, thread_id: u64) {
            if let Some(thread) = self.get_thread_mut(thread_id) {
                thread.set_ready();
                thread.blocked_in_syscall = false;
            }
        }
    "#;
    assert!(validate_unblock_source(synthetic).is_err());
}

#[test]
fn switch_dispatches_saved_kernel_frames_to_kernel_resume() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(validate_switch_source(&source), Ok(()));
}

#[test]
fn switch_validator_rejects_flag_only_dispatch() {
    let synthetic = r#"
        fn saved_context_is_kernel_frame(thread_id: u64) -> bool {
            is_kernel_code_selector(saved_context(thread_id).cs)
        }

        fn switch_to_thread(thread_id: u64) {
            if is_idle {
                setup_idle_return();
            } else if is_kernel_thread {
                setup_kernel_thread_return(thread_id);
            } else if blocked_in_syscall {
                setup_kernel_thread_return(thread_id);
            } else {
                restore_userspace_thread_context(thread_id);
            }
        }
    "#;
    assert!(validate_switch_source(synthetic).is_err());
}

#[test]
fn restore_rejects_kernel_frames_before_context_writes() {
    let source = repo_text("kernel/src/task/process_context.rs");
    assert_eq!(validate_restore_source(&source), Ok(()));
}

#[test]
fn restore_validator_rejects_missing_kernel_selector_guard() {
    let synthetic = r#"
        fn restore_userspace_context(
            thread: &Thread,
            interrupt_frame: &mut InterruptStackFrame,
            saved_regs: &mut SavedRegisters,
        ) -> Result<(), RestoreError> {
            let rip = match VirtAddr::try_new(thread.context.rip) {
                Ok(addr) => addr,
                Err(_) => return Err(RestoreError::NonCanonicalRip),
            };
            let rsp = match VirtAddr::try_new(thread.context.rsp) {
                Ok(addr) => addr,
                Err(_) => return Err(RestoreError::NonCanonicalRsp),
            };
            saved_regs.rax = thread.context.rax;
            interrupt_frame.as_mut().update(|frame| {
                frame.instruction_pointer = rip;
                frame.stack_pointer = rsp;
            });
            Ok(())
        }
    "#;
    assert!(validate_restore_source(synthetic).is_err());
}

#[test]
fn saved_frame_routing_matches_userspace_restore_enforcement() {
    let routing_source = repo_text("kernel/src/interrupts/context_switch.rs");
    let enforcement_source = repo_text("kernel/src/task/process_context.rs");
    assert_eq!(
        validate_routing_matches_enforcement(&routing_source, &enforcement_source),
        Ok(())
    );
}

#[test]
fn routing_enforcement_validator_rejects_has_started_conjunct() {
    let routing_mutant = r#"
        fn saved_context_is_kernel_frame(thread_id: u64) -> bool {
            process_for_thread(thread_id)
                .main_thread
                .as_ref()
                .is_some_and(|thread| {
                    thread.has_started && is_kernel_code_selector(thread.context.cs)
                })
        }
    "#;
    let enforcement_source = repo_text("kernel/src/task/process_context.rs");
    assert!(validate_routing_matches_enforcement(routing_mutant, &enforcement_source).is_err());
}

#[test]
fn routing_enforcement_validator_rejects_missing_selector_check() {
    let routing_mutant = r#"
        fn saved_context_is_kernel_frame(thread_id: u64) -> bool {
            process_for_thread(thread_id)
                .main_thread
                .as_ref()
                .is_some_and(|thread| thread.privilege == ThreadPrivilege::User)
        }
    "#;
    let enforcement_source = repo_text("kernel/src/task/process_context.rs");
    assert!(validate_routing_matches_enforcement(routing_mutant, &enforcement_source).is_err());
}

#[test]
fn dispatch_guard_is_shared_and_acquired_before_scheduling() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(validate_dispatch_guard_precheck(&source), Ok(()));
}

#[test]
fn dispatch_guard_validator_rejects_userspace_only_acquisition() {
    let synthetic = r#"
        fn check_need_resched_and_switch() {
            let mut process_manager_guard = if from_userspace {
                match crate::process::try_manager() {
                    Some(guard) => guard,
                    None => {
                        scheduler::set_need_resched();
                        return;
                    }
                }
            } else {
                None
            };
            let schedule_result = scheduler::schedule();
        }
    "#;
    assert!(validate_dispatch_guard_precheck(synthetic).is_err());
}

#[test]
fn dispatch_guard_validator_rejects_acquisition_after_scheduling() {
    let synthetic = r#"
        fn check_need_resched_and_switch() {
            let schedule_result = scheduler::schedule();
            let mut process_manager_guard = match crate::process::try_manager() {
                Some(guard) => guard,
                None => {
                    scheduler::set_need_resched();
                    return;
                }
            };
        }
    "#;
    assert!(validate_dispatch_guard_precheck(synthetic).is_err());
}

#[test]
fn dispatch_guard_validator_rejects_unavailable_fallthrough() {
    let synthetic = r#"
        fn check_need_resched_and_switch() {
            let mut process_manager_guard = match crate::process::try_manager() {
                Some(guard) => guard,
                None => {
                    scheduler::set_need_resched();
                    scheduler::schedule();
                }
            };
            let schedule_result = scheduler::schedule();
        }
    "#;
    assert!(validate_dispatch_guard_precheck(synthetic).is_err());
}

#[test]
fn first_run_dispatch_logs_only_after_installation() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(validate_first_run_dispatch_has_no_logging(&source), Ok(()));
}

#[test]
fn first_run_logging_validator_rejects_aborted_arm_logging() {
    let synthetic = r#"
        fn restore_userspace_thread_context() {
            if !has_started {
                match setup_first_userspace_entry() {
                    FirstUserspaceEntry::Installed => {
                        log::info!("First run installed");
                        thread.has_started = true;
                    }
                    FirstUserspaceEntry::Aborted(reason) => {
                        log::info!("First run aborted: {}", reason);
                        scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
                        scheduler::set_need_resched();
                    }
                }
            }
        }

        fn setup_first_userspace_entry() {
            interrupt_frame.as_mut().update(|frame| {
                frame.code_segment = crate::gdt::user_code_selector();
                log::info!("ring 3 committed");
            });
        }
    "#;
    assert!(validate_first_run_dispatch_has_no_logging(synthetic).is_err());
}

#[test]
fn first_run_logging_validator_rejects_setup_prologue_logging() {
    let synthetic = r#"
        fn restore_userspace_thread_context() {
            if !has_started {
                match setup_first_userspace_entry() {
                    FirstUserspaceEntry::Installed => {
                        log::info!("First run installed");
                        thread.has_started = true;
                    }
                    FirstUserspaceEntry::Aborted(reason) => {
                        raw_serial_str(reason);
                        scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
                        scheduler::set_need_resched();
                    }
                }
            }
        }

        fn setup_first_userspace_entry() {
            log::info!("attempting first entry");
            interrupt_frame.as_mut().update(|frame| {
                frame.code_segment = crate::gdt::user_code_selector();
                log::info!("ring 3 committed");
            });
        }
    "#;
    assert!(validate_first_run_dispatch_has_no_logging(synthetic).is_err());
}

#[test]
fn first_userspace_entry_installs_address_space_before_ring3_commit() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(validate_first_userspace_entry_source(&source), Ok(()));
}

#[test]
fn first_userspace_entry_validator_rejects_write_first_dispatch() {
    let synthetic = r#"
        fn restore_userspace_thread_context(
            thread_id: u64,
            interrupt_frame: &mut InterruptStackFrame,
            saved_regs: &mut SavedRegisters,
        ) {
            thread.has_started = true;
            setup_first_userspace_entry(thread_id, interrupt_frame, saved_regs);
        }

        fn setup_first_userspace_entry(
            interrupt_frame: &mut InterruptStackFrame,
            saved_regs: &mut SavedRegisters,
        ) {
            interrupt_frame.as_mut().update(|frame| {
                frame.code_segment = crate::gdt::user_code_selector();
            });
            saved_regs.rax = 0;
            if let Some(cr3_value) = process.cr3_value() {
                crate::per_cpu::set_next_cr3(cr3_value);
                crate::gdt::set_kernel_stack(kernel_stack_top);
            }
        }
    "#;
    assert!(validate_first_userspace_entry_source(synthetic).is_err());
}

#[test]
fn first_userspace_entry_validator_rejects_idle_abort_recovery() {
    let synthetic = r#"
        fn restore_userspace_thread_context(
            thread_id: u64,
            resume_thread_id: u64,
            interrupt_frame: &mut InterruptStackFrame,
            saved_regs: &mut SavedRegisters,
        ) {
            match setup_first_userspace_entry(thread_id, interrupt_frame, saved_regs) {
                FirstUserspaceEntry::Installed => {
                    scheduler::with_thread_mut(thread_id, |thread| {
                        thread.has_started = true;
                    });
                }
                FirstUserspaceEntry::Aborted(_) => {
                    scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
                    setup_idle_return(interrupt_frame);
                    scheduler::set_need_resched();
                }
            }
        }

        fn setup_first_userspace_entry(
            interrupt_frame: &mut InterruptStackFrame,
            saved_regs: &mut SavedRegisters,
        ) {
            if let Some(cr3_value) = process.cr3_value() {
                crate::per_cpu::set_next_cr3(cr3_value);
            } else {
                return FirstUserspaceEntry::Aborted("missing CR3");
            }
            if let Some(kernel_stack_top) = thread.kernel_stack_top {
                crate::gdt::set_kernel_stack(kernel_stack_top);
            } else {
                return FirstUserspaceEntry::Aborted("missing RSP0");
            }
            interrupt_frame.as_mut().update(|frame| {
                frame.code_segment = crate::gdt::user_code_selector();
            });
            saved_regs.rax = 0;
            FirstUserspaceEntry::Installed
        }
    "#;
    assert!(validate_first_userspace_entry_source(synthetic).is_err());
}

fn synthetic_blocked_syscall_dispatch_source(dispatch_body: &str) -> String {
    format!(
        r#"
        fn switch_to_thread(thread_id: u64) {{
            if is_idle {{
                setup_idle_return(interrupt_frame);
            }} else if is_kernel_thread {{
                setup_kernel_thread_return(thread_id);
            }} else if blocked_in_syscall
                || saved_context_is_kernel_frame(thread_id, process_manager_guard.as_ref())
            {{
                if let Some((pid, process)) = manager.find_process_by_thread_mut(thread_id) {{
                    {dispatch_body}
                }}
            }}
        }}
        "#,
        dispatch_body = dispatch_body,
    )
}

fn valid_blocked_syscall_no_cr3_arm() -> &'static str {
    r#"
        if refuse_unpublished_dispatch(process, thread_id, pid.as_u64()) {
            scheduler::set_need_resched();
            setup_idle_return(interrupt_frame);
            scheduler::switch_to_idle();
            scheduler::requeue_refused_dispatch(thread_id);
            unsafe {
                crate::memory::process_memory::switch_to_kernel_page_table();
            }
            return;
        }
        let process_cr3 = match process.cr3_value() {
            Some(cr3_value) => cr3_value,
            None => {
                USERSPACE_DISPATCH_NO_CR3_REFUSED.fetch_add(1, Ordering::Relaxed);
                if !USERSPACE_DISPATCH_NO_CR3_LOGGED.swap(true, Ordering::Relaxed) {
                    raw_serial_str("[PMGUARD] no-cr3 dispatch refused tid=");
                    raw_serial_u64(thread_id);
                    raw_serial_str(" pid=");
                    raw_serial_u64(pid.as_u64());
                    raw_serial_str("\n");
                }
                if let Some(ref mut thread) = process.main_thread {
                    thread.set_terminated();
                }
                scheduler::with_thread_mut(thread_id, |sched_thread| {
                    sched_thread.set_terminated();
                });
                scheduler::set_need_resched();
                setup_idle_return(interrupt_frame);
                scheduler::switch_to_idle();
                unsafe {
                    crate::memory::process_memory::switch_to_kernel_page_table();
                }
                return;
            }
        };
        saved_regs.rax = 0;
        interrupt_frame.as_mut().update(|frame| {
            frame.instruction_pointer = VirtAddr::new(0);
        });
        unsafe {
            crate::per_cpu::set_next_cr3(process_cr3);
        }
    "#
}

#[test]
fn blocked_syscall_dispatch_resolves_cr3_before_commit() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(
        validate_blocked_syscall_dispatch_resolves_cr3(&source),
        Ok(())
    );
}

#[test]
fn blocked_syscall_dispatch_validator_rejects_per_use_cr3_guards() {
    let synthetic = synthetic_blocked_syscall_dispatch_source(
        r#"
            saved_regs.rax = 0;
            interrupt_frame.as_mut().update(|_| {});
            if has_pending_signals {
                if let Some(cr3_val) = process.cr3_value() {
                    unsafe { Cr3::write(PhysFrame::containing_address(PhysAddr::new(cr3_val)), Cr3Flags::empty()); }
                }
            }
            if let Some(cr3_val) = process.cr3_value() {
                unsafe { Cr3::write(PhysFrame::containing_address(PhysAddr::new(cr3_val)), Cr3Flags::empty()); }
            }
            if let Some(cr3_val) = process.cr3_value() {
                unsafe { crate::per_cpu::set_next_cr3(cr3_val); }
            }
        "#,
    );
    assert!(validate_blocked_syscall_dispatch_resolves_cr3(&synthetic).is_err());
}

#[test]
fn blocked_syscall_dispatch_validator_rejects_log_only_no_cr3() {
    let synthetic = synthetic_blocked_syscall_dispatch_source(
        r#"
            let process_cr3 = match process.cr3_value() {
                Some(cr3_value) => cr3_value,
                None => {
                    raw_serial_str("missing CR3");
                }
            };
            saved_regs.rax = 0;
            interrupt_frame.as_mut().update(|_| {});
            unsafe { crate::per_cpu::set_next_cr3(process_cr3); }
        "#,
    );
    assert!(validate_blocked_syscall_dispatch_resolves_cr3(&synthetic).is_err());
}

#[test]
fn blocked_syscall_dispatch_validator_rejects_late_no_cr3_refusal() {
    let refusal = valid_blocked_syscall_no_cr3_arm();
    let synthetic = synthetic_blocked_syscall_dispatch_source(&format!(
        r#"
            saved_regs.rax = 0;
            interrupt_frame.as_mut().update(|_| {{}});
            {refusal}
        "#,
        refusal = refusal,
    ));
    assert!(validate_blocked_syscall_dispatch_resolves_cr3(&synthetic).is_err());
}

fn synthetic_no_cr3_restore_source(else_arm: &str) -> String {
    format!(
        r#"
        fn restore_userspace_thread_context() {{
            if refuse_unpublished_dispatch(process, thread_id, pid.as_u64()) {{
                scheduler::set_need_resched();
                setup_idle_return(interrupt_frame);
                scheduler::switch_to_idle();
                scheduler::requeue_refused_dispatch(thread_id);
                return;
            }}
            let process_cr3 = process.cr3_value();
            if let Some(cr3_value) = process_cr3 {{
                crate::per_cpu::set_next_cr3(cr3_value);
            }} else {{
                {else_arm}
            }}
        }}
        "#
    )
}

fn validate_clone_publication_lifecycle(clone: &str) -> Result<(), String> {
    let sources = rust_sources_below("kernel/src");
    let mut ready_writes = Vec::new();
    for (path, source) in &sources {
        let mask = code_mask(source);
        for (offset, _) in source.match_indices("ProcessState::Ready") {
            if !mask.get(offset).copied().unwrap_or(false) {
                continue;
            }
            let line_start = source[..offset]
                .rfind('\n')
                .map(|line| line + 1)
                .unwrap_or(0);
            let prefix = source[line_start..offset].trim_end();
            let Some(operator) = prefix.as_bytes().last().copied() else {
                continue;
            };
            if operator != b'=' {
                continue;
            }
            let before_operator = prefix
                .as_bytes()
                .get(prefix.len().saturating_sub(2))
                .copied();
            if before_operator.is_some_and(|byte| matches!(byte, b'=' | b'!' | b'<' | b'>')) {
                continue;
            }
            ready_writes.push(path.clone());
        }
    }
    if ready_writes.is_empty()
        || ready_writes
            .iter()
            .any(|path| !path.ends_with("kernel/src/process/process.rs"))
    {
        return Err("ProcessState::Ready writes escaped the lifecycle module".to_string());
    }

    let process = repo_text("kernel/src/process/process.rs");
    let admits = function_body(&process, "admits_clone")
        .ok_or_else(|| "missing Process::admits_clone".to_string())?;
    let unpublished = function_body(&process, "is_unpublished")
        .ok_or_else(|| "missing Process::is_unpublished".to_string())?;
    let admits = normalized_code(admits).replace(' ', "");
    let unpublished = normalized_code(unpublished).replace(' ', "");
    if admits.contains("_=>")
        || !admits.contains("matchself.state{")
        || !admits.contains("ProcessState::Creating=>false")
        || !admits.contains("ProcessState::Ready|ProcessState::Running|ProcessState::Blocked=>true")
        || !admits.contains("ProcessState::Terminated(_)=>false")
        || admits.contains("ProcessState::Creating=>true")
        || admits.contains("ProcessState::Terminated(_)=>true")
    {
        return Err("admits_clone is not an exhaustive live-row predicate".to_string());
    }
    if unpublished.contains("_=>")
        || !unpublished.contains("matchself.state{")
        || !unpublished.contains("ProcessState::Creating=>true")
        || !unpublished.contains("ProcessState::Ready=>false")
        || !unpublished.contains("ProcessState::Running=>false")
        || !unpublished.contains("ProcessState::Blocked=>false")
        || !unpublished.contains("ProcessState::Terminated(_)=>false")
        || unpublished.matches("=>true").count() != 1
    {
        return Err("is_unpublished does not accept exactly Creating".to_string());
    }

    let clone_body =
        function_body(clone, "sys_clone").ok_or_else(|| "missing sys_clone body".to_string())?;
    let clone_mask = code_mask(clone_body);

    let admission_calls = call_offsets(clone_body, &clone_mask, "admit_clone_into");
    if admission_calls.len() != 1 {
        return Err(format!(
            "sys_clone must call admit_clone_into exactly once, found {}",
            admission_calls.len()
        ));
    }
    let admission = admission_calls[0];

    let insert_calls = call_offsets(clone_body, &clone_mask, "insert_process");
    if insert_calls.len() != 1 {
        return Err(format!(
            "sys_clone must call insert_process exactly once, found {}",
            insert_calls.len()
        ));
    }
    let insert = insert_calls[0];

    let cwd_clones = call_offsets(clone_body, &clone_mask, "clone")
        .into_iter()
        .filter(|offset| {
            let prefix = &clone_body[..*offset];
            let prefix_mask = &clone_mask[..*offset];
            identifier_offsets(prefix, prefix_mask, "cwd")
                .last()
                .is_some_and(|cwd| normalized_code(&clone_body[*cwd..*offset]) == "cwd.")
        })
        .collect::<Vec<_>>();
    let fd_table_clones = call_offsets(clone_body, &clone_mask, "clone")
        .into_iter()
        .filter(|offset| {
            let prefix = &clone_body[..*offset];
            let prefix_mask = &clone_mask[..*offset];
            identifier_offsets(prefix, prefix_mask, "fd_table")
                .last()
                .is_some_and(|fd_table| {
                    normalized_code(&clone_body[*fd_table..*offset]) == "fd_table."
                })
        })
        .collect::<Vec<_>>();
    if cwd_clones.len() != 1 || fd_table_clones.len() != 1 {
        return Err(format!(
            "sys_clone must copy parent cwd and fd_table exactly once, found cwd={} fd_table={}",
            cwd_clones.len(),
            fd_table_clones.len()
        ));
    }
    let parent_state_binding = identifier_offsets(clone_body, &clone_mask, "parent_cr3")
        .first()
        .copied()
        .ok_or_else(|| "sys_clone has no parent-state copy block".to_string())?;
    let parent_state_block = braced_block(clone_body, &clone_mask, parent_state_binding)
        .ok_or_else(|| "sys_clone parent-state copy block is not brace balanced".to_string())?;
    let parent_state_end = parent_state_binding + parent_state_block.len();
    if !(parent_state_binding..parent_state_end).contains(&cwd_clones[0])
        || !(parent_state_binding..parent_state_end).contains(&fd_table_clones[0])
    {
        return Err("sys_clone cwd/fd_table reads escaped the parent-state copy block".to_string());
    }
    if admission >= insert || admission >= parent_state_binding {
        return Err(
            "sys_clone admits the clone after deriving parent state or publishing the child"
                .to_string(),
        );
    }

    let admission_arm = identifier_offsets(clone_body, &clone_mask, "if")
        .into_iter()
        .find_map(|if_offset| {
            let block = braced_block(clone_body, &clone_mask, if_offset)?;
            let open = block.find('{')?;
            (!call_offsets(
                &block[..open],
                &code_mask(&block[..open]),
                "admit_clone_into",
            )
            .is_empty())
            .then_some(block)
        })
        .ok_or_else(|| {
            "sys_clone admission call is not the condition of a refusal arm".to_string()
        })?;
    let admission_arm_mask = code_mask(admission_arm);
    let compact_admission_arm = normalized_code(admission_arm).replace(' ', "");
    if identifier_offsets(admission_arm, &admission_arm_mask, "return").len() != 1
        || identifier_offsets(admission_arm, &admission_arm_mask, "Err").len() != 1
        || identifier_offsets(admission_arm, &admission_arm_mask, "EAGAIN").len() != 1
        || !compact_admission_arm.contains("returnSyscallResult::Err(super::errno::EAGAINasu64);")
    {
        return Err("sys_clone admission refusal must return super::errno::EAGAIN".to_string());
    }

    let manager_guard_bindings = binding_offsets(clone_body, &clone_mask, "manager_guard");
    if manager_guard_bindings.len() != 1 {
        return Err(format!(
            "sys_clone must bind manager_guard exactly once, found {}",
            manager_guard_bindings.len()
        ));
    }
    let manager_guard_statement_end = (manager_guard_bindings[0]..clone_body.len())
        .find(|offset| clone_mask[*offset] && clone_body.as_bytes()[*offset] == b';')
        .ok_or_else(|| "sys_clone manager_guard binding has no terminator".to_string())?;
    if !normalized_code(&clone_body[manager_guard_bindings[0]..=manager_guard_statement_end])
        .contains("crate::process::manager()")
    {
        return Err("sys_clone manager_guard is not bound from the process manager".to_string());
    }
    if manager_guard_bindings[0] >= admission {
        return Err("sys_clone manager_guard is bound after clone admission".to_string());
    }

    let manager_guard_drops = call_offsets(clone_body, &clone_mask, "drop")
        .into_iter()
        .filter(|offset| {
            let statement_end = (*offset..clone_body.len())
                .find(|index| clone_mask[*index] && clone_body.as_bytes()[*index] == b';')
                .unwrap_or(clone_body.len());
            normalized_code(&clone_body[*offset..statement_end]).replace(' ', "")
                == "drop(manager_guard)"
        })
        .collect::<Vec<_>>();
    let (init_refusal_start, init_refusal_arm) = identifier_offsets(clone_body, &clone_mask, "if")
        .into_iter()
        .find_map(|if_offset| {
            let block = braced_block(clone_body, &clone_mask, if_offset)?;
            let open = block.find('{')?;
            (call_offsets(
                &block[..open],
                &code_mask(&block[..open]),
                "refuses_init_group_clone",
            )
            .len()
                == 1)
                .then_some((if_offset, block))
        })
        .ok_or_else(|| "sys_clone init-group refusal is not an if guard".to_string())?;
    let init_refusal_range = init_refusal_start..init_refusal_start + init_refusal_arm.len();
    let init_refusal_mask = code_mask(init_refusal_arm);
    let compact_init_refusal = normalized_code(init_refusal_arm).replace(' ', "");
    if manager_guard_drops
        .iter()
        .filter(|drop| init_refusal_range.contains(drop))
        .count()
        != 1
        || identifier_offsets(init_refusal_arm, &init_refusal_mask, "return").len() != 1
        || !compact_init_refusal
            .contains("returnSyscallResult::Err(super::errno::EINVALasu64);")
    {
        return Err(
            "sys_clone init-group refusal must drop the guard and return EINVAL".to_string(),
        );
    }
    if manager_guard_drops.iter().any(|drop| {
        admission < *drop && *drop < insert && !init_refusal_range.contains(drop)
    }) {
        return Err(
            "sys_clone drops manager_guard between clone admission and child publication"
                .to_string(),
        );
    }

    let child_thread_bindings = binding_offsets(clone_body, &clone_mask, "child_thread");
    if child_thread_bindings.len() != 1 {
        return Err(format!(
            "sys_clone must construct child_thread exactly once, found {} bindings",
            child_thread_bindings.len()
        ));
    }
    let child_thread_literal = braced_block(clone_body, &clone_mask, child_thread_bindings[0])
        .ok_or_else(|| "sys_clone child_thread binding has no Thread literal".to_string())?;
    let child_thread_literal_mask = code_mask(child_thread_literal);
    if code_offsets(
        child_thread_literal,
        &child_thread_literal_mask,
        "crate::task::thread::ThreadState::Blocked",
    )
    .len()
        != 1
        || !normalized_code(child_thread_literal)
            .replace(' ', "")
            .contains("state:crate::task::thread::ThreadState::Blocked,")
    {
        return Err("sys_clone child Thread must be constructed Blocked".to_string());
    }

    let attach_calls = call_offsets(clone_body, &clone_mask, "attach_main_thread_unpublished");
    if attach_calls.len() != 1 {
        return Err(format!(
            "sys_clone must call attach_main_thread_unpublished exactly once, found {}",
            attach_calls.len()
        ));
    }
    let set_main_thread_calls = call_offsets(clone_body, &clone_mask, "set_main_thread");
    if !set_main_thread_calls.is_empty() {
        return Err(format!(
            "sys_clone must not call set_main_thread, found {} calls",
            set_main_thread_calls.len()
        ));
    }
    if !assigned_value_offsets(clone_body, &clone_mask, "ProcessState::Ready").is_empty() {
        return Err(
            "sys_clone writes ProcessState::Ready outside the lifecycle module".to_string(),
        );
    }

    let set_ready_calls = call_offsets(clone_body, &clone_mask, "set_ready");
    let runnable_thread_writes = assigned_value_offsets(
        clone_body,
        &clone_mask,
        "crate::task::thread::ThreadState::Ready",
    );
    let publication_manager_guard_drops = manager_guard_drops
        .into_iter()
        .filter(|drop| !init_refusal_range.contains(drop))
        .collect::<Vec<_>>();
    let spawn_calls = call_offsets(clone_body, &clone_mask, "spawn");
    let publication_steps = [
        ("attach_main_thread_unpublished", attach_calls.as_slice()),
        ("insert_process", insert_calls.as_slice()),
        ("set_ready", set_ready_calls.as_slice()),
        ("ThreadState::Ready", runnable_thread_writes.as_slice()),
        (
            "drop(manager_guard)",
            publication_manager_guard_drops.as_slice(),
        ),
        ("spawn", spawn_calls.as_slice()),
    ];
    let mut publication_sequence = Vec::new();
    for (name, offsets) in publication_steps {
        if offsets.len() != 1 {
            return Err(format!(
                "sys_clone publication step {name} must appear exactly once, found {}",
                offsets.len()
            ));
        }
        publication_sequence.push((name, offsets[0]));
    }
    if let Some(link) = publication_sequence
        .windows(2)
        .find(|link| link[0].1 >= link[1].1)
    {
        return Err(format!(
            "sys_clone publication sequence link broke: {} must precede {}",
            link[0].0, link[1].0
        ));
    }

    let manager = repo_text("kernel/src/process/manager.rs");
    let manager_bodies = module_function_bodies(&manager);
    let sibling_guard_definitions = manager_bodies
        .get("find_live_clone_vm_sibling_holding_cr3")
        .map_or(0, Vec::len);
    if sibling_guard_definitions != 1 {
        return Err(format!(
            "find_live_clone_vm_sibling_holding_cr3 must be defined exactly once, found {sibling_guard_definitions}"
        ));
    }

    // Issue #468 is open and this phase does not close it. Census this guard
    // because it sits immediately adjacent to the exec-commit code this phase edits.
    let mut exec_bodies = Vec::new();
    for name in ["exec_process", "exec_process_with_argv"] {
        for body in manager_bodies.get(name).into_iter().flatten() {
            exec_bodies.push((name, *body));
        }
    }
    let aarch64_exec_count = exec_bodies
        .iter()
        .filter(|(_, body)| body.contains("[ARM64]"))
        .count();
    let x86_exec_count = exec_bodies.len() - aarch64_exec_count;
    if exec_bodies.len() != 4 || aarch64_exec_count != 2 || x86_exec_count != 2 {
        return Err(format!(
            "exec body census must be four total (two aarch64, two x86), found total={} aarch64={} x86={}",
            exec_bodies.len(),
            aarch64_exec_count,
            x86_exec_count
        ));
    }
    for (name, body) in exec_bodies {
        let body_mask = code_mask(body);
        let sibling_guard_calls =
            call_offsets(body, &body_mask, "find_live_clone_vm_sibling_holding_cr3");
        if !body.contains("[ARM64]") {
            if !sibling_guard_calls.is_empty() {
                return Err(format!(
                    "x86 {name} must not call find_live_clone_vm_sibling_holding_cr3"
                ));
            }
            continue;
        }
        if sibling_guard_calls.len() != 1 {
            return Err(format!(
                "aarch64 {name} must call find_live_clone_vm_sibling_holding_cr3 exactly once, found {}",
                sibling_guard_calls.len()
            ));
        }
        let allocation = code_offsets(body, &body_mask, "UnpublishedPageTable::new(");
        if allocation.len() != 1 {
            return Err(format!(
                "aarch64 {name} must allocate one UnpublishedPageTable, found {}",
                allocation.len()
            ));
        }
        if sibling_guard_calls[0] >= allocation[0] {
            return Err(format!(
                "aarch64 {name} checks live CLONE_VM siblings after allocating its new address space"
            ));
        }
        let guard_arm = identifier_offsets(body, &body_mask, "if")
            .into_iter()
            .find_map(|if_offset| {
                let block = braced_block(body, &body_mask, if_offset)?;
                let open = block.find('{')?;
                (call_offsets(
                    &block[..open],
                    &code_mask(&block[..open]),
                    "find_live_clone_vm_sibling_holding_cr3",
                )
                .len()
                    == 1)
                    .then_some(block)
            })
            .ok_or_else(|| format!("aarch64 {name} sibling check is not an if guard"))?;
        if !normalized_code(guard_arm).contains("return Err(") {
            return Err(format!(
                "aarch64 {name} live-sibling guard does not return Err"
            ));
        }
    }
    Ok(())
}

fn validate_aarch64_row_unpublished_dispatch(source: &str) -> Result<(), String> {
    let set_next = function_body(source, "set_next_ttbr0_for_thread")
        .ok_or_else(|| "missing set_next_ttbr0_for_thread".to_string())?;
    let set_next_mask = code_mask(set_next);
    let refusal = identifier_offsets(set_next, &set_next_mask, "refuse_unpublished_dispatch");
    let page_table = identifier_offsets(set_next, &set_next_mask, "page_table");
    let inherited = identifier_offsets(set_next, &set_next_mask, "inherited_cr3");
    if refusal.len() != 1
        || page_table.is_empty()
        || inherited.is_empty()
        || refusal[0] >= page_table[0]
        || refusal[0] >= inherited[0]
        || !set_next.contains("return TtbrResult::RowUnpublished;")
    {
        return Err("aarch64 computes TTBR0 before refusing an unpublished row".to_string());
    }

    let enum_offset = source
        .find("enum TtbrResult")
        .ok_or_else(|| "missing TtbrResult enum".to_string())?;
    let enum_body = braced_block(source, &code_mask(source), enum_offset)
        .ok_or_else(|| "TtbrResult enum is not brace balanced".to_string())?;
    if !enum_body.contains("RowUnpublished") {
        return Err("TtbrResult lacks RowUnpublished".to_string());
    }

    let source_mask = code_mask(source);
    let ttbr_matches: Vec<&str> = identifier_offsets(source, &source_mask, "match")
        .into_iter()
        .filter_map(|offset| braced_block(source, &source_mask, offset))
        .filter(|block| {
            let header = block.split_once('{').map_or(*block, |(header, _)| header);
            header.contains("set_next_ttbr0_for_thread") || header.contains("ttbr_result")
        })
        .collect();
    if ttbr_matches.len() != 3
        || ttbr_matches
            .iter()
            .any(|block| !block.contains("TtbrResult::RowUnpublished"))
    {
        return Err("RowUnpublished is not handled at every TtbrResult match".to_string());
    }
    let refusal_body = function_body(source, "refuse_unpublished_dispatch")
        .ok_or_else(|| "missing aarch64 unpublished-row predicate".to_string())?;
    if !refusal_body.contains("USERSPACE_DISPATCH_CREATING_REFUSED.fetch_add")
        || !source.contains("pub fn userspace_dispatch_creating_refused() -> u64")
        || source
            .matches("#[cfg(feature = \"boot_tests\")]\npub fn userspace_dispatch_creating_refused")
            .count()
            != 1
    {
        return Err("aarch64 unpublished-row counter is not readable".to_string());
    }
    Ok(())
}

fn validate_aarch64_failed_exec_ttbr0_rollback(source: &str) -> Result<(), String> {
    let exec = function_body(source, "sys_exec_aarch64")
        .ok_or_else(|| "missing sys_exec_aarch64".to_string())?;
    let exec_mask = code_mask(exec);
    let capture = call_offsets(exec, &exec_mask, "read_ttbr0_for_exec");
    let switch = call_offsets(exec, &exec_mask, "switch_ttbr0_to_kernel");
    let manager_exec = call_offsets(exec, &exec_mask, "exec_process_with_argv");
    if capture.len() != 1
        || switch.len() != 1
        || manager_exec.len() != 1
        || capture[0] >= switch[0]
        || switch[0] >= manager_exec[0]
    {
        return Err(
            "aarch64 exec must capture TTBR0 immediately before its fallible kernel-root transition"
                .to_string(),
        );
    }

    let err_offset = code_offsets(exec, &exec_mask, "Err(e) =>")
        .into_iter()
        .next()
        .ok_or_else(|| "aarch64 exec result lacks an Err arm".to_string())?;
    let err_arm = braced_block(exec, &exec_mask, err_offset)
        .ok_or_else(|| "aarch64 exec Err arm is not brace balanced".to_string())?;
    let err_mask = code_mask(err_arm);
    let rollback = call_offsets(err_arm, &err_mask, "restore_ttbr0_after_failed_exec");
    let returns = identifier_offsets(err_arm, &err_mask, "return");
    if rollback.len() != 1
        || returns.is_empty()
        || rollback[0] >= returns[0]
        || !normalized_code(err_arm)
            .contains("restore_ttbr0_after_failed_exec(previous_ttbr0);")
    {
        return Err("aarch64 failed exec can return with the kernel TTBR0 installed".to_string());
    }

    let restore = function_body(source, "restore_ttbr0_after_failed_exec")
        .ok_or_else(|| "missing failed-exec TTBR0 rollback helper".to_string())?;
    let restore_mask = code_mask(restore);
    if restore.matches("\"msr ttbr0_el1, {}\"").count() != 1
        || restore.matches("\"tlbi vmalle1is\"").count() != 1
        || call_offsets(restore, &restore_mask, "set_saved_process_cr3").len() != 1
        || call_offsets(restore, &restore_mask, "set_next_cr3").len() != 1
        || !normalized_code(restore).contains("set_next_cr3(0);")
    {
        return Err("failed-exec TTBR0 rollback does not restore hardware and both return shadows"
            .to_string());
    }

    Ok(())
}

const AARCH64_CONTEXT_SWITCH: &str = "kernel/src/arch_impl/aarch64/context_switch.rs";
const INLINE_ASM_ANCHOR: &str = "global_asm aarch64_inline_schedule_switch";

#[rustfmt::skip]
const INLINE_ASM_X30_SAVES: &[(&str, &str, usize)] = &[
    (AARCH64_CONTEXT_SWITCH, INLINE_ASM_ANCHOR, 1),
];
#[rustfmt::skip]
const INLINE_ELR_OFFSET_ASSERTS: &[(&str, &str, usize)] = &[
    (AARCH64_CONTEXT_SWITCH, "", 1),
];
#[rustfmt::skip]
const INLINE_RESUME_SELECTOR_SITES: &[(&str, &str, usize)] = &[
    (AARCH64_CONTEXT_SWITCH, "fn restore_kernel_context_inline", 1),
];
#[rustfmt::skip]
const CTX596_ORACLE_FAIL_SITES: &[(&str, &str, usize)] = &[
    (AARCH64_CONTEXT_SWITCH, "fn check_inline_save_resume_point", 1),
    (AARCH64_CONTEXT_SWITCH, "fn check_inline_eret_resume_pc", 1),
];
#[rustfmt::skip]
const INLINE_ELR_DIVERGENCE_COUNTER_SITES: &[(&str, &str, usize)] = &[
    (AARCH64_CONTEXT_SWITCH, "", 1),
    (AARCH64_CONTEXT_SWITCH, "fn record_inline_elr_divergence", 1),
];
#[rustfmt::skip]
const INLINE_ELR_DIVERGENCE_MARKER_SITES: &[(&str, &str, usize)] = &[
    (AARCH64_CONTEXT_SWITCH, "fn record_inline_elr_divergence", 1),
];
#[rustfmt::skip]
const FORCE_ERET_DISPATCH_DEFINITIONS: &[(&str, &str, usize)] = &[
    (AARCH64_CONTEXT_SWITCH, "#[cfg(feature=force_eret_dispatch_596)] fn inline_ret_dispatch_info_if_ready", 1),
    (AARCH64_CONTEXT_SWITCH, "#[cfg(not(feature=force_eret_dispatch_596))] fn inline_ret_dispatch_info_if_ready", 1),
];
fn context_switch_sources(source: &str) -> Vec<(String, String)> {
    vec![(AARCH64_CONTEXT_SWITCH.to_owned(), source.to_owned())]
}

fn census_error(actual: &Census, expected: &[(&str, &str, usize)]) -> Result<(), String> {
    validate_census(actual, expected).map_err(|diff| diff.join("\n"))
}

fn literal_offsets(source: &str, literal: &str) -> Vec<usize> {
    source
        .match_indices(literal)
        .map(|(offset, _)| offset)
        .collect()
}

fn statement_bounds(source: &str, mask: &[bool], offset: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let start = (0..offset)
        .rev()
        .find(|index| mask[*index] && matches!(bytes[*index], b';' | b'{' | b'}'))
        .map_or(0, |delimiter| delimiter + 1);
    let end = (offset..bytes.len()).find(|index| mask[*index] && bytes[*index] == b';')?;
    Some((start, end + 1))
}

fn compact_statement_at(source: &str, mask: &[bool], offset: usize) -> Option<String> {
    let (start, end) = statement_bounds(source, mask, offset)?;
    Some(normalized_code(&source[start..end]).replace(' ', ""))
}

fn resume_pc_frame_assignment_offsets(source: &str, mask: &[bool]) -> Vec<usize> {
    assigned_value_offsets(source, mask, "resume_pc")
        .into_iter()
        .filter(|offset| {
            compact_statement_at(source, mask, *offset)
                .is_some_and(|statement| statement.contains("frame.elr=resume_pc"))
        })
        .collect()
}

fn global_asm_body_for_symbol<'a>(source: &'a str, symbol: &str) -> Option<&'a str> {
    let label = source.find(&format!("{symbol}:"))?;
    let invocation = source[..label].rfind("core::arch::global_asm!(")?;
    let raw_open = invocation + source[invocation..label].find("r#\"")? + "r#\"".len();
    let raw_close = source[label..]
        .match_indices("\"#")
        .find_map(|(relative_close, _)| {
            let bytes = source.as_bytes();
            let mut cursor = label + relative_close + "\"#".len();
            while bytes
                .get(cursor)
                .is_some_and(|byte| byte.is_ascii_whitespace())
            {
                cursor += 1;
            }
            let mut closing_parens = 0usize;
            while bytes.get(cursor) == Some(&b')') {
                closing_parens += 1;
                cursor += 1;
                while bytes
                    .get(cursor)
                    .is_some_and(|byte| byte.is_ascii_whitespace())
                {
                    cursor += 1;
                }
            }
            (closing_parens >= 1 && bytes.get(cursor) == Some(&b';'))
                .then_some(label + relative_close)
        })?;
    Some(&source[raw_open..raw_close])
}

const RESUME_PC_GUARD_INCLUDE: &str =
    "kernel/src/arch_impl/aarch64/resume_pc_guard.inc";
const RESUME_PC_ASSEMBLY_ROOT: &str = "kernel/src/arch_impl/aarch64";
const SYSCALL_ENTRY_ASM: &str = "kernel/src/arch_impl/aarch64/syscall_entry.S";
const BOOT_ASM: &str = "kernel/src/arch_impl/aarch64/boot.S";

#[rustfmt::skip]
const RESUME_PC_EL1_INVOCATIONS: &[(&str, &str, usize)] = &[
    (SYSCALL_ENTRY_ASM, "assembly RESUME_PC_EL1_OK", 1),
    (BOOT_ASM, "assembly RESUME_PC_EL1_OK", 2),
    (AARCH64_CONTEXT_SWITCH, "assembly RESUME_PC_EL1_OK", 2),
];

#[rustfmt::skip]
const RESUME_PC_EL0_INVOCATIONS: &[(&str, &str, usize)] = &[
    (SYSCALL_ENTRY_ASM, "assembly RESUME_PC_EL0_OK", 1),
    (BOOT_ASM, "assembly RESUME_PC_EL0_OK", 2),
    (AARCH64_CONTEXT_SWITCH, "assembly RESUME_PC_EL0_OK", 1),
];

#[rustfmt::skip]
const USER_FRAME_ELR_PRODUCERS: &[(&str, &str, usize)] = &[
    (AARCH64_CONTEXT_SWITCH, "fn restore_userspace_context_inline", 1),
    (AARCH64_CONTEXT_SWITCH, "fn setup_first_entry_inline", 1),
];

#[derive(Clone, Debug)]
struct AssemblyMacroInvocation {
    offset: usize,
    arguments: Vec<String>,
}

fn assembly_line_code(line: &str) -> &str {
    line.split_once("//").map_or(line, |(code, _)| code).trim()
}

fn assembly_macro_invocations(source: &str, name: &str) -> Vec<AssemblyMacroInvocation> {
    let mut invocations = Vec::new();
    let mut line_offset = 0usize;
    for line in source.split_inclusive('\n') {
        let code = assembly_line_code(line);
        let first = code.split_whitespace().next();
        if first == Some(name) {
            let arguments = code[name.len()..]
                .split(',')
                .map(str::trim)
                .filter(|argument| !argument.is_empty())
                .map(str::to_owned)
                .collect();
            let name_in_line = line.find(name).expect("assembly macro token in source line");
            invocations.push(AssemblyMacroInvocation {
                offset: line_offset + name_in_line,
                arguments,
            });
        }
        line_offset += line.len();
    }
    invocations
}

fn assembly_macro_census(sources: &[(String, String)], name: &str) -> Census {
    let mut result = Census::new();
    for (path, source) in sources {
        let count = assembly_macro_invocations(source, name).len();
        if count != 0 {
            result.insert((path.clone(), format!("assembly {name}")), count);
        }
    }
    result
}

fn validate_resume_pc_macro_definitions(source: &str) -> Result<(), String> {
    for name in [
        "RESUME_PC_EL1_OK",
        "RESUME_PC_EL0_OK",
        "RESUME_PC_RECORD",
        "RESUME_PC_RECORD_NOFRAME",
    ] {
        let definitions = source
            .lines()
            .map(assembly_line_code)
            .filter(|line| {
                let mut fields = line.split_whitespace();
                fields.next() == Some(".macro")
                    && fields.next().is_some_and(|field| field.trim_end_matches(',') == name)
            })
            .count();
        if definitions != 1 {
            return Err(format!(
                "{RESUME_PC_GUARD_INCLUDE} must define .macro {name} exactly once, found {definitions}"
            ));
        }
    }
    Ok(())
}

fn assembly_macro_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let mut line_offset = 0usize;
    let mut body_start = None;
    for line in source.split_inclusive('\n') {
        let code = assembly_line_code(line);
        if body_start.is_none() {
            let mut fields = code.split_whitespace();
            if fields.next() == Some(".macro")
                && fields
                    .next()
                    .is_some_and(|field| field.trim_end_matches(',') == name)
            {
                body_start = Some(line_offset + line.len());
            }
        } else if code == ".endm" {
            return body_start.map(|start| &source[start..line_offset]);
        }
        line_offset += line.len();
    }
    None
}

fn validate_resume_pc_macro_census(
    sources: &[(String, String)],
    name: &str,
    expected: &[(&str, &str, usize)],
) -> Result<(), String> {
    census_error(&assembly_macro_census(sources, name), expected)
}

fn assembly_line_is_label_or_directive(line: &str) -> bool {
    line.is_empty()
        || line.ends_with(':')
        || line.starts_with('.')
        || line.starts_with("/*")
        || line.starts_with('*')
        || line.starts_with("*/")
}

fn validate_no_private_resume_pc_admission(
    sources: &[(String, String)],
) -> Result<(), String> {
    for (path, source) in sources {
        let invocations = assembly_macro_invocations(source, "RESUME_PC_EL1_OK");
        if invocations.is_empty() {
            continue;
        }
        let resume_pcs: BTreeSet<_> = invocations
            .iter()
            .filter_map(|invocation| invocation.arguments.first().cloned())
            .collect();
        let admission_targets: BTreeSet<_> = invocations
            .iter()
            .filter_map(|invocation| invocation.arguments.last().cloned())
            .collect();

        let mut in_macro = false;
        let mut lines = Vec::new();
        for source_line in source.lines() {
            let line = assembly_line_code(source_line);
            if line.starts_with(".macro ") {
                in_macro = true;
            }
            lines.push((line, in_macro));
            if line == ".endm" {
                in_macro = false;
            }
        }

        for (index, (line, line_in_macro)) in lines.iter().enumerate() {
            if *line_in_macro {
                continue;
            }
            let mut fields = line.split_whitespace();
            if fields.next() != Some("cmp") {
                continue;
            }
            let compared_pc = fields
                .next()
                .map(|operand| operand.trim_end_matches(','));
            if !compared_pc.is_some_and(|pc| resume_pcs.contains(pc)) {
                continue;
            }

            let next_instruction = lines[index + 1..]
                .iter()
                .find(|(candidate, candidate_in_macro)| {
                    !*candidate_in_macro && !assembly_line_is_label_or_directive(candidate)
                })
                .map(|(candidate, _)| *candidate);
            let Some(next_instruction) = next_instruction else {
                continue;
            };
            let mut branch_fields = next_instruction.split_whitespace();
            let branch = branch_fields.next();
            let target = branch_fields
                .next()
                .map(|operand| operand.trim_end_matches(','));
            if matches!(branch, Some("b.hs" | "b.lo"))
                && target.is_some_and(|target| admission_targets.contains(target))
            {
                return Err(format!(
                    "{path} privately admits a resume PC with `{line}` followed by `{next_instruction}`"
                ));
            }
        }
    }
    Ok(())
}

fn direct_user_frame_elr_assignment_offsets(source: &str, mask: &[bool]) -> Vec<usize> {
    assigned_value_offsets(source, mask, "thread.context.elr_el1")
        .into_iter()
        .filter(|offset| {
            compact_statement_at(source, mask, *offset)
                .is_some_and(|statement| statement == "frame.elr=thread.context.elr_el1;")
        })
        .collect()
}

fn validate_user_resume_pc_producers(source: &str) -> Result<(), String> {
    let sources = context_switch_sources(source);
    census_error(
        &census(&sources, direct_user_frame_elr_assignment_offsets),
        USER_FRAME_ELR_PRODUCERS,
    )?;

    for producer in [
        "restore_userspace_context_inline",
        "setup_first_entry_inline",
    ] {
        let body = function_body(source, producer)
            .ok_or_else(|| format!("missing fn {producer}"))?;
        let mask = code_mask(body);
        let guards = call_offsets(body, &mask, "resume_pc_is_user_dispatchable");
        if guards.len() != 1 {
            return Err(format!(
                "{producer} must call resume_pc_is_user_dispatchable exactly once, found {}",
                guards.len()
            ));
        }
        let guard = guards[0];
        let guarded_return = identifier_offsets(body, &mask, "if")
            .into_iter()
            .find_map(|if_offset| {
                let open = (if_offset..body.len())
                    .find(|offset| mask[*offset] && body.as_bytes()[*offset] == b'{')?;
                if !(if_offset < guard && guard < open) {
                    return None;
                }
                let condition = normalized_code(&body[if_offset + "if".len()..open])
                    .replace(' ', "");
                if !condition.starts_with("!resume_pc_is_user_dispatchable(") {
                    return None;
                }
                let block = braced_block(body, &mask, open)?;
                normalized_code(block)
                    .contains("return false;")
                    .then_some(())
            })
            .is_some();
        if !guarded_return {
            return Err(format!(
                "{producer} resume-PC predicate must guard an early return false"
            ));
        }
        let assignments = direct_user_frame_elr_assignment_offsets(body, &mask);
        if assignments.len() != 1 || assignments[0] <= guard {
            return Err(format!(
                "{producer} must assign frame.elr from thread.context.elr_el1 once after its guard"
            ));
        }
    }
    Ok(())
}

fn call_expression_close(source: &str, mask: &[bool], call: usize, name: &str) -> Option<usize> {
    let open = next_code(source, mask, call + name.len())?;
    if source.as_bytes()[open] != b'(' {
        return None;
    }
    let mut depth = 0usize;
    for index in open..source.len() {
        if !mask[index] {
            continue;
        }
        match source.as_bytes()[index] {
            b'(' => depth += 1,
            b')' => {
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

fn validate_user_resume_pc_verdict_consumers(source: &str) -> Result<(), String> {
    let mask = code_mask(source);
    for producer in [
        "restore_userspace_context_inline",
        "setup_first_entry_inline",
    ] {
        let definitions = definition_offsets(source, &mask, producer);
        let [(definition, open)] = definitions.as_slice() else {
            return Err(format!(
                "{producer} must have exactly one definition, found {}",
                definitions.len()
            ));
        };
        let signature = normalized_code(&source[*definition..*open]).replace(' ', "");
        if !signature.contains(")->bool") {
            return Err(format!("{producer} must return bool"));
        }

        let calls: Vec<_> = call_offsets(source, &mask, producer)
            .into_iter()
            .filter(|offset| !preceded_by_keyword(source, &mask, *offset, "fn"))
            .collect();
        if calls.is_empty() {
            return Err(format!("{producer} must have at least one call site"));
        }
        for call in calls {
            let close = call_expression_close(source, &mask, call, producer)
                .ok_or_else(|| format!("unbalanced {producer} call"))?;
            if next_code(source, &mask, close + 1)
                .is_some_and(|next| source.as_bytes()[next] == b';')
            {
                return Err(format!("{producer} verdict is discarded as a statement"));
            }
        }
    }
    Ok(())
}

fn validate_user_resume_pc_predicate_is_independent(source: &str) -> Result<(), String> {
    let body = function_body(source, "resume_pc_is_user_dispatchable")
        .ok_or_else(|| "missing fn resume_pc_is_user_dispatchable".to_string())?;
    let mask = code_mask(body);
    for forbidden in [
        "resume_pc_is_dispatchable",
        "__kernel_text_start",
        "__kernel_text_end",
    ] {
        if !identifier_offsets(body, &mask, forbidden).is_empty() {
            return Err(format!(
                "resume_pc_is_user_dispatchable must not reference {forbidden}"
            ));
        }
    }
    Ok(())
}

fn anchored_count(path: &str, item: &str, count: usize) -> Census {
    let mut result = Census::new();
    if count != 0 {
        result.insert((path.to_owned(), item.to_owned()), count);
    }
    result
}

fn validate_inline_asm_resume_store(source: &str) -> Result<(), String> {
    let asm = global_asm_body_for_symbol(source, "aarch64_inline_schedule_switch")
        .ok_or_else(|| "missing aarch64_inline_schedule_switch global_asm block".to_string())?;
    let x30_saves = asm.matches("stp x29, x30, [x0,").count();
    let resume_stores = asm.matches("str x30, [x0, #264]").count();
    census_error(
        &anchored_count(AARCH64_CONTEXT_SWITCH, INLINE_ASM_ANCHOR, x30_saves),
        INLINE_ASM_X30_SAVES,
    )?;
    if resume_stores < x30_saves {
        return Err(format!(
            "{AARCH64_CONTEXT_SWITCH} :: {INLINE_ASM_ANCHOR} stores x30 in {x30_saves} callee-saved blocks but publishes only {resume_stores} resume PCs"
        ));
    }
    Ok(())
}

fn offset_assert_offsets(source: &str, mask: &[bool]) -> Vec<usize> {
    identifier_offsets(source, mask, "elr_el1")
        .into_iter()
        .filter(|offset| {
            compact_statement_at(source, mask, *offset).is_some_and(|statement| {
                statement
                    == "const_:()=assert!(core::mem::offset_of!(CpuContext,elr_el1)==264);"
            })
        })
        .collect()
}

fn validate_inline_elr_offset_assert(source: &str) -> Result<(), String> {
    let sources = context_switch_sources(source);
    census_error(
        &census(&sources, offset_assert_offsets),
        INLINE_ELR_OFFSET_ASSERTS,
    )
}

fn resume_selector_span(source: &str, mask: &[bool]) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    identifier_offsets(source, mask, "resume_pc")
        .into_iter()
        .filter(|offset| preceded_by_keyword(source, mask, *offset, "let"))
        .find_map(|resume_pc| {
            let let_end = previous_code(source, mask, resume_pc)?;
            let start = let_end + 1 - "let".len();
            let end = (resume_pc..bytes.len())
                .find(|index| mask[*index] && bytes[*index] == b';')?
                + 1;
            normalized_code(&source[start..end])
                .replace(' ', "")
                .contains("ifthread.saved_by_inline_schedule")
                .then_some((start, end))
        })
}

fn resume_selector_offsets(source: &str, mask: &[bool], identifier: &str) -> Vec<usize> {
    const SELECTOR: &str = "letresume_pc=ifthread.saved_by_inline_schedule{thread.context.x30}else{thread.context.elr_el1};";
    let Some((start, end)) = resume_selector_span(source, mask) else {
        return Vec::new();
    };
    if normalized_code(&source[start..end]).replace(' ', "") != SELECTOR {
        return Vec::new();
    }
    identifier_offsets(&source[start..end], &mask[start..end], identifier)
        .into_iter()
        .map(|offset| start + offset)
        .collect()
}

fn validate_inline_eret_resume_selector(source: &str) -> Result<(), String> {
    let sources = context_switch_sources(source);
    for identifier in ["saved_by_inline_schedule", "x30", "elr_el1"] {
        census_error(
            &census(&sources, |source, mask| {
                resume_selector_offsets(source, mask, identifier)
            }),
            INLINE_RESUME_SELECTOR_SITES,
        )
        .map_err(|error| format!("resume selector {identifier}: {error}"))?;
    }
    census_error(
        &census(&sources, resume_pc_frame_assignment_offsets),
        INLINE_RESUME_SELECTOR_SITES,
    )
    .map_err(|error| format!("frame ELR assignment: {error}"))?;

    let mut direct_elr_resumes = census(&sources, |source, mask| {
        identifier_offsets(source, mask, "elr_el1")
            .into_iter()
            .filter(|offset| {
                compact_statement_at(source, mask, *offset).is_some_and(|statement| {
                    statement.contains("frame.elr=thread.context.elr_el1")
                })
            })
            .collect()
    });
    direct_elr_resumes.retain(|(_, item), _| item == "fn restore_kernel_context_inline");
    if !direct_elr_resumes.is_empty() {
        return Err(format!(
            "direct inline-ambiguous ERET resume sites remain: {direct_elr_resumes:?}"
        ));
    }
    Ok(())
}

fn definition_census(source: &str, name: &str) -> Census {
    let sources = context_switch_sources(source);
    census(&sources, |source, mask| {
        definition_offsets(source, mask, name)
            .into_iter()
            .map(|(_, brace)| brace)
            .collect()
    })
}

fn definition_is_cfg_gated(source: &str, identifier: &str) -> bool {
    let mask = code_mask(source);
    identifier_offsets(source, &mask, identifier)
        .into_iter()
        .filter(|offset| {
            rendered_item_spans(&item_spans(source, &mask))
                .iter()
                .all(|(open, close, _)| *offset < *open || *offset > *close)
        })
        .any(|offset| {
            statement_bounds(source, &mask, offset).is_some_and(|(start, _)| {
                !code_offsets(&source[start..offset], &mask[start..offset], "#[cfg").is_empty()
            })
        })
}

fn validate_ctx596_oracle_liveness(source: &str) -> Result<(), String> {
    let sources = context_switch_sources(source);
    census_error(
        &census(&sources, |source, _| {
            literal_offsets(source, "CTX596_ORACLE:FAIL")
        }),
        CTX596_ORACLE_FAIL_SITES,
    )?;
    census_error(
        &census(&sources, |source, mask| {
            identifier_offsets(source, mask, "INLINE_ELR_DIVERGENCE")
        }),
        INLINE_ELR_DIVERGENCE_COUNTER_SITES,
    )?;
    census_error(
        &census(&sources, |source, _| {
            literal_offsets(source, "[CTX596_ELR_DIVERGENCE]")
        }),
        INLINE_ELR_DIVERGENCE_MARKER_SITES,
    )?;
    if definition_is_cfg_gated(source, "INLINE_ELR_DIVERGENCE") {
        return Err("INLINE_ELR_DIVERGENCE definition is feature-gated".to_string());
    }
    census_error(
        &definition_census(source, "inline_ret_dispatch_info_if_ready"),
        FORCE_ERET_DISPATCH_DEFINITIONS,
    )
}

fn trampoline_repair_offsets(source: &str, mask: &[bool]) -> Vec<usize> {
    identifier_offsets(source, mask, "elr_el1")
        .into_iter()
        .filter(|offset| {
            compact_statement_at(source, mask, *offset).is_some_and(|statement| {
                statement == "old_thread.context.elr_el1=old_thread.context.x30;"
            })
        })
        .collect()
}

fn brace_depth_before(source: &str, mask: &[bool], offset: usize) -> usize {
    source.as_bytes()[..offset]
        .iter()
        .zip(&mask[..offset])
        .fold(0usize, |depth, (byte, code)| {
            if !code {
                depth
            } else {
                match byte {
                    b'{' => depth + 1,
                    b'}' => depth.saturating_sub(1),
                    _ => depth,
                }
            }
        })
}

/// The single identifier passed as the first argument of the call at `offset`,
/// or `None` when the argument is not a bare identifier (possibly with a cast).
fn call_argument_identifier<'a>(source: &'a str, mask: &[bool], offset: usize) -> Option<&'a str> {
    let bytes = source.as_bytes();
    let open = (offset..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'(')?;
    let mut cursor = open + 1;
    while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
        cursor += 1;
    }
    let start = cursor;
    while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
        cursor += 1;
    }
    (cursor > start).then(|| &source[start..cursor])
}

/// The identifier bound by the first `let <ident> = ... <call>(...)` in `source`.
fn binding_from_call<'a>(source: &'a str, mask: &[bool], call: &str) -> Option<&'a str> {
    let bytes = source.as_bytes();
    for offset in identifier_offsets(source, mask, "let") {
        let mut cursor = offset + "let".len();
        while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == start {
            continue;
        }
        let name = &source[start..cursor];
        let statement_end = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b';')?;
        let initializer = &source[cursor..statement_end];
        let initializer_mask = code_mask(initializer);
        if !call_offsets(initializer, &initializer_mask, call).is_empty() {
            return Some(name);
        }
    }
    None
}

fn braced_block_bounds(source: &str, mask: &[bool], start: usize) -> Option<(usize, usize)> {
    let open =
        (start..source.len()).find(|index| mask[*index] && source.as_bytes()[*index] == b'{')?;
    let block = braced_block(source, mask, start)?;
    Some((open, start + block.len() - 1))
}

fn identifier_starts_at(source: &str, mask: &[bool], offset: usize, identifier: &str) -> bool {
    identifier_offsets(source, mask, identifier)
        .into_iter()
        .any(|candidate| candidate == offset)
}

fn validate_inline_trampoline_repair(source: &str) -> Result<(), String> {
    let trampoline = function_body(source, "inline_schedule_trampoline")
        .ok_or_else(|| "missing fn inline_schedule_trampoline".to_string())?;
    let mask = code_mask(trampoline);
    let bytes = trampoline.as_bytes();

    let fallback_guards: Vec<(usize, usize)> = identifier_offsets(trampoline, &mask, "if")
        .into_iter()
        .filter(|if_offset| {
            brace_depth_before(trampoline, &mask, *if_offset) == 1
                && previous_code(trampoline, &mask, *if_offset)
                    .is_some_and(|before| matches!(bytes[before], b'{' | b'}' | b';'))
        })
        .filter_map(|if_offset| {
            let (open, close) = braced_block_bounds(trampoline, &mask, if_offset)?;
            (normalized_predicate(&trampoline[if_offset + "if".len()..open])
                == "sched_ptr.is_null()")
                .then_some((open, close))
        })
        .collect();
    let &[(fallback_open, fallback_close)] = fallback_guards.as_slice() else {
        return Err(format!(
            "inline_schedule_trampoline must have one top-level scheduler_ptr-null fallback guard, found {}",
            fallback_guards.len()
        ));
    };

    let mut arms = vec![(
        "scheduler_ptr-null fallback".to_string(),
        &trampoline[fallback_open + 1..fallback_close],
    )];
    let mut cursor = fallback_close + 1;
    let mut branch_number = 2usize;
    let mut has_final_else = false;
    loop {
        let Some(else_offset) = next_code(trampoline, &mask, cursor) else {
            break;
        };
        if !identifier_starts_at(trampoline, &mask, else_offset, "else") {
            break;
        }
        let after_else = next_code(trampoline, &mask, else_offset + "else".len())
            .ok_or_else(|| "unterminated scheduler_ptr-null fallback else".to_string())?;
        if identifier_starts_at(trampoline, &mask, after_else, "if") {
            let (open, close) = braced_block_bounds(trampoline, &mask, after_else)
                .ok_or_else(|| "scheduler_ptr-null fallback else-if has no body".to_string())?;
            arms.push((
                format!("scheduler_ptr fallback branch {branch_number}"),
                &trampoline[open + 1..close],
            ));
            branch_number += 1;
            cursor = close + 1;
            continue;
        }
        if bytes[after_else] != b'{' {
            return Err("scheduler_ptr-null fallback else has no body".to_string());
        }
        let (_, close) = braced_block_bounds(trampoline, &mask, after_else)
            .ok_or_else(|| "scheduler_ptr-null fallback else has no body".to_string())?;
        arms.push((
            "scheduler_ptr non-null else".to_string(),
            &trampoline[after_else + 1..close],
        ));
        cursor = close + 1;
        has_final_else = true;
        break;
    }
    if !has_final_else {
        arms.push((
            "scheduler_ptr non-null fallthrough".to_string(),
            &trampoline[cursor..trampoline.len() - 1],
        ));
    }

    for (label, arm) in arms {
        let count = trampoline_repair_offsets(arm, &code_mask(arm)).len();
        if count != 1 {
            return Err(format!(
                "inline_schedule_trampoline {label} arm must carry exactly one old-thread ELR normalization, found {count}"
            ));
        }
    }
    Ok(())
}

#[test]
fn clone_publication_lifecycle_is_closed() {
    let clone = repo_text("kernel/src/syscall/clone.rs");
    assert_eq!(validate_clone_publication_lifecycle(&clone), Ok(()));

    let refusal = clone
        .find("if refuses_init_group_clone(manager, parent_tg_id) {")
        .expect("init-group refusal arm");
    let return_offset = clone[refusal..]
        .find("return SyscallResult::Err(super::errno::EINVAL as u64);")
        .map(|offset| refusal + offset)
        .expect("init-group refusal return");
    let mut nonterminal_refusal = clone.clone();
    nonterminal_refusal.replace_range(
        return_offset..return_offset + "return".len(),
        "let _ignored =",
    );
    assert!(validate_clone_publication_lifecycle(&nonterminal_refusal).is_err());
}

#[test]
fn unpublished_dispatch_is_retry_only_on_both_architectures() {
    let x86 = repo_text("kernel/src/interrupts/context_switch.rs");
    let x86_refusal = function_body(&x86, "refuse_unpublished_dispatch")
        .expect("missing x86 unpublished-row refusal predicate");
    assert!(identifier_offsets(x86_refusal, &code_mask(x86_refusal), "set_terminated").is_empty());
    assert_eq!(
        validate_aarch64_row_unpublished_dispatch(&repo_text(
            "kernel/src/arch_impl/aarch64/context_switch.rs"
        )),
        Ok(())
    );
}

#[test]
fn aarch64_failed_exec_restores_the_pretransition_ttbr0() {
    let source = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    assert_eq!(validate_aarch64_failed_exec_ttbr0_rollback(&source), Ok(()));
}

#[test]
fn aarch64_failed_exec_ttbr0_validator_rejects_missing_rollback() {
    let source = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let mutant = source.replacen(
        "restore_ttbr0_after_failed_exec(previous_ttbr0);",
        "",
        1,
    );
    assert!(validate_aarch64_failed_exec_ttbr0_rollback(&mutant).is_err());
}

#[test]
fn aarch64_inline_save_blocks_publish_their_resume_pc() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    assert_eq!(validate_inline_asm_resume_store(&source), Ok(()));

    let mutant = source.replacen("    str x30, [x0, #264]\n", "", 1);
    assert_ne!(mutant, source, "inline ELR store mutation anchor");
    assert!(validate_inline_asm_resume_store(&mutant).is_err());
}

#[test]
fn shared_resume_pc_macros_each_have_one_definition() {
    let source = repo_text(RESUME_PC_GUARD_INCLUDE);
    assert_eq!(validate_resume_pc_macro_definitions(&source), Ok(()));

    for name in [
        "RESUME_PC_EL1_OK",
        "RESUME_PC_EL0_OK",
        "RESUME_PC_RECORD",
        "RESUME_PC_RECORD_NOFRAME",
    ] {
        let mutant = format!("{source}\n.macro {name}\n.endm\n");
        assert!(
            validate_resume_pc_macro_definitions(&mutant).is_err(),
            "a second .macro {name} definition must fail the definition census"
        );
    }
}

#[test]
fn aarch64_ret_dispatch_refusal_leaves_the_refused_stack_before_custody_stops_naming_it() {
    let source = repo_text(RESUME_PC_GUARD_INCLUDE);
    let noframe = assembly_macro_body(&source, "RESUME_PC_RECORD_NOFRAME")
        .expect("missing RESUME_PC_RECORD_NOFRAME macro body");
    let frame = assembly_macro_body(&source, "RESUME_PC_RECORD")
        .expect("missing RESUME_PC_RECORD macro body");
    let failure = "the ret-dispatch refusal arm must leave the refused thread's kernel stack before the per-CPU words stop naming it";

    let instruction_lines = |body: &str| {
        let mut offset = 0usize;
        body.split_inclusive('\n')
            .filter_map(|line| {
                let line_offset = offset;
                offset += line.len();
                let code = assembly_line_code(line);
                (!assembly_line_is_label_or_directive(code))
                    .then_some((line_offset, code.to_owned()))
            })
            .collect::<Vec<_>>()
    };
    let mov_sp_offsets = |body: &str| {
        instruction_lines(body)
            .into_iter()
            .filter_map(|(offset, code)| {
                let mut fields = code.splitn(2, char::is_whitespace);
                let mnemonic = fields.next()?;
                let destination = fields
                    .next()
                    .and_then(|operands| operands.split(',').next())
                    .map(str::trim);
                (mnemonic == "mov" && destination == Some("sp")).then_some(offset)
            })
            .collect::<Vec<_>>()
    };
    let store_offsets = |body: &str, displacement: &str| {
        instruction_lines(body)
            .into_iter()
            .filter_map(|(offset, code)| {
                let mut fields = code.splitn(2, char::is_whitespace);
                let mnemonic = fields.next()?;
                let operands = fields.next()?.replace(' ', "");
                let destination = operands.split_once(',')?.1;
                let inner = destination.strip_prefix('[')?.strip_suffix(']')?;
                let (base, actual_displacement) = inner.rsplit_once(',')?;
                (mnemonic.starts_with("st")
                    && !base.is_empty()
                    && actual_displacement == displacement)
                    .then_some(offset)
            })
            .collect::<Vec<_>>()
    };

    let noframe_mov_sp = mov_sp_offsets(noframe);
    let noframe_stores_16 = store_offsets(noframe, "#16");
    let noframe_stores_40 = store_offsets(noframe, "#40");
    assert_eq!(noframe_mov_sp.len(), 1, "{failure}: expected exactly one mov to sp");
    assert!(
        !noframe_stores_16.is_empty() && !noframe_stores_40.is_empty(),
        "{failure}: expected stores to both #16 and #40 displacements"
    );
    let pivot = noframe_mov_sp[0];
    assert!(
        noframe_stores_16
            .iter()
            .chain(&noframe_stores_40)
            .all(|store| pivot < *store),
        "{failure}: mov to sp must precede every #16/#40 store"
    );

    assert!(
        mov_sp_offsets(frame).is_empty(),
        "RESUME_PC_RECORD must not move to sp because the ERET epilogue selects SP"
    );
}

/// Locate the `if` statements in `body` whose condition text satisfies
/// `condition_ok`, returning `(condition_text, block_start, block_end)` for each.
/// `block_start` is the offset of the `{` opening the consequent and `block_end`
/// the offset of its matching `}`.
fn guarded_if_blocks<'a>(
    body: &'a str,
    mask: &[bool],
    condition_ok: impl Fn(&str) -> bool,
) -> Vec<(&'a str, usize, usize)> {
    let mut blocks = Vec::new();
    for offset in identifier_offsets(body, mask, "if") {
        let Some((open, close)) = braced_block_bounds(body, mask, offset) else {
            continue;
        };
        let condition = &body[offset + "if".len()..open];
        if condition_ok(condition) {
            blocks.push((condition, open, close));
        }
    }
    blocks
}

#[test]
fn aarch64_refusal_drain_acts_only_on_records_this_cpu_published() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    let body = function_body(&source, "drain_asm_resume_pc_refusals")
        .expect("missing drain_asm_resume_pc_refusals body");
    let mask = code_mask(body);
    let failure = "the drain may only act on records the draining CPU published";

    // Both operands of the guard are DERIVED, not named by this test: the record
    // CPU is whatever identifier indexes the per-CPU record read, and the drain
    // CPU is whatever identifier is bound from this CPU's own id.
    let record_read = call_offsets(body, &mask, "eret_guard_record_full");
    assert_eq!(
        record_read.len(),
        1,
        "{failure}: expected exactly one per-CPU record read"
    );
    let record_cpu_ident = call_argument_identifier(body, &mask, record_read[0])
        .expect("the per-CPU record read must be indexed by an identifier");
    let drain_cpu_ident = binding_from_call(body, &mask, "cpu_id")
        .expect("the drain must bind this CPU's id to an identifier");
    assert_ne!(
        record_cpu_ident, drain_cpu_ident,
        "{failure}: the record CPU and the drain CPU must be distinct identifiers"
    );

    let claim_calls = call_offsets(body, &mask, "eret_guard_claim_source");
    assert_eq!(
        claim_calls.len(),
        1,
        "{failure}: expected exactly one record claim"
    );
    let claim = claim_calls[0];

    // THE SHAPE: an `if` comparing the two, whose consequent both reports the
    // foreign record and leaves the iteration, and which dominates the claim.
    let comparisons = guarded_if_blocks(body, &mask, |condition| {
        let condition_mask = code_mask(condition);
        let mentions = |identifier: &str| {
            !identifier_offsets(condition, &condition_mask, identifier).is_empty()
        };
        let compares = condition.contains("!=") || condition.contains("==");
        compares && mentions(record_cpu_ident) && mentions(drain_cpu_ident)
    });
    assert_eq!(
        comparisons.len(),
        1,
        "{failure}: expected exactly one `if` comparing {record_cpu_ident} against \
         {drain_cpu_ident}; found {}",
        comparisons.len()
    );
    let (_, foreign_open, foreign_close) = comparisons[0];

    let reporter_calls = call_offsets(body, &mask, "report_foreign_resume_pc_refusal");
    assert_eq!(
        reporter_calls.len(),
        1,
        "{failure}: the foreign reporter must have exactly one call site in the drain"
    );
    assert!(
        reporter_calls[0] > foreign_open && reporter_calls[0] < foreign_close,
        "{failure}: the foreign reporter must be reachable only from the CPU comparison"
    );
    let source_mask = code_mask(&source);
    let reporter_file_calls = call_offsets(
        &source,
        &source_mask,
        "report_foreign_resume_pc_refusal",
    )
    .into_iter()
    .filter(|offset| !preceded_by_keyword(&source, &source_mask, *offset, "fn"))
    .collect::<Vec<_>>();
    assert_eq!(
        reporter_file_calls.len(),
        1,
        "{failure}: the foreign reporter must have exactly one call site in the file"
    );
    let continues = identifier_offsets(body, &mask, "continue");
    assert!(
        continues
            .iter()
            .any(|offset| *offset > foreign_open && *offset < foreign_close),
        "{failure}: the foreign branch must leave the iteration"
    );
    assert!(
        claim > foreign_close,
        "{failure}: the CPU comparison must dominate the record claim"
    );

    let calls = |name| {
        let offsets = call_offsets(body, &mask, name);
        assert!(!offsets.is_empty(), "{failure}: missing {name} call");
        offsets
    };
    let record_calls = calls("record_resume_pc_refusal_locked");
    let terminate_calls = calls("set_terminated");
    let dequeue_calls = calls("remove_from_ready_queue");
    for (name, offsets) in [
        (
            "record_resume_pc_refusal_locked",
            record_calls.as_slice(),
        ),
        ("set_terminated", terminate_calls.as_slice()),
        ("remove_from_ready_queue", dequeue_calls.as_slice()),
    ] {
        assert!(
            offsets.iter().all(|offset| *offset > foreign_close),
            "{failure}: {name} must occur after the foreign early-out"
        );
        assert!(
            offsets.iter().all(|offset| *offset > claim),
            "{failure}: {name} must occur after the owner record is claimed"
        );
    }

    assert!(
        !body.contains("FOREIGN"),
        "{failure}: the acting drain body must not carry a FOREIGN verdict"
    );
    let foreign = function_body(&source, "report_foreign_resume_pc_refusal")
        .expect("missing report_foreign_resume_pc_refusal body");
    assert!(
        foreign.contains("FOREIGN_REPORT_ONLY"),
        "{failure}: the FOREIGN-tagged emission must live in the report-only helper"
    );
    let foreign_mask = code_mask(foreign);
    for name in [
        "set_terminated",
        "remove_from_ready_queue",
        "eret_guard_claim_source",
        "with_scheduler",
    ] {
        assert_eq!(
            call_offsets(foreign, &foreign_mask, name).len(),
            0,
            "{failure}: the foreign reporter must not call {name}"
        );
    }
}

#[test]
fn aarch64_refusal_drain_acts_only_after_it_has_departed_the_victim_stack() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    let body = function_body(&source, "drain_asm_resume_pc_refusals")
        .expect("missing drain_asm_resume_pc_refusals body");
    let mask = code_mask(body);
    let failure = "the drain may only act once it has left the victim's kernel stack";

    // DERIVED, not named: every identifier assigned from a kernel-stack
    // containment test, plus anything `let`-bound from one of those.
    let bytes = body.as_bytes();
    let mut departure: BTreeSet<&str> = BTreeSet::new();
    for call in call_offsets(body, &mask, "sp_within_kernel_stack") {
        let statement_start = (0..call)
            .rev()
            .find(|index| mask[*index] && matches!(bytes[*index], b';' | b'{' | b'}'))
            .map(|index| index + 1)
            .unwrap_or(0);
        let mut cursor = statement_start;
        while cursor < call && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
            cursor += 1;
        }
        if body[cursor..call].starts_with("let ") {
            cursor += "let ".len();
            while cursor < call && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
        }
        let start = cursor;
        while cursor < call && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        if cursor > start {
            departure.insert(&body[start..cursor]);
        }
    }
    assert!(
        !departure.is_empty(),
        "{failure}: the drain must test whether it stands on the victim's kernel stack"
    );
    // One level of alias: `let departed = !on_victim_stack;`.
    for _ in 0..2 {
        let mut discovered: Vec<&str> = Vec::new();
        for offset in identifier_offsets(body, &mask, "let") {
            let mut cursor = offset + "let".len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            let start = cursor;
            while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
                cursor += 1;
            }
            if cursor == start {
                continue;
            }
            let name = &body[start..cursor];
            let Some(end) = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b';')
            else {
                continue;
            };
            let initializer = &body[cursor..end];
            let initializer_mask = code_mask(initializer);
            if departure.iter().any(|identifier| {
                !identifier_offsets(initializer, &initializer_mask, identifier).is_empty()
            }) {
                discovered.push(name);
            }
        }
        for name in discovered {
            departure.insert(name);
        }
    }

    let guarded = guarded_if_blocks(body, &mask, |condition| {
        let condition_mask = code_mask(condition);
        departure.iter().any(|identifier| {
            !identifier_offsets(condition, &condition_mask, identifier).is_empty()
        })
    });
    assert!(
        !guarded.is_empty(),
        "{failure}: no branch in the drain is controlled by the departure test"
    );

    for name in [
        "set_terminated",
        "record_cpu_state_change",
        "set_current_thread_ptr",
    ] {
        let offsets = call_offsets(body, &mask, name);
        assert!(!offsets.is_empty(), "{failure}: missing {name} call");
        for offset in offsets {
            assert!(
                guarded
                    .iter()
                    .any(|(_, open, close)| offset > *open && offset < *close),
                "{failure}: {name} is not dominated by the departure test"
            );
        }
    }
}

#[test]
fn aarch64_resume_pc_records_publish_their_source_behind_a_store_barrier() {
    let source = repo_text(RESUME_PC_GUARD_INCLUDE);
    let failure = "the record payload has to be visible before the validity word";

    let instruction_lines = |body: &str| {
        let mut offset = 0usize;
        body.split_inclusive('\n')
            .filter_map(|line| {
                let line_offset = offset;
                offset += line.len();
                let code = assembly_line_code(line);
                (!assembly_line_is_label_or_directive(code))
                    .then_some((line_offset, code.to_owned()))
            })
            .collect::<Vec<_>>()
    };

    for name in ["RESUME_PC_RECORD", "RESUME_PC_RECORD_NOFRAME"] {
        let body = assembly_macro_body(&source, name)
            .unwrap_or_else(|| panic!("missing {name} macro body"));
        let instructions = instruction_lines(body);
        let stores = instructions
            .iter()
            .filter_map(|(offset, code)| {
                let mut fields = code.splitn(2, char::is_whitespace);
                let mnemonic = fields.next()?;
                let operands = fields.next()?.replace(' ', "");
                let destination = operands.split_once(',')?.1;
                let inner = destination.strip_prefix('[')?.strip_suffix(']')?;
                let (_base, displacement) = inner.rsplit_once(',')?;
                mnemonic
                    .starts_with("st")
                    .then_some((*offset, displacement.to_owned()))
            })
            .collect::<Vec<_>>();
        let source_stores = stores
            .iter()
            .filter_map(|(offset, displacement)| {
                (displacement == "#PERCPU_ERET_GUARD_SOURCE").then_some(*offset)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            source_stores.len(),
            1,
            "{failure}: {name} must have exactly one source store"
        );
        let barriers = instructions
            .iter()
            .filter_map(|(offset, code)| {
                code.split_whitespace()
                    .next()
                    .is_some_and(|mnemonic| mnemonic.starts_with("dmb"))
                    .then_some(*offset)
            })
            .collect::<Vec<_>>();
        assert!(
            !barriers.is_empty(),
            "{failure}: {name} must contain a store barrier"
        );
        let payload_stores = stores
            .iter()
            .filter_map(|(offset, displacement)| {
                (displacement.starts_with("#PERCPU_ERET_GUARD_")
                    && displacement != "#PERCPU_ERET_GUARD_SOURCE")
                    .then_some(*offset)
            })
            .collect::<Vec<_>>();
        assert!(
            !payload_stores.is_empty(),
            "{failure}: {name} must publish a guard payload"
        );
        assert!(
            barriers.iter().any(|barrier| {
                *barrier < source_stores[0]
                    && payload_stores.iter().all(|store| *store < *barrier)
            }),
            "{failure}: {name} must place the barrier after payload stores and before source"
        );
    }
}

#[test]
fn aarch64_refusal_drain_repoints_this_cpus_current_thread_before_terminating() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    let body = function_body(&source, "drain_asm_resume_pc_refusals")
        .expect("missing drain_asm_resume_pc_refusals body");
    let mask = code_mask(body);
    let failure = "a CPU may not mark its published current thread Terminated while still publishing it";

    let pointer_repoints = call_offsets(body, &mask, "set_current_thread_ptr");
    assert!(
        !pointer_repoints.is_empty(),
        "{failure}: missing current-thread pointer repoint"
    );
    let bytes = body.as_bytes();
    let current_thread_assignments = identifier_offsets(body, &mask, "current_thread")
        .into_iter()
        .filter(|offset| {
            let mut cursor = *offset + "current_thread".len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            bytes.get(cursor) == Some(&b'=') && bytes.get(cursor + 1) != Some(&b'=')
        })
        .collect::<Vec<_>>();
    assert!(
        !current_thread_assignments.is_empty(),
        "{failure}: missing scheduler current-thread assignment"
    );
    let terminate_calls = call_offsets(body, &mask, "set_terminated");
    assert!(
        !terminate_calls.is_empty(),
        "{failure}: missing termination call"
    );
    assert!(
        pointer_repoints
            .iter()
            .chain(&current_thread_assignments)
            .all(|repoint| terminate_calls.iter().all(|terminate| *repoint < *terminate)),
        "{failure}: every current-thread repoint must precede every termination"
    );
    for counter in [
        "RESUME_PC_CURRENT_DANGLING",
        "RESUME_PC_CURRENT_REPOINTED",
    ] {
        assert!(
            !identifier_offsets(body, &mask, counter).is_empty(),
            "{failure}: missing {counter} census reference"
        );
    }
}

#[test]
fn every_el1_resume_pc_consumer_uses_the_shared_admission_macro() {
    let sources = text_sources_below(RESUME_PC_ASSEMBLY_ROOT);
    assert_eq!(
        validate_resume_pc_macro_census(
            &sources,
            "RESUME_PC_EL1_OK",
            RESUME_PC_EL1_INVOCATIONS,
        ),
        Ok(())
    );
    assert_eq!(validate_no_private_resume_pc_admission(&sources), Ok(()));

    // Derive both the resume-PC register and admission target from each file's
    // macro invocation. The mutation therefore has no pinned line or closed
    // list of today's admission-label names.
    for path in [SYSCALL_ENTRY_ASM, BOOT_ASM, AARCH64_CONTEXT_SWITCH] {
        let mut mutant = sources.clone();
        let (_, source) = mutant
            .iter_mut()
            .find(|(candidate, _)| candidate == path)
            .unwrap_or_else(|| panic!("missing resume-PC assembly source {path}"));
        let invocation = assembly_macro_invocations(source, "RESUME_PC_EL1_OK")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("missing RESUME_PC_EL1_OK invocation in {path}"));
        let resume_pc = invocation.arguments.first().expect("resume-PC macro argument");
        let admission = invocation
            .arguments
            .last()
            .expect("resume-PC admission-label argument");
        let line_start = source[..invocation.offset]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
        source.insert_str(
            line_start,
            &format!("    cmp {resume_pc}, #0x1000\n    b.hs {admission}\n"),
        );
        assert!(
            validate_no_private_resume_pc_admission(&mutant).is_err(),
            "a private cmp/b.hs admission pair in {path} must fail"
        );
    }
}

#[test]
fn every_el0_resume_pc_consumer_has_a_shared_admission_arm() {
    let sources = text_sources_below(RESUME_PC_ASSEMBLY_ROOT);
    assert_eq!(
        validate_resume_pc_macro_census(
            &sources,
            "RESUME_PC_EL0_OK",
            RESUME_PC_EL0_INVOCATIONS,
        ),
        Ok(())
    );

    for (path, source) in &sources {
        for invocation in assembly_macro_invocations(source, "RESUME_PC_EL0_OK") {
            let mut mutant = sources.clone();
            let (_, mutant_source) = mutant
                .iter_mut()
                .find(|(candidate, _)| candidate == path)
                .expect("mutated resume-PC assembly source");
            mutant_source.replace_range(
                invocation.offset..invocation.offset + "RESUME_PC_EL0_OK".len(),
                "REMOVED_RESUME_PC_EL0_OK",
            );
            assert!(
                validate_resume_pc_macro_census(
                    &mutant,
                    "RESUME_PC_EL0_OK",
                    RESUME_PC_EL0_INVOCATIONS,
                )
                .is_err(),
                "removing any EL0 admission arm from {path} must fail the census"
            );
        }
    }
}

#[test]
fn userspace_resume_pc_producers_refuse_before_publishing_frame_elr() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    assert_eq!(validate_user_resume_pc_producers(&source), Ok(()));

    for producer in [
        "restore_userspace_context_inline",
        "setup_first_entry_inline",
    ] {
        let body = function_body(&source, producer).expect("userspace resume-PC producer");
        let body_start = source.find(body).expect("producer body offset");
        let mask = code_mask(body);
        let guard = call_offsets(body, &mask, "resume_pc_is_user_dispatchable")
            .into_iter()
            .next()
            .expect("producer resume-PC guard");
        let mut missing_guard = source.clone();
        missing_guard.replace_range(
            body_start + guard..body_start + guard + "resume_pc_is_user_dispatchable".len(),
            "removed_resume_pc_is_user_dispatchable",
        );
        assert!(
            validate_user_resume_pc_producers(&missing_guard).is_err(),
            "deleting {producer}'s guard must fail"
        );

        let assignment = "frame.elr = thread.context.elr_el1;";
        let assignment_in_body = body.find(assignment).expect("producer ELR assignment");
        let mut write_first = source.clone();
        write_first.replace_range(
            body_start + assignment_in_body..body_start + assignment_in_body + assignment.len(),
            "",
        );
        let body_open = body_start + body.find('{').expect("producer body brace") + 1;
        write_first.insert_str(body_open, &format!("\n    {assignment}"));
        assert!(
            validate_user_resume_pc_producers(&write_first).is_err(),
            "moving {producer}'s frame.elr assignment above its guard must fail"
        );
    }
}

#[test]
fn every_userspace_resume_pc_producer_verdict_is_consumed() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    assert_eq!(validate_user_resume_pc_verdict_consumers(&source), Ok(()));
    let mask = code_mask(&source);

    for producer in [
        "restore_userspace_context_inline",
        "setup_first_entry_inline",
    ] {
        for call in call_offsets(&source, &mask, producer)
            .into_iter()
            .filter(|offset| !preceded_by_keyword(&source, &mask, *offset, "fn"))
        {
            let close = call_expression_close(&source, &mask, call, producer)
                .expect("producer call expression");
            let mut mutant = source.clone();
            mutant.insert(close + 1, ';');
            assert!(
                validate_user_resume_pc_verdict_consumers(&mutant).is_err(),
                "turning a {producer} call site into a bare statement must fail"
            );
        }
    }
}

#[test]
fn userspace_resume_pc_predicate_stays_outside_the_kernel_text_window() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    assert_eq!(validate_user_resume_pc_predicate_is_independent(&source), Ok(()));

    // Parallels identity-maps kernel text around 0x4008_0000, overlapping the
    // 0x4000_0000/0x4100_0000 ranges where userspace programs are linked. The
    // EL0 predicate must therefore never reuse the kernel-text-window test.
    let body = function_body(&source, "resume_pc_is_user_dispatchable")
        .expect("userspace resume-PC predicate");
    let body_start = source.find(body).expect("userspace predicate body offset");
    let expression = body.find("addr >=").expect("userspace predicate expression");
    let mut mutant = source.clone();
    mutant.insert_str(
        body_start + expression,
        "resume_pc_is_dispatchable(addr) && ",
    );
    assert!(validate_user_resume_pc_predicate_is_independent(&mutant).is_err());
}

#[test]
fn aarch64_inline_elr_slot_has_one_durable_layout_assert() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    assert_eq!(validate_inline_elr_offset_assert(&source), Ok(()));

    let mutant = source.replacen(
        "const _: () = assert!(core::mem::offset_of!(CpuContext, elr_el1) == 264);\n",
        "",
        1,
    );
    assert_ne!(mutant, source, "inline ELR offset mutation anchor");
    assert!(validate_inline_elr_offset_assert(&mutant).is_err());
}

#[test]
fn aarch64_eret_resume_selection_disambiguates_inline_saves() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    assert_eq!(validate_inline_eret_resume_selector(&source), Ok(()));

    let mutant = source.replacen(
        "let resume_pc = if thread.saved_by_inline_schedule {",
        "let resume_pc = if false && thread.saved_by_inline_schedule {",
        1,
    );
    assert_ne!(mutant, source, "inline resume selector mutation anchor");
    assert!(validate_inline_eret_resume_selector(&mutant).is_err());
}

#[test]
fn ctx596_oracles_are_permanent_while_forcing_stays_feature_gated() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    assert_eq!(validate_ctx596_oracle_liveness(&source), Ok(()));

    let missing_oracle = source.replacen("CTX596_ORACLE:FAIL", "CTX596_ORACLE:REMOVED", 1);
    assert_ne!(missing_oracle, source, "CTX596 oracle mutation anchor");
    assert!(validate_ctx596_oracle_liveness(&missing_oracle).is_err());

    let gated_oracle = source.replacen(
        "#[inline(always)]\nfn check_inline_save_resume_point",
        "#[cfg(feature = \"force_eret_dispatch_596\")]\n#[inline(always)]\nfn check_inline_save_resume_point",
        1,
    );
    assert_ne!(gated_oracle, source, "CTX596 oracle cfg mutation anchor");
    assert!(validate_ctx596_oracle_liveness(&gated_oracle).is_err());

    let missing_divergence = source.replacen(
        "[CTX596_ELR_DIVERGENCE]",
        "[CTX596_ELR_DIVERGENCE_REMOVED]",
        1,
    );
    assert_ne!(
        missing_divergence, source,
        "CTX596 divergence marker mutation anchor"
    );
    assert!(validate_ctx596_oracle_liveness(&missing_divergence).is_err());

    let gated_counter = source.replacen(
        "crate::define_trace_counter!(\n    INLINE_ELR_DIVERGENCE,",
        "#[cfg(feature = \"force_eret_dispatch_596\")]\ncrate::define_trace_counter!(\n    INLINE_ELR_DIVERGENCE,",
        1,
    );
    assert_ne!(gated_counter, source, "CTX596 counter cfg mutation anchor");
    assert!(validate_ctx596_oracle_liveness(&gated_counter).is_err());

    let ungated_forcing = source.replacen(
        "#[cfg(feature = \"force_eret_dispatch_596\")]\nfn inline_ret_dispatch_info_if_ready",
        "fn inline_ret_dispatch_info_if_ready",
        1,
    );
    assert_ne!(
        ungated_forcing, source,
        "CTX596 forced-dispatch cfg mutation anchor"
    );
    assert!(validate_ctx596_oracle_liveness(&ungated_forcing).is_err());
}

#[test]
fn inline_schedule_trampoline_retains_the_redundant_elr_guard() {
    let source = repo_text(AARCH64_CONTEXT_SWITCH);
    assert_eq!(validate_inline_trampoline_repair(&source), Ok(()));

    let repair = "old_thread.context.elr_el1 = old_thread.context.x30;";
    let fallback_repair = source
        .find(repair)
        .expect("inline trampoline fallback repair mutation anchor");
    let normal_repair = source
        .rfind(repair)
        .expect("inline trampoline normal repair mutation anchor");
    assert_ne!(
        fallback_repair, normal_repair,
        "inline trampoline repair arms must have distinct anchors"
    );

    let mut missing_fallback_repair = source.clone();
    missing_fallback_repair.replace_range(fallback_repair..fallback_repair + repair.len(), "");
    assert!(validate_inline_trampoline_repair(&missing_fallback_repair).is_err());

    let mut missing_normal_repair = source.clone();
    missing_normal_repair.replace_range(normal_repair..normal_repair + repair.len(), "");
    assert!(validate_inline_trampoline_repair(&missing_normal_repair).is_err());

    let third_arm_without_repair = source.replacen(
        "    }\n\n    let sched = unsafe { &mut *sched_ptr };",
        "    } else if old_id == new_id {\n        core::hint::spin_loop();\n    }\n\n    let sched = unsafe { &mut *sched_ptr };",
        1,
    );
    assert_ne!(
        third_arm_without_repair, source,
        "inline trampoline third-arm mutation anchor"
    );
    assert!(validate_inline_trampoline_repair(&third_arm_without_repair).is_err());
}

#[test]
fn no_cr3_dispatch_fails_closed() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(validate_no_cr3_dispatch_fails_closed(&source), Ok(()));
}

#[test]
fn no_cr3_dispatch_validator_rejects_warn_only_arm() {
    let synthetic = synthetic_no_cr3_restore_source(r#"log::warn!("Process has no page table!");"#);
    assert!(validate_no_cr3_dispatch_fails_closed(&synthetic).is_err());
}

#[test]
fn no_cr3_dispatch_validator_rejects_missing_return() {
    let synthetic = synthetic_no_cr3_restore_source(
        r#"
            thread.set_terminated();
            scheduler::with_thread_mut(thread_id, |sched_thread| {
                sched_thread.set_terminated();
            });
            scheduler::set_need_resched();
            setup_idle_return(interrupt_frame);
            scheduler::switch_to_idle();
        "#,
    );
    assert!(validate_no_cr3_dispatch_fails_closed(&synthetic).is_err());
}

#[test]
fn no_cr3_dispatch_validator_rejects_scheduler_only_termination() {
    let synthetic = synthetic_no_cr3_restore_source(
        r#"
            scheduler::with_thread_mut(thread_id, |sched_thread| {
                sched_thread.set_terminated();
            });
            scheduler::set_need_resched();
            setup_idle_return(interrupt_frame);
            scheduler::switch_to_idle();
            return;
        "#,
    );
    assert!(validate_no_cr3_dispatch_fails_closed(&synthetic).is_err());
}

#[test]
fn userspace_fault_termination_returns_on_the_idle_thread_stack() {
    let source = repo_text("kernel/src/interrupts.rs");
    assert_eq!(validate_fault_idle_return_source(&source), Ok(()));
}

#[test]
fn userspace_fault_idle_return_validator_rejects_private_stack_derivation() {
    let kernel_stack_mutant = r#"
        fn page_fault_handler(mut stack_frame: InterruptStackFrame) {
            switch_to_idle();
            setup_idle_return(&mut stack_frame);
            let idle_stack = crate::per_cpu::kernel_stack_top();
            stack_frame.as_mut().update(|frame| {
                frame.stack_pointer = VirtAddr::new(idle_stack);
            });
        }

        fn general_protection_fault_handler(mut stack_frame: InterruptStackFrame) {
            switch_to_idle();
            setup_idle_return(&mut stack_frame);
        }
    "#;
    assert!(validate_fault_idle_return_source(kernel_stack_mutant).is_err());

    let ist_stack_mutant = r#"
        fn page_fault_handler(mut stack_frame: InterruptStackFrame) {
            switch_to_idle();
            setup_idle_return(&mut stack_frame);
        }

        fn general_protection_fault_handler(mut stack_frame: InterruptStackFrame) {
            switch_to_idle();
            setup_idle_return(&mut stack_frame);
            let current_rsp: u64;
            core::arch::asm!("mov {}, rsp", out(reg) current_rsp);
            stack_frame.as_mut().update(|frame| {
                frame.stack_pointer = VirtAddr::new(current_rsp + 256);
            });
        }
    "#;
    assert!(validate_fault_idle_return_source(ist_stack_mutant).is_err());
}

fn validate_coherent_rsp0_publishers(source: &str) -> Result<(), String> {
    let mask = code_mask(source);
    if !identifier_offsets(source, &mask, "set_kernel_stack").is_empty() {
        return Err("context_switch.rs still calls the TSS-only RSP0 writer".to_string());
    }
    let first_entry = function_body(source, "setup_first_userspace_entry")
        .ok_or_else(|| "missing fn setup_first_userspace_entry".to_string())?;
    let first_entry_mask = code_mask(first_entry);
    if identifier_offsets(first_entry, &first_entry_mask, "update_tss_rsp0").is_empty() {
        return Err(
            "first userspace entry does not publish RSP0 through per-CPU state".to_string(),
        );
    }
    Ok(())
}

#[test]
fn every_context_switch_rsp0_publish_updates_tss_and_per_cpu_cache() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(validate_coherent_rsp0_publishers(&source), Ok(()));
}

#[test]
fn rsp0_publisher_validator_rejects_a_tss_only_first_entry_write() {
    let synthetic = r#"
        fn setup_first_userspace_entry(kernel_stack_top: VirtAddr) {
            crate::gdt::set_kernel_stack(kernel_stack_top);
        }
        fn another_restore(kernel_stack_top: VirtAddr) {
            crate::per_cpu::update_tss_rsp0(kernel_stack_top.as_u64());
        }
    "#;
    assert!(validate_coherent_rsp0_publishers(synthetic).is_err());
}

fn validate_interrupt_return_scheduler_acquisitions(source: &str) -> Result<(), String> {
    let check = function_body(source, "check_need_resched_and_switch")
        .ok_or_else(|| "missing fn check_need_resched_and_switch".to_string())?;
    let pair_start = check
        .find("let (blocked_in_syscall, old_thread_is_user) =")
        .ok_or_else(|| "old-thread fields are not read as one pair".to_string())?;
    let pair_end = check[pair_start..]
        .find("if from_userspace {")
        .map(|offset| pair_start + offset)
        .ok_or_else(|| "missing from-userspace branch after old-thread read".to_string())?;
    let pair = &check[pair_start..pair_end];
    if pair
        .matches("scheduler::with_thread_mut(old_thread_id")
        .count()
        != 1
        || !pair.contains("thread.blocked_in_syscall")
        || !pair.contains("thread.privilege == ThreadPrivilege::User")
    {
        return Err("old-thread fields require more than one scheduler acquisition".to_string());
    }

    let switch = function_body(source, "switch_to_thread")
        .ok_or_else(|| "missing fn switch_to_thread".to_string())?;
    let normalized_switch = normalized_code(switch);
    if !normalized_switch.contains(
        "} else if blocked_in_syscall || saved_context_is_kernel_frame(thread_id, process_manager_guard.as_ref()) {",
    ) || normalized_switch.contains("let saved_context_is_kernel_frame =")
    {
        return Err(
            "saved-frame lookup is not lazy inside the non-idle user branch".to_string(),
        );
    }
    let helper = function_body(source, "saved_context_is_kernel_frame")
        .ok_or_else(|| "missing fn saved_context_is_kernel_frame".to_string())?;
    let helper_mask = code_mask(helper);
    if !identifier_offsets(helper, &helper_mask, "with_scheduler").is_empty()
        || !identifier_offsets(helper, &helper_mask, "with_thread_mut").is_empty()
    {
        return Err("saved-frame helper recomputes caller-known scheduler state".to_string());
    }
    Ok(())
}

fn validate_abort_dispatch_preserves_resume_state(source: &str) -> Result<(), String> {
    let body = function_body(source, "abort_dispatch_and_resume")
        .ok_or_else(|| "missing fn abort_dispatch_and_resume".to_string())?;
    let mask = code_mask(body);
    let bytes = body.as_bytes();

    let guard = identifier_offsets(body, &mask, "let")
        .into_iter()
        .find_map(|let_offset| {
            let mut cursor = let_offset + "let".len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            let name_start = cursor;
            while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
                cursor += 1;
            }
            if cursor == name_start {
                return None;
            }
            let binding_end =
                (cursor..bytes.len()).find(|offset| mask[*offset] && bytes[*offset] == b';')?;
            let binding = &body[cursor..binding_end];
            let normalized_binding = normalized_code(binding);
            (normalized_binding.contains("thread.state")
                && normalized_binding.contains("ThreadState::Ready"))
            .then(|| body[name_start..cursor].to_string())
        })
        .ok_or_else(|| {
            "resume-thread runnable guard does not derive from thread.state and ThreadState::Ready"
                .to_string()
        })?;

    let guarded_blocks: Vec<_> = identifier_offsets(body, &mask, "if")
        .into_iter()
        .filter_map(|if_offset| {
            let block = braced_block(body, &mask, if_offset)?;
            let open =
                (if_offset..bytes.len()).find(|offset| mask[*offset] && bytes[*offset] == b'{')?;
            let condition = &body[if_offset + "if".len()..open];
            let condition_mask = code_mask(condition);
            (!identifier_offsets(condition, &condition_mask, &guard).is_empty())
                .then_some((open + 1, if_offset + block.len() - 1))
        })
        .collect();
    if guarded_blocks.is_empty() {
        return Err(format!(
            "resume-thread runnable guard `{guard}` does not control an if block"
        ));
    }
    let inside_guard = |offset: usize| {
        guarded_blocks
            .iter()
            .any(|(start, end)| *start <= offset && offset < *end)
    };

    let running_transitions = identifier_offsets(body, &mask, "set_running");
    if running_transitions.is_empty() {
        return Err("abort rollback never transitions a runnable resume thread".to_string());
    }
    if running_transitions
        .into_iter()
        .any(|offset| !inside_guard(offset))
    {
        return Err("resume-thread set_running is outside the runnable guard".to_string());
    }

    let resume_dequeues: Vec<_> = identifier_offsets(body, &mask, "per_cpu_queues")
        .into_iter()
        .filter(|queue_offset| {
            let statement_end = (*queue_offset..bytes.len())
                .find(|offset| mask[*offset] && bytes[*offset] == b';')
                .unwrap_or(bytes.len());
            let statement = &body[*queue_offset..statement_end];
            let statement_mask = code_mask(statement);
            !identifier_offsets(statement, &statement_mask, "resume_thread_id").is_empty()
                && !identifier_offsets(statement, &statement_mask, "remove").is_empty()
        })
        .collect();
    if resume_dequeues.is_empty() {
        return Err("abort rollback does not dequeue a runnable resume thread".to_string());
    }
    if resume_dequeues
        .into_iter()
        .any(|offset| !inside_guard(offset))
    {
        return Err("resume-thread queue removal is outside the runnable guard".to_string());
    }
    Ok(())
}

fn validate_terminated_signal_dispatch_switches_to_idle(source: &str) -> Result<(), String> {
    let body = function_body(source, "switch_to_thread")
        .ok_or_else(|| "missing fn switch_to_thread".to_string())?;
    let mask = code_mask(body);
    let bytes = body.as_bytes();
    let arm = identifier_offsets(body, &mask, "Terminated")
        .into_iter()
        .find_map(|terminated_offset| {
            let prefix_start = terminated_offset.saturating_sub(160);
            let pattern_end = terminated_offset + "Terminated".len();
            if !normalized_code(&body[prefix_start..pattern_end])
                .contains("SignalDeliveryResult::Terminated")
            {
                return None;
            }
            let equals = (pattern_end..bytes.len())
                .find(|offset| mask[*offset] && bytes[*offset] == b'=')?;
            let arrow = (equals + 1..bytes.len())
                .find(|offset| mask[*offset] && !bytes[*offset].is_ascii_whitespace())?;
            if bytes[arrow] != b'>' {
                return None;
            }
            braced_block(body, &mask, arrow + 1)
        })
        .ok_or_else(|| {
            "switch_to_thread has no SignalDeliveryResult::Terminated arm".to_string()
        })?;
    let arm_mask = code_mask(arm);

    if !identifier_offsets(arm, &arm_mask, "abort_dispatch_and_resume").is_empty() {
        return Err("Terminated arm rolls back scheduler bookkeeping".to_string());
    }
    if identifier_offsets(arm, &arm_mask, "setup_idle_return").is_empty() {
        return Err("Terminated arm does not install an idle return frame".to_string());
    }
    let switch_offset = identifier_offsets(arm, &arm_mask, "switch_to_idle")
        .first()
        .copied()
        .ok_or_else(|| "Terminated arm does not switch scheduler state to idle".to_string())?;
    let return_offset = identifier_offsets(arm, &arm_mask, "return")
        .first()
        .copied()
        .ok_or_else(|| "Terminated arm has no early return".to_string())?;
    if switch_offset > return_offset {
        return Err("Terminated arm returns before switching scheduler state to idle".to_string());
    }
    Ok(())
}

fn validate_rollback_return_alternation(region: &str, description: &str) -> Result<(), String> {
    let region_mask = code_mask(region);
    let mut sequence: Vec<_> =
        identifier_offsets(region, &region_mask, "abort_dispatch_and_resume")
            .into_iter()
            .map(|offset| (offset, "abort_dispatch_and_resume"))
            .chain(
                identifier_offsets(region, &region_mask, "switch_to_idle")
                    .into_iter()
                    .map(|offset| (offset, "switch_to_idle")),
            )
            .chain(
                identifier_offsets(region, &region_mask, "return")
                    .into_iter()
                    .map(|offset| (offset, "return")),
            )
            .collect();
    sequence.sort_unstable_by_key(|(offset, _)| *offset);

    if !sequence
        .iter()
        .any(|(_, identifier)| *identifier == "return")
    {
        return Err(format!(
            "{description} contains no early return to validate"
        ));
    }
    for (index, (_, identifier)) in sequence.iter().enumerate() {
        let valid = if index % 2 == 0 {
            matches!(*identifier, "abort_dispatch_and_resume" | "switch_to_idle")
        } else {
            *identifier == "return"
        };
        if !valid {
            let expected = if index % 2 == 0 {
                "abort_dispatch_and_resume or switch_to_idle"
            } else {
                "return"
            };
            return Err(format!(
                "{description} rollback/return sequence item {index} is `{identifier}`, expected `{expected}`"
            ));
        }
    }
    if sequence.len() % 2 != 0 {
        return Err(format!(
            "{description} has a dispatch commitment without a following return"
        ));
    }
    Ok(())
}

fn validate_save_failure_rollback(source: &str) -> Result<(), String> {
    let body = function_body(source, "check_need_resched_and_switch")
        .ok_or_else(|| "missing fn check_need_resched_and_switch".to_string())?;
    let pair_start = body
        .find("let (blocked_in_syscall, old_thread_is_user) =")
        .ok_or_else(|| "missing old-thread field binding before save region".to_string())?;
    let save_start = body[pair_start..]
        .find("if from_userspace {")
        .map(|offset| pair_start + offset)
        .ok_or_else(|| "missing from-userspace save branch after old-thread binding".to_string())?;

    let save_tail = &body[save_start..];
    let save_tail_mask = code_mask(save_tail);
    let switch_offset = identifier_offsets(save_tail, &save_tail_mask, "switch_to_thread")
        .into_iter()
        .find(|offset| {
            let mut cursor = *offset + "switch_to_thread".len();
            while cursor < save_tail.len()
                && (!save_tail_mask[cursor] || save_tail.as_bytes()[cursor].is_ascii_whitespace())
            {
                cursor += 1;
            }
            save_tail.as_bytes().get(cursor) == Some(&b'(')
        })
        .ok_or_else(|| "missing switch_to_thread call after save region".to_string())?;

    validate_rollback_return_alternation(&save_tail[..switch_offset], "save region")
}

fn validate_switch_dispatch_rollback(source: &str) -> Result<(), String> {
    let body = function_body(source, "switch_to_thread")
        .ok_or_else(|| "missing fn switch_to_thread".to_string())?;
    let body_mask = code_mask(body);
    let committed_start = identifier_offsets(body, &body_mask, "set_current_thread")
        .first()
        .copied()
        .ok_or_else(|| {
            "switch_to_thread has no set_current_thread dispatch-commit anchor".to_string()
        })?;

    validate_rollback_return_alternation(
        &body[committed_start..],
        "switch_to_thread committed region",
    )
}

#[test]
fn interrupt_return_reads_old_thread_once_and_lazily_checks_saved_cs() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(
        validate_interrupt_return_scheduler_acquisitions(&source),
        Ok(())
    );
}

#[test]
fn interrupt_return_validator_rejects_unconditional_saved_frame_lookup() {
    let synthetic = r#"
        fn check_need_resched_and_switch(old_thread_id: u64) {
            let (blocked_in_syscall, old_thread_is_user) =
                scheduler::with_thread_mut(old_thread_id, |thread| {
                    (thread.blocked_in_syscall, thread.privilege == ThreadPrivilege::User)
                });
            if from_userspace {}
        }
        fn saved_context_is_kernel_frame(thread_id: u64, guard: Option<&Guard>) -> bool {
            manager_has_kernel_frame(thread_id, guard)
        }
        fn switch_to_thread(thread_id: u64, process_manager_guard: Option<Guard>) {
            let saved_context_is_kernel_frame =
                saved_context_is_kernel_frame(thread_id, process_manager_guard.as_ref());
            if is_idle {
                setup_idle_return();
            } else if is_kernel_thread {
                setup_kernel_thread_return();
            } else if blocked_in_syscall || saved_context_is_kernel_frame {
                restore_kernel_frame();
            }
        }
    "#;
    assert!(validate_interrupt_return_scheduler_acquisitions(synthetic).is_err());
}

#[test]
fn abort_dispatch_rollback_preserves_resume_thread_state() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    assert_eq!(
        validate_abort_dispatch_preserves_resume_state(&source),
        Ok(())
    );
}

#[test]
fn abort_dispatch_validator_rejects_unconditional_running_transition() {
    let synthetic = r#"
        fn abort_dispatch_and_resume(resume_thread_id: u64) {
            let (resume_runnable, thread_ptr) = match sched.get_thread_mut(resume_thread_id) {
                Some(thread) => {
                    let resume_runnable = matches!(
                        thread.state,
                        ThreadState::Ready | ThreadState::Running
                    );
                    thread.set_running();
                    (resume_runnable, thread as *mut Thread)
                }
                None => return,
            };
            if resume_runnable {
                for queue in sched.per_cpu_queues.iter_mut() {
                    if let Some(position) =
                        queue.iter().position(|&id| id == resume_thread_id)
                    {
                        queue.remove(position);
                    }
                }
            }
        }
    "#;
    assert!(validate_abort_dispatch_preserves_resume_state(synthetic).is_err());
}

#[test]
fn abort_dispatch_validator_rejects_unguarded_resume_dequeue() {
    let synthetic = r#"
        fn abort_dispatch_and_resume(resume_thread_id: u64) {
            let (resume_runnable, thread_ptr) = match sched.get_thread_mut(resume_thread_id) {
                Some(thread) => {
                    let resume_runnable = matches!(
                        thread.state,
                        ThreadState::Ready | ThreadState::Running
                    );
                    if resume_runnable {
                        thread.set_running();
                    }
                    (resume_runnable, thread as *mut Thread)
                }
                None => return,
            };
            for queue in sched.per_cpu_queues.iter_mut() {
                if let Some(position) = queue.iter().position(|&id| id == resume_thread_id) {
                    queue.remove(position);
                }
            }
        }
    "#;
    assert!(validate_abort_dispatch_preserves_resume_state(synthetic).is_err());
}

#[test]
fn terminated_signal_dispatch_completes_switch_to_idle() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(
        validate_terminated_signal_dispatch_switches_to_idle(&source),
        Ok(())
    );
}

#[test]
fn terminated_signal_validator_rejects_bookkeeping_rollback() {
    let synthetic = r#"
        fn switch_to_thread(thread_id: u64, resume_thread_id: u64) {
            match signal_result {
                SignalDeliveryResult::Terminated(notification) => {
                    scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
                    scheduler::set_need_resched();
                    return;
                }
                SignalDeliveryResult::NoAction => {}
            }
        }
    "#;
    assert!(validate_terminated_signal_dispatch_switches_to_idle(synthetic).is_err());
}

#[test]
fn terminated_signal_validator_rejects_return_without_completed_switch() {
    let synthetic = r#"
        fn switch_to_thread() {
            match signal_result {
                SignalDeliveryResult::Terminated(notification) => {
                    scheduler::set_need_resched();
                    return;
                }
                SignalDeliveryResult::NoAction => {}
            }
        }
    "#;
    assert!(validate_terminated_signal_dispatch_switches_to_idle(synthetic).is_err());
}

#[test]
fn every_save_failure_rolls_back_the_committed_dispatch() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(validate_save_failure_rollback(&source), Ok(()));
}

#[test]
fn save_failure_rollback_validator_rejects_unpaired_return() {
    let synthetic = r#"
        fn check_need_resched_and_switch() {
            let (blocked_in_syscall, old_thread_is_user) = current_thread_fields();
            if from_userspace {
                if save_failed {
                    return;
                }
            }
            switch_to_thread(new_thread_id);
        }
    "#;
    assert!(validate_save_failure_rollback(synthetic).is_err());
}

#[test]
fn save_failure_rollback_validator_rejects_rollback_after_return() {
    let synthetic = r#"
        fn check_need_resched_and_switch() {
            let (blocked_in_syscall, old_thread_is_user) = current_thread_fields();
            if from_userspace {
                if save_failed {
                    return;
                    scheduler::abort_dispatch_and_resume(new_thread_id, old_thread_id);
                }
            }
            switch_to_thread(new_thread_id);
        }
    "#;
    assert!(validate_save_failure_rollback(synthetic).is_err());
}

#[test]
fn every_switch_return_rolls_back_the_committed_dispatch() {
    let source = repo_text("kernel/src/interrupts/context_switch.rs");
    assert_eq!(validate_switch_dispatch_rollback(&source), Ok(()));
}

#[test]
fn switch_dispatch_rollback_validator_rejects_unpaired_return() {
    let synthetic = r#"
        fn switch_to_thread() {
            per_cpu::set_current_thread(incoming);
            return;
        }
    "#;
    assert!(validate_switch_dispatch_rollback(synthetic).is_err());
}

#[test]
fn switch_dispatch_rollback_validator_rejects_rollback_after_return() {
    let synthetic = r#"
        fn switch_to_thread() {
            per_cpu::set_current_thread(incoming);
            return;
            scheduler::abort_dispatch_and_resume(thread_id, resume_thread_id);
        }
    "#;
    assert!(validate_switch_dispatch_rollback(synthetic).is_err());
}

#[test]
fn rollback_return_alternation_rejects_return_without_commitment() {
    assert!(validate_rollback_return_alternation("return;", "synthetic region").is_err());
}

const FATAL_EXCEPTION_PATH: &str = "kernel/src/arch_impl/aarch64/exception.rs";
const SCHEDULER_PATH: &str = "kernel/src/task/scheduler.rs";

#[derive(Clone, Debug)]
struct SourceFunction {
    name: String,
    name_offset: usize,
    open: usize,
    close: usize,
    is_public: bool,
    is_extern_entry: bool,
}

#[derive(Clone, Copy, Debug)]
struct LexicalBlock {
    open: usize,
    close: usize,
    el0_guarded: bool,
}

fn compact_code(fragment: &str) -> String {
    normalized_code(fragment)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn header_is_positive_el0_guard(header: &str) -> bool {
    let compact = compact_code(header);
    let Some(condition) = compact.strip_prefix("if") else {
        return false;
    };
    if condition.contains("||")
        || condition.contains("from_el0==false")
        || condition.contains("false==from_el0")
        || condition.contains("from_el0!=true")
        || condition.contains("true!=from_el0")
    {
        return false;
    }

    let condition_mask = code_mask(condition);
    identifier_offsets(condition, &condition_mask, "from_el0")
        .into_iter()
        .any(|offset| {
            let bytes = condition.as_bytes();
            let mut cursor = offset;
            while cursor > 0 && bytes[cursor - 1] == b'(' {
                cursor -= 1;
            }
            cursor == 0 || bytes[cursor - 1] != b'!'
        })
}

fn lexical_blocks(source: &str, mask: &[bool]) -> Vec<LexicalBlock> {
    let bytes = source.as_bytes();
    let mut blocks = Vec::new();
    let mut stack = Vec::new();
    let mut header_start = 0usize;

    for index in 0..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'{' => {
                let el0_guarded = header_is_positive_el0_guard(&source[header_start..index]);
                stack.push((index, el0_guarded));
                header_start = index + 1;
            }
            b'}' => {
                if let Some((open, el0_guarded)) = stack.pop() {
                    blocks.push(LexicalBlock {
                        open,
                        close: index,
                        el0_guarded,
                    });
                }
                header_start = index + 1;
            }
            b';' => header_start = index + 1,
            _ => {}
        }
    }
    blocks
}

fn source_functions(source: &str, mask: &[bool], blocks: &[LexicalBlock]) -> Vec<SourceFunction> {
    let bytes = source.as_bytes();
    let mut functions = Vec::new();

    for fn_offset in identifier_offsets(source, mask, "fn") {
        let mut cursor = fn_offset + "fn".len();
        while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
            cursor += 1;
        }
        let name_offset = cursor;
        while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == name_offset {
            continue;
        }
        let Some((_, open)) = definition_span(source, mask, name_offset, cursor) else {
            continue;
        };
        let Some(block) = blocks.iter().find(|block| block.open == open) else {
            continue;
        };
        let header_start = (0..fn_offset)
            .rev()
            .find(|index| mask[*index] && matches!(bytes[*index], b';' | b'{' | b'}'))
            .map_or(0, |delimiter| delimiter + 1);
        let header = &source[header_start..open];
        let header_mask = code_mask(header);
        let relative_fn = fn_offset - header_start;
        let is_public = identifier_offsets(header, &header_mask, "pub")
            .into_iter()
            .any(|offset| offset < relative_fn);
        let is_extern_entry = is_public
            && identifier_offsets(header, &header_mask, "extern")
                .into_iter()
                .any(|offset| offset < relative_fn);

        functions.push(SourceFunction {
            name: source[name_offset..cursor].to_owned(),
            name_offset,
            open,
            close: block.close,
            is_public,
            is_extern_entry,
        });
    }
    functions
}

fn derived_blocking_scheduler_accessors(
    scheduler_source: &str,
) -> Result<BTreeSet<String>, String> {
    let mask = code_mask(scheduler_source);
    let blocks = lexical_blocks(scheduler_source, &mask);
    let functions = source_functions(scheduler_source, &mask, &blocks);
    let bodies = module_function_bodies(scheduler_source);
    let mut blocking = BTreeSet::new();

    if bodies.contains_key("lock_scheduler") {
        blocking.insert("lock_scheduler".to_string());
    }
    for function in functions.iter().filter(|function| function.is_public) {
        if bodies.get(&function.name).is_some_and(|definitions| {
            definitions.iter().any(|body| {
                let body_mask = code_mask(body);
                !call_offsets(body, &body_mask, "lock_scheduler").is_empty()
            })
        }) {
            blocking.insert(function.name.clone());
        }
    }

    if blocking.is_empty() {
        return Err("blocking scheduler accessor derivation is empty".to_string());
    }
    let missing: Vec<_> = ["current_thread_id", "with_scheduler", "with_thread_mut"]
        .into_iter()
        .filter(|anchor| !blocking.contains(*anchor))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "blocking scheduler accessor derivation missing anchors: {}",
            missing.join(", ")
        ));
    }
    if blocking.contains("try_dump_state") {
        return Err("try_dump_state was misclassified as blocking".to_string());
    }
    Ok(blocking)
}

fn call_is_el0_guarded(blocks: &[LexicalBlock], offset: usize) -> bool {
    blocks
        .iter()
        .any(|block| block.el0_guarded && block.open < offset && offset < block.close)
}

fn enclosing_function<'a>(
    functions: &'a [SourceFunction],
    offset: usize,
) -> Option<&'a SourceFunction> {
    functions
        .iter()
        .filter(|function| function.open < offset && offset < function.close)
        .max_by_key(|function| function.open)
}

fn el0_only_functions(
    exception_source: &str,
    mask: &[bool],
    blocks: &[LexicalBlock],
    functions: &[SourceFunction],
) -> BTreeSet<String> {
    let names: BTreeSet<_> = functions
        .iter()
        .map(|function| function.name.clone())
        .collect();
    let definition_names: BTreeSet<_> = functions
        .iter()
        .map(|function| function.name_offset)
        .collect();
    let entry_points: BTreeSet<_> = functions
        .iter()
        .filter(|function| function.is_extern_entry)
        .map(|function| function.name.clone())
        .collect();
    let call_sites: BTreeMap<_, _> = names
        .iter()
        .map(|name| {
            let calls = call_offsets(exception_source, mask, name)
                .into_iter()
                .filter(|offset| !definition_names.contains(offset))
                .collect::<Vec<_>>();
            (name.clone(), calls)
        })
        .collect();

    let mut el0_only = BTreeSet::new();
    loop {
        let newly_el0_only: Vec<_> = call_sites
            .iter()
            .filter(|(name, calls)| {
                !entry_points.contains(*name)
                    && !calls.is_empty()
                    && calls.iter().all(|offset| {
                        call_is_el0_guarded(blocks, *offset)
                            || enclosing_function(functions, *offset)
                                .is_some_and(|caller| el0_only.contains(&caller.name))
                    })
            })
            .map(|(name, _)| name.clone())
            .filter(|name| !el0_only.contains(name))
            .collect();
        if newly_el0_only.is_empty() {
            break;
        }
        el0_only.extend(newly_el0_only);
    }
    el0_only
}

fn blocking_calls(
    source: &str,
    mask: &[bool],
    blocking: &BTreeSet<String>,
) -> Vec<(usize, String)> {
    let mut calls = Vec::new();
    for accessor in blocking {
        calls.extend(
            call_offsets(source, mask, accessor)
                .into_iter()
                .map(|offset| (offset, accessor.clone())),
        );
    }
    calls.sort_unstable();
    calls
}

fn scanned_fatal_sources(
    exception_source: &str,
    scheduler_source: &str,
    aarch64_sources: &[(String, String)],
) -> Vec<(String, String)> {
    let mut sources: Vec<_> = aarch64_sources
        .iter()
        .filter(|(path, _)| path != FATAL_EXCEPTION_PATH && path != SCHEDULER_PATH)
        .cloned()
        .collect();
    sources.push((FATAL_EXCEPTION_PATH.to_owned(), exception_source.to_owned()));
    sources.push((SCHEDULER_PATH.to_owned(), scheduler_source.to_owned()));
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

fn resolved_callees_in_range(
    source: &str,
    mask: &[bool],
    definition_names: &BTreeSet<usize>,
    resolved_names: &BTreeSet<String>,
    open: usize,
    close: usize,
) -> BTreeSet<String> {
    resolved_names
        .iter()
        .filter(|name| {
            call_offsets(source, mask, name).into_iter().any(|offset| {
                open < offset && offset < close && !definition_names.contains(&offset)
            })
        })
        .cloned()
        .collect()
}

fn derived_fatal_callees(
    exception_source: &str,
    exception_mask: &[bool],
    exception_blocks: &[LexicalBlock],
    exception_functions: &[SourceFunction],
    el0_only: &BTreeSet<String>,
    scanned_sources: &[(String, String)],
) -> Result<BTreeSet<String>, String> {
    let resolved_names: BTreeSet<_> = scanned_sources
        .iter()
        .flat_map(|(_, source)| module_function_bodies(source).into_keys())
        .collect();
    let exception_definition_names: BTreeSet<_> = exception_functions
        .iter()
        .map(|function| function.name_offset)
        .collect();

    // Seeds are derived from every resolved call site that the existing
    // exception-region analysis says an EL1 exception can reach.
    let mut fatal_callees: BTreeSet<_> = resolved_names
        .iter()
        .filter(|name| {
            call_offsets(exception_source, exception_mask, name)
                .into_iter()
                .filter(|offset| !exception_definition_names.contains(offset))
                .any(|offset| {
                    !call_is_el0_guarded(exception_blocks, offset)
                        && !enclosing_function(exception_functions, offset)
                            .is_some_and(|function| el0_only.contains(&function.name))
                })
        })
        .cloned()
        .collect();

    // Follow only callees whose definitions are inside the bounded source
    // domain. This is deliberately a downward closure: callers of a fatal
    // emitter do not become fatal merely because they can invoke it.
    loop {
        let mut newly_reached = BTreeSet::new();
        for (_, source) in scanned_sources {
            let mask = code_mask(source);
            let blocks = lexical_blocks(source, &mask);
            let functions = source_functions(source, &mask, &blocks);
            let definition_names: BTreeSet<_> = functions
                .iter()
                .map(|function| function.name_offset)
                .collect();
            for function in functions
                .iter()
                .filter(|function| fatal_callees.contains(&function.name))
            {
                newly_reached.extend(resolved_callees_in_range(
                    source,
                    &mask,
                    &definition_names,
                    &resolved_names,
                    function.open,
                    function.close,
                ));
            }
        }
        newly_reached.retain(|name| !fatal_callees.contains(name));
        if newly_reached.is_empty() {
            break;
        }
        fatal_callees.extend(newly_reached);
    }

    if fatal_callees.is_empty() {
        return Err("fatal-callee derivation is empty".to_string());
    }
    let missing: Vec<_> = [
        "dump_el1_fatal_frame_and_dispatch_trace",
        "dump_all_save_skew_snapshots",
        "dump_dispatch_trace",
        "dump_cpu_state_history_postmortem",
    ]
    .into_iter()
    .filter(|anchor| !fatal_callees.contains(*anchor))
    .collect();
    if !missing.is_empty() {
        return Err(format!(
            "fatal-callee derivation missing anchors: {}",
            missing.join(", ")
        ));
    }

    // Neither ordinary function is reachable in the callee direction from an
    // EL1-reachable fatal region: one is a syscall, the other a thread-context
    // placement diagnostic. This precision assertion is not an allowlist.
    let unexpected: Vec<_> = ["sys_fork_aarch64", "dump_thread_placement"]
        .into_iter()
        .filter(|name| fatal_callees.contains(*name))
        .collect();
    if !unexpected.is_empty() {
        return Err(format!(
            "fatal-callee derivation reached ordinary functions: {}",
            unexpected.join(", ")
        ));
    }

    Ok(fatal_callees)
}

fn validate_fatal_scheduler_accessor_census(
    exception_source: &str,
    scheduler_source: &str,
    aarch64_sources: &[(String, String)],
) -> Result<(), String> {
    let blocking = derived_blocking_scheduler_accessors(scheduler_source)?;
    let exception_sources = vec![(FATAL_EXCEPTION_PATH.to_owned(), exception_source.to_owned())];
    let total_blocking_calls = census(&exception_sources, |source, mask| {
        blocking_calls(source, mask, &blocking)
            .into_iter()
            .map(|(offset, _)| offset)
            .collect()
    })
    .values()
    .sum::<usize>();
    if total_blocking_calls == 0 {
        return Err("exception.rs blocking-accessor call-site census is empty".to_string());
    }

    let exception_mask = code_mask(exception_source);
    let exception_blocks = lexical_blocks(exception_source, &exception_mask);
    let exception_functions =
        source_functions(exception_source, &exception_mask, &exception_blocks);
    let el0_only = el0_only_functions(
        exception_source,
        &exception_mask,
        &exception_blocks,
        &exception_functions,
    );
    let mut violations = census_tagged(&exception_sources, |source, mask| {
        blocking_calls(source, mask, &blocking)
            .into_iter()
            .filter(|(offset, _)| {
                !call_is_el0_guarded(&exception_blocks, *offset)
                    && !enclosing_function(&exception_functions, *offset)
                        .is_some_and(|function| el0_only.contains(&function.name))
            })
            .collect()
    });

    let scanned_sources =
        scanned_fatal_sources(exception_source, scheduler_source, aarch64_sources);
    let fatal_callees = derived_fatal_callees(
        exception_source,
        &exception_mask,
        &exception_blocks,
        &exception_functions,
        &el0_only,
        &scanned_sources,
    )?;
    let external_sources: Vec<_> = scanned_sources
        .iter()
        .filter(|(path, _)| path != FATAL_EXCEPTION_PATH)
        .cloned()
        .collect();
    let external_violations = census_tagged(&external_sources, |source, mask| {
        let blocks = lexical_blocks(source, mask);
        let functions = source_functions(source, mask, &blocks);
        let mut calls = BTreeSet::new();
        for function in functions
            .iter()
            .filter(|function| fatal_callees.contains(&function.name))
        {
            let body = &source[function.open..=function.close];
            let body_mask = code_mask(body);
            for (offset, accessor) in blocking_calls(body, &body_mask, &blocking) {
                calls.insert((function.open + offset, accessor));
            }
        }
        calls.into_iter().collect()
    });
    violations.extend(external_violations);

    census_error(&violations, &[])
}

fn aarch64_sources_with_exception(exception_source: &str) -> Vec<(String, String)> {
    let root = repo_root();
    let mut sources: Vec<_> = rust_sources_below("kernel/src/arch_impl/aarch64")
        .into_iter()
        .map(|(path, source)| {
            let relative = path
                .strip_prefix(&root)
                .expect("AArch64 source below repository root")
                .to_string_lossy()
                .into_owned();
            let source = if relative == FATAL_EXCEPTION_PATH {
                exception_source.to_owned()
            } else {
                source
            };
            (relative, source)
        })
        .collect();
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

#[test]
fn fatal_exception_reports_never_take_blocking_scheduler_accessors() {
    let exception_source = repo_text(FATAL_EXCEPTION_PATH);
    let scheduler_source = repo_text(SCHEDULER_PATH);
    let aarch64_sources = aarch64_sources_with_exception(&exception_source);
    assert_eq!(
        validate_fatal_scheduler_accessor_census(
            &exception_source,
            &scheduler_source,
            &aarch64_sources,
        ),
        Ok(())
    );
}

#[test]
fn fatal_scheduler_census_m_a_rejects_blocking_call_at_fixed_site() {
    let exception_source = repo_text(FATAL_EXCEPTION_PATH);
    let scheduler_source = repo_text(SCHEDULER_PATH);
    let marker = exception_source
        .find("\"[FATAL_THREAD] tid=\"")
        .expect("M-A [FATAL_THREAD] marker");
    let call = exception_source[..marker]
        .rfind("current_thread_lock_free()")
        .expect("M-A fixed-site lock-free call");
    let mask = code_mask(&exception_source);
    let (statement_start, statement_end) =
        statement_bounds(&exception_source, &mask, call).expect("M-A enclosing statement");
    assert!(statement_start <= call && call < statement_end);

    let mut mutant = exception_source.clone();
    mutant.replace_range(
        call..call + "current_thread_lock_free()".len(),
        "crate::task::scheduler::current_thread_id()",
    );
    assert_ne!(
        mutant, exception_source,
        "M-A fixed [FATAL_THREAD] lock-free lookup mutation anchor"
    );
    let aarch64_sources = aarch64_sources_with_exception(&mutant);
    assert_eq!(
        validate_fatal_scheduler_accessor_census(&mutant, &scheduler_source, &aarch64_sources),
        Err("+ kernel/src/arch_impl/aarch64/exception.rs :: fn handle_sync_exception => current_thread_id  (1 occurrences, expected none)\n+ kernel/src/task/scheduler.rs :: fn current_thread_id => lock_scheduler  (1 occurrences, expected none)".to_string())
    );
}

#[test]
fn fatal_scheduler_census_m_b_rejects_blocking_call_in_direct_fatal_callee() {
    let exception_source = repo_text(FATAL_EXCEPTION_PATH);
    let scheduler_source = repo_text(SCHEDULER_PATH);
    assert!(
        function_body(&exception_source, "dump_el1_fatal_frame_and_dispatch_trace").is_some(),
        "M-B direct fatal-callee body anchor"
    );
    let mask = code_mask(&exception_source);
    let (_, open) = definition_offsets(
        &exception_source,
        &mask,
        "dump_el1_fatal_frame_and_dispatch_trace",
    )
    .into_iter()
    .next()
    .expect("M-B direct fatal-callee definition anchor");

    let mut mutant = exception_source.clone();
    mutant.insert_str(
        open + 1,
        "\n    let _ = crate::task::scheduler::current_thread_id();",
    );
    assert_ne!(
        mutant, exception_source,
        "M-B direct fatal-callee insertion mutation anchor"
    );
    let aarch64_sources = aarch64_sources_with_exception(&mutant);
    assert_eq!(
        validate_fatal_scheduler_accessor_census(&mutant, &scheduler_source, &aarch64_sources),
        Err("+ kernel/src/arch_impl/aarch64/exception.rs :: fn dump_el1_fatal_frame_and_dispatch_trace => current_thread_id  (1 occurrences, expected none)\n+ kernel/src/task/scheduler.rs :: fn current_thread_id => lock_scheduler  (1 occurrences, expected none)".to_string())
    );
}

#[test]
fn fatal_scheduler_census_m_c_rejects_broken_blocking_accessor_derivation() {
    let exception_source = repo_text(FATAL_EXCEPTION_PATH);
    let scheduler_source = repo_text(SCHEDULER_PATH);
    let mutant = scheduler_source.replace("lock_scheduler(", "renamed_lock_scheduler(");
    assert_ne!(
        mutant, scheduler_source,
        "M-C lock_scheduler derivation mutation anchor"
    );
    let aarch64_sources = aarch64_sources_with_exception(&exception_source);
    assert_eq!(
        validate_fatal_scheduler_accessor_census(&exception_source, &mutant, &aarch64_sources),
        Err("blocking scheduler accessor derivation is empty".to_string())
    );
}

#[test]
fn fatal_scheduler_census_m_e_rejects_blocking_call_in_transitive_callee() {
    let exception_source = repo_text(FATAL_EXCEPTION_PATH);
    let scheduler_source = repo_text(SCHEDULER_PATH);
    assert!(
        function_body(&scheduler_source, "dump_cpu_state_history_postmortem").is_some(),
        "M-E transitive fatal-callee body anchor"
    );
    let mask = code_mask(&scheduler_source);
    let (_, open) = definition_offsets(
        &scheduler_source,
        &mask,
        "dump_cpu_state_history_postmortem",
    )
    .into_iter()
    .next()
    .expect("M-E transitive fatal-callee definition anchor");

    let mut mutant = scheduler_source.clone();
    mutant.insert_str(
        open + 1,
        "\n    let _ = crate::task::scheduler::current_thread_id();",
    );
    assert_ne!(
        mutant, scheduler_source,
        "M-E transitive fatal-callee insertion mutation anchor"
    );
    let aarch64_sources = aarch64_sources_with_exception(&exception_source);
    assert_eq!(
        validate_fatal_scheduler_accessor_census(&exception_source, &mutant, &aarch64_sources),
        Err("+ kernel/src/task/scheduler.rs :: #[cfg(target_arch=aarch64)] fn dump_cpu_state_history_postmortem => current_thread_id  (1 occurrences, expected none)\n+ kernel/src/task/scheduler.rs :: fn current_thread_id => lock_scheduler  (1 occurrences, expected none)".to_string())
    );
}

#[test]
fn fatal_scheduler_census_m_f_rejects_broken_fatal_callee_derivation() {
    let exception_source = repo_text(FATAL_EXCEPTION_PATH);
    let scheduler_source = repo_text(SCHEDULER_PATH);
    let mask = code_mask(&exception_source);
    let blocks = lexical_blocks(&exception_source, &mask);
    let functions = source_functions(&exception_source, &mask, &blocks);
    let definition_name = functions
        .iter()
        .find(|function| function.name == "dump_el1_fatal_frame_and_dispatch_trace")
        .map(|function| function.name_offset)
        .expect("M-F fatal-callee definition anchor");
    let mut calls: Vec<_> = call_offsets(
        &exception_source,
        &mask,
        "dump_el1_fatal_frame_and_dispatch_trace",
    )
    .into_iter()
    .filter(|offset| *offset != definition_name)
    .collect();
    assert!(!calls.is_empty(), "M-F fatal-region call anchors");

    let mut mutant = exception_source.clone();
    calls.sort_unstable_by(|left, right| right.cmp(left));
    for offset in calls {
        mutant.replace_range(
            offset..offset + "dump_el1_fatal_frame_and_dispatch_trace".len(),
            "removed_el1_fatal_frame_and_dispatch_trace",
        );
    }
    assert_ne!(
        mutant, exception_source,
        "M-F fatal-callee derivation mutation anchor"
    );
    let aarch64_sources = aarch64_sources_with_exception(&mutant);
    assert_eq!(
        validate_fatal_scheduler_accessor_census(&mutant, &scheduler_source, &aarch64_sources),
        Err(
            "fatal-callee derivation missing anchors: dump_el1_fatal_frame_and_dispatch_trace"
                .to_string()
        )
    );
}
