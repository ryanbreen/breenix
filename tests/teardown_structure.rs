use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn rust_sources_below(relative: &str) -> Vec<(String, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).expect("read source directory") {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(root)
                    .expect("source below repository root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((
                    relative,
                    fs::read_to_string(path).expect("read Rust source"),
                ));
            }
        }
    }

    let root = repo_root();
    let mut sources = Vec::new();
    visit(&root, &root.join(relative), &mut sources);
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
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

fn with_replaced_source(
    sources: &[(String, String)],
    path: &str,
    replacement: String,
) -> Vec<(String, String)> {
    sources
        .iter()
        .map(|(candidate, contents)| {
            if candidate == path {
                (candidate.clone(), replacement.clone())
            } else {
                (candidate.clone(), contents.clone())
            }
        })
        .collect()
}

fn source<'a>(sources: &'a [(String, String)], path: &str) -> &'a str {
    &sources
        .iter()
        .find(|(candidate, _)| candidate == path)
        .unwrap_or_else(|| panic!("missing source {path}"))
        .1
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
                for hash in 1..=hashes {
                    mask[index + hash] = false;
                }
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
                index += 1;
                continue;
            }
        }
        index += 1;
    }
    mask
}

fn code_offsets(source: &str, mask: &[bool], needle: &str) -> Vec<usize> {
    source
        .match_indices(needle)
        .filter_map(|(offset, _)| mask.get(offset).copied().unwrap_or(false).then_some(offset))
        .collect()
}

fn identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric() || !byte.is_ascii()
}

fn identifier_offsets(source: &str, mask: &[bool], identifier: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    code_offsets(source, mask, identifier)
        .into_iter()
        .filter(|offset| {
            let end = *offset + identifier.len();
            !offset
                .checked_sub(1)
                .and_then(|before| bytes.get(before))
                .is_some_and(|byte| identifier_byte(*byte))
                && !bytes.get(end).is_some_and(|byte| identifier_byte(*byte))
        })
        .collect()
}

/// Offsets of identifiers whose final bytes are `suffix`. The identifier's
/// start may precede the suffix, but its end must coincide with the suffix end.
fn identifier_suffix_offsets(source: &str, mask: &[bool], suffix: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    code_offsets(source, mask, suffix)
        .into_iter()
        .filter(|offset| {
            let end = *offset + suffix.len();
            !bytes.get(end).is_some_and(|byte| identifier_byte(*byte))
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
                depth -= 1;
                if depth == 0 {
                    return Some(&source[start..index + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Code text with comments and string literals removed and whitespace
/// collapsed, so a structural pin compares what the compiler sees rather than
/// one formatting of it.
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

/// The statements inside a brace-matched block returned by `braced_block`.
fn block_statements(block: &str) -> Option<&str> {
    let mask = code_mask(block);
    let open = (0..block.len()).find(|index| mask[*index] && block.as_bytes()[*index] == b'{')?;
    let close = block.len().checked_sub(1)?;
    (block.as_bytes()[close] == b'}' && open < close).then(|| &block[open + 1..close])
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

/// The transitive callee closure of `roots` within one module.
///
/// R7's no-log/no-heap property must hold for everything `return_lease` reaches,
/// so the span is derived from the call graph rather than enumerated: a helper
/// added tomorrow is covered the day it is called.
fn transitively_called_functions(source: &str, roots: &[&str]) -> BTreeSet<String> {
    let bodies = module_function_bodies(source);
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut pending: Vec<String> = roots.iter().map(|name| (*name).to_owned()).collect();
    while let Some(name) = pending.pop() {
        if !reached.insert(name.clone()) {
            continue;
        }
        for body in bodies.get(&name).into_iter().flatten() {
            let body_mask = code_mask(body);
            let body_bytes = body.as_bytes();
            for callee in bodies.keys() {
                if reached.contains(callee) {
                    continue;
                }
                let called = identifier_offsets(body, &body_mask, callee)
                    .into_iter()
                    .any(|offset| {
                        let mut cursor = offset + callee.len();
                        while cursor < body_bytes.len()
                            && (!body_mask[cursor] || body_bytes[cursor].is_ascii_whitespace())
                        {
                            cursor += 1;
                        }
                        body_bytes.get(cursor) == Some(&b'(')
                    });
                if called {
                    pending.push(callee.clone());
                }
            }
        }
    }
    reached
}

/// Code offsets for calls whose balanced argument subtree contains `needle`.
/// Matching the final identifier covers both `drop(...)` and qualified forms
/// such as `core::mem::drop({ ... })` without depending on their spelling.
fn call_sites_with_argument(source: &str, callee: &str, needle: &str) -> Vec<usize> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut matches = Vec::new();
    for offset in identifier_offsets(source, &mask, callee) {
        let mut open = offset + callee.len();
        while open < bytes.len() && (!mask[open] || bytes[open].is_ascii_whitespace()) {
            open += 1;
        }
        if bytes.get(open) != Some(&b'(') {
            continue;
        }
        let mut depth = 0usize;
        for close in open..bytes.len() {
            if !mask[close] {
                continue;
            }
            match bytes[close] {
                b'(' => depth += 1,
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let argument = &source[open + 1..close];
                        let argument_mask = code_mask(argument);
                        if !code_offsets(argument, &argument_mask, needle).is_empty() {
                            matches.push(offset);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    matches
}

fn enclosing_test_def<'a>(source: &'a str, mask: &[bool], offset: usize) -> Option<&'a str> {
    let start = code_offsets(source, mask, "TestDef {")
        .into_iter()
        .filter(|start| *start <= offset)
        .next_back()?;
    let block = braced_block(source, mask, start)?;
    (offset < start + block.len()).then_some(block)
}

fn function_span(source: &str, name: &str) -> std::ops::Range<usize> {
    let body = function_body(source, name);
    let start = body.as_ptr() as usize - source.as_ptr() as usize;
    start..start + body.len()
}

fn pattern_binding_identifiers(pattern: &str) -> BTreeSet<String> {
    let mask = code_mask(pattern);
    let bytes = pattern.as_bytes();
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut pattern_end = pattern.len();
    for index in 0..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b'{' => brace_depth += 1,
            b'}' => brace_depth = brace_depth.saturating_sub(1),
            b':' if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && bytes.get(index.wrapping_sub(1)) != Some(&b':')
                && bytes.get(index + 1) != Some(&b':') =>
            {
                pattern_end = index;
                break;
            }
            _ => {}
        }
    }

    let pattern = &pattern[..pattern_end];
    let mask = &mask[..pattern_end];
    let bytes = pattern.as_bytes();
    let mut bindings = BTreeSet::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !mask[cursor] || !(bytes[cursor] == b'_' || bytes[cursor].is_ascii_alphabetic()) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        let candidate = &pattern[start..cursor];
        if matches!(
            candidate,
            "_" | "let"
                | "if"
                | "match"
                | "mut"
                | "ref"
                | "Some"
                | "None"
                | "Ok"
                | "Err"
                | "self"
                | "Self"
                | "super"
                | "crate"
                | "true"
                | "false"
        ) || candidate
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_uppercase)
        {
            continue;
        }

        let previous = (0..start)
            .rev()
            .find(|index| mask[*index] && !bytes[*index].is_ascii_whitespace());
        let next = (cursor..bytes.len())
            .find(|index| mask[*index] && !bytes[*index].is_ascii_whitespace());
        let path_segment = previous.is_some_and(|index| {
            bytes[index] == b':'
                && index
                    .checked_sub(1)
                    .is_some_and(|before| mask[before] && bytes[before] == b':')
        }) || next.is_some_and(|index| {
            bytes[index] == b':'
                && bytes
                    .get(index + 1)
                    .is_some_and(|byte| *byte == b':' && mask[index + 1])
        });
        let field_label = next.is_some_and(|index| {
            bytes[index] == b':'
                && bytes
                    .get(index + 1)
                    .is_none_or(|byte| *byte != b':' || !mask[index + 1])
        });
        if !path_segment && !field_label {
            bindings.insert(candidate.to_owned());
        }
    }
    bindings
}

/// The `]` closing the `[` at `open`, honouring nesting.
fn matching_bracket(bytes: &[u8], mask: &[bool], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for index in open..bytes.len() {
        if !mask.get(index).copied().unwrap_or(false) {
            continue;
        }
        match bytes[index] {
            b'[' => depth += 1,
            b']' => {
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

/// Blanks every index/slice suffix applied to `alias`, so an indexed element
/// reads as the alias itself.
///
/// `free_list[0]`, `free_list[..]` and `free_list[0..1]` are the same physical
/// free-list storage as `free_list`: exporting one of them
/// (`core::mem::replace(&mut free_list[0], frame)`) publishes a frame into the
/// reuse pool exactly as exporting the alias does. Blanking the subscript also
/// removes any alias occurrence *inside* the subscript, leaving the single
/// occurrence `direct_alias_expression` requires.
fn strip_index_suffixes(expression: &str, alias: &str) -> String {
    let mask = code_mask(expression);
    let bytes = expression.as_bytes();
    let mut stripped = bytes.to_vec();
    for offset in identifier_offsets(expression, &mask, alias) {
        let mut cursor = offset + alias.len();
        loop {
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            if bytes.get(cursor) != Some(&b'[') {
                break;
            }
            let Some(close) = matching_bracket(bytes, &mask, cursor) else {
                break;
            };
            for byte in &mut stripped[cursor..=close] {
                *byte = b' ';
            }
            cursor = close + 1;
        }
    }
    String::from_utf8(stripped).expect("blanked spans are whole bracket groups")
}

/// The identifier immediately left of `offset`, ignoring trivia.
fn preceding_identifier(source: &str, mask: &[bool], offset: usize) -> Option<String> {
    let bytes = source.as_bytes();
    let mut cursor = offset;
    loop {
        cursor = cursor.checked_sub(1)?;
        if mask[cursor] && !bytes[cursor].is_ascii_whitespace() {
            break;
        }
    }
    if !identifier_byte(bytes[cursor]) {
        return None;
    }
    let end = cursor + 1;
    while cursor > 0 && identifier_byte(bytes[cursor - 1]) {
        cursor -= 1;
    }
    Some(source[cursor..end].to_owned())
}

/// The `;` that ends the statement starting at `from`, traversing balanced
/// bracket groups so a block-valued initializer stays inside the statement.
fn statement_end(source: &str, mask: &[bool], from: usize) -> usize {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    for index in from..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth = depth.saturating_sub(1),
            b';' if depth == 0 => return index,
            _ => {}
        }
    }
    bytes.len()
}

fn direct_alias_expression(expression: &str, alias: &str) -> bool {
    let mask = code_mask(expression);
    let alias_offsets = identifier_offsets(expression, &mask, alias);
    if alias_offsets.len() != 1 {
        return false;
    }
    let alias_offset = alias_offsets[0];
    let mut cursor = 0usize;
    while cursor < expression.len() {
        if !mask[cursor] || expression.as_bytes()[cursor].is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        if cursor == alias_offset {
            cursor += alias.len();
            continue;
        }
        if ["mut", "ref"].iter().any(|modifier| {
            expression[cursor..].starts_with(modifier)
                && identifier_offsets(expression, &mask, modifier).contains(&cursor)
        }) {
            cursor += if expression[cursor..].starts_with("mut") {
                "mut".len()
            } else {
                "ref".len()
            };
            continue;
        }
        if matches!(expression.as_bytes()[cursor], b'&' | b'*' | b'(' | b')') {
            cursor += 1;
            continue;
        }
        return false;
    }
    true
}

fn expression_derives_alias(expression: &str, aliases: &BTreeSet<String>) -> bool {
    let mask = code_mask(expression);
    aliases.iter().any(|alias| {
        !identifier_offsets(expression, &mask, alias).is_empty()
            && (direct_alias_expression(expression, alias)
                || ["try_lock", ".lock", "&mut", "as_mut"]
                    .iter()
                    .any(|capability| !code_offsets(expression, &mask, capability).is_empty()))
    })
}

fn aliases_derived_from_free_frames(body: &str) -> BTreeSet<String> {
    let mask = code_mask(body);
    let bytes = body.as_bytes();
    let mut aliases = BTreeSet::from(["FREE_FRAMES".to_owned()]);
    let mut changed = true;
    while changed {
        changed = false;
        for let_offset in identifier_offsets(body, &mask, "let") {
            // `if let` / `while let` bind from a scrutinee that ends at the block
            // brace. A plain `let` may initialise from a block expression
            // (`let dest = if cond { &mut spare } else { &mut free_list };`),
            // whose braces belong to the initializer and must be traversed
            // rather than treated as the statement terminator.
            let binds_from_scrutinee = preceding_identifier(body, &mask, let_offset)
                .is_some_and(|keyword| keyword == "if" || keyword == "while");
            let end = if binds_from_scrutinee {
                (let_offset..bytes.len())
                    .find(|index| mask[*index] && matches!(bytes[*index], b';' | b'{'))
                    .unwrap_or(bytes.len())
            } else {
                statement_end(body, &mask, let_offset)
            };
            let Some(equals) =
                (let_offset..end).find(|index| mask[*index] && bytes[*index] == b'=')
            else {
                continue;
            };
            let rhs = &body[equals + 1..end];
            if !expression_derives_alias(rhs, &aliases) {
                continue;
            }
            let lhs = &body[let_offset + 3..equals];
            for binding in pattern_binding_identifiers(lhs) {
                changed |= aliases.insert(binding);
            }
        }
        for match_offset in identifier_offsets(body, &mask, "match") {
            let Some(open) =
                (match_offset..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{')
            else {
                continue;
            };
            let expression = &body[match_offset + "match".len()..open];
            if !expression_derives_alias(expression, &aliases) {
                continue;
            }

            let mut depth = 1usize;
            let close = ((open + 1)..bytes.len()).find(|index| {
                if !mask[*index] {
                    return false;
                }
                match bytes[*index] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                depth == 0
            });
            let Some(close) = close else {
                continue;
            };
            let arms = &body[open + 1..close];
            let arms_mask = code_mask(arms);
            for arrow in code_offsets(arms, &arms_mask, "=>") {
                let mut paren_depth = 0usize;
                let mut bracket_depth = 0usize;
                let mut brace_depth = 0usize;
                let mut pattern_start = 0usize;
                for index in 0..arrow {
                    if !arms_mask[index] {
                        continue;
                    }
                    match arms.as_bytes()[index] {
                        b'(' => paren_depth += 1,
                        b')' => paren_depth = paren_depth.saturating_sub(1),
                        b'[' => bracket_depth += 1,
                        b']' => bracket_depth = bracket_depth.saturating_sub(1),
                        b'{' => brace_depth += 1,
                        b'}' => brace_depth = brace_depth.saturating_sub(1),
                        b',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                            pattern_start = index + 1;
                        }
                        _ => {}
                    }
                }
                let pattern = &arms[pattern_start..arrow];
                for binding in pattern_binding_identifiers(pattern) {
                    changed |= aliases.insert(binding);
                }
            }
        }
    }
    aliases
}

fn alias_method_calls(body: &str) -> Vec<String> {
    fn skip_trivia(bytes: &[u8], mask: &[bool], cursor: &mut usize) {
        while bytes
            .get(*cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace())
            || mask.get(*cursor).is_some_and(|code| !*code)
        {
            *cursor += 1;
        }
    }

    let mask = code_mask(body);
    let bytes = body.as_bytes();
    let aliases = aliases_derived_from_free_frames(body);
    let mut calls = Vec::new();
    for alias in aliases {
        for offset in identifier_offsets(body, &mask, &alias) {
            let mut cursor = offset + alias.len();
            loop {
                skip_trivia(bytes, &mask, &mut cursor);
                // A closing paren, an index and a slice all leave the walk on the
                // same physical storage: `(free_list)`, `free_list[0]` and
                // `free_list[..]` are the alias, so a method called on any of them
                // is a method called on the alias.
                while mask.get(cursor).copied().unwrap_or(false)
                    && matches!(bytes.get(cursor), Some(&b')') | Some(&b'['))
                {
                    if bytes[cursor] == b')' {
                        cursor += 1;
                    } else if let Some(close) = matching_bracket(bytes, &mask, cursor) {
                        cursor = close + 1;
                    } else {
                        break;
                    }
                    skip_trivia(bytes, &mask, &mut cursor);
                }
                if bytes.get(cursor) != Some(&b'.') || !mask[cursor] {
                    break;
                }
                cursor += 1;
                skip_trivia(bytes, &mask, &mut cursor);
                let name_start = cursor;
                while bytes.get(cursor).is_some_and(|byte| identifier_byte(*byte)) {
                    cursor += 1;
                }
                if cursor == name_start {
                    break;
                }
                let name = body[name_start..cursor].to_owned();
                skip_trivia(bytes, &mask, &mut cursor);
                if bytes.get(cursor) != Some(&b'(') || !mask[cursor] {
                    break;
                }
                calls.push(name);
                let mut depth = 0usize;
                let mut close = None;
                for index in cursor..bytes.len() {
                    if !mask[index] {
                        continue;
                    }
                    match bytes[index] {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                close = Some(index);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let Some(argument_end) = close else {
                    break;
                };
                cursor = argument_end + 1;
            }
        }
    }
    calls
}

fn alias_argument_exports(body: &str) -> Vec<String> {
    fn matching_close(bytes: &[u8], mask: &[bool], open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for index in open..bytes.len() {
            if !mask[index] {
                continue;
            }
            match bytes[index] {
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

    fn call_name_before(body: &str, mask: &[bool], open: usize) -> Option<String> {
        let bytes = body.as_bytes();
        let mut cursor = open;
        while cursor > 0 {
            cursor -= 1;
            if mask[cursor] && !bytes[cursor].is_ascii_whitespace() {
                break;
            }
        }
        if bytes.get(cursor) == Some(&b'!') {
            cursor = cursor.checked_sub(1)?;
            while !mask[cursor] || bytes[cursor].is_ascii_whitespace() {
                cursor = cursor.checked_sub(1)?;
            }
        }
        if !identifier_byte(*bytes.get(cursor)?) {
            return None;
        }
        let end = cursor + 1;
        while cursor > 0 && identifier_byte(bytes[cursor - 1]) {
            cursor -= 1;
        }
        let name = &body[cursor..end];
        (!matches!(name, "if" | "while" | "for" | "match" | "return")).then(|| name.to_owned())
    }

    let mask = code_mask(body);
    let bytes = body.as_bytes();
    let aliases = aliases_derived_from_free_frames(body);
    let mut exports = Vec::new();
    for alias in aliases {
        for offset in identifier_offsets(body, &mask, &alias) {
            let mut opens = Vec::new();
            for open in code_offsets(&body[..offset], &mask[..offset], "(") {
                if matching_close(bytes, &mask, open).is_some_and(|close| close > offset) {
                    opens.push(open);
                }
            }
            for open in opens.into_iter().rev() {
                let Some(call_name) = call_name_before(body, &mask, open) else {
                    continue;
                };
                let close = matching_close(bytes, &mask, open).expect("enclosing call close");
                let mut after_close = close + 1;
                while bytes
                    .get(after_close)
                    .is_some_and(|byte| !mask[after_close] || byte.is_ascii_whitespace())
                {
                    after_close += 1;
                }
                if bytes.get(after_close) == Some(&b'=')
                    && bytes.get(after_close + 1) != Some(&b'=')
                {
                    continue;
                }
                let mut depth = 0usize;
                let mut argument_start = open + 1;
                let mut argument = None;
                for index in open + 1..=close {
                    if !mask[index] {
                        continue;
                    }
                    match bytes[index] {
                        b'(' | b'[' | b'{' => depth += 1,
                        b')' | b']' | b'}' if depth > 0 => depth -= 1,
                        b',' | b')' if depth == 0 => {
                            if (argument_start..index).contains(&offset) {
                                argument = Some(&body[argument_start..index]);
                                break;
                            }
                            argument_start = index + 1;
                        }
                        _ => {}
                    }
                }
                if argument.is_some_and(|argument| {
                    direct_alias_expression(&strip_index_suffixes(argument, &alias), &alias)
                }) {
                    exports.push(call_name);
                    break;
                }
            }
        }
    }
    exports
}

/// Assignments whose left-hand side is rooted at a free-list alias.
///
/// `alias_method_calls` and `alias_argument_exports` only see calls; an index
/// or deref store (`alias[i] = frame`, `*alias = other`) publishes a frame into
/// the reuse pool without any method and without any ledger transition, which
/// is precisely the physical-return bypass R1 forbids. Every such write is
/// reported by its left-hand side so the failure names the offending store.
fn alias_write_targets(body: &str) -> Vec<String> {
    let mask = code_mask(body);
    let bytes = body.as_bytes();
    let aliases = aliases_derived_from_free_frames(body);
    let mut writes = Vec::new();
    for index in 0..bytes.len() {
        if !mask[index] || bytes[index] != b'=' {
            continue;
        }
        // `==` and `=>` are not stores.
        if bytes
            .get(index + 1)
            .is_some_and(|byte| matches!(byte, b'=' | b'>') && mask[index + 1])
        {
            continue;
        }
        // `==`/`!=`/`<=`/`>=` are comparisons; `<<=`/`>>=` are stores.
        let previous = index.checked_sub(1).map(|before| bytes[before]);
        if matches!(previous, Some(b'=' | b'!' | b'<' | b'>')) {
            let two_back = index.checked_sub(2).map(|before| bytes[before]);
            let shift_assign = matches!(
                (previous, two_back),
                (Some(b'<'), Some(b'<')) | (Some(b'>'), Some(b'>'))
            );
            if !shift_assign {
                continue;
            }
        }
        let start = (0..index)
            .rev()
            .find(|offset| mask[*offset] && matches!(bytes[*offset], b';' | b'{' | b'}'))
            .map(|offset| offset + 1)
            .unwrap_or(0);
        let lhs = &body[start..index];
        let lhs_mask = &mask[start..index];
        // A `let` anywhere left of the `=` makes this a binding, not a store;
        // bindings are already followed by `aliases_derived_from_free_frames`.
        if !identifier_offsets(lhs, lhs_mask, "let").is_empty() {
            continue;
        }
        if aliases
            .iter()
            .any(|alias| !identifier_offsets(lhs, lhs_mask, alias).is_empty())
        {
            writes.push(lhs.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    writes
}

fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let plain_marker = format!("fn {name}(");
    let generic_marker = format!("fn {name}<");
    let mask = code_mask(source);
    let start = [
        code_offsets(source, &mask, &plain_marker)
            .into_iter()
            .next(),
        code_offsets(source, &mask, &generic_marker)
            .into_iter()
            .next(),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or_else(|| panic!("missing function {name}"));
    let bytes = source.as_bytes();
    let mut line_comment = false;
    let mut block_comment_depth = 0usize;
    let mut string = false;
    let mut character = false;
    let mut raw_string_hashes = None;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut open = None;
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];

        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
            index += 1;
            continue;
        }
        if block_comment_depth != 0 {
            if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
                block_comment_depth += 1;
                index += 2;
            } else if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                block_comment_depth -= 1;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(hashes) = raw_string_hashes {
            if byte == b'"'
                && bytes
                    .get(index + 1..index + 1 + hashes)
                    .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
            {
                raw_string_hashes = None;
                index += hashes + 1;
            }
            index += 1;
            continue;
        }
        if string || character {
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
            line_comment = true;
            index += 2;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
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
                raw_string_hashes = Some(quote - index - 1);
                index = quote + 1;
                continue;
            }
        }
        if byte == b'"' {
            string = true;
            index += 1;
            continue;
        }
        if byte == b'\'' {
            let plain_char = bytes.get(index + 2) == Some(&b'\'');
            let escaped_char =
                bytes.get(index + 1) == Some(&b'\\') && bytes.get(index + 3) == Some(&b'\'');
            if plain_char || escaped_char {
                character = true;
                index += 1;
                continue;
            }
        }

        match byte {
            b'{' => {
                open.get_or_insert(index);
                depth += 1;
            }
            b'}' if open.is_some() => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..index + 1];
                }
            }
            _ => {}
        }
        index += 1;
    }
    panic!("unterminated function {name}")
}

// ---------------------------------------------------------------------------
// Structural anchors
//
// A ratchet site is named by `(repo-relative path, canonical item path)` plus an
// occurrence count -- never by a source line, so reflowing or moving code inside
// a pinned function costs nothing while an added, removed or relocated site is
// still red. To re-verify that no line pin has crept back in, run from the repo
// root (expected output `0`):
//
//   tr '\n' ' ' < tests/teardown_structure.rs | grep -coE '\(&str, *usize[,)]|"(kernel|docker|docs|libs|xtask)/[^"]*" *, *[0-9]+|\.lines\(\) *\.(nth|enumerate)\(|\.filter\([^)]*b.\\n.[^)]*\) *\.count\(\)'
// The last alternative catches offset-to-line-number conversion by filtering
// newline bytes and counting them.
// ---------------------------------------------------------------------------

/// (repo-relative path, canonical item path).
type Anchor = (String, String);

/// Every anchor holding at least one match, with its match count.
type Census = BTreeMap<Anchor, usize>;

/// The first code byte at or after `from`, skipping trivia.
fn next_code(source: &str, mask: &[bool], from: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    (from..bytes.len()).find(|index| mask[*index] && !bytes[*index].is_ascii_whitespace())
}

/// The last code byte before `before`, skipping trivia.
fn previous_code(source: &str, mask: &[bool], before: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    (0..before)
        .rev()
        .find(|index| mask[*index] && !bytes[*index].is_ascii_whitespace())
}

/// Whether the token immediately before `offset`, modulo trivia, is `keyword`.
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

/// Whether the code text at `from`, modulo trivia, begins with `expected`.
fn code_follows(source: &str, mask: &[bool], from: usize, expected: &str) -> bool {
    let bytes = source.as_bytes();
    let mut cursor = from;
    for wanted in expected.bytes() {
        let Some(next) = next_code(source, mask, cursor) else {
            return false;
        };
        if bytes[next] != wanted {
            return false;
        }
        cursor = next + 1;
    }
    true
}

/// The `)` closing the `(` at `open`, honouring nesting.
fn matching_paren(source: &str, mask: &[bool], open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    for index in open..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
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

/// The `#[cfg(...)]` attributes decorating the item whose keyword sits at
/// `keyword`, in source order with whitespace and quotes removed, or `""` when
/// it has none. Keeping every attribute in the anchor makes `cfg`-split siblings
/// distinct items, so a call migrating between configurations is a key change.
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

/// The `impl` header text from its keyword to the body brace, comments removed,
/// whitespace collapsed and any `where` clause dropped.
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

/// How an item declared by `header` is named in an anchor: `fn NAME`,
/// `impl TYPE`, `impl TRAIT for TYPE`, `mod NAME` or `trait NAME`, prefixed by
/// its `#[cfg(...)]` attribute when it has one.
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

    // A `fn NAME` declaration wins over every other keyword in the same header,
    // so neither an `-> impl Iterator` return type nor a bare `fn()` pointer
    // parameter can rename the item.
    let declaration = identifier_offsets(header, mask, "fn")
        .into_iter()
        .filter_map(|offset| named(offset, "fn".len()).map(|name| (offset, format!("fn {name}"))))
        .next_back()
        .or_else(|| {
            ["impl", "mod", "trait"]
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

/// Every `fn` / `impl` / `mod` / `trait` block in the file, as
/// `(open brace offset, close brace offset, anchor segment)`.
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

/// Render each item span's complete path in one source-order sweep. Duplicate
/// renderings are marked so distinct items can never silently share an anchor.
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

/// The innermost item path containing `offset`. Empty at file top level.
fn item_path_at(spans: &[(usize, usize, String)], offset: usize) -> String {
    spans
        .iter()
        .filter(|(open, close, _)| *open <= offset && offset <= *close)
        .max_by_key(|(open, _, _)| *open)
        .map(|(_, _, path)| path.clone())
        .unwrap_or_default()
}

/// Every match of `matcher`, bucketed by enclosing item. A non-empty tag is
/// appended to the item path, so a matcher can put a payload (the abandon
/// reason) into the key instead of re-reading a pinned line.
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

/// Every match of `matcher`, bucketed by enclosing item.
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

/// Offsets of calls to `name`: the identifier followed, modulo trivia, by `(`.
/// The definition is excluded, so a validator never has to subtract it.
fn call_offsets(source: &str, mask: &[bool], name: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    identifier_offsets(source, mask, name)
        .into_iter()
        .filter(|offset| {
            next_code(source, mask, offset + name.len()).is_some_and(|open| bytes[open] == b'(')
                && !preceded_by_keyword(source, mask, *offset, "fn")
        })
        .collect()
}

/// Offsets of method calls `.name(`.
fn method_call_offsets(source: &str, mask: &[bool], name: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    identifier_offsets(source, mask, name)
        .into_iter()
        .filter(|offset| {
            next_code(source, mask, offset + name.len()).is_some_and(|open| bytes[open] == b'(')
                && previous_code(source, mask, *offset).is_some_and(|dot| {
                    bytes[dot] == b'.'
                        && !previous_code(source, mask, dot)
                            .is_some_and(|before| bytes[before] == b'.')
                })
        })
        .collect()
}

/// Assignment sites for one field identifier, excluding comparisons. This is
/// spelling-independent with respect to the receiver (`thread.field`,
/// `candidate.field`, and so on) and comment/string aware through `mask`.
fn assignment_offsets(source: &str, mask: &[bool], identifier: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    identifier_offsets(source, mask, identifier)
        .into_iter()
        .filter(|offset| {
            let Some(operator) = next_code(source, mask, offset + identifier.len()) else {
                return false;
            };
            if bytes[operator] == b'=' {
                return next_code(source, mask, operator + 1)
                    .is_none_or(|next| bytes[next] != b'=');
            }
            matches!(bytes[operator], b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')
                && next_code(source, mask, operator + 1)
                    .is_some_and(|equals| bytes[equals] == b'=')
        })
        .collect()
}

/// `(fn keyword offset, body brace offset)` for the definition whose name spans
/// `offset..end`, or `None` when that identifier is not a definition name or the
/// definition has no body. The body brace is found at paren/bracket depth zero,
/// so an array type in the signature (`[u64; 32]`) is not read as a bodyless
/// declaration.
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

/// `(fn keyword offset, body brace offset)` for every `fn NAME` definition that
/// has a body. A census anchors on the brace, so a definition is named by the
/// item path that already includes the definition itself.
fn definition_offsets(source: &str, mask: &[bool], name: &str) -> Vec<(usize, usize)> {
    identifier_offsets(source, mask, name)
        .into_iter()
        .filter_map(|offset| definition_span(source, mask, offset, offset + name.len()))
        .collect()
}

/// The same, for every definition whose name *begins* with `prefix`. A family
/// pinned by prefix stays sensitive to a newly named member such as
/// `block_current_probe`, which an exact-name list cannot see at all.
fn definition_prefix_offsets(source: &str, mask: &[bool], prefix: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    code_offsets(source, mask, prefix)
        .into_iter()
        .filter(|offset| {
            !offset
                .checked_sub(1)
                .and_then(|before| bytes.get(before))
                .is_some_and(|byte| identifier_byte(*byte))
        })
        .filter_map(|offset| {
            let mut end = offset + prefix.len();
            while bytes.get(end).is_some_and(|byte| identifier_byte(*byte)) {
                end += 1;
            }
            definition_span(source, mask, offset, end)
        })
        .collect()
}

fn expected_census(anchors: &[(&str, &str, usize)]) -> Census {
    let mut census = Census::new();
    for (path, item, count) in anchors {
        let anchor = ((*path).to_owned(), (*item).to_owned());
        assert!(
            census.insert(anchor, *count).is_none(),
            "duplicate census anchor {path} :: {item}; the two entries would otherwise sum into one allowance"
        );
    }
    census
}

/// Every divergence between the observed and the pinned census, never just the
/// first: a legitimate kernel change is re-anchored in one pass.
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

/// Accumulate a census-carrying outcome under `label` instead of failing on the
/// first divergence, so one run reports every anchor that needs re-pinning.
fn record(failures: &mut Vec<String>, label: &str, outcome: Result<(), Vec<String>>) {
    if let Err(details) = outcome {
        failures.push(label.to_owned());
        failures.extend(details.into_iter().map(|detail| format!("    {detail}")));
    }
}

/// Accumulate an outcome that carries no diff detail of its own.
fn record_unit(failures: &mut Vec<String>, label: &str, outcome: Result<(), ()>) {
    if outcome.is_err() {
        failures.push(label.to_owned());
    }
}

/// Accumulate a boolean structural condition under `label`.
fn check(failures: &mut Vec<String>, label: &str, holds: bool) {
    if !holds {
        failures.push(label.to_owned());
    }
}

#[test]
fn item_path_is_cfg_and_impl_scoped() {
    let fixture = r##"
#[cfg(target_arch = "x86_64")]
fn split() { needle(); }
#[cfg(target_arch = "aarch64")]
fn split() { needle(); needle(); }
#[cfg(feature = "boot_tests")]
#[cfg(target_arch = "aarch64")]
fn stacked() { needle(); }
impl Drop for Guard { fn drop(&mut self) { needle(); } }
impl Guard { fn drop_all(&mut self) { fn inner() { needle(); } } }
fn plain() { let _s = "needle()"; /* needle() */ }
"##;
    let sources = vec![("fixture.rs".to_owned(), fixture.to_owned())];
    let actual = census(&sources, |source, mask| {
        code_offsets(source, mask, "needle()")
    });

    assert_eq!(
        actual.get(&(
            "fixture.rs".to_owned(),
            "#[cfg(target_arch=x86_64)] fn split".to_owned(),
        )),
        Some(&1)
    );
    assert_eq!(
        actual.get(&(
            "fixture.rs".to_owned(),
            "#[cfg(target_arch=aarch64)] fn split".to_owned(),
        )),
        Some(&2)
    );
    assert_eq!(
        actual.get(&(
            "fixture.rs".to_owned(),
            "#[cfg(feature=boot_tests)] #[cfg(target_arch=aarch64)] fn stacked".to_owned(),
        )),
        Some(&1)
    );
    assert_eq!(
        actual.get(&(
            "fixture.rs".to_owned(),
            "impl Drop for Guard::fn drop".to_owned(),
        )),
        Some(&1)
    );
    assert_eq!(
        actual.get(&(
            "fixture.rs".to_owned(),
            "impl Guard::fn drop_all::fn inner".to_owned(),
        )),
        Some(&1)
    );
    assert!(!actual.contains_key(&("fixture.rs".to_owned(), "fn plain".to_owned())));
    let split_entries = actual
        .iter()
        .filter(|((_, item), _)| item.ends_with("fn split"))
        .collect::<Vec<_>>();
    assert_eq!(split_entries.len(), 2);
    assert_eq!(actual.len(), 5);

    let colliding_fixture = r#"
#[cfg(feature = "same")]
fn duplicate() { needle(); }
#[cfg(feature = "same")]
fn duplicate() { needle(); }
"#;
    let colliding_sources = vec![("collision.rs".to_owned(), colliding_fixture.to_owned())];
    let colliding = census(&colliding_sources, |source, mask| {
        code_offsets(source, mask, "needle()")
    });
    assert_eq!(
        colliding.get(&(
            "collision.rs".to_owned(),
            "#[cfg(feature=same)] fn duplicate [duplicate item path]".to_owned(),
        )),
        Some(&2)
    );
    assert!(!colliding.contains_key(&(
        "collision.rs".to_owned(),
        "#[cfg(feature=same)] fn duplicate".to_owned(),
    )));

    let array_signature = "pub fn block_current(saved: [u64; 32]) -> [u8; 4] { [0; 4] }\ntrait Requirement { fn block_current(); }";
    let array_sources = vec![("array.rs".to_owned(), array_signature.to_owned())];
    let definitions = census(&array_sources, |source, mask| {
        definition_offsets(source, mask, "block_current")
            .into_iter()
            .map(|(_, brace)| brace)
            .collect()
    });
    assert_eq!(
        definitions.get(&("array.rs".to_owned(), "fn block_current".to_owned())),
        Some(&1)
    );

    let family_signature = "pub fn block_current(saved: [u64; 32]) {}\npub fn block_current_probe(saved: [u64; 32]) {}\npub fn unblock_current() {}";
    let family_sources = vec![("family.rs".to_owned(), family_signature.to_owned())];
    let family = census(&family_sources, |source, mask| {
        definition_prefix_offsets(source, mask, "block_current")
            .into_iter()
            .map(|(_, brace)| brace)
            .collect()
    });
    assert_eq!(
        family.get(&("family.rs".to_owned(), "fn block_current".to_owned())),
        Some(&1)
    );
    assert_eq!(
        family.get(&("family.rs".to_owned(), "fn block_current_probe".to_owned())),
        Some(&1)
    );
    assert_eq!(family.len(), 2);

    let scheduler_locks = "SCHEDULER.lock(); GLOBAL_SCHEDULER.try_lock(); SCHEDULERX.lock();";
    let scheduler_mask = code_mask(scheduler_locks);
    let raw_locks = identifier_suffix_offsets(scheduler_locks, &scheduler_mask, "SCHEDULER")
        .into_iter()
        .filter(|offset| {
            code_follows(
                scheduler_locks,
                &scheduler_mask,
                offset + "SCHEDULER".len(),
                ".lock()",
            ) || code_follows(
                scheduler_locks,
                &scheduler_mask,
                offset + "SCHEDULER".len(),
                ".try_lock()",
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        raw_locks,
        vec![
            scheduler_locks
                .find("SCHEDULER.lock()")
                .expect("SCHEDULER.lock() occurrence"),
            scheduler_locks
                .rfind("SCHEDULER.try_lock()")
                .expect("GLOBAL_SCHEDULER.try_lock() occurrence"),
        ]
    );
}

#[test]
fn function_body_is_lexically_scoped_and_exactly_named() {
    let fixture = r##"
fn target_helper() { panic!("}") }
fn target() {
    let _ordinary = "}";
    let _raw = r#"}"#;
    let _character = '}';
    // }
    /* } */
    if true { }
}
fn later() {}
"##;
    let body = function_body(fixture, "target");
    assert!(body.starts_with("fn target()"));
    assert!(body.contains("if true { }"));
    assert!(!body.contains("target_helper"));
    assert!(!body.contains("fn later"));
}

#[test]
fn code_mask_reports_only_real_code_occurrences() {
    let fixture = r###"
let _string = "needle";
// needle
/* needle */
let _raw = r#"needle"#;
let _same_line = "needle"; let _real = needle();
"###;
    let mask = code_mask(fixture);
    assert_eq!(
        code_offsets(fixture, &mask, "needle"),
        vec![fixture.rfind("needle();").expect("real code occurrence")]
    );

    let aliases = aliases_derived_from_free_frames(
        "if let Some(mut first) = FREE_FRAMES.try_lock() { let second = &mut *first; second.insert(0, frame); }",
    );
    assert!(aliases.contains("first"));
    assert!(aliases.contains("second"));
    let typed_aliases = aliases_derived_from_free_frames(
        "if let Some(mut first) = FREE_FRAMES.try_lock() { let list: &mut Vec<PhysFrame> = &mut first; list.insert(0, frame); }",
    );
    assert!(typed_aliases.contains("list"));
    assert!(!typed_aliases.contains("PhysFrame"));
    let destructured_aliases = aliases_derived_from_free_frames(
        "if let Some(mut first) = FREE_FRAMES.try_lock() { let (list, _): (&mut Vec<PhysFrame>, ()) = (&mut first, ()); list.insert(0, frame); }",
    );
    assert!(destructured_aliases.contains("list"));
    assert!(alias_method_calls(
        "if let Some(mut first) = FREE_FRAMES.try_lock() { let second = &mut *first; second.insert(0, frame); }"
    )
    .contains(&"insert".to_owned()));
    assert!(alias_method_calls(
        "match FREE_FRAMES.try_lock() { Some(mut renamed) => renamed.insert(0, frame), None => {} }"
    )
    .contains(&"insert".to_owned()));
    assert!(alias_method_calls(
        "if let Some(mut free) = FREE_FRAMES.try_lock() { (*free).insert(0, frame); }"
    )
    .contains(&"insert".to_owned()));
    assert_eq!(
        alias_argument_exports(
            "if let Some(mut free) = FREE_FRAMES.try_lock() { stash_frame(&mut free, frame); }"
        ),
        vec!["stash_frame"]
    );
    assert_eq!(
        alias_argument_exports(
            "if let Some(mut free) = FREE_FRAMES.try_lock() { Vec::insert(&mut free, 0, frame); }"
        ),
        vec!["insert"]
    );
    assert_eq!(
        alias_write_targets("if let Some(mut free) = FREE_FRAMES.try_lock() { free[0] = frame; }"),
        vec!["free[0]"]
    );
    assert_eq!(
        alias_write_targets(
            "if let Some(mut free) = FREE_FRAMES.try_lock() { *free = alloc::vec![frame]; }"
        ),
        vec!["*free"]
    );
}

#[rustfmt::skip]
const TERMINATE_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/interrupts/context_switch.rs", "fn restore_userspace_thread_context", 1),
    ("kernel/src/process/manager.rs", "impl ProcessManager::fn exit_process_locked", 1),
    ("kernel/src/signal/delivery.rs", "fn deliver_default_action", 2),
];
#[rustfmt::skip]
const TERMINATE_MINIMAL_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/task/process_task.rs", "impl ProcessScheduler::fn handle_thread_exit", 1),
];
#[rustfmt::skip]
const PRODUCTION_INIT_PID_SITES: &[(&str, &str, usize)] = &[
    ("kernel/src/process/manager.rs", "impl ProcessManager::fn exit_process_locked", 1),
    ("kernel/src/task/process_task.rs", "impl ProcessScheduler::fn handle_thread_exit", 2),
];
#[rustfmt::skip]
const TEST_INIT_PID_SITES: &[(&str, &str, usize)] = &[
    ("kernel/src/test_userspace.rs", "fn test_minimal_userspace", 3),
];
#[rustfmt::skip]
const QUARANTINE_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/exception.rs", "fn handle_sync_exception", 4),
    ("kernel/src/syscall/signal.rs", "fn send_signal_to_process", 1),
];
#[rustfmt::skip]
const KERNEL_STACK_MUTATIONS: &[(&str, &str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/syscall_entry.rs", "fn sys_fork_aarch64", 1),
    ("kernel/src/process/manager.rs", "impl ProcessManager::#[cfg(target_arch=aarch64)] fn complete_fork_aarch64", 1),
    ("kernel/src/syscall/clone.rs", "fn sys_clone", 1),
];
#[rustfmt::skip]
const RECLAIM_ENQUEUE_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/process/mod.rs", "fn exit_process_and_retire", 1),
    ("kernel/src/process/mod.rs", "impl Drop for RetirementReceipt::fn drop", 1),
    ("kernel/src/task/process_task.rs", "impl ProcessScheduler::fn handle_thread_exit", 1),
    ("kernel/src/task/process_task.rs", "#[cfg(feature=boot_tests)] fn reclaim_progress_gate_test", 2),
    ("kernel/src/tracing/providers/teardown.rs", "#[cfg(feature=boot_tests)] fn fork_exit_defer_reclaim_pairing_test", 1),
    ("kernel/src/tracing/providers/teardown.rs", "#[cfg(all(feature=boot_tests,target_arch=x86_64))] fn exec_supersede_cohort_test", 1),
];
#[rustfmt::skip]
const EXIT_PROCESS_AND_RETIRE_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/exception.rs", "fn handle_sync_exception", 4),
    ("kernel/src/interrupts.rs", "fn general_protection_fault_handler", 1),
    ("kernel/src/interrupts.rs", "fn page_fault_handler", 1),
    ("kernel/src/process/mod.rs", "fn exit_process_by_pid", 1),
    ("kernel/src/syscall/signal.rs", "fn send_signal_to_process", 1),
];
#[rustfmt::skip]
const EXIT_PROCESS_LOCKED_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/process/mod.rs", "fn exit_process_and_retire", 1),
];
#[rustfmt::skip]
const EXIT_PROCESS_BY_PID_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/process/mod.rs", "#[cfg(feature=boot_tests)] fn exit_process_for_teardown_test", 1),
    ("kernel/src/process/mod.rs", "fn exit_current", 1),
];
#[rustfmt::skip]
const EXIT_PROCESS_FOR_TEARDOWN_TEST_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/tracing/providers/teardown.rs", "#[cfg(feature=boot_tests)] fn fork_exit_defer_reclaim_pairing_test", 1),
    ("kernel/src/tracing/providers/teardown.rs", "#[cfg(all(feature=boot_tests,target_arch=x86_64))] fn exec_supersede_cohort_test", 1),
];
#[rustfmt::skip]
const BLOCKING_PRIMITIVES: &[(&str, &str, usize)] = &[
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current_for_child_exit", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current_for_compositor", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current_for_io", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current_for_io_with_timeout", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current_for_signal", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current_for_signal_with_context", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current_for_timer", 1),
    ("kernel/src/task/waitqueue.rs", "impl WaitQueueHead::fn prepare_to_wait", 1),
];
#[rustfmt::skip]
const RAW_SCHEDULER_LOCK_SITES: &[(&str, &str, usize)] = &[
    ("kernel/src/task/scheduler.rs", "fn lock_scheduler", 1),
    ("kernel/src/task/scheduler.rs", "fn try_lock_scheduler", 1),
];
#[rustfmt::skip]
const PROCESS_MEMORY_FRAME_RETURNS: &[(&str, &str, usize)] = &[
    ("kernel/src/memory/process_memory.rs", "#[cfg(feature=boot_tests)] fn page_table_custody_disposition_gate_test", 2),
];
#[rustfmt::skip]
const RETURN_LEASE_DEFINITION: &[(&str, &str, usize)] = &[
    ("kernel/src/memory/frame_allocator.rs", "fn return_lease", 1),
];
#[rustfmt::skip]
const RETURN_LEASE_PRODUCTION_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/memory/frame_allocator.rs", "fn deallocate_frame", 1),
    ("kernel/src/memory/frame_allocator.rs", "fn deallocate_leaf_frame", 1),
    ("kernel/src/memory/process_memory.rs", "impl ProcessPageTable::fn retire_bounded", 2),
];
#[rustfmt::skip]
const RETURN_LEASE_BOOT_FIXTURE_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/memory/frame_allocator_tests.rs", "fn frame_custody_refusal_gate_test", 6),
    ("kernel/src/memory/frame_allocator_tests.rs", "fn healthy_round_trip", 1),
    ("kernel/src/memory/frame_allocator_tests.rs", "fn restore_lease", 1),
    ("kernel/src/memory/frame_allocator_tests.rs", "fn stale_lease_fixture", 1),
];
#[rustfmt::skip]
const TABLE_RECORDER_SITES: &[(&str, &str, usize)] = &[
    ("kernel/src/memory/process_memory.rs", "impl ProcessPageTable::fn map_page", 1),
    ("kernel/src/memory/process_memory.rs", "impl ProcessPageTable::fn update_page_flags", 1),
];
#[rustfmt::skip]
const PROCESS_PAGE_TABLE_ABANDON_SITES: &[(&str, &str, usize)] = &[
    ("kernel/src/process/manager.rs", "impl ProcessManager::fn exit_process_locked => AbandonReason::AlreadyTerminated", 1),
    ("kernel/src/task/process_task.rs", "fn abandon_unqueued_reclaim => AbandonReason::NoArchPipeline", 1),
    ("kernel/src/task/process_task.rs", "fn abandon_unqueued_reclaim => AbandonReason::NoProofPipeline", 1),
    ("kernel/src/task/process_task.rs", "#[cfg(any(target_arch=aarch64,feature=boot_tests))] fn release_process_resources => AbandonReason::NoProofPipeline", 1),
    ("kernel/src/task/process_task.rs", "impl ProcessScheduler::fn handle_thread_exit => AbandonReason::AlreadyTerminated", 1),
];
#[rustfmt::skip]
const PROCESS_PAGE_TABLE_RETIRE_SITES: &[(&str, &str, usize)] = &[
    ("kernel/src/memory/frame_allocator_tests.rs", "fn retire_with_free_list_contended", 1),
    ("kernel/src/memory/process_memory.rs", "#[cfg(target_arch=aarch64)] impl Drop for UnpublishedPageTable::fn drop", 1),
    ("kernel/src/memory/process_memory.rs", "#[cfg(feature=boot_tests)] fn page_table_custody_disposition_gate_test", 1),
    ("kernel/src/memory/process_memory.rs", "impl ProcessPageTable::fn cleanup_for_exec", 1),
    ("kernel/src/task/process_task.rs", "#[cfg(feature=boot_tests)] fn reclaim_progress_gate_test", 4),
    ("kernel/src/task/process_task.rs", "impl PendingProcessReclaim::fn reclaim_bounded", 1),
];
#[rustfmt::skip]
const PENDING_RECLAIM_BOUNDED_SITES: &[(&str, &str, usize)] = &[
    ("kernel/src/task/process_task.rs", "fn reclaim_deferred_process_resources_for_pass", 1),
];
#[rustfmt::skip]
const FRAME_LEDGER_INIT_CALLS: &[(&str, &str, usize)] = &[
    ("kernel/src/main_aarch64.rs", "#[cfg(target_arch=aarch64)] fn kernel_main", 1),
    ("kernel/src/memory/mod.rs", "fn init", 1),
];
#[rustfmt::skip]
const PROCESS_PAGE_TABLE_CONSTRUCTORS: &[(&str, &str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/syscall_entry.rs", "fn sys_fork_aarch64", 1),
    ("kernel/src/memory/process_memory.rs", "#[cfg(feature=boot_tests)] fn page_table_custody_disposition_gate_test", 5),
    ("kernel/src/process/manager.rs", "impl ProcessManager::#[cfg(target_arch=aarch64)] fn create_process", 1),
    ("kernel/src/process/manager.rs", "impl ProcessManager::#[cfg(target_arch=aarch64)] fn create_process_with_argv", 1),
    ("kernel/src/process/manager.rs", "impl ProcessManager::#[cfg(target_arch=aarch64)] fn exec_process", 1),
    ("kernel/src/process/manager.rs", "impl ProcessManager::#[cfg(target_arch=aarch64)] fn exec_process_with_argv", 1),
    ("kernel/src/process/manager.rs", "impl ProcessManager::#[cfg(target_arch=x86_64)] fn create_process", 1),
    ("kernel/src/process/manager.rs", "impl ProcessManager::#[cfg(target_arch=x86_64)] fn exec_process", 1),
    ("kernel/src/process/manager.rs", "impl ProcessManager::#[cfg(target_arch=x86_64)] fn exec_process_with_argv", 1),
    ("kernel/src/process/manager.rs", "impl ProcessManager::#[cfg(target_arch=x86_64)] fn fork_process_with_context", 1),
    ("kernel/src/syscall/handlers.rs", "#[cfg(target_arch=x86_64)] fn sys_fork_with_parent_context", 1),
    ("kernel/src/task/process_task.rs", "#[cfg(all(feature=boot_tests,target_arch=x86_64))] fn boot_page_table_reclaim", 1),
    ("kernel/src/task/process_task.rs", "#[cfg(feature=boot_tests)] fn boot_oversized_page_table", 1),
    ("kernel/src/task/process_task.rs", "#[cfg(feature=boot_tests)] fn reclaim_progress_gate_test", 3),
    ("kernel/src/tracing/providers/teardown.rs", "#[cfg(feature=boot_tests)] fn fork_exit_defer_reclaim_pairing_test", 4),
    ("kernel/src/tracing/providers/teardown.rs", "#[cfg(all(feature=boot_tests,target_arch=x86_64))] fn exec_supersede_cohort_test", 3),
];
#[rustfmt::skip]
const DEFERRED_RECLAIM_DRAIN_SITES: &[(&str, &str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/context_switch.rs", "fn schedule_from_kernel", 1),
    ("kernel/src/arch_impl/aarch64/syscall_entry.rs", "fn sys_fork_aarch64", 1),
    ("kernel/src/interrupts/context_switch.rs", "fn idle_loop", 1),
    ("kernel/src/process/mod.rs", "fn exit_process_and_retire", 1),
];
#[rustfmt::skip]
const THREAD_GROUP_WRITES: &[(&str, &str, usize)] = &[
    ("kernel/src/syscall/clone.rs", "fn sys_clone", 1),
];
#[rustfmt::skip]
const BTRT_PROCESS_EXIT_REPORTS: &[(&str, &str, usize)] = &[
    ("kernel/src/task/process_task.rs", "impl ProcessScheduler::fn handle_thread_exit", 1),
];
#[rustfmt::skip]
const ROW_REMOVAL_EPOCH_BUMPS: &[(&str, &str, usize)] = &[
    ("kernel/src/process/manager.rs", "impl ProcessManager::fn remove_process", 1),
];

/// The blocking-primitive families, pinned by name *prefix* so that a tenth
/// primitive is caught however it is named: an exact-name list only ever sees
/// the nine that already exist, so `block_current_probe` would be invisible.
/// The nine current definitions are still pinned individually by
/// `BLOCKING_PRIMITIVES`.
const BLOCKING_NAME_PREFIXES: &[&str] = &["block_current", "prepare_to_wait"];

fn validate_reclaim_enqueue_callers(
    sources: &[(String, String)],
) -> Result<(), Vec<String>> {
    validate_census(
        &census(sources, |source, mask| {
            call_offsets(source, mask, "enqueue_process_reclaim")
        }),
        RECLAIM_ENQUEUE_CALLS,
    )
}

fn validate_exit_process_entry_points(
    sources: &[(String, String)],
) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    record(
        &mut failures,
        "exit_process_and_retire callers",
        validate_census(
            &census(sources, |source, mask| {
                call_offsets(source, mask, "exit_process_and_retire")
            }),
            EXIT_PROCESS_AND_RETIRE_CALLS,
        ),
    );
    record(
        &mut failures,
        "exit_process_locked callers",
        validate_census(
            &census(sources, |source, mask| {
                method_call_offsets(source, mask, "exit_process_locked")
            }),
            EXIT_PROCESS_LOCKED_CALLS,
        ),
    );
    if !census(sources, |source, mask| {
        method_call_offsets(source, mask, "exit_process")
    })
    .is_empty()
    {
        failures.push("unexpected exit_process method caller".to_owned());
    }
    record(
        &mut failures,
        "exit_process_by_pid callers",
        validate_census(
            &census(sources, |source, mask| {
                call_offsets(source, mask, "exit_process_by_pid")
            }),
            EXIT_PROCESS_BY_PID_CALLS,
        ),
    );
    record(
        &mut failures,
        "exit_process_for_teardown_test callers",
        validate_census(
            &census(sources, |source, mask| {
                call_offsets(source, mask, "exit_process_for_teardown_test")
            }),
            EXIT_PROCESS_FOR_TEARDOWN_TEST_CALLS,
        ),
    );
    failures.is_empty().then_some(()).ok_or(failures)
}

fn validate_blocking_primitives(sources: &[(String, String)]) -> Result<(), Vec<String>> {
    validate_census(
        &census(sources, |source, mask| {
            BLOCKING_NAME_PREFIXES
                .iter()
                .flat_map(|prefix| {
                    definition_prefix_offsets(source, mask, prefix)
                        .into_iter()
                        .filter_map(|(keyword, brace)| {
                            preceded_by_keyword(source, mask, keyword, "pub").then_some(brace)
                        })
                })
                .collect()
        }),
        BLOCKING_PRIMITIVES,
    )
}

fn validate_group_writes(sources: &[(String, String)]) -> Result<(), Vec<String>> {
    validate_census(
        &census(sources, |source, mask| {
            code_offsets(source, mask, "thread_group_id = Some(")
        }),
        THREAD_GROUP_WRITES,
    )
}

fn validate_exit_sgi_is_teardown_only(sources: &[(String, String)]) -> Result<(), ()> {
    let scheduler = source(sources, "kernel/src/task/scheduler.rs");
    (scheduler.contains("fn send_exit_expedite_sgi(")
        && scheduler.contains("fn send_resched_ipi(")
        && scheduler.contains("fn send_resched_ipi_to_cpu(")
        && function_body(scheduler, "send_exit_expedite_sgi").contains("EXIT_SGI_SENT")
        && !function_body(scheduler, "send_resched_ipi").contains("EXIT_SGI_SENT")
        && !function_body(scheduler, "send_resched_ipi_to_cpu").contains("EXIT_SGI_SENT"))
    .then_some(())
    .ok_or(())
}

fn validate_alias_methods(
    body: &str,
    allowed: &[&str],
    allowed_exports: &[&str],
) -> Result<(), ()> {
    for target in alias_write_targets(body) {
        eprintln!("free-list alias write target: {target}");
        return Err(());
    }
    for method in alias_method_calls(body) {
        if !allowed.contains(&method.as_str()) {
            eprintln!("unexpected free-list alias method: {method}");
            return Err(());
        }
    }
    for callee in alias_argument_exports(body) {
        if !allowed_exports.contains(&callee.as_str()) {
            eprintln!("unexpected free-list alias export to: {callee}");
            return Err(());
        }
    }
    Ok(())
}

fn validate_free_frame_capabilities(
    path: &str,
    module: &str,
    allowed_functions: &[&str],
) -> Result<(), ()> {
    let mask = code_mask(module);
    let spans = allowed_functions
        .iter()
        .map(|name| function_span(module, name))
        .collect::<Vec<_>>();
    let declaration = (path == "kernel/src/memory/frame_allocator.rs")
        .then(|| module.find("static FREE_FRAMES:"))
        .flatten();
    for offset in identifier_offsets(module, &mask, "FREE_FRAMES") {
        if declaration == Some(offset.saturating_sub("static ".len())) {
            continue;
        }
        if !spans.iter().any(|span| span.contains(&offset)) {
            eprintln!("FREE_FRAMES capability outside allowed span in {path} at byte {offset}");
            return Err(());
        }
    }
    Ok(())
}

fn validate_frame_return_choke_point(
    sources: &[(String, String)],
) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();

    let mut process_memory_returns = census(sources, |source, mask| {
        call_offsets(source, mask, "deallocate_frame")
    });
    process_memory_returns
        .retain(|(path, _), _| path == "kernel/src/memory/process_memory.rs");
    record(
        &mut failures,
        "process-memory frame returns",
        validate_census(&process_memory_returns, PROCESS_MEMORY_FRAME_RETURNS),
    );

    let definitions = census(sources, |source, mask| {
        definition_offsets(source, mask, "return_lease")
            .into_iter()
            .map(|(_, brace)| brace)
            .collect()
    });
    record(
        &mut failures,
        "return_lease definition",
        validate_census(&definitions, RETURN_LEASE_DEFINITION),
    );

    let return_lease_calls = census(sources, |source, mask| {
        call_offsets(source, mask, "return_lease")
    });
    let mut allowed_return_lease_calls = RETURN_LEASE_PRODUCTION_CALLS.to_vec();
    allowed_return_lease_calls.extend_from_slice(RETURN_LEASE_BOOT_FIXTURE_CALLS);
    record(
        &mut failures,
        "return_lease callers",
        validate_census(&return_lease_calls, &allowed_return_lease_calls),
    );

    let allocator = source(sources, "kernel/src/memory/frame_allocator.rs");
    let init = function_body(allocator, "init_frame_ledger");
    record_unit(
        &mut failures,
        "allocator FREE_FRAMES capability escaped",
        validate_free_frame_capabilities(
            "kernel/src/memory/frame_allocator.rs",
            allocator,
            &[
                "init_frame_ledger",
                "ensure_free_frame_capacity",
                "allocate_candidate",
                "return_lease",
                "memory_stats",
            ],
        ),
    );
    record_unit(
        &mut failures,
        "init_frame_ledger free-list alias methods changed",
        validate_alias_methods(
            init,
            &[
                "lock",
                "capacity",
                "len",
                "try_reserve",
                "expect",
                "push",
                "swap_remove",
                "iter",
                "copied",
            ],
            &[],
        ),
    );
    let init_methods = alias_method_calls(init);
    if init_methods.iter().filter(|method| *method == "push").count() != 1
        || !init.contains(
            "if seed_free_frame(&ledger, frame) {\n                free_list.push(frame);\n            }",
        )
    {
        eprintln!("bootstrap free-list insertion escaped its seeded-frame span");
        failures.push("bootstrap free-list insertion escaped its seeded-frame span".to_owned());
    }
    record_unit(
        &mut failures,
        "ensure_free_frame_capacity alias methods changed",
        validate_alias_methods(
            function_body(allocator, "ensure_free_frame_capacity"),
            &["try_lock", "capacity", "len", "try_reserve", "is_err"],
            &[],
        ),
    );
    record_unit(
        &mut failures,
        "allocate_candidate alias methods changed",
        validate_alias_methods(
            function_body(allocator, "allocate_candidate"),
            &["try_lock", "pop", "len"],
            &[],
        ),
    );
    record_unit(
        &mut failures,
        "return_lease alias methods changed",
        validate_alias_methods(
            function_body(allocator, "return_lease"),
            &["try_lock", "len", "capacity", "push"],
            &[],
        ),
    );
    record_unit(
        &mut failures,
        "memory_stats alias methods changed",
        validate_alias_methods(
            function_body(allocator, "memory_stats"),
            &["try_lock", "len"],
            &[],
        ),
    );

    let fixture = source(sources, "kernel/src/memory/frame_allocator_tests.rs");
    record_unit(
        &mut failures,
        "fixture FREE_FRAMES capability escaped",
        validate_free_frame_capabilities(
            "kernel/src/memory/frame_allocator_tests.rs",
            fixture,
            &[
                "inject_duplicate_candidates",
                "remove_duplicate_candidates",
                "republish_lost_frame",
                "retire_with_free_list_contended",
                "free_frame_count",
                "free_list_len_for_gate",
                "take_free_frame",
                "frame_custody_refusal_gate_test",
            ],
        ),
    );

    for (path, module) in sources.iter().filter(|(path, _)| {
        path != "kernel/src/memory/frame_allocator.rs"
            && path != "kernel/src/memory/frame_allocator_tests.rs"
    }) {
        let mask = code_mask(module);
        if !identifier_offsets(module, &mask, "FREE_FRAMES").is_empty() {
            eprintln!("unexpected FREE_FRAMES capability in {path}");
            failures.push(format!("unexpected FREE_FRAMES capability in {path}"));
        }
    }

    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let all_returns = code_offsets(process_memory, &code_mask(process_memory), "return_lease(");
    let retire = function_body(process_memory, "retire_bounded");
    record_unit(
        &mut failures,
        "process-memory return_lease calls escaped retire_bounded",
        (all_returns.len() == 2 && retire.matches("return_lease(").count() == all_returns.len())
            .then_some(())
            .ok_or(()),
    );

    failures.is_empty().then_some(()).ok_or(failures)
}

fn validate_frame_ledger_hot_paths(sources: &[(String, String)]) -> Result<(), ()> {
    let allocator = source(sources, "kernel/src/memory/frame_allocator.rs");
    const ROOTS: [&str; 8] = [
        "frame_ordinal",
        "get",
        "claim_frame",
        "counted",
        "return_lease",
        "acquire_leaf_mapping",
        "decref_leaf_mapping",
        "deallocate_leaf_frame",
    ];
    let bodies = module_function_bodies(allocator);
    let reached = transitively_called_functions(allocator, &ROOTS);
    if !ROOTS
        .iter()
        .all(|name| reached.contains(*name) && bodies.contains_key(*name))
    {
        return Err(());
    }
    for name in reached {
        for body in bodies.get(&name).into_iter().flatten() {
            for forbidden in [
                "log::",
                "serial_println!",
                "format!",
                "vec!",
                "Vec::new",
                "Vec::with_capacity",
                "alloc::",
            ] {
                if body.contains(forbidden) {
                    return Err(());
                }
            }
        }
    }
    let returned = function_body(allocator, "return_lease");
    for forbidden in ["reserve(", "try_reserve(", "resize(", "with_capacity("] {
        if returned.contains(forbidden) {
            return Err(());
        }
    }
    let boundary = returned
        .find("if free_list.len() == free_list.capacity() {")
        .ok_or(())?;
    let push = returned.find("free_list.push(lease.frame);").ok_or(())?;
    (boundary < push).then_some(()).ok_or(())
}

fn validate_frame_ledger_bounded_boot_allocation(sources: &[(String, String)]) -> Result<(), ()> {
    let allocator = source(sources, "kernel/src/memory/frame_allocator.rs");
    let init = function_body(allocator, "init_frame_ledger");
    let ledger_init = function_body(allocator, "try_new");
    let ensure_chunk = function_body(allocator, "ensure_chunk");
    let prepare = function_body(allocator, "prepare_frame_for_allocation");
    let reserve = function_body(allocator, "ensure_free_frame_capacity");
    let sequential_allocate = function_body(allocator, "allocate_frame");
    let prepare_before_publish = sequential_allocate
        .find("prepare_frame_for_allocation(current)")
        .ok_or(())?
        < sequential_allocate
            .find("NEXT_FREE_FRAME.compare_exchange(")
            .ok_or(())?;
    let built_outside_once = ensure_chunk.find("slots.try_reserve_exact").ok_or(())?
        < ensure_chunk.find(".call_once(||").ok_or(())?;
    let contention = sequential_allocate
        .find("PrepareFrame::Contended if capacity_contentions < 8")
        .ok_or(())?;
    let exhausted = sequential_allocate
        .find("PrepareFrame::Exhausted => return None")
        .ok_or(())?;

    (allocator.contains("const LEDGER_CHUNK_FRAMES: usize = 64 * 1024;")
        && init.contains("FrameLedger::try_new(total_frames, frontier)")
        && init.contains("while chunk_start < frontier")
        && init.contains(".ensure_chunk(chunk_start)")
        && init.contains("advertised_frames.min(MAX_TRACKED_FRAMES)")
        && init.contains("assert_eq!(NEXT_FREE_FRAME.load(Ordering::Acquire), frontier_snapshot)")
        && !init.contains("(0..total_frames)")
        && !init.contains("reserve_exact(total_frames)")
        && ledger_init.contains("try_reserve_exact(chunk_count)")
        && prepare.contains("ledger.ensure_chunk(index)")
        && prepare.contains("ensure_free_frame_capacity(index + 1)")
        && reserve.contains("try_reserve(additional)")
        && built_outside_once
        && prepare_before_publish
        && contention < exhausted)
        .then_some(())
        .ok_or(())
}

fn validate_frame_ledger_boot_order(sources: &[(String, String)]) -> Result<(), ()> {
    let memory_init = function_body(source(sources, "kernel/src/memory/mod.rs"), "init");
    let x86_ledger = memory_init
        .find("frame_allocator::init_frame_ledger();")
        .ok_or(())?;
    if memory_init.rfind("heap::init(&mapper)").ok_or(())? > x86_ledger
        || x86_ledger > memory_init.find("slab::init();").ok_or(())?
        || memory_init[..x86_ledger].contains("ProcessPageTable::new(")
    {
        return Err(());
    }

    let arm_main = function_body(source(sources, "kernel/src/main_aarch64.rs"), "kernel_main");
    let arm_ledger = arm_main
        .find("frame_allocator::init_frame_ledger();")
        .ok_or(())?;
    if arm_main.find("memory::init_aarch64_heap();").ok_or(())? > arm_ledger
        || arm_ledger > arm_main.find("memory::kernel_stack::init();").ok_or(())?
        || arm_ledger > arm_main.find("kernel::process::init();").ok_or(())?
        || arm_main[..arm_ledger].contains("ProcessPageTable::new(")
    {
        return Err(());
    }
    Ok(())
}

fn validate_frame_ledger_init(sources: &[(String, String)]) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    record(
        &mut failures,
        "frame-ledger init calls",
        validate_census(
            &census(sources, |source, mask| {
                code_offsets(source, mask, "init_frame_ledger();")
            }),
            FRAME_LEDGER_INIT_CALLS,
        ),
    );
    record(
        &mut failures,
        "ProcessPageTable constructors",
        validate_census(
            &census(sources, |source, mask| {
                code_offsets(source, mask, "ProcessPageTable::new(")
            }),
            PROCESS_PAGE_TABLE_CONSTRUCTORS,
        ),
    );
    record_unit(
        &mut failures,
        "frame-ledger boot order",
        validate_frame_ledger_boot_order(sources),
    );
    failures.is_empty().then_some(()).ok_or(failures)
}

fn validate_frame_ledger_runtime_oracles(sources: &[(String, String)]) -> Result<(), ()> {
    let tests = source(sources, "kernel/src/memory/frame_allocator_tests.rs");
    let take_free = function_body(tests, "take_free_frame");
    let stale = function_body(tests, "stale_lease_fixture");
    let gate = function_body(tests, "frame_custody_refusal_gate_test");
    let healthy_guard = function_body(tests, "frame_custody_healthy_counters_test");
    let above_top = function_body(tests, "above_top_of_ram_frame");
    let never_allocated = function_body(tests, "reserve_never_allocated_frame");
    let allocator = source(sources, "kernel/src/memory/frame_allocator.rs");
    let returned = function_body(allocator, "return_lease");
    let counted = function_body(allocator, "counted");
    let claim = function_body(allocator, "claim_frame");
    let ordinal = function_body(allocator, "frame_ordinal");
    let registry = source(sources, "kernel/src/test_framework/registry.rs");
    let registry_mask = code_mask(registry);
    if !code_offsets(
        registry,
        &registry_mask,
        "use crate::memory::frame_allocator::frame_custody_refusal_gate_test as",
    )
    .is_empty()
    {
        return Err(());
    }
    let refusal_offsets =
        identifier_offsets(registry, &registry_mask, "frame_custody_refusal_gate_test");
    if refusal_offsets.len() != 1 {
        return Err(());
    }
    let refusal_def = enclosing_test_def(registry, &registry_mask, refusal_offsets[0]).ok_or(())?;
    if !code_offsets(
        registry,
        &registry_mask,
        "use crate::memory::frame_allocator::frame_custody_healthy_counters_test as",
    )
    .is_empty()
    {
        return Err(());
    }
    let healthy_offsets = identifier_offsets(
        registry,
        &registry_mask,
        "frame_custody_healthy_counters_test",
    );
    if healthy_offsets.len() != 1 {
        return Err(());
    }
    let healthy_def = enclosing_test_def(registry, &registry_mask, healthy_offsets[0]).ok_or(())?;
    let executor = source(sources, "kernel/src/test_framework/executor.rs");
    let run_all = function_body(executor, "run_all_tests");
    let run_staged = function_body(executor, "run_staged_tests");
    let serial = run_all
        .find("run_staged_tests(TestStage::SerialBoot)")
        .ok_or(())?;
    let parallel = run_all
        .find("run_staged_tests(TestStage::EarlyBoot)")
        .ok_or(())?;
    let serial_condition = run_staged
        .find("if target_stage == TestStage::SerialBoot {")
        .ok_or(())?;
    let run_staged_mask = code_mask(run_staged);
    let serial_block = braced_block(run_staged, &run_staged_mask, serial_condition).ok_or(())?;
    if normalized_code(block_statements(serial_block).ok_or(())?)
        != "total_failed += join_test_thread(subsystem.id, handle);"
    {
        return Err(());
    }
    let mut else_cursor = serial_condition + serial_block.len();
    while else_cursor < run_staged.len()
        && (!run_staged_mask[else_cursor]
            || run_staged.as_bytes()[else_cursor].is_ascii_whitespace())
    {
        else_cursor += 1;
    }
    if !run_staged[else_cursor..].starts_with("else") {
        return Err(());
    }
    else_cursor += "else".len();
    while else_cursor < run_staged.len()
        && (!run_staged_mask[else_cursor]
            || run_staged.as_bytes()[else_cursor].is_ascii_whitespace())
    {
        else_cursor += 1;
    }
    if run_staged.as_bytes().get(else_cursor) != Some(&b'{') {
        return Err(());
    }
    let parallel_block = braced_block(run_staged, &run_staged_mask, else_cursor).ok_or(())?;
    if normalized_code(block_statements(parallel_block).ok_or(())?)
        != "handles.push((subsystem.id, handle));"
    {
        return Err(());
    }
    let memory_init = function_body(source(sources, "kernel/src/memory/mod.rs"), "init");

    const DOUBLE_RETURN_ASSERTION: &str = "if return_lease(lease) != ReturnOutcome::Returned\n        || return_lease(lease) != ReturnOutcome::RefusedDoubleRelease\n        || FREE_FRAMES.lock().len() != free_before + 1\n        || free_frame_count(lease.frame) != 1\n        || !healthy_round_trip()\n    {";
    const STALE_FIXTURE_ASSERTION: &str =
        "if current.index != stale.index || current.generation == stale.generation {";
    const STALE_RETURN_ASSERTION: &str = "if return_lease(stale) != ReturnOutcome::RefusedStale {";
    const GATE_PRECONDITION_ASSERTION: &str = "if start[..5] != [0; 5] {";
    const UNTRACKED_ASSERTION: &str =
        "if after_untracked[3] != before_untracked[3] + 1 || free_after_untracked != free_before {";
    const NEVER_ALLOCATED_ASSERTION: &str = "if counters()[2] != before_never[2] + 1
        || FREE_FRAMES.lock().len() != free_before
        || !healthy_round_trip()
    {";
    const CONTENTION_ASSERTION: &str = "if outcome != ReturnOutcome::LostContended {";
    const DUPLICATE_CLEANUP: &str = "let replacement = allocate_frame_leased();\n    remove_duplicate_candidates(live.frame);\n    let live_after =";
    const DUPLICATE_OWNER_ASSERTION: &str = "if live_before.is_none()\n        || live_after != live_before\n        || !replacement_is_distinct\n        || counters()[4] != duplicate_before + 3\n    {";
    const AGGREGATE_ASSERTION: &str = "if end[0] != start[0] + 1\n        || end[1] != start[1] + 1\n        || end[2] != start[2] + 1\n        || end[3] != start[3] + 1\n        || end[4] != start[4] + 3\n        || end[5] < start[5] + 1\n    {";
    // The O2/B, O2/D and O2/E guards. Pinned through their opening brace so an
    // always-false operand of ANY spelling — not just the nine the vacuity rule
    // knows — reds the suite (review-sweep-r4 finding 2).
    const STALE_OWNER_ASSERTION: &str = "if state\n        .is_none_or(|state| state & STATE_MASK != ST_ALLOCATED || state >> 2 != current.generation)\n    {";
    const STALE_RECOVERY_ASSERTION: &str =
        "if return_lease(current) != ReturnOutcome::Returned || !healthy_round_trip() {";
    const DUPLICATE_FIXTURE_ASSERTION: &str = "if !inject_duplicate_candidates(live.frame, 3) {";
    const DUPLICATE_CLEANUP_ASSERTION: &str = "if !replacement_returned || !live_returned {";
    const DUPLICATE_RECOVERY_ASSERTION: &str = "if !healthy_round_trip() {";
    const CONTENDED_ISOLATION_ASSERTION: &str = "if lost_state.is_none_or(|state| state & STATE_MASK != ST_FREE)\n        || free_frame_count(contended.frame) != 0\n    {";
    const CONTENDED_RECOVERY_ASSERTION: &str = "if return_lease(repaired) != ReturnOutcome::Returned\n        || counters()[..5] != before_healthy[..5]\n        || !healthy_round_trip()\n    {";

    (take_free
        .trim_end()
        .ends_with("claim_frame(candidate).ok().flatten()\n}")
        && !take_free.contains("FrameLease {")
        && stale.contains("return_lease(stale)")
        && stale.contains("take_free_frame(stale.frame)")
        && stale.contains(STALE_FIXTURE_ASSERTION)
        && gate.contains(STALE_RETURN_ASSERTION)
        && gate.contains(GATE_PRECONDITION_ASSERTION)
        && gate.contains(UNTRACKED_ASSERTION)
        && gate.contains(NEVER_ALLOCATED_ASSERTION)
        && gate.contains(CONTENTION_ASSERTION)
        && gate.contains(DOUBLE_RETURN_ASSERTION)
        && gate.contains(STALE_OWNER_ASSERTION)
        && gate.contains(STALE_RECOVERY_ASSERTION)
        && gate.contains(DUPLICATE_FIXTURE_ASSERTION)
        && gate.contains(DUPLICATE_CLEANUP_ASSERTION)
        && gate.contains(DUPLICATE_RECOVERY_ASSERTION)
        && gate.contains(CONTENDED_ISOLATION_ASSERTION)
        && gate.contains(CONTENDED_RECOVERY_ASSERTION)
        && gate.contains("above_top_of_ram_frame()")
        && gate.contains("deallocate_frame(untracked)")
        && gate.contains("reserve_never_allocated_frame()")
        && gate.contains("deallocate_frame(never_frame)")
        && never_allocated.contains("BootInfoFrameAllocator::get_usable_frame(index)")
        && never_allocated.contains("prepare_frame_for_allocation(index)")
        && never_allocated.contains("NEXT_FREE_FRAME\n            .compare_exchange(")
        && never_allocated.find("prepare_frame_for_allocation(index)").ok_or(())?
            < never_allocated.find(".compare_exchange(").ok_or(())?
        && never_allocated.contains(".is_ok()\n        {\n            return Some(frame);")
        && !gate.contains("FrameLease {")
        && above_top.contains("MEMORY_INFO.get()")
        && above_top.contains("region.end")
        && above_top.contains(".max()")
        && returned.contains("if observed >> 2 != lease.generation {")
        && returned.contains(
            "ST_FREE => return counted(ReturnOutcome::RefusedDoubleRelease),",
        )
        && ordinal.trim_end().ends_with("None\n}")
        && counted.contains(
            "ReturnOutcome::RefusedStale => teardown::FRAME_RETURN_REFUSED_STALE.increment()",
        )
        && counted.contains(
            "ReturnOutcome::RefusedDoubleRelease => teardown::FRAME_RETURN_REFUSED_DOUBLE.increment()",
        )
        && counted.contains(
            "teardown::FRAME_RETURN_REFUSED_NEVER_ALLOCATED.increment()",
        )
        && counted.contains(
            "ReturnOutcome::RefusedUntracked => teardown::FRAME_RETURN_REFUSED_UNTRACKED.increment()",
        )
        && counted.contains("ReturnOutcome::LostContended => teardown::FRAME_LOST_CONTENDED.increment()")
        && claim.contains("FRAME_DUPLICATE_ALLOC_REFUSED.increment()")
        && claim.contains(
            "ST_ALLOCATED => {\n                crate::tracing::providers::teardown::FRAME_DUPLICATE_ALLOC_REFUSED.increment();\n                return Err(ClaimError::Duplicate);",
        )
        && gate.contains(DUPLICATE_CLEANUP)
        && gate.contains(DUPLICATE_OWNER_ASSERTION)
        && gate.contains("let free_guard = FREE_FRAMES.lock();")
        && gate.matches("healthy_round_trip()").count() == 5
        && gate.contains(AGGREGATE_ASSERTION)
        && refusal_def.contains("name: \"frame_custody_refusal_gate\"")
        && refusal_def.contains("arch: Arch::Aarch64")
        && refusal_def.contains("stage: TestStage::SerialBoot")
        && healthy_def.contains("name: \"frame_custody_healthy_counters\"")
        && healthy_def.contains("arch: Arch::Aarch64")
        && healthy_def.contains("stage: TestStage::ProcessContext")
        && healthy_guard.contains("if counters()[..5] != [1, 1, 1, 1, 3] {")
        && healthy_guard.contains("TestResult::Fail(")
        && serial < parallel
        && function_body(registry, "test_timer_init").contains("test_timer_ticks()")
        && memory_init
            .matches("frame_allocator::run_x86_frame_custody_gate();")
            .count()
            == 1)
        .then_some(())
        .ok_or(())
}

/// Always-true / always-false operands in the boot-test oracle surface.
/// HT-2's prefix-pin class was closed instance-by-instance twice; this closes the
/// spelling itself, so a new assertion cannot be neutered without a red suite.
fn validate_no_vacuous_test_conditions(sources: &[(String, String)]) -> Result<(), ()> {
    const PATHS: [&str; 4] = [
        "kernel/src/memory/frame_allocator_tests.rs",
        "kernel/src/test_framework/registry.rs",
        "kernel/src/test_framework/executor.rs",
        "kernel/src/tracing/providers/teardown.rs",
    ];
    const SHAPES: [&str; 9] = [
        "&& false",
        "false &&",
        "|| true",
        "true ||",
        "if false",
        "if true",
        "while false",
        "#[cfg(never)]",
        "#[cfg(any())]",
    ];

    for path in PATHS {
        let normalized = normalized_code(source(sources, path));
        for shape in SHAPES {
            if normalized.contains(shape) {
                eprintln!("vacuous boot-test condition in {path}: {shape}");
                return Err(());
            }
        }
    }
    Ok(())
}

/// The x86 harness's *pass mechanism*, not just its shape. The five `contains`
/// checks below prove the gate looks for the right markers; the block above them
/// proves the recorded verdict is actually spent — `set -e` still on, `$passed`
/// executed bare (so a false verdict ends the run), and no early or zero exit
/// able to pre-empt it (review-sweep-r4 finding 4).
fn validate_x86_frame_custody_harness(script: &str) -> Result<(), ()> {
    const FRAME_VECTOR: &str = "FRAME_CUSTODY_PATTERN='^\\[FRAME_CUSTODY_COUNTERS:x86:double=1:stale=1:never=1:untracked=1:duplicate=3:contended=[1-9][0-9]*\\]$'";
    const PT_CUSTODY_VECTOR: &str = "PT_CUSTODY_LITERAL='[PT_CUSTODY_COUNTERS:x86:recorded=11:no_proof=0:no_arch=0:terminated=1:undecided=1:retired=1:returned=10:lost=0:requeued=0]'";
    const PT_COHORT_VECTOR: &str = "PT_COHORT_LITERAL='[PT_RETIRE_COHORT:x86:children=64:retired=65:returned=642:recorded=577:lost=0:no_arch=0:undecided=0:mid_retire=0:balance=0]'";
    const PT_EXEC_COHORT_VECTOR: &str = "PT_EXEC_COHORT_LITERAL='[PT_EXEC_COHORT:x86:children=16:superseded=3:roots=64:returned=640:recorded=576:lost=0:leaf_recorded=192:leaf_released=192:leaf_returned=192:custody_refused=0:decref_unregistered=0:undecided=0:mid_retire=0:no_arch=0:balance=0]' # The returned and recorded table-frame fields are pinned from the measured run.";
    let exact_marker_count = |marker: &str| {
        let needle = format!("grep -h -c '\\[TEST:process:{marker}:PASS\\]'");
        script.find(&needle).is_some_and(|start| {
            let tail = &script[start..];
            let end = tail.find("\n    test ").unwrap_or(tail.len());
            tail[..end].contains(")\" -eq 1")
        })
    };
    let mut bare_verdict = false;
    for line in script.lines() {
        let statement = line.trim();
        if statement == "$passed" {
            bare_verdict = true;
        }
        if statement.split_whitespace().next() == Some("exit") && statement != "exit 1" {
            eprintln!("x86 harness gained an exit that pre-empts its verdict: {statement}");
            return Err(());
        }
    }
    let verdict_false = script.find("passed=false").ok_or(())?;
    let verdict_true = script.find("passed=true").ok_or(())?;
    let verdict_spent = script.find("\n    $passed\n").ok_or(())?;
    let counter_check = script.find("-eq 1").ok_or(())?;

    (bare_verdict
        && script.contains("set -euo pipefail")
        && !script.contains("set +")
        && script.matches("passed=false").count() == 1
        && script.matches("passed=true").count() == 1
        && script.matches("$passed").count() == 1
        && verdict_false < verdict_true
        && verdict_true < verdict_spent
        && verdict_spent < counter_check
        && !script.contains("grep -q '\\[BOOT_TESTS:PASS\\]'")
        && !script.contains("grep -q 'KERNEL_POST_TESTS_COMPLETE'")
        && script.contains("advance_stage_marker_only")
        && script.contains("[TESTS_COMPLETE:0/0]")
        && script.contains(FRAME_VECTOR)
        && script.contains(PT_CUSTODY_VECTOR)
        && script.contains(PT_COHORT_VECTOR)
        && script.contains(PT_EXEC_COHORT_VECTOR)
        && script.matches("frame_custody_refusal_gate:PASS").count() == 2
        && script.matches("page_table_custody_disposition_gate:PASS").count() == 2
        && script.matches("x86_retire_cohort:PASS").count() == 2
        && script.matches("x86_exec_cohort:PASS").count() == 2
        && exact_marker_count("frame_custody_refusal_gate")
        && exact_marker_count("page_table_custody_disposition_gate")
        && exact_marker_count("x86_retire_cohort")
        && exact_marker_count("x86_exec_cohort")
        && script.contains("grep -qE \"$FRAME_CUSTODY_PATTERN\"")
        && script.contains("grep -qF -x \"$PT_CUSTODY_LITERAL\"")
        && script.contains("grep -qF -x \"$PT_COHORT_LITERAL\"")
        && script.contains("grep -qF -x \"$PT_EXEC_COHORT_LITERAL\"")
        && script.contains("grep -h -E -c \"$FRAME_CUSTODY_PATTERN\"")
        && script.contains("grep -h -F -x -c \"$PT_CUSTODY_LITERAL\"")
        && script.contains("grep -h -F -x -c \"$PT_COHORT_LITERAL\"")
        && script.contains("grep -h -F -x -c \"$PT_EXEC_COHORT_LITERAL\"")
        && script.contains("-eq 1")
        && script.contains("x86 frame-custody gate run")
        && script.matches("BOOT_TESTS:FAIL|KERNEL PANIC|panic!").count() == 2)
    .then_some(())
    .ok_or(())
}

/// The PostScheduler workqueue probe must wait on the workqueue's own completion
/// handshake and score a counter that advances only when the scheduled closure
/// ran. The #519/#521 campaign's progress-bounded-wait norm forbids the spin
/// budget this replaced, and TRAP-LIST HT-3 requires the fall-through verdict to
/// be pinned so reverting it to `TestResult::Pass` cannot pass the suite.
fn validate_workqueue_progress_wait(registry: &str) -> Result<(), ()> {
    let body = function_body(registry, "test_workqueue_operational");
    for forbidden in [
        "for _ in 0..",
        "spin_loop",
        "get_monotonic_time",
        "rdtsc",
        "elapsed",
    ] {
        if body.contains(forbidden) {
            eprintln!("workqueue probe regained a timing-based wait: {forbidden}");
            return Err(());
        }
    }
    let wait = body.find("work.wait();").ok_or(())?;
    let completion = body.find("if !work.is_completed() {").ok_or(())?;

    (body.contains("static WORK_RUNS: AtomicU32 = AtomicU32::new(0);")
        && body.contains("let before = WORK_RUNS.load(Ordering::SeqCst);")
        && body.contains("WORK_RUNS.fetch_add(1, Ordering::SeqCst);")
        && body.contains("if !workqueue::schedule_work(Arc::clone(&work)) {")
        && body.contains("TestResult::Fail(\"workqueue refused the scheduled work\")")
        && body.contains("TestResult::Fail(\"workqueue wait returned before the work completed\")")
        && body.contains("if WORK_RUNS.load(Ordering::SeqCst) != before.wrapping_add(1) {")
        && body.contains(
            "TestResult::Fail(\"workqueue did not execute scheduled work exactly once\")",
        )
        && wait < completion)
        .then_some(())
        .ok_or(())
}

fn validate_frame_ledger_counter_inventory(provider: &str) -> Result<(), ()> {
    const EXPECTED: [&str; 7] = [
        "FRAME_RETURN_REFUSED_DOUBLE",
        "FRAME_RETURN_REFUSED_STALE",
        "FRAME_RETURN_REFUSED_NEVER_ALLOCATED",
        "FRAME_RETURN_REFUSED_UNTRACKED",
        "FRAME_DUPLICATE_ALLOC_REFUSED",
        "FRAME_LOST_CONTENDED",
        "FRAME_RETURN_REFUSED_LIVE_LEAF",
    ];
    let expected: BTreeSet<_> = EXPECTED.into_iter().map(str::to_owned).collect();
    let declared: BTreeSet<_> = provider
        .split("counter!(")
        .skip(1)
        .filter_map(|rest| {
            rest.trim_start()
                .split_once(',')
                .map(|(name, _)| name.trim())
        })
        .filter(|name| {
            name.starts_with("FRAME_RETURN_REFUSED_")
                || *name == "FRAME_DUPLICATE_ALLOC_REFUSED"
                || *name == "FRAME_LOST_CONTENDED"
        })
        .map(str::to_owned)
        .collect();
    let inventory = provider
        .split("pub static COUNTERS")
        .nth(1)
        .ok_or(())?
        .split("];")
        .next()
        .ok_or(())?;

    (declared == expected
        && EXPECTED
            .iter()
            .all(|counter| inventory.contains(&format!("&{counter},")))
        && provider.contains("pub const COUNTER_COUNT: usize = 72;"))
    .then_some(())
    .ok_or(())
}

fn validate_process_table_recorder(
    sources: &[(String, String)],
) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    let mut recorder_sites = census(sources, |source, mask| {
        code_offsets(source, mask, "TableRecorder(tables)")
    });
    recorder_sites.retain(|(path, _), _| path == "kernel/src/memory/process_memory.rs");
    record(
        &mut failures,
        "process mapper recorder sites",
        validate_census(&recorder_sites, TABLE_RECORDER_SITES),
    );

    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let mask = code_mask(process_memory);
    for forbidden in [
        "GlobalFrameAllocator",
        "pub fn mapper(",
        "pub fn allocate_stack(",
        "fn deep_copy_pml4_entry",
        "fn deep_copy_l3_entry",
        "fn deep_copy_l2_entry",
    ] {
        if !code_offsets(process_memory, &mask, forbidden).is_empty() {
            eprintln!("R2 process mapper escape hatch restored: {forbidden}");
            failures.push(format!("R2 process mapper escape hatch restored: {forbidden}"));
        }
    }

    let recorder = function_body(process_memory, "allocate_frame");
    record_unit(
        &mut failures,
        "TableRecorder allocation flow changed",
        (recorder.contains("let lease = allocate_frame_leased()?")
            && recorder.contains("self.0.record(lease);")
            && recorder.contains("Some(frame)")
            && !recorder.contains("deallocate_frame")
            && !recorder.contains("return_lease"))
        .then_some(())
        .ok_or(()),
    );
    failures.is_empty().then_some(()).ok_or(failures)
}

fn validate_process_page_table_drop_is_non_freeing(sources: &[(String, String)]) -> Result<(), ()> {
    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let process_drop = process_memory
        .find("impl Drop for ProcessPageTable")
        .ok_or(())?;
    let drop_body = function_body(&process_memory[process_drop..], "drop");
    (drop_body.contains("Disposition::Undecided")
        && drop_body.contains("trace_count!")
        && drop_body.contains("PT_ROOT_DROPPED_UNDECIDED")
        && ["deallocate_frame", "return_lease", "retire_bounded"]
            .iter()
            .all(|forbidden| !drop_body.contains(forbidden)))
    .then_some(())
    .ok_or(())
}

fn validate_process_page_table_dispositions(
    sources: &[(String, String)],
) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    let adapted_paths = [
        "kernel/src/task/process_task.rs",
        "kernel/src/process/manager.rs",
    ];
    let mut abandon_sites = census_tagged(sources, |source, mask| {
        let bytes = source.as_bytes();
        let reason_prefix = "AbandonReason::";
        let reason_offsets = code_offsets(source, mask, reason_prefix);
        let mut sites: Vec<_> = method_call_offsets(source, mask, "abandon")
            .into_iter()
            .flat_map(|call| {
                let Some(open) = next_code(source, mask, call + "abandon".len()) else {
                    return Vec::new();
                };
                let Some(close) = matching_paren(source, mask, open) else {
                    return Vec::new();
                };
                reason_offsets
                    .iter()
                    .copied()
                    .filter(|offset| open < *offset && *offset < close)
                    .filter_map(|offset| {
                        let start = next_code(source, mask, offset + reason_prefix.len())?;
                        let mut end = start;
                        while end < bytes.len() && mask[end] && identifier_byte(bytes[end]) {
                            end += 1;
                        }
                        (end > start).then(|| {
                            (
                                call,
                                format!("{reason_prefix}{}", &source[start..end]),
                            )
                        })
                    })
                    .collect()
            })
            .collect();

        // The reservation-failure fallback chooses its reason under cfg and
        // passes the local through the one consuming call. Treat both cfg arms
        // as disposition producers and pin the call through its semicolon.
        if code_offsets(source, mask, "fn abandon_unqueued_reclaim").len() == 1 {
            let body = function_body(source, "abandon_unqueued_reclaim");
            let normalized = normalized_code(body);
            if normalized.contains("page_table.abandon(reason);") {
                let base = body.as_ptr() as usize - source.as_ptr() as usize;
                let body_mask = code_mask(body);
                for reason in ["NoProofPipeline", "NoArchPipeline"] {
                    let needle = format!("AbandonReason::{reason}");
                    sites.extend(code_offsets(body, &body_mask, &needle).into_iter().map(
                        |offset| (base + offset, needle.clone()),
                    ));
                }
            }
        }
        sites
    });
    abandon_sites.retain(|(path, _), _| adapted_paths.contains(&path.as_str()));
    record(
        &mut failures,
        "process page-table abandon sites",
        validate_census(&abandon_sites, PROCESS_PAGE_TABLE_ABANDON_SITES),
    );

    for path in adapted_paths {
        let module = source(sources, path);
        if !call_sites_with_argument(module, "drop", "page_table.take()").is_empty() {
            eprintln!("R5 raw page-table drop restored in {path}");
            failures.push(format!("R5 raw page-table drop restored in {path}"));
        }
    }

    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let bodies = module_function_bodies(process_memory);
    if let Some(cleanup_bodies) = bodies.get("cleanup_for_exec") {
        let custody_shaped = cleanup_bodies.iter().filter(|body| {
            body.contains("self.release_mapped_leaves();")
                && body.contains("self.retire_bounded(pid, budget)")
                && !body.contains("Disposition::")
                && !body.contains("deallocate_frame")
                && !body.contains("Vec::new")
                && !body.contains("log::")
                && !body.contains("PhysFrame::containing_address")
        });
        if cleanup_bodies.len() != 1 || custody_shaped.count() != 1 {
            failures.push("cleanup_for_exec is no longer a single custody-shaped body".to_owned());
        }
    } else {
        failures.push("cleanup_for_exec disposition bodies disappeared".to_owned());
    }
    failures.is_empty().then_some(()).ok_or(failures)
}

fn validate_process_page_table_retire_site(
    sources: &[(String, String)],
) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    let process_task = source(sources, "kernel/src/task/process_task.rs");
    let retire_sites = census(sources, |source, mask| {
        call_offsets(source, mask, "retire_bounded")
    });
    record(
        &mut failures,
        "retire_bounded callers",
        validate_census(&retire_sites, PROCESS_PAGE_TABLE_RETIRE_SITES),
    );

    let mut reclaim_sites = census(sources, |source, mask| {
        call_offsets(source, mask, "reclaim_bounded")
    });
    reclaim_sites.retain(|(path, _), _| path == "kernel/src/task/process_task.rs");
    record(
        &mut failures,
        "reclaim_bounded callers",
        validate_census(&reclaim_sites, PENDING_RECLAIM_BOUNDED_SITES),
    );

    let reclaim_present = process_task.contains("fn reclaim_bounded(&mut self)");
    let drain_present = process_task.contains("fn reclaim_deferred_process_resources_for_pass(");
    if !reclaim_present {
        failures.push("missing fn reclaim_bounded(&mut self)".to_owned());
    }
    if !drain_present {
        failures.push("missing fn reclaim_deferred_process_resources_for_pass".to_owned());
    }
    if reclaim_present && drain_present {
        let reclaim = function_body(process_task, "reclaim_bounded");
        let drain = function_body(process_task, "reclaim_deferred_process_resources_for_pass");
        let structural = (|| -> Result<(), ()> {
            (reclaim.matches(".retire_bounded(").count() == 1
                && drain.matches(".reclaim_bounded(").count() == 1
                && drain
                    .find("if let Some(blocker) = proof.blocker()")
                    .ok_or(())?
                    < drain.find(".reclaim_bounded(").ok_or(())?
                && drain.find(".reclaim_bounded(").ok_or(())?
                    < drain.find("record_reclaim(reclaim.pid)").ok_or(())?)
            .then_some(())
            .ok_or(())
        })();
        record_unit(
            &mut failures,
            "proof-gated reclaim ordering changed",
            structural,
        );
    }
    failures.is_empty().then_some(()).ok_or(failures)
}

fn validate_process_page_table_counter_inventory(sources: &[(String, String)]) -> Result<(), ()> {
    const EXPECTED: [&str; 12] = [
        "PT_TABLE_FRAMES_RECORDED",
        "PT_ROOT_ABANDONED_NO_PROOF",
        "PT_ROOT_ABANDONED_NO_ARCH",
        "PT_ROOT_ABANDONED_TERMINATED",
        "PT_ROOT_DROPPED_UNDECIDED",
        "PT_ROOTS_RETIRED",
        "PT_TABLE_FRAMES_RETURNED",
        "PT_RETIRE_FRAMES_LOST",
        "PT_ROOT_DROPPED_MID_RETIRE",
        "PT_RETIRE_BUDGET_REQUEUED",
        "PT_ROOT_SLOT_REFUSED",
        "PT_SHADOW_ROOT_CLEARED",
    ];
    let provider = source(sources, "kernel/src/tracing/providers/teardown.rs");
    let expected: BTreeSet<_> = EXPECTED.into_iter().map(str::to_owned).collect();
    let declared: BTreeSet<_> = provider
        .split("counter!(")
        .skip(1)
        .filter_map(|rest| {
            rest.trim_start()
                .split_once(',')
                .map(|(name, _)| name.trim())
        })
        .filter(|name| name.starts_with("PT_"))
        .map(str::to_owned)
        .collect();
    let inventory = provider
        .split("pub static COUNTERS")
        .nth(1)
        .ok_or(())?
        .split("];")
        .next()
        .ok_or(())?;
    if declared != expected
        || !EXPECTED
            .iter()
            .all(|counter| inventory.contains(&format!("&{counter},")))
        || !provider.contains("pub const COUNTER_COUNT: usize = 72;")
    {
        return Err(());
    }
    for counter in EXPECTED {
        let declaration = provider.find(counter).ok_or(())?;
        if provider[declaration.saturating_sub(80)..declaration].contains("#[cfg(") {
            return Err(());
        }
    }

    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let record = function_body(process_memory, "record");
    let abandon = function_body(process_memory, "abandon");
    let process_drop = process_memory
        .find("impl Drop for ProcessPageTable")
        .ok_or(())?;
    let drop_body = function_body(&process_memory[process_drop..], "drop");
    if !process_memory.contains("pub(crate) fn retire_bounded") {
        return Err(());
    }
    let retire = function_body(process_memory, "retire_bounded");
    let map_page = function_body(process_memory, "map_page");
    let task = source(sources, "kernel/src/task/process_task.rs");
    let cleanup_bodies = module_function_bodies(process_memory)
        .remove("cleanup_for_exec")
        .ok_or(())?;
    (record.contains("PT_TABLE_FRAMES_RECORDED")
        && abandon.contains("PT_ROOT_ABANDONED_NO_PROOF")
        && abandon.contains("PT_ROOT_ABANDONED_NO_ARCH")
        && abandon.contains("PT_ROOT_ABANDONED_TERMINATED")
        && drop_body.contains("PT_ROOT_DROPPED_UNDECIDED")
        && drop_body.contains("PT_ROOT_DROPPED_MID_RETIRE")
        && retire.contains("record_pt_frame_returned")
        && retire.contains("record_pt_frame_lost")
        && retire.contains("record_pt_root_retired")
        && task.contains("PT_RETIRE_BUDGET_REQUEUED")
        && map_page.contains("PT_ROOT_SLOT_REFUSED")
        && cleanup_bodies.len() == 1
        && cleanup_bodies
            .iter()
            .filter(|body| {
                body.contains("self.release_mapped_leaves();")
                    && body.contains("self.retire_bounded(pid, budget)")
            })
            .count()
            == 1)
    .then_some(())
    .ok_or(())
}

fn validate_root_slot_custody(sources: &[(String, String)]) -> Result<(), ()> {
    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    if !process_memory.contains("enum RootSlotCustody")
        || !process_memory.contains("struct RootSlotOwnership")
        || !process_memory.contains("owned_root_slots: RootSlotOwnership,")
    {
        return Err(());
    }
    let custody = function_body(process_memory, "root_slot_custody");
    if !custody.contains("self.root_slot_populated(page_addr)")
        || !custody.contains("self.owned_root_slots.contains(slot)")
        || !custody.contains("RootSlotCustody::Vacant")
        || !custody.contains("RootSlotCustody::Owned")
        || !custody.contains("RootSlotCustody::Inherited")
    {
        return Err(());
    }
    let map = function_body(process_memory, "map_page");
    let refusal = map
        .find("root_custody == RootSlotCustody::Inherited")
        .ok_or(())?;
    let counter = map.find("PT_ROOT_SLOT_REFUSED").ok_or(())?;
    let refused = map
        .find("return Err(\"Cannot map into an inherited root page-table slot\");")
        .ok_or(())?;
    let reserve = map.find(".try_reserve(1)").ok_or(())?;
    let publish = map.find("mapper.map_to_with_table_flags(").ok_or(())?;
    let claim = map.find("self.owned_root_slots.insert(").ok_or(())?;
    if !(refusal < counter
        && counter < refused
        && refused < reserve
        && reserve < publish
        && publish < claim)
    {
        return Err(());
    }
    let flags = function_body(process_memory, "update_page_flags");
    let flags_refusal = flags.find("RootSlotCustody::Inherited").ok_or(())?;
    if !flags.contains("PT_ROOT_SLOT_REFUSED") || flags_refusal > flags.find(".unmap(").ok_or(())? {
        return Err(());
    }
    let mask = code_mask(process_memory);
    if code_offsets(process_memory, &mask, "owned_root_slots.insert(").len() != 2 {
        return Err(());
    }
    let cost = function_body(process_memory, "gate_sentinel_cost");
    let sentinels = function_body(process_memory, "gate_sentinels");
    if !cost.contains("RootSlotCustody::Inherited")
        || !sentinels.contains("(sentinels.len() == count).then_some(sentinels)")
    {
        return Err(());
    }
    Ok(())
}

fn validate_process_page_table_exit_paths_are_minimal(
    sources: &[(String, String)],
) -> Result<(), ()> {
    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    if !process_memory.contains("pub(crate) fn retire_bounded") {
        return Err(());
    }
    let task = source(sources, "kernel/src/task/process_task.rs");
    if !task.contains("fn reclaim_bounded(&mut self)") {
        return Err(());
    }
    let process_drop = process_memory
        .find("impl Drop for ProcessPageTable")
        .ok_or(())?;
    let unpublished_drop = process_memory
        .find("impl Drop for UnpublishedPageTable")
        .ok_or(())?;
    for body in [
        function_body(process_memory, "abandon"),
        function_body(&process_memory[process_drop..], "drop"),
        function_body(&process_memory[unpublished_drop..], "drop"),
        function_body(process_memory, "release_leaf_record"),
        function_body(process_memory, "release_mapped_leaves"),
        function_body(process_memory, "retire_bounded"),
        function_body(task, "reclaim_bounded"),
    ] {
        for forbidden in [
            "log::",
            "serial_println!",
            "format!",
            "vec!",
            "Vec::",
            "alloc::",
            "reserve(",
            "try_reserve(",
        ] {
            if body.contains(forbidden) {
                eprintln!("R7 page-table exit path gained {forbidden}");
                return Err(());
            }
        }
    }
    Ok(())
}

fn validate_leaf_custody(sources: &[(String, String)]) -> Result<(), ()> {
    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let allocator = source(sources, "kernel/src/memory/frame_allocator.rs");
    let metadata = source(sources, "kernel/src/memory/frame_metadata.rs");
    let fork = source(sources, "kernel/src/process/fork.rs");
    let interrupts = source(sources, "kernel/src/interrupts.rs");
    let arm_exception = source(sources, "kernel/src/arch_impl/aarch64/exception.rs");
    let stack = source(sources, "kernel/src/memory/stack.rs");
    let graphics = source(sources, "kernel/src/syscall/graphics.rs");
    let manager = source(sources, "kernel/src/process/manager.rs");
    let arm_elf = source(sources, "kernel/src/arch_impl/aarch64/elf.rs");

    let map = function_body(process_memory, "map_page");
    let reserve = map.find(".try_reserve(1)").ok_or(())?;
    let acquire = map.find("acquire_leaf_mapping(frame)").ok_or(())?;
    let insert = map.find("self.leaves.records.insert(").ok_or(())?;
    let publish = map.find("mapper.map_to_with_table_flags(").ok_or(())?;
    let committed = map.find("LEAF_MAPPINGS_RECORDED").ok_or(())?;
    if !(reserve < acquire && acquire < insert && insert < publish && publish < committed)
        || !map.contains("self.leaves.records.remove(index);")
        || !map.contains("frame_decref(frame)")
    {
        return Err(());
    }

    let unmap = function_body(process_memory, "unmap_page");
    let release = function_body(process_memory, "release_leaf_record");
    let drain = function_body(process_memory, "release_mapped_leaves");
    if !process_memory.contains("struct LeafRecord")
        || !process_memory.contains("page: u64")
        || !process_memory.contains("binary_search_by_key(&page")
        || !unmap.contains("self.leaves.search(page_addr)")
        || !unmap.contains("Self::release_leaf_record(record, frame)")
        || !release.contains("frame_decref(frame)")
        || !release.contains("deallocate_leaf_frame(frame) == ReturnOutcome::Returned")
        || !drain.contains("self.walk_mapped_pages(")
        || !drain.contains("records.binary_search_by_key(&page")
        || !drain.contains("self.leaves.records.clear();")
        || !drain.contains("self.leaves.released = true;")
    {
        return Err(());
    }

    let decref = function_body(allocator, "decref_leaf_mapping");
    let returned = function_body(allocator, "return_lease");
    if !decref.contains("if refs == 0 {")
        || !decref.contains("LEAF_DECREF_UNREGISTERED")
        || !decref.contains("return false;")
        || decref.contains("return true;")
        || !returned.contains("if slot.leaf_refs.load(Ordering::Acquire) != 0 {")
        || !returned.contains("ReturnOutcome::RefusedLiveLeaf")
        || !metadata.contains("decref_leaf_mapping(frame)")
    {
        return Err(());
    }

    for converted in [fork, interrupts, arm_exception] {
        let mask = code_mask(converted);
        if !code_offsets(converted, &mask, "frame_register(").is_empty()
            || !code_offsets(converted, &mask, "frame_incref(").is_empty()
        {
            return Err(());
        }
    }
    if stack.matches("register_external_leaf_frame(frame)?;").count() != 2
        || !graphics.contains("register_external_leaf_span(")
    {
        return Err(());
    }

    let unpublished = process_memory
        .find("impl Drop for UnpublishedPageTable")
        .ok_or(())?;
    let unpublished_drop = function_body(&process_memory[unpublished..], "drop");
    if unpublished_drop.find("page_table.release_mapped_leaves();").ok_or(())?
        > unpublished_drop
            .find("page_table.retire_bounded(self.pid, &mut budget)")
            .ok_or(())?
        || !arm_elf.contains("deallocate_leaf_frame(new_frame)")
    {
        return Err(());
    }
    let exec_bodies = module_function_bodies(manager);
    let mut arm_execs = Vec::new();
    for name in ["exec_process", "exec_process_with_argv"] {
        for body in exec_bodies.get(name).into_iter().flatten() {
            if body.contains("[ARM64]") {
                arm_execs.push(*body);
            }
        }
    }
    if arm_execs.len() != 2 {
        return Err(());
    }
    for body in arm_execs {
        let guard = body.find("UnpublishedPageTable::new(").ok_or(())?;
        let load = body.find("load_elf_into_page_table(").ok_or(())?;
        let supersede = body.find("process.page_table.take()").ok_or(())?;
        let publish = body.find("new_page_table.publish()").ok_or(())?;
        if !(guard < load && load < supersede && supersede < publish)
            || body.matches("UnpublishedPageTable::new(").count() != 1
            || body.matches("new_page_table.publish()").count() != 1
        {
            return Err(());
        }
    }

    let gate = function_body(process_memory, "page_table_custody_disposition_gate_test");
    for required in [
        "corrupt_executable_fixture()",
        "Err(\"Segment data out of bounds\")",
        "drop(unpublished);",
        "used_after != used_before",
        "LEAF_MAPPINGS_RECORDED.aggregate() != leaf_recorded_before + 1",
        "LEAF_MAPPINGS_RELEASED.aggregate() != leaf_released_before + 1",
        "LEAF_FRAMES_RETURNED.aggregate() != leaf_returned_before + 1",
        "PT_TABLE_FRAMES_RETURNED.aggregate() != tables_returned_before + 4",
        "PT_ROOTS_RETIRED.aggregate() != roots_retired_before + 1",
        "[EXEC_FAILED_RELEASE_ORACLE:aarch64:",
    ] {
        if !gate.contains(required) {
            return Err(());
        }
    }

    for body in [release, drain, decref, function_body(allocator, "deallocate_leaf_frame")] {
        for forbidden in [
            "log::",
            "serial_println!",
            "format!",
            "vec!",
            "Vec::",
            "alloc::",
            "reserve(",
            "try_reserve(",
        ] {
            if body.contains(forbidden) {
                return Err(());
            }
        }
    }
    Ok(())
}

/// Q2's asymmetric cached-root proof is intentional: x86 has no per-thread
/// cached CR3 writer. Discover assignments structurally, then require every
/// writer to belong to the one aarch64 cache helper span.
fn validate_cached_ttbr0_single_writer(sources: &[(String, String)]) -> Result<(), ()> {
    const ALLOWED_PATH: &str = "kernel/src/arch_impl/aarch64/context_switch.rs";
    let mut writers = Vec::new();
    for (path, module) in sources {
        let mask = code_mask(module);
        writers.extend(
            assignment_offsets(module, &mask, "cached_ttbr0")
                .into_iter()
                .map(|offset| (path.as_str(), offset)),
        );
    }
    if writers.len() != 1 || writers[0].0 != ALLOWED_PATH {
        return Err(());
    }
    let module = source(sources, ALLOWED_PATH);
    function_span(module, "cache_thread_ttbr0")
        .contains(&writers[0].1)
        .then_some(())
        .ok_or(())
}

/// The shared proof must dispatch to both architectures' real hardware and
/// shadow readers without taking the scheduler or process-manager lock in its
/// lock-free phase.
fn validate_root_proof_architecture_legs(sources: &[(String, String)]) -> Result<(), ()> {
    let process = source(sources, "kernel/src/task/process_task.rs");
    let shared = function_body(process, "lock_free_root_proof");
    if !shared.contains("self.any_root_matches(local_hardware_root())")
        || !shared.contains("shadow_root_is_live(self, self.after_epoch.online_mask)")
        || shared.contains("with_scheduler")
        || shared.contains("process::manager")
    {
        return Err(());
    }

    let bodies = module_function_bodies(process);
    let hardware = bodies.get("local_hardware_root").ok_or(())?;
    let shadow = bodies.get("shadow_root_is_live").ok_or(())?;
    let clear_shadow = bodies.get("clear_shadow_root").ok_or(())?;
    if hardware.len() != 2 || shadow.len() != 2 || clear_shadow.len() != 1 {
        return Err(());
    }
    let clear_shadow = clear_shadow[0];
    let defer = function_body(process, "defer_process_resources");
    if clear_shadow
        .matches("crate::per_cpu::get_saved_process_cr3()")
        .count()
        != 1
        || clear_shadow
            .matches("crate::per_cpu::set_saved_process_cr3(0)")
            .count()
            != 1
        || defer.matches("clear_shadow_root(").count() != 1
    {
        return Err(());
    }
    let arm_hardware = hardware
        .iter()
        .find(|body| body.contains("local_ttbr0_root()"))
        .ok_or(())?;
    let x86_hardware = hardware
        .iter()
        .find(|body| body.contains("x86_64::registers::control::Cr3::read()"))
        .ok_or(())?;
    let arm_shadow = shadow
        .iter()
        .find(|body| body.contains("is_ttbr0_root_live_in_mask("))
        .ok_or(())?;
    let x86_shadow = shadow
        .iter()
        .find(|body| body.contains("get_next_cr3()"))
        .ok_or(())?;
    if !x86_hardware.contains(".start_address()")
        || !x86_hardware.contains(".as_u64()")
        || !arm_shadow.contains("page_table.level_4_frame().start_address().as_u64()")
        || !x86_shadow.contains("online_mask & 1 != 0")
        || !x86_shadow.contains("get_saved_process_cr3()")
    {
        return Err(());
    }

    let spans = rendered_item_spans(&item_spans(process, &code_mask(process)));
    for (body, needle, expected_path) in [
        (*arm_hardware, "local_ttbr0_root()", "#[cfg(target_arch=aarch64)] fn local_hardware_root"),
        (*x86_hardware, "x86_64::registers::control::Cr3::read()", "#[cfg(target_arch=x86_64)] fn local_hardware_root"),
        (*arm_shadow, "is_ttbr0_root_live_in_mask(", "#[cfg(target_arch=aarch64)] fn shadow_root_is_live"),
        (*x86_shadow, "get_saved_process_cr3()", "#[cfg(target_arch=x86_64)] fn shadow_root_is_live"),
    ] {
        let offsets = code_offsets(body, &code_mask(body), needle);
        let base = body.as_ptr() as usize - process.as_ptr() as usize;
        if offsets.len() != 1 || item_path_at(&spans, base + offsets[0]) != expected_path {
            return Err(());
        }
    }
    Ok(())
}

/// Exact production drain membership. Both x86 calls are normal-context sites;
/// the interrupt-return function is explicitly excluded.
fn validate_deferred_reclaim_drain_sites(
    sources: &[(String, String)],
) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    record(
        &mut failures,
        "deferred-reclaim drain callers",
        validate_census(
            &census(sources, |module, mask| {
                call_offsets(module, mask, "reclaim_deferred_process_resources")
            }),
            DEFERRED_RECLAIM_DRAIN_SITES,
        ),
    );

    let context_switch = source(sources, "kernel/src/interrupts/context_switch.rs");
    let interrupt_return = function_body(context_switch, "check_need_resched_and_switch");
    let idle = function_body(context_switch, "idle_loop");
    if !call_offsets(
        interrupt_return,
        &code_mask(interrupt_return),
        "reclaim_deferred_process_resources",
    )
    .is_empty()
        || call_offsets(
            idle,
            &code_mask(idle),
            "reclaim_deferred_process_resources",
        )
        .len()
            != 1
    {
        failures.push("x86 drain entered interrupt-return context or left idle_loop".to_owned());
    }

    let process_mod = source(sources, "kernel/src/process/mod.rs");
    let exit = function_body(process_mod, "exit_process_and_retire");
    let exit_mask = code_mask(exit);
    let calls = call_offsets(exit, &exit_mask, "reclaim_deferred_process_resources");
    if calls.len() != 1 {
        failures.push("x86 post-exit drain site changed".to_owned());
    } else {
        let cfg = code_offsets(exit, &exit_mask, "#[cfg")
            .into_iter()
            .filter(|offset| *offset < calls[0])
            .next_back();
        let cfg_is_x86 = cfg.and_then(|offset| {
            let open = exit[offset..].find('[').map(|relative| offset + relative)?;
            let close = matching_bracket(exit.as_bytes(), &exit_mask, open)?;
            Some(&exit[offset..=close])
        });
        if !cfg_is_x86.is_some_and(|attribute| {
            attribute.contains("target_arch") && attribute.contains("\"x86_64\"")
        }) {
            failures.push("post-exit drain lost its x86-only statement cfg".to_owned());
        }
    }
    failures.is_empty().then_some(()).ok_or(failures)
}

/// The interrupt-return hook may stamp one epoch and do nothing before it.
fn validate_x86_epoch_stamp_is_minimal(sources: &[(String, String)]) -> Result<(), ()> {
    let context_switch = source(sources, "kernel/src/interrupts/context_switch.rs");
    let body = function_body(context_switch, "check_need_resched_and_switch");
    let mask = code_mask(body);
    let stamps = call_offsets(body, &mask, "note_scheduling_epoch");
    if stamps.len() != 1 {
        return Err(());
    }
    let prefix = &body[..stamps[0]];
    let prefix_mask = code_mask(prefix);
    if ["log::", "serial_println!", "format!", "Vec::", "alloc::"]
        .iter()
        .any(|forbidden| !code_offsets(prefix, &prefix_mask, forbidden).is_empty())
        || !call_offsets(prefix, &prefix_mask, "lock").is_empty()
    {
        return Err(());
    }
    let statements = block_statements(body).ok_or(())?;
    if !normalized_code(statements)
        .starts_with("crate::task::scheduler::note_scheduling_epoch(0);")
    {
        return Err(());
    }
    Ok(())
}

fn validate_pr1c_retirement_oracles(sources: &[(String, String)]) -> Result<(), ()> {
    let provider = source(sources, "kernel/src/tracing/providers/teardown.rs");
    let gate = function_body(provider, "fork_exit_defer_reclaim_pairing_test");
    let sentinels = function_body(provider, "map_retire_sentinels");
    for required in [
        "map_retire_sentinels(child_page_table.as_mut())",
        "let allocator_used_before = frame_allocator_used_frames();",
        "if allocator_used_after != allocator_used_before {",
        "retire leak oracle did not return frame accounting to baseline",
        "let pending_old_pid = pairing_child_pids[0];",
        "let pending_old_roots = u64::from(has_pending_old_root);",
        "if counts.roots_retired != 1 + pending_old_roots {",
        "if counts.table_frames_recorded != expected_tables + pending_old_tables {",
        "if counts.table_frames_returned != counts.table_frames_recorded + counts.roots_retired {",
        "counts.table_frames_lost != 0",
        "retire cohort per-PID anti-vacuity table count was not exact",
        "retire cohort per-PID committed return equality failed",
        "refusal_counters_after != refusal_counters_before",
        "PT_ROOT_ABANDONED_NO_ARCH",
        "saturating_sub(no_arch_before)",
        "[PT_RETIRE_ORACLE:aarch64:cycles=64:",
        "[PT_LEAF_ORACLE:aarch64:cycles=64:",
        "let expected_leaves = (RETIRE_SENTINEL_SUBTREES * pairing_child_pids.len()) as u64;",
        "leaf_mappings_recorded_delta != expected_leaves",
        "leaf_mappings_released_delta != expected_leaves",
        "leaf_frames_returned_delta != expected_leaves",
        "leaf leak oracle committed-effect accounting was not exact",
        "let fork_result = manager.fork_process_with_page_table(",
        "let cohort_recorded = expected_tables * pairing_child_pids.len() as u64",
        "+ expected_pending_old_tables;",
        "let allocator_balance = allocator_used_after as i64 - allocator_used_before as i64;",
        "|| no_arch_delta != 0",
        "[TEST:process:x86_retire_cohort:PASS]",
        "[PT_RETIRE_COHORT:x86:children={}:retired={}:returned={}:recorded={}:lost={}:no_arch={}:undecided={}:mid_retire={}:balance={}]",
        "pairing sentinel hierarchy cost changed between children",
    ] {
        if !gate.contains(required) {
            return Err(());
        }
    }
    if !sentinels.contains(".gate_sentinels(RETIRE_SENTINEL_SUBTREES)")
        || !sentinels.contains("page_table.gate_page_is_unmapped(page)")
        || !sentinels.contains("expected += sentinel.table_frames as u64;")
        || gate.contains("let expected_tables = 9")
    {
        return Err(());
    }
    Ok(())
}

fn validate_process_page_table_runtime_oracle(sources: &[(String, String)]) -> Result<(), ()> {
    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let gate = function_body(process_memory, "page_table_custody_disposition_gate_test");
    let normalized_gate = normalized_code(gate);
    if !gate.contains("terminated.abandon(AbandonReason::AlreadyTerminated);")
        || !gate.contains("drop(undecided);")
        || !gate.contains("after_abandon[3] != start[3] + 1")
        || !gate.contains("after_drop[4] != after_abandon[4] + 1")
        || !gate.contains("retire_with_free_list_contended(")
        || !gate.contains("PT_RETIRE_FRAMES_LOST.aggregate() != lost_before + 1")
        || !gate.contains("republish_frame_for_gate(root)")
        || !gate.contains("for sentinel in sentinels")
        || !gate.contains("retiring.map_page(page, frame, flags)")
        || !gate.contains("let recorded = retiring.recorded_table_frames_for_gate();")
        || !gate.contains("if recorded != expected_recorded {")
        || !gate.contains("retiring.gate_sentinels(X86_CUSTODY_SENTINEL_COUNT)")
        || !gate.contains("retiring.gate_page_is_unmapped(page)")
        || !gate.contains("O3: sentinel address was already mapped")
        || !gate.contains("retiring.gate_inherited_slot_address()")
        || !gate.contains("O4: inherited root slot was not refused fail-closed")
        || !gate.contains("PT_ROOT_SLOT_REFUSED.aggregate() != refused_before + 1")
        || !gate.contains("retiring.release_mapped_leaves();")
        || !gate.contains("retiring.retire_bounded(u64::MAX - 3, &mut budget)")
        || !normalized_gate.contains("if after_retire[2] != after_drop[2] ||")
        || !gate.contains("PT_ROOTS_RETIRED.aggregate() != retired_before + 1")
        || !gate.contains("!= returned_before + recorded as u64 + 1")
        || !gate.contains("PT_RETIRE_FRAMES_LOST.aggregate() != lost_before")
        || !gate.contains("PT_RETIRE_BUDGET_REQUEUED.aggregate() != requeued_before")
        || gate.contains("no_arch.abandon(AbandonReason::NoArchPipeline);")
        || gate.contains("after_no_arch[2] != after_drop[2] + 1")
        || !gate.contains("PT_TABLE_FRAMES_RETURNED.aggregate()")
        || gate.matches("free_list_len_for_gate()").count() != 9
        || gate.contains("&& false")
        || gate.contains("|| true")
    {
        return Err(());
    }

    let registry = source(sources, "kernel/src/test_framework/registry.rs");
    let registry_mask = code_mask(registry);
    let registrations = identifier_offsets(
        registry,
        &registry_mask,
        "page_table_custody_disposition_gate_test",
    );
    if registrations.len() != 1 || registry.contains("page_table_custody_disposition_gate_test as")
    {
        return Err(());
    }
    let test_def = enclosing_test_def(registry, &registry_mask, registrations[0]).ok_or(())?;
    if !test_def.contains("name: \"page_table_custody_disposition_gate\"")
        || !test_def.contains("arch: Arch::Aarch64")
        || !test_def.contains("stage: TestStage::SerialBoot")
    {
        return Err(());
    }

    let memory = source(sources, "kernel/src/memory/mod.rs");
    if code_offsets(
        memory,
        &code_mask(memory),
        "process_memory::run_x86_page_table_custody_gate();",
    )
    .len()
        != 1
    {
        return Err(());
    }

    let main = source(sources, "kernel/src/main.rs");
    if code_offsets(
        main,
        &code_mask(main),
        "teardown::run_x86_retire_cohort_gate();",
    )
    .len()
        != 1
    {
        return Err(());
    }
    if code_offsets(
        main,
        &code_mask(main),
        "teardown::run_x86_exec_cohort_gate();",
    )
    .len()
        != 1
    {
        return Err(());
    }
    let teardown = source(sources, "kernel/src/tracing/providers/teardown.rs");
    let retire_cohort_wrapper = function_body(teardown, "run_x86_retire_cohort_gate");
    let wrapper_mask = code_mask(retire_cohort_wrapper);
    if call_offsets(
        retire_cohort_wrapper,
        &wrapper_mask,
        "fork_exit_defer_reclaim_pairing_test",
    )
    .len()
        != 1
        || !retire_cohort_wrapper.contains("assert!(result.is_pass()")
        || retire_cohort_wrapper.contains("x86_retire_cohort:PASS")
    {
        return Err(());
    }
    let exec_cohort_wrapper = function_body(teardown, "run_x86_exec_cohort_gate");
    let exec_wrapper_mask = code_mask(exec_cohort_wrapper);
    if call_offsets(
        exec_cohort_wrapper,
        &exec_wrapper_mask,
        "exec_supersede_cohort_test",
    )
    .len()
        != 1
        || !exec_cohort_wrapper.contains("assert!(result.is_pass()")
        || exec_cohort_wrapper.contains("x86_exec_cohort:PASS")
    {
        return Err(());
    }
    let harness = repo_text("docker/qemu/run-x86-boot-tests.sh");
    (harness.contains("page_table_custody_disposition_gate:PASS")
        && harness.contains("x86_retire_cohort:PASS")
        && harness.contains("x86_exec_cohort:PASS")
        && harness.contains("[PT_CUSTODY_COUNTERS:x86:recorded=11:no_proof=0:no_arch=0:terminated=1:undecided=1:retired=1:returned=10:lost=0:requeued=0]")
        && harness.contains("[PT_RETIRE_COHORT:x86:children=64:retired=65:returned=642:recorded=577:lost=0:no_arch=0:undecided=0:mid_retire=0:balance=0]")
        && harness.contains("[PT_EXEC_COHORT:x86:children=16:superseded=3:roots=64:returned=640:recorded=576:lost=0:leaf_recorded=192:leaf_released=192:leaf_returned=192:custody_refused=0:decref_unregistered=0:undecided=0:mid_retire=0:no_arch=0:balance=0]")
        && harness
            .matches("page_table_custody_disposition_gate:PASS")
            .count()
            == 2
        && harness.matches("x86_retire_cohort:PASS").count() == 2
        && harness.matches("x86_exec_cohort:PASS").count() == 2
        && harness.matches("PT_CUSTODY_COUNTERS:x86:").count() == 1
        && harness.matches("PT_RETIRE_COHORT:x86:").count() == 1
        && harness.matches("PT_EXEC_COHORT:x86:").count() == 1
        && harness.contains("grep -h -c 'Refusing to map'"))
        .then_some(())
        .ok_or(())
}

#[test]
fn process_page_table_custody_ratchets_are_exact() {
    let sources = rust_sources_below("kernel/src");
    let mut failures = Vec::new();
    record(
        &mut failures,
        "R2 process mapper recorder was bypassed",
        validate_process_table_recorder(&sources),
    );
    record_unit(
        &mut failures,
        "R4 ProcessPageTable Drop gained a freeing path",
        validate_process_page_table_drop_is_non_freeing(&sources),
    );
    record(
        &mut failures,
        "R3 process page-table retirement escaped the proof-gated site",
        validate_process_page_table_retire_site(&sources),
    );
    record(
        &mut failures,
        "R5 process page-table disposition set changed",
        validate_process_page_table_dispositions(&sources),
    );
    record_unit(
        &mut failures,
        "R6 process page-table counter inventory changed",
        validate_process_page_table_counter_inventory(&sources),
    );
    record_unit(
        &mut failures,
        "R7 process page-table exit path gained log/format/heap work",
        validate_process_page_table_exit_paths_are_minimal(&sources),
    );
    record_unit(
        &mut failures,
        "PR-2 virtual-page leaf custody or fail-closed release changed",
        validate_leaf_custody(&sources),
    );
    record_unit(
        &mut failures,
        "PR-3 root-slot custody guard changed",
        validate_root_slot_custody(&sources),
    );
    record_unit(
        &mut failures,
        "O2/G-H process page-table runtime oracle became vacuous",
        validate_process_page_table_runtime_oracle(&sources),
    );
    record_unit(
        &mut failures,
        "PR-1c leak or per-PID retirement oracle became vacuous",
        validate_pr1c_retirement_oracles(&sources),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn frame_ledger_return_and_initialization_ratchets_are_exact() {
    let sources = rust_sources_below("kernel/src");
    let mut failures = Vec::new();
    record_unit(
        &mut failures,
        "R7 frame-ledger hot path gained log/format/heap work",
        validate_frame_ledger_hot_paths(&sources),
    );
    record_unit(
        &mut failures,
        "frame ledger regained eager or post-publication allocation",
        validate_frame_ledger_bounded_boot_allocation(&sources),
    );
    record(
        &mut failures,
        "R1 frame-return choke point changed",
        validate_frame_return_choke_point(&sources),
    );
    record_unit(
        &mut failures,
        "R9 ARM/x86 frame-ledger boot order changed",
        validate_frame_ledger_boot_order(&sources),
    );
    record(
        &mut failures,
        "R9 frame-ledger initialization moved",
        validate_frame_ledger_init(&sources),
    );
    record_unit(
        &mut failures,
        "frame-custody runtime oracle became vacuous",
        validate_frame_ledger_runtime_oracles(&sources),
    );
    record_unit(
        &mut failures,
        "boot-test oracle gained an always-true/always-false vacuity shape",
        validate_no_vacuous_test_conditions(&sources),
    );
    record_unit(
        &mut failures,
        "x86 frame-custody harness became vacuous",
        validate_x86_frame_custody_harness(&repo_text("docker/qemu/run-x86-boot-tests.sh")),
    );
    record_unit(
        &mut failures,
        "workqueue probe lost its progress key or regained a timing-based wait",
        validate_workqueue_progress_wait(source(
            &sources,
            "kernel/src/test_framework/registry.rs",
        )),
    );

    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    record_unit(
        &mut failures,
        "frame-ledger counter inventory changed",
        validate_frame_ledger_counter_inventory(provider),
    );
    for counter in [
        "FRAME_RETURN_REFUSED_DOUBLE",
        "FRAME_RETURN_REFUSED_STALE",
        "FRAME_RETURN_REFUSED_NEVER_ALLOCATED",
        "FRAME_RETURN_REFUSED_UNTRACKED",
        "FRAME_DUPLICATE_ALLOC_REFUSED",
        "FRAME_LOST_CONTENDED",
        "FRAME_RETURN_REFUSED_LIVE_LEAF",
    ] {
        check(
            &mut failures,
            &format!("counter declaration changed: {counter}"),
            provider.matches(&format!("counter!({counter},")).count()
                + provider
                    .matches(&format!("counter!(\n    {counter},"))
                    .count()
                == 1,
        );
        match provider.find(counter) {
            Some(declaration) => {
                let prefix = &provider[declaration.saturating_sub(80)..declaration];
                check(
                    &mut failures,
                    &format!("counter became conditional: {counter}"),
                    !prefix.contains("#[cfg("),
                );
            }
            None => failures.push(format!("missing counter {counter}")),
        }
    }
    check(
        &mut failures,
        "COUNTER_COUNT is no longer 72",
        provider.contains("pub const COUNTER_COUNT: usize = 72;"),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn current_teardown_bypass_surface_is_exact() {
    let sources = rust_sources_below("kernel/src");
    let mut failures = Vec::new();

    record(
        &mut failures,
        "Process::terminate callers",
        validate_census(
            &census(&sources, |source, mask| {
                method_call_offsets(source, mask, "terminate")
            }),
            TERMINATE_CALLS,
        ),
    );
    record(
        &mut failures,
        "Process::terminate_minimal callers",
        validate_census(
            &census(&sources, |source, mask| {
                method_call_offsets(source, mask, "terminate_minimal")
            }),
            TERMINATE_MINIMAL_CALLS,
        ),
    );

    let init_sites = census(&sources, |source, mask| {
        code_offsets(source, mask, "ProcessId::new(1)")
    });
    let test_sites: Census = init_sites
        .iter()
        .filter(|((path, _), _)| path == "kernel/src/test_userspace.rs")
        .map(|(anchor, count)| (anchor.clone(), *count))
        .collect();
    let production_sites: Census = init_sites
        .iter()
        .filter(|((path, _), _)| path != "kernel/src/test_userspace.rs")
        .map(|(anchor, count)| (anchor.clone(), *count))
        .collect();
    record(
        &mut failures,
        "production PID-1 literals",
        validate_census(&production_sites, PRODUCTION_INIT_PID_SITES),
    );
    record(
        &mut failures,
        "test_minimal_userspace PID-1 allowlist",
        validate_census(&test_sites, TEST_INIT_PID_SITES),
    );
    let test_userspace = source(&sources, "kernel/src/test_userspace.rs");
    let test_minimal_count = test_userspace
        .matches("pub fn test_minimal_userspace()")
        .count();
    check(
        &mut failures,
        "test_minimal_userspace must remain uniquely nameable",
        test_minimal_count == 1,
    );
    if test_minimal_count == 1 {
        check(
            &mut failures,
            "the three test PID-1 sites must remain in test_minimal_userspace",
            function_body(test_userspace, "test_minimal_userspace")
                .matches("ProcessId::new(1)")
                .count()
                == 3,
        );
    }

    record(
        &mut failures,
        "terminate_process_threads callers",
        validate_census(
            &census(&sources, |source, mask| {
                method_call_offsets(source, mask, "terminate_process_threads")
            }),
            QUARANTINE_CALLS,
        ),
    );
    record(
        &mut failures,
        "kernel_stack_allocation ownership mutations",
        validate_census(
            &census(&sources, |source, mask| {
                code_offsets(source, mask, ".kernel_stack_allocation =")
            }),
            KERNEL_STACK_MUTATIONS,
        ),
    );
    record(
        &mut failures,
        "enqueue_process_reclaim caller ratchet changed",
        validate_reclaim_enqueue_callers(&sources),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn v3_structural_closures_are_exact() {
    let sources = rust_sources_below("kernel/src");
    let mut failures = Vec::new();
    record(
        &mut failures,
        "process-exit entry-point ratchet changed",
        validate_exit_process_entry_points(&sources),
    );
    record(
        &mut failures,
        "the nine P0 blocking primitives changed",
        validate_blocking_primitives(&sources),
    );
    record(
        &mut failures,
        "thread_group_id production writers changed",
        validate_group_writes(&sources),
    );
    record(
        &mut failures,
        "raw scheduler-lock acquisitions outside the instrumented wrappers",
        validate_census(
            &census(&sources, |source, mask| {
                identifier_suffix_offsets(source, mask, "SCHEDULER")
                    .into_iter()
                    .filter(|offset| {
                        code_follows(source, mask, offset + "SCHEDULER".len(), ".lock()")
                            || code_follows(
                                source,
                                mask,
                                offset + "SCHEDULER".len(),
                                ".try_lock()",
                            )
                    })
                    .collect()
            }),
            RAW_SCHEDULER_LOCK_SITES,
        ),
    );
    record(
        &mut failures,
        "btrt::on_process_exit callers",
        validate_census(
            &census(&sources, |source, mask| {
                call_offsets(source, mask, "on_process_exit")
            }),
            BTRT_PROCESS_EXIT_REPORTS,
        ),
    );

    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    check(
        &mut failures,
        "EXIT_SGI_SENT counter declaration changed",
        provider.matches("counter!(EXIT_SGI_SENT,").count() == 1,
    );
    check(
        &mut failures,
        "EXIT_KICK_PUBLISHED counter declaration changed",
        provider.matches("counter!(EXIT_KICK_PUBLISHED,").count() == 1,
    );
    record_unit(
        &mut failures,
        "EXIT_SGI_SENT escaped the teardown-only producer",
        validate_exit_sgi_is_teardown_only(&sources),
    );
    let expedite_present = scheduler.contains("fn send_exit_expedite_sgi(");
    check(
        &mut failures,
        "send_exit_expedite_sgi disappeared",
        expedite_present,
    );
    if expedite_present {
        let expedite = function_body(scheduler, "send_exit_expedite_sgi");
        check(
            &mut failures,
            "send_exit_expedite_sgi EXIT_SGI_SENT count changed",
            expedite.matches("EXIT_SGI_SENT").count() == 1,
        );
        check(
            &mut failures,
            "send_exit_expedite_sgi EXIT_KICK_PUBLISHED count changed",
            expedite
                .matches("trace_count!(EXIT_KICK_PUBLISHED)")
                .count()
                == 1,
        );
        check(
            &mut failures,
            "exit kick must be published before the SGI is sent",
            matches!(
                (
                    expedite.find("slot.publish("),
                    expedite.find("gic::send_sgi(")
                ),
                (Some(publish), Some(send)) if publish < send
            ),
        );
        check(
            &mut failures,
            "send_exit_expedite_sgi regained current_thread coupling",
            !expedite.contains("current_thread"),
        );
    }
    check(
        &mut failures,
        "send_exit_expedite_sgi occurrence count changed",
        scheduler.matches("send_exit_expedite_sgi(").count() == 1,
    );
    check(
        &mut failures,
        "KickSlot disappeared",
        provider.contains("struct KickSlot"),
    );
    check(
        &mut failures,
        "KickSlot pid field changed",
        provider.contains("pub(crate) pid: AtomicU64"),
    );
    check(
        &mut failures,
        "KickSlot at field changed",
        provider.contains("pub(crate) at: AtomicU64"),
    );
    check(
        &mut failures,
        "KickSlot state field changed",
        provider.contains("pub(crate) state: AtomicU64"),
    );
    check(
        &mut failures,
        "provider gained an EXIT_SGI_SENT producer",
        !provider.contains("trace_count!(EXIT_SGI_SENT"),
    );
    check(
        &mut failures,
        "provider gained an EXIT_KICK_PUBLISHED producer",
        !provider.contains("trace_count!(EXIT_KICK_PUBLISHED"),
    );

    let process_mod = source(&sources, "kernel/src/process/mod.rs");
    check(
        &mut failures,
        "RetirementReceipt is no longer crate-visible",
        process_mod.contains("pub(crate) struct RetirementReceipt"),
    );
    check(
        &mut failures,
        "RetirementReceipt became public",
        !process_mod.contains("pub struct RetirementReceipt"),
    );
    check(
        &mut failures,
        "RetirementReceipt::from_reclaim became public",
        !process_mod.contains("pub fn from_reclaim"),
    );
    let receipt_drop_present = process_mod.contains("fn drop(");
    check(
        &mut failures,
        "RetirementReceipt Drop disappeared",
        receipt_drop_present,
    );
    if receipt_drop_present {
        check(
            &mut failures,
            "RetirementReceipt Drop no longer enqueues reclaim",
            function_body(process_mod, "drop").contains("enqueue_process_reclaim("),
        );
    }

    let process = source(&sources, "kernel/src/process/process.rs");
    for state in ["Absent", "Pending", "Claimed", "Completed"] {
        check(
            &mut failures,
            &format!("process teardown state disappeared: {state}"),
            process.contains(state),
        );
    }
    check(
        &mut failures,
        "report_marker was restored",
        !process.contains("report_marker"),
    );
    check(
        &mut failures,
        "claim_exit_slot was restored",
        !process.contains("claim_exit_slot"),
    );
    check(
        &mut failures,
        "a process-local record_exit helper was restored",
        !process.contains("fn record_exit"),
    );
    check(
        &mut failures,
        "process.rs gained a record_exit call outside the exit-tally seam",
        process.matches("record_exit(").count()
            == process
                .matches("crate::task::exit_tally::record_exit(")
                .count(),
    );
    check(
        &mut failures,
        "scheduler-side record_exit was restored",
        !source(&sources, "kernel/src/task/process_task.rs").contains("record_exit("),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn phase_one_retirement_fence_and_lock_domains_are_structural() {
    let sources = rust_sources_below("kernel/src");
    let mut failures = Vec::new();
    let process = source(&sources, "kernel/src/task/process_task.rs");
    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    let manager = source(&sources, "kernel/src/process/manager.rs");
    let ttbr0 = source(&sources, "kernel/src/arch_impl/aarch64/ttbr0.rs");
    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");

    assert_eq!(
        process.matches("static PENDING_PROCESS_RECLAIMS:").count(),
        1
    );
    assert_eq!(
        process.matches("static PARKED_PROCESS_RECLAIMS:").count(),
        1
    );
    assert!(process.contains("last_pass: u32"));
    assert!(process.contains("proof_failures: u8"));
    assert!(process.contains("parked: Option<ParkRecord>"));
    assert!(process.contains("fence_at_park: scheduler::RetirementFence"));
    assert!(process.contains("row_epoch_at_park: u64"));
    assert!(process.contains("age_epoch_sum_at_park: u64"));
    assert!(process.contains("const PARK_AGE_BACKSTOP_EPOCHS: u64 = 64"));

    let drain = function_body(process, "reclaim_deferred_process_resources");
    assert_eq!(drain.matches("RECLAIM_PASS_ID").count(), 1);
    assert!(drain.contains(".fetch_add(1, Ordering::Relaxed)"));
    let cycle = function_body(process, "reclaim_deferred_process_resources_for_pass");
    assert!(
        cycle.find("unpark_sweep();").unwrap() < cycle.find("PENDING_PROCESS_RECLAIMS").unwrap()
    );
    assert!(cycle.contains("reclaim.last_pass = my_pass"));
    assert!(cycle.contains("pending.swap_remove(index)"));
    assert!(cycle.contains("reclaim.cached_root_is_live()"));
    assert!(cycle.contains("reclaim.live_row_names_root()"));

    let lock_free = function_body(process, "lock_free_root_proof");
    assert!(lock_free.contains("fence_elapsed(&self.after_epoch)"));
    record_unit(
        &mut failures,
        "shared root proof lost an architecture leg or gained a lock",
        validate_root_proof_architecture_legs(&sources),
    );
    record_unit(
        &mut failures,
        "cached_ttbr0 gained a second or non-aarch64 writer",
        validate_cached_ttbr0_single_writer(&sources),
    );
    record(
        &mut failures,
        "deferred-reclaim drain site set changed",
        validate_deferred_reclaim_drain_sites(&sources),
    );
    record_unit(
        &mut failures,
        "x86 interrupt-return epoch stamp gained work",
        validate_x86_epoch_stamp_is_minimal(&sources),
    );

    let park = function_body(process, "park_reclaim");
    assert!(park.contains("let snapshot_at_park = scheduler::RetirementSnapshot::capture();"));
    assert!(park.contains("let fence_at_park = snapshot_at_park.as_fence();"));
    assert!(!park.contains("reclaim.after_epoch"));
    let unpark = function_body(process, "unpark_sweep_with_snapshot");
    assert!(
        unpark.find("PARKED_PROCESS_RECLAIMS.try_lock()").unwrap()
            < unpark.find("PENDING_PROCESS_RECLAIMS.try_lock()").unwrap()
    );

    assert!(scheduler.contains("pub(crate) struct RetirementFence"));
    assert!(scheduler.contains("pub(crate) struct RetirementSnapshot"));
    let capture = function_body(scheduler, "capture");
    assert!(capture.contains("core::sync::atomic::fence(Ordering::Acquire)"));
    let elapsed = function_body(scheduler, "fence_elapsed");
    assert!(
        elapsed.find("fence.online_mask == 0").unwrap()
            < elapsed.find("(0..MAX_CPUS).all").unwrap()
    );
    let stack_reclaim = function_body(scheduler, "reclaim_terminated_threads");
    assert!(
        stack_reclaim.find("retirement_grace_elapsed").unwrap()
            < stack_reclaim.find("is_kernel_stack_slot_live").unwrap()
    );

    record(
        &mut failures,
        "ROW_REMOVAL_EPOCH bump sites",
        validate_census(
            &census(&sources, |source, mask| {
                call_offsets(source, mask, "note_process_row_removed")
            }),
            ROW_REMOVAL_EPOCH_BUMPS,
        ),
    );
    check(
        &mut failures,
        "remove_process no longer removes the process row",
        manager.contains("fn remove_process(")
            && function_body(manager, "remove_process").contains("self.processes.remove(&pid)"),
    );
    check(
        &mut failures,
        "local TTBR0 root read changed",
        ttbr0.contains("core::arch::asm!(\"mrs {}, ttbr0_el1\""),
    );

    for counter in [
        "ROOT_PROOF_BLOCKED_EPOCH",
        "ROOT_PROOF_BLOCKED_HW",
        "ROOT_PROOF_BLOCKED_SHADOW",
        "ROOT_PROOF_BLOCKED_CACHED",
        "ROOT_PROOF_BLOCKED_LIVE_ROW",
        "RETIRE_EMPTY_ONLINE_MASK",
    ] {
        check(
            &mut failures,
            &format!("retirement counter disappeared: {counter}"),
            provider.contains(counter),
        );
    }
    let declaration_only = provider
        .split_once("// Declaration-only until the phase named in PLAN.md.")
        .and_then(|(_, after)| {
            after
                .split_once("pub const COUNTER_COUNT")
                .map(|(declarations, _)| declarations)
        });
    check(
        &mut failures,
        "declaration-only counter boundaries changed",
        declaration_only.is_some(),
    );
    if let Some(declaration_only) = declaration_only {
        check(
            &mut failures,
            "RECLAIM_PASS_SKIPPED became declaration-only",
            !declaration_only.contains("RECLAIM_PASS_SKIPPED"),
        );
        check(
            &mut failures,
            "RETIRE_EMPTY_ONLINE_MASK became declaration-only",
            !declaration_only.contains("RETIRE_EMPTY_ONLINE_MASK"),
        );
    }

    let registry = source(&sources, "kernel/src/test_framework/registry.rs");
    check(
        &mut failures,
        "retirement_fence_gate registry entry disappeared",
        registry.contains("name: \"retirement_fence_gate\""),
    );
    check(
        &mut failures,
        "reclaim_progress_gate registry entry disappeared",
        registry.contains("name: \"reclaim_progress_gate\""),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn all_phase_zero_counters_have_registered_readers_and_honest_runtime_gates() {
    let sources = rust_sources_below("kernel/src");
    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let declarations: BTreeSet<_> = provider
        .split("counter!(")
        .skip(1)
        .filter_map(|rest| {
            rest.trim_start()
                .split_once(',')
                .map(|(name, _)| name.trim().to_owned())
        })
        .filter(|name| name != "$name")
        .collect();
    let inventory = provider
        .split("pub static COUNTERS")
        .nth(1)
        .expect("COUNTERS inventory")
        .split("];")
        .next()
        .expect("COUNTERS terminator");
    let readers: BTreeSet<_> = inventory
        .lines()
        .filter_map(|line| line.trim().strip_prefix('&'))
        .filter_map(|rest| rest.strip_suffix(','))
        .map(str::to_owned)
        .collect();
    assert_eq!(declarations.len(), 72);
    assert_eq!(
        readers, declarations,
        "every counter must have an inventory reader"
    );
    assert!(provider.contains("core::array::from_fn(|index| COUNTERS[index].aggregate())"));
    assert!(provider.contains("for iteration in 0..64"));
    assert!(provider.contains("reset_boot_test_pid_counts();"));
    assert!(provider.contains("for pid in pairing_child_pids"));
    for exact_failure in [
        "adapted-site per-PID defer proof was absent",
        "adapted-site per-PID defer proof was duplicated",
        "adapted-site per-PID reclaim proof was absent",
        "adapted-site per-PID reclaim proof was duplicated",
    ] {
        assert!(provider.contains(exact_failure));
    }
    assert!(!provider.contains("deferred_delta != reclaimed_delta || reclaimed_delta < 64"));
    assert!(!provider.contains("TeardownPairingEvidence"));
    assert!(!provider.contains("defer_reclaim_events_are_paired("));
    assert!(!provider.contains("deferred_pids"));
    assert!(!provider.contains("iter_events()"));
    assert!(!provider.contains("TRACE_BUFFERS"));
    assert!(!provider.contains("super::disable_all()"));
    assert!(!provider.contains("crate::tracing::enable()"));
    assert!(!provider.contains("crate::tracing::disable()"));
    assert!(!provider.contains("TEARDOWN_PROVIDER.enable_all()"));
    assert!(!provider.contains("TEARDOWN_PROVIDER.disable_all()"));
    assert!(source(&sources, "kernel/src/task/process_task.rs").contains("for tid in 1..=17"));
    assert_eq!(
        function_body(provider, "exit_kick_protocol_gate_test")
            .matches("EXIT_SGI_SENT.aggregate()")
            .count(),
        5
    );
    assert!(!provider.contains("TEARDOWN_ENTRY_GROUP.aggregate()"));

    let declaration_only = provider
        .split("// Declaration-only until the phase named in PLAN.md.")
        .nth(1)
        .expect("declaration-only counter boundary");
    assert!(declaration_only.contains("counter!(TEARDOWN_ENTRY_GROUP,"));
    assert!(declaration_only.contains("counter!(EXIT_REQUEST_OBSERVED,"));
    assert!(!declaration_only.contains("counter!(EXIT_SGI_SENT,"));
    assert!(!declaration_only.contains("counter!(EXIT_KICK_PUBLISHED,"));
    assert!(!declaration_only.contains("counter!(RECEIPT_DROPPED_UNRETIRED,"));

    let registry = source(&sources, "kernel/src/test_framework/registry.rs");
    assert!(registry.contains("name: \"fork_exit_defer_reclaim_pairing_test\""));
    assert!(registry.contains("name: \"deferred_fault_ring_overflow_injection\""));
    assert!(registry.contains("name: \"exit_kick_protocol_gate\""));
    assert!(registry.contains("name: \"retirement_receipt_drop_gate\""));

    let plan = repo_text("docs/planning/teardown-unification/PLAN.md");
    assert!(plan.contains("./docker/qemu/run-aarch64-full-test.sh --rebuild --boot-tests-only"));
    let aarch64_gate = repo_text("docker/qemu/run-aarch64-full-test.sh");
    assert!(aarch64_gate.contains("cargo build --release --features boot_tests"));
    assert!(aarch64_gate.contains("--boot-tests-only"));
    assert!(aarch64_gate.contains("grep -q \"\\[BOOT_TESTS:PASS\\]\""));
}

#[test]
fn aarch64_exit_kick_waits_are_progress_bounded() {
    let provider = repo_text("kernel/src/tracing/providers/teardown.rs");
    let gate = function_body(&provider, "exit_kick_protocol_gate_test");
    assert!(provider.contains("const EXIT_KICK_GATE_CEILING_MILLISECONDS: u64 = 45_000;"));
    for required in [
        "const FIRST_PROGRESS_WINDOW_MILLISECONDS: u64 = 8_000;",
        "const NO_PROGRESS_WINDOW_MILLISECONDS: u64 = 3_000;",
        "const ABSOLUTE_WAIT_CEILING_MILLISECONDS: u64 = 15_000;",
        "const GATE_CEILING_MILLISECONDS: u64 = EXIT_KICK_GATE_CEILING_MILLISECONDS;",
        "const RESCHED_REKICK_INTERVAL_MILLISECONDS: u64 = 50;",
        "const CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS: u64 = 100_000;",
        "It cannot lose the race against the five-second soft-lockup detector",
        "if counter_frequency_hz == 0",
        "let counter_delta = crate::arch_impl::aarch64::timer::elapsed_ticks(",
        "if counter_delta == 0",
        "progress_current.advanced_from(last_progress)",
        "let no_progress_deadline_elapsed =",
        "crate::arch_impl::aarch64::timer::elapsed_ticks(last_advance, wait_start)",
        ".saturating_add(no_progress_ticks);",
        "let progress_deadline_elapsed =",
        "core::cmp::max(first_progress_ticks, no_progress_deadline_elapsed);",
        "elapsed >= progress_deadline_elapsed",
        "elapsed >= absolute_ceiling_ticks",
        "crate::arch_impl::aarch64::timer::elapsed_ticks(now, gate_started_at)",
        "matches!(failure, WaitFailureKind::NoProgress) && late_true",
        "crate::arch_impl::aarch64::timer::elapsed_ticks(now, last_re_kick)",
        "record_exit_kick_gate_watchdog_heartbeat();",
        "if elapsed >= no_progress_ticks",
        "let late_condition = condition_value();",
        "struct WaitEvidence<'a>",
        "fn print_wait_evidence(evidence: WaitEvidence<'_>)",
        "cause={} elapsed_ms={} window_budget_ms={}",
        "worker_1_progress_start={}",
        "worker_1_progress_final={}",
        "worker_2_progress_start={}",
        "worker_2_progress_final={}",
        "worker_3_progress_start={}",
        "worker_3_progress_final={}",
        "ticks_to_milliseconds(progress_deadline_elapsed, counter_frequency_hz)",
        "WaitFailureKind::AbsoluteCeiling => ABSOLUTE_WAIT_CEILING_MILLISECONDS",
        "WaitFailureKind::GateCeiling => GATE_CEILING_MILLISECONDS",
        "| WaitFailureKind::JoinFailed => 0",
        "last_advance_ms_ago={}",
        "breadcrumb=1 elapsed_ms={}",
        "WaitProgress::work(publisher_a_progress.load(Ordering::Acquire))",
        "WaitProgress::work(publisher_b_progress.load(Ordering::Acquire))",
        "WaitProgress::workers(",
        "let storm_publisher_a_progress =",
        "let storm_publisher_b_progress =",
        "let storm_observer_progress =",
        "exit: kthread_exit_progress_for_test(tid)",
        "WaitFailureKind::ProgressUnavailable",
        "struct StormAbortGuard",
        "abort: AtomicBool",
        "self.accounting.abort.store(true, Ordering::Release);",
        "exit-kick reservation-loss publisher CPU is not online",
        "exit-kick storm requires four online CPUs",
        "storm publisher A progress/exit stalled; a worker CPU (1/2/3) is unresponsive",
        "storm publisher B progress/exit stalled; a worker CPU (1/2/3) is unresponsive",
        "storm observer progress/exit stalled; a worker CPU (1/2/3) is unresponsive",
    ] {
        assert!(
            gate.contains(required),
            "missing exit-kick bound: {required}"
        );
    }
    assert!(!gate.contains("HANDSHAKE_SPIN_CAP"));
    assert!(!gate.contains("progress_rearmed"));
    assert!(!gate.contains("governing_window_ticks"));
    assert!(!gate.contains("TIMER_TICK_COUNT"));
    assert!(!gate.contains("WHOLE_GATE_BUDGET"));
    assert!(!gate.contains(".wrapping_sub("));
    assert_eq!(
        gate.matches("record_exit_kick_gate_watchdog_heartbeat();")
            .count(),
        3,
        "the soft-lockup heartbeat must cover gate entry, wait entry, and each periodic re-kick"
    );
    assert!(!gate.contains("let storm_progress ="));
    assert_eq!(
        gate.matches("&worker_cpus").count(),
        4,
        "workers_ready and all three storm joins must kick every worker CPU"
    );
    let storm_union_start = gate.find("\"workers_ready\"").expect("workers_ready wait");
    let storm_union_end = gate[storm_union_start..]
        .find("accounting.start.store(true, Ordering::Release);")
        .map(|end| storm_union_start + end)
        .expect("workers_ready wait terminator");
    let storm_union = &gate[storm_union_start..storm_union_end];
    for counter in [
        "publisher_a_progress",
        "publisher_b_progress",
        "observer_progress",
    ] {
        assert!(
            storm_union.contains(counter),
            "workers_ready dependency union lost {counter}"
        );
    }
    for (progress_source, counter) in [
        ("storm_publisher_b_progress", "publisher_b_progress"),
        ("storm_observer_progress", "observer_progress"),
    ] {
        let declaration = gate
            .find(&format!("let {progress_source} ="))
            .unwrap_or_else(|| panic!("missing {progress_source} declaration"));
        let call = gate[declaration..]
            .find(';')
            .map(|end| &gate[declaration..declaration + end])
            .expect("storm progress closure terminator");
        assert!(
            call.contains(counter),
            "{progress_source} must read only its awaited worker's progress"
        );
        for unrelated_counter in [
            "publisher_a_progress",
            "publisher_b_progress",
            "observer_progress",
        ] {
            assert_eq!(
                call.contains(unrelated_counter),
                unrelated_counter == counter,
                "{progress_source} has the wrong worker-progress attribution"
            );
        }
        assert_eq!(
            gate.matches(&format!("&{progress_source}")).count(),
            1,
            "{progress_source} must govern exactly one storm join"
        );
    }
    let publisher_a_progress_declaration = gate
        .find("let storm_publisher_a_progress =")
        .expect("publisher A dependency-progress declaration");
    let publisher_a_progress = gate[publisher_a_progress_declaration..]
        .find(';')
        .map(|end| &gate[publisher_a_progress_declaration..publisher_a_progress_declaration + end])
        .expect("publisher A dependency-progress closure terminator");
    assert!(publisher_a_progress.contains("publisher_a_progress"));
    assert!(publisher_a_progress.contains("observer_progress"));
    assert!(!publisher_a_progress.contains("publisher_b_progress"));
    assert_eq!(gate.matches("&storm_publisher_a_progress").count(), 1);

    assert!(
        !gate.contains("while observer_accounting.publishers_done.load(Ordering::Acquire) != 2")
    );
    for required in [
        "let mut publishers_done_seen = 0u64;",
        "if publishers_done > publishers_done_seen",
        "publishers_done - publishers_done_seen",
        "observer_progress",
    ] {
        assert!(
            gate.contains(required),
            "observer progress is not tied to a genuine state transition: {required}"
        );
    }
    let observer_loop_start = gate
        .find("let mut publishers_done_seen = 0u64;")
        .and_then(|start| gate[start..].find("loop {").map(|offset| start + offset))
        .expect("observer publisher-completion loop");
    let observer_loop_end = gate[observer_loop_start..]
        .find(".observer_done")
        .map(|offset| observer_loop_start + offset)
        .expect("observer loop completion publication");
    let observer_loop = &gate[observer_loop_start..observer_loop_end];
    assert_eq!(
        observer_loop.matches(".observer_progress").count(),
        3,
        "observer progress must advance only for a publisher-done transition or one of the two claimed publications"
    );
    for forbidden in [
        "&[worker_cpus[0]]",
        "&[worker_cpus[1]]",
        "&[worker_cpus[2]]",
        "storm publisher A join stuck, CPU 1 unresponsive",
        "storm publisher B join stuck, CPU 2 unresponsive",
        "storm observer join stuck, CPU 3 unresponsive",
    ] {
        assert!(
            !gate.contains(forbidden),
            "storm wait retained a single-CPU dependency: {forbidden}"
        );
    }
    assert_eq!(
        gate.matches("EXIT_KICK_TEST_HOOK_RESERVED.load").count(),
        1,
        "the reservation wait must remain coordinator-owned"
    );

    let failure_message = function_body(gate, "message");
    for required in [
        "Self::NoProgress | Self::AbsoluteCeiling => site_message",
        "Self::GateCeiling =>",
        "exit_kick_gate: gate liveness ceiling exhausted before this wait's condition was observed (not a per-CPU stall)",
        "Self::PhaseOneCeiling =>",
        "exit_kick_gate: shared Phase-1 liveness budget exhausted before this wait's condition was observed (not a per-CPU stall)",
        "Self::CounterStall =>",
        "exit_kick_gate: CNTVCT stalled while enforcing wait deadline",
        "Self::CounterUnavailable =>",
        "exit_kick_gate: CNTFRQ unavailable; cannot enforce wait deadline",
        "Self::ProgressUnavailable =>",
        "exit_kick_gate: exit-progress tracking unavailable",
        "Self::JoinFailed =>",
        "exit_kick_gate: kthread join failed after exit observation",
    ] {
        assert!(
            failure_message.contains(required),
            "wait failure message lost cause-specific diagnostic: {required}"
        );
    }
    assert!(!failure_message.contains("let _ = self"));
    assert!(!failure_message.contains("unresponsive"));

    let gate_start_capture = gate
        .find("let gate_started_at = crate::arch_impl::aarch64::timer::rdtsc_serialized();")
        .expect("gate CNTVCT anchor capture");
    let exit_progress_arm = gate
        .find("let _exit_progress_guard = KthreadExitProgressGuard::arm();")
        .expect("exit progress guard arm");
    assert!(
        gate_start_capture < exit_progress_arm,
        "the local gate anchor must be captured before gate work begins"
    );
    assert!(!provider.contains("EXIT_KICK_GATE_STARTED_AT"));
    assert!(!provider.contains("EXIT_KICK_GATE_HAS_STARTED"));

    let gate_ceiling_check = gate
        .find("crate::arch_impl::aarch64::timer::elapsed_ticks(now, gate_started_at)")
        .expect("aggregate gate-ceiling check");
    let absolute_ceiling_check = gate
        .find("elapsed >= absolute_ceiling_ticks")
        .expect("absolute wait-ceiling check");
    let no_progress_check = gate
        .find("elapsed >= progress_deadline_elapsed")
        .expect("no-progress check");
    let counter_stall_check = gate
        .find("iterations % CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS == 0")
        .expect("counter-stall sampling");
    let failure_verdict = gate
        .find("if let Some(failure) = failure")
        .expect("failure verdict");
    let periodic_rekick = gate
        .find("crate::arch_impl::aarch64::timer::elapsed_ticks(now, last_re_kick)")
        .expect("periodic reschedule re-kick");
    assert!(
        gate_ceiling_check < absolute_ceiling_check
            && absolute_ceiling_check < no_progress_check
            && no_progress_check < counter_stall_check,
        "real wait deadlines must take precedence over a simultaneous CNTVCT-stall sample"
    );
    assert!(
        failure_verdict < periodic_rekick,
        "a decided failure must return before sending another reschedule SGI"
    );

    let progress_slot = function_body(&provider, "kthread_exit_progress_slot");
    assert!(progress_slot.contains("if current == 0 && !create"));
    assert!(progress_slot.contains("return None;"));
    let progress_guard_arm = function_body(&provider, "arm");
    assert!(!progress_guard_arm.contains("slot.tid.store(0"));
    assert!(!progress_guard_arm.contains("slot.steps.store(0"));
    let progress_watch = function_body(&provider, "watch_kthread_exit_progress_for_test");
    assert!(progress_watch.contains("-> bool"));
    assert!(progress_watch.contains(".is_some()"));
    assert_eq!(
        gate.matches("if !watch_kthread_exit_progress_for_test")
            .count(),
        6,
        "every exit-progress registration must fail explicitly"
    );
    let registration_failures = gate
        .lines()
        .filter(|line| line.contains("exit-progress registration failed"))
        .collect::<Vec<_>>();
    assert_eq!(registration_failures.len(), 5);
    assert!(registration_failures
        .iter()
        .all(|line| !line.contains("unresponsive")));
    let storm_abort_arm = gate
        .find("let mut storm_abort_guard = StormAbortGuard::arm(&accounting);")
        .expect("storm abort guard arm");
    let first_storm_spawn = gate
        .find("let publisher_a = match spawn_publisher")
        .expect("first storm spawn");
    let storm_abort_disarm = gate
        .find("storm_abort_guard.disarm();")
        .expect("storm abort guard disarm");
    let last_storm_join = gate
        .find("storm observer progress/exit stalled")
        .expect("last storm join failure");
    assert!(
        storm_abort_arm < first_storm_spawn && last_storm_join < storm_abort_disarm,
        "storm workers must be released on every coordinator failure before all joins complete"
    );
    assert!(gate.matches("abort.load(Ordering::Acquire)").count() >= 8);

    let exit_progress_reader = function_body(&provider, "kthread_exit_progress_for_test");
    assert!(exit_progress_reader.contains("kthread_exit_progress_slot(tid, false)"));
    assert!(!exit_progress_reader.contains("kthread_exit_progress_slot(tid, true)"));

    let kthread = repo_text("kernel/src/task/kthread.rs");
    let kthread_exit = function_body(&kthread, "kthread_exit");
    assert_eq!(
        kthread_exit
            .matches("record_kthread_exit_stage_for_test")
            .count(),
        4
    );
    assert!(
        kthread_exit
            .rfind("record_kthread_exit_stage_for_test")
            .expect("terminal exit progress bump")
            > kthread_exit
                .find("handle.inner.exited.store(true, Ordering::SeqCst);")
                .expect("exited store"),
        "terminal exit progress must follow the exited store"
    );

    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let clear_affinity = function_body(&scheduler, "clear_cpu_affinity_for_test");
    assert_eq!(
        clear_affinity
            .matches("record_kthread_exit_stage_for_test")
            .count(),
        2
    );
    assert!(!clear_affinity.contains("#[cfg("));

    let main = repo_text("kernel/src/main_aarch64.rs");
    let compact_main: String = main.chars().filter(|ch| !ch.is_whitespace()).collect();
    for required in [
        "#[cfg(feature = \"boot_tests\")]\n            const SMP_ONLINE_NO_PROGRESS_WINDOW_SECONDS: u64 = 20;",
        "#[cfg(not(feature = \"boot_tests\"))]\n            const SMP_ONLINE_NO_PROGRESS_WINDOW_SECONDS: u64 = 2;",
        "#[cfg(feature = \"boot_tests\")]\n            const SMP_ONLINE_ABSOLUTE_CEILING_SECONDS: u64 = 40;",
        "#[cfg(not(feature = \"boot_tests\"))]\n            const SMP_ONLINE_ABSOLUTE_CEILING_SECONDS: u64 = 4;",
    ] {
        assert!(main.contains(required), "missing SMP timeout profile: {required}");
    }
    assert!(main.contains("const SMP_ONLINE_BREADCRUMB_INTERVAL_SECONDS: u64 = 1;"));
    assert!(main.contains("const SMP_ONLINE_STAGE_SAMPLE_INTERVAL_ITERATIONS: u64 = 4_096;"));
    assert!(main
        .contains("const SMP_ONLINE_CNTVCT_STALL_SAMPLE_INTERVAL_ITERATIONS: u64 = 10_000_000;"));
    assert!(main.contains("let no_progress_ticks ="));
    assert!(main
        .contains("counter_frequency_hz.saturating_mul(SMP_ONLINE_NO_PROGRESS_WINDOW_SECONDS);"));
    assert!(main.contains("let absolute_ceiling_ticks ="));
    assert!(
        main.contains("counter_frequency_hz.saturating_mul(SMP_ONLINE_ABSOLUTE_CEILING_SECONDS);")
    );
    assert!(compact_main.contains(
        "letcurrent_bringup_progress=kernel::arch_impl::aarch64::smp::bringup_progress();"
    ));
    assert!(main.contains("if current_online > last_online"));
    assert!(compact_main.contains(
        "ifcurrent_online>last_online{last_online=current_online;last_advance=now;}elseifiterations%SMP_ONLINE_STAGE_SAMPLE_INTERVAL_ITERATIONS==0{letcurrent_bringup_progress="
    ));
    assert!(main.contains("current_bringup_progress > last_bringup_progress"));
    assert!(main.contains("last_advance = now;"));
    assert!(compact_main
        .contains("letprogress_at_verdict=kernel::arch_impl::aarch64::smp::bringup_progress();"));
    assert!(compact_main
        .contains("ifonline_at_verdict>last_online||progress_at_verdict>last_bringup_progress"));
    assert!(main.contains("let counter_delta = timer::elapsed_ticks(now, last_counter_sample);"));
    assert!(main.contains("if counter_delta == 0"));
    for deadline in [
        "timer::elapsed_ticks(now, phase_one_started_at)",
        "timer::elapsed_ticks(now, start)",
        "timer::elapsed_ticks(now, last_advance)",
        "timer::elapsed_ticks(now, last_breadcrumb)",
    ] {
        assert!(
            main.contains(deadline),
            "missing forward-only SMP delta: {deadline}"
        );
    }
    assert!(main.contains("[smp] CNTVCT stalled"));
    assert!(main.contains("[smp] still waiting, {} online"));
    assert!(main.contains("cpu{} stage={} {}"));
    assert!(main.contains("bringup_stage_name(stage_now)"));
    assert!(main.contains("stage_at_start={} stage_advanced={}"));
    assert!(main.contains("[smp] Timeout waiting for CPUs: absolute ceiling"));
    assert!(main.contains("last_psci_return_code(cpu)"));
    assert!(main.contains("psci_return_code_name(last_psci)"));
    assert!(main.contains("iterations = iterations.wrapping_add(1);"));
    assert!(!main.contains("iterations = iterations.saturating_add(1);"));
    assert!(main.contains("PSCI CPU_ON success (raw_status={})"));

    let smp = repo_text("kernel/src/arch_impl/aarch64/smp.rs");
    assert!(smp.contains("const PSCI_CPU_ON_MAX_ATTEMPTS: usize = 4;"));
    for required in [
        "const PSCI_RETURN_NOT_SUPPORTED: i64 = -1;",
        "const PSCI_RETURN_INVALID_PARAMS: i64 = -2;",
        "const PSCI_RETURN_DENIED: i64 = -3;",
        "const PSCI_RETURN_INTERNAL_FAILURE: i64 = -6;",
        "const PSCI_RETURN_NOT_PRESENT: i64 = -7;",
        "matches!(ret, PSCI_RETURN_DENIED | PSCI_RETURN_INTERNAL_FAILURE)",
        "let attempt_retryable = psci_cpu_on_failure_is_transient(hvc64_ret)",
        "hvc32_ret.is_some_and(psci_cpu_on_failure_is_transient)",
        "|| attempt + 1 == PSCI_CPU_ON_MAX_ATTEMPTS",
    ] {
        assert!(
            smp.contains(required),
            "missing PSCI retry invariant: {required}"
        );
    }
    assert!(smp.contains("PSCI_CPU_ON_BACKOFF_ITERATION_CAP"));
    let retry_backoff = function_body(&smp, "psci_cpu_on_retry_backoff");
    assert!(retry_backoff.contains("super::timer::elapsed_ticks("));
    assert!(!retry_backoff.contains("wrapping_sub"));
    assert!(smp.contains("LAST_PSCI_RETURN_CODE[cpu_id].store(ret, Ordering::Release);"));
    let release_cpu = function_body(&smp, "release_cpu");
    let reset_last_status = release_cpu
        .find("LAST_PSCI_RETURN_CODE[cpu_id].store(PSCI_RETURN_NOT_ATTEMPTED")
        .expect("per-call PSCI diagnostic reset");
    let cpu_zero_reject = release_cpu.find("if cpu_id == 0").expect("CPU 0 rejection");
    assert!(reset_last_status < cpu_zero_reject);
    assert!(release_cpu.contains(
        "LAST_PSCI_RETURN_CODE[cpu_id].store(PSCI_RETURN_INVALID_PARAMS, Ordering::Release);"
    ));
    let last_psci_return_code = function_body(&smp, "last_psci_return_code");
    assert!(last_psci_return_code.contains(".unwrap_or(PSCI_RETURN_INVALID_PARAMS)"));
    let psci_return_code_name = function_body(&smp, "psci_return_code_name");
    assert!(psci_return_code_name.contains("PSCI_RETURN_NOT_ATTEMPTED => \"not-attempted\""));
    assert!(smp.contains("SMC would trap to EL2 and may fault."));
    let accepted = function_body(&smp, "psci_cpu_on_was_accepted");
    assert!(accepted.contains("PSCI_RETURN_SUCCESS | PSCI_RETURN_ON_PENDING"));
    assert!(!accepted.contains("PSCI_RETURN_ALREADY_ON"));
    assert!(smp.contains("PSCI claimed ALREADY_ON"));
    assert!(smp.contains("CPU is not online"));
    assert!(smp.contains("`ALREADY_ON` is accepted only"));
    assert!(release_cpu.contains("ret == PSCI_RETURN_ALREADY_ON && is_cpu_online(cpu_id)"));
    assert!(smp.contains("[`last_psci_return_code()`]"));
    assert!(smp.contains("PSCI CPU_ON accepted after {} attempts (raw_status={})"));
    assert!(smp.contains("attempt {}/{}: HVC64 failed"));
    assert!(smp.contains("PSCI CPU_ON failed for CPU {} after {} attempts"));
    assert!(!smp.contains("fn psci_cpu_on_smc"));
    assert!(!smp.contains("pub const CNTVCT_FALLBACK_FREQUENCY_HZ"));
    let timer = repo_text("kernel/src/arch_impl/aarch64/timer.rs");
    assert!(timer.contains("pub const BOOT_COUNTER_FALLBACK_FREQUENCY_HZ: u64 = 1_000_000;"));
    let milliseconds_to_ticks = function_body(&timer, "milliseconds_to_ticks");
    assert!(milliseconds_to_ticks.contains("frequency_hz.saturating_mul(milliseconds)"));
    assert!(milliseconds_to_ticks.contains("/ 1_000"));
    let elapsed_ticks = function_body(&timer, "elapsed_ticks");
    assert!(elapsed_ticks.contains("now.checked_sub(start).unwrap_or(0)"));
    assert!(provider.contains("crate::arch_impl::aarch64::timer::milliseconds_to_ticks("));
    assert!(compact_main.contains(
        "timer::milliseconds_to_ticks(counter_frequency_hz,kernel::test_framework::PHASE_ONE_LIVENESS_BUDGET_MILLISECONDS,)"
    ));

    let timer_interrupt = repo_text("kernel/src/arch_impl/aarch64/timer_interrupt.rs");
    assert!(timer_interrupt.contains("static EXIT_KICK_GATE_WATCHDOG_HEARTBEAT: AtomicU64"));
    let heartbeat = function_body(&timer_interrupt, "record_exit_kick_gate_watchdog_heartbeat");
    assert!(heartbeat.contains("fetch_add(1, Ordering::Relaxed)"));
    let soft_lockup = function_body(&timer_interrupt, "check_soft_lockup");
    assert!(soft_lockup.contains("exit_kick_gate_heartbeat_progressed"));
    assert!(soft_lockup
        .contains("ctx_progressed || syscall_progressed || exit_kick_gate_heartbeat_progressed"));
    assert!(smp.contains("super::timer::BOOT_COUNTER_FALLBACK_FREQUENCY_HZ"));
    assert!(main.contains("kernel::arch_impl::aarch64::timer::BOOT_COUNTER_FALLBACK_FREQUENCY_HZ"));
    assert!(smp.contains("#[repr(C, align(64))]"));
    assert!(smp.contains("struct CpuBringupStage"));
    assert!(smp.contains("_padding: [u8; 60]"));
    assert!(smp.contains("core::mem::size_of::<CpuBringupStage>() == 64"));
    assert!(smp.contains("core::mem::align_of::<CpuBringupStage>() == 64"));
    assert!(smp.contains("static CPU_BRINGUP_STAGE: [CpuBringupStage; MAX_CPUS]"));
    assert!(smp.contains("[const { CpuBringupStage::new() }; MAX_CPUS]"));
    assert!(smp.contains("pub fn bringup_stage_of(cpu_id: usize) -> u32"));
    assert!(smp.contains("pub fn bringup_stage_name(stage: u32) -> &'static str"));
    assert!(smp.contains("pub fn bringup_progress() -> u64"));
    for stage_constant in [
        "const BRINGUP_STAGE_ALLOCATING_IDLE_THREAD: u32 = 9;",
        "const BRINGUP_STAGE_IDLE_THREAD_ALLOCATED: u32 = 10;",
        "const BRINGUP_STAGE_REGISTERING_IDLE_THREAD: u32 = 11;",
        "const BRINGUP_STAGE_IDLE_THREAD_REGISTERED: u32 = 12;",
        "const BRINGUP_STAGE_INTERRUPT_HANDOFF_READY: u32 = 13;",
        "const BRINGUP_STAGE_ONLINE: u32 = 14;",
    ] {
        assert!(smp.contains(stage_constant));
    }
    assert!(smp.contains("BRINGUP_STAGE_INTERRUPT_HANDOFF_READY => \"interrupt-handoff-ready\""));

    let stage_setter = function_body(&smp, "set_bringup_stage");
    assert!(stage_setter.contains("CPU_BRINGUP_STAGE.get(cpu_id)"));
    assert_eq!(stage_setter.matches(".store(").count(), 1);
    assert!(stage_setter.contains("cpu_stage.value.store(stage, Ordering::Release);"));

    let bringup_progress = function_body(&smp, "bringup_progress");
    assert!(bringup_progress.contains("CPU_BRINGUP_STAGE"));
    assert!(bringup_progress.contains("stage.value.load(Ordering::Acquire)"));
    assert!(bringup_progress.contains(".sum()"));

    let secondary_entry = function_body(&smp, "secondary_cpu_entry_rust");
    assert_eq!(secondary_entry.matches("set_bringup_stage(").count(), 10);
    let interrupt_handoff_ready = secondary_entry
        .find("BRINGUP_STAGE_INTERRUPT_HANDOFF_READY")
        .expect("secondary interrupt-handoff-ready stage");
    let online_flag = secondary_entry
        .find("CPU_ONLINE[cpu_id as usize].store(true, Ordering::Release);")
        .expect("secondary per-CPU online publication");
    let online_stage = secondary_entry
        .find("set_bringup_stage(cpu_id as usize, BRINGUP_STAGE_ONLINE);")
        .expect("secondary online stage publication");
    let online_count = secondary_entry
        .find("CPUS_ONLINE.fetch_add(1, Ordering::Release);")
        .expect("secondary online-count publication");
    let interrupt_enable = secondary_entry
        .find("super::cpu::enable_interrupts();")
        .expect("secondary interrupt enable");
    assert!(
        interrupt_handoff_ready < online_flag
            && online_flag < online_stage
            && online_stage < online_count
            && online_count < interrupt_enable,
        "secondary readiness and online state must be published before a pending IRQ can schedule away the bootstrap continuation"
    );
    assert!(
        !secondary_entry[interrupt_enable..].contains("set_bringup_stage("),
        "no bring-up stage may depend on returning from the first unmasked IRQ"
    );

    let create_idle = function_body(&smp, "create_and_register_idle_thread");
    assert_eq!(create_idle.matches("set_bringup_stage(").count(), 4);
    let allocating = create_idle
        .find("set_bringup_stage(cpu_id, BRINGUP_STAGE_ALLOCATING_IDLE_THREAD);")
        .expect("idle-thread allocation entry stage");
    let allocation = create_idle
        .find("let mut idle_task = Box::new(Thread::new(")
        .expect("idle-thread allocation");
    let allocated = create_idle
        .find("set_bringup_stage(cpu_id, BRINGUP_STAGE_IDLE_THREAD_ALLOCATED);")
        .expect("idle-thread allocation completion stage");
    assert!(allocating < allocation && allocation < allocated);

    let registering = create_idle
        .find("set_bringup_stage(cpu_id, BRINGUP_STAGE_REGISTERING_IDLE_THREAD);")
        .expect("idle-thread scheduler registration entry stage");
    let registration = create_idle
        .find("crate::task::scheduler::register_cpu_idle_thread(cpu_id, idle_task);")
        .expect("idle-thread scheduler registration");
    let registered = create_idle
        .find("set_bringup_stage(cpu_id, BRINGUP_STAGE_IDLE_THREAD_REGISTERED);")
        .expect("idle-thread scheduler registration completion stage");
    assert!(registering < registration && registration < registered);
}

#[test]
fn deliberately_broken_variants_fail_the_ratchet() {
    let sources = rust_sources_below("kernel/src");

    let broken_exit = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_exit.rs",
        "fn rogue_exit(pm: &mut ProcessManager, pid: ProcessId) { pm.exit_process(pid, 0); }",
    );
    assert!(validate_exit_process_entry_points(&broken_exit).is_err());

    let broken_by_pid = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_by_pid.rs",
        "fn rogue_exit(pid: ProcessId) { exit_process_by_pid(pid, 0); }",
    );
    assert!(validate_exit_process_entry_points(&broken_by_pid).is_err());

    let broken_test_helper = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_test_helper.rs",
        "fn rogue_test_exit(pid: ProcessId) { exit_process_for_teardown_test(pid, 0); }",
    );
    assert!(validate_exit_process_entry_points(&broken_test_helper).is_err());

    let broken_enqueue = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_enqueue.rs",
        "fn rogue_enqueue(reclaim: DeferredProcessReclaim) { enqueue_process_reclaim(reclaim); }",
    );
    assert!(validate_reclaim_enqueue_callers(&broken_enqueue).is_err());

    let broken_blocking = with_synthetic_source(
        &sources,
        "kernel/src/task/synthetic_blocking.rs",
        "pub fn block_current() {}",
    );
    assert!(validate_blocking_primitives(&broken_blocking).is_err());

    let broken_blocking_family = with_synthetic_source(
        &sources,
        "kernel/src/task/synthetic_blocking_family.rs",
        "pub fn block_current_probe(saved_regs: [u64; 32]) { let _ = saved_regs; }",
    );
    assert!(validate_blocking_primitives(&broken_blocking_family).is_err());

    let broken_group_write = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_group.rs",
        "fn rogue_group(thread: &mut Thread) { thread.thread_group_id = Some(1); }",
    );
    assert!(validate_group_writes(&broken_group_write).is_err());

    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    let broken_scheduler = scheduler.replacen(
        "fn send_resched_ipi(&self) {",
        "fn send_resched_ipi(&self) { crate::trace_count!(EXIT_SGI_SENT);",
        1,
    );
    let broken_sgi =
        with_replaced_source(&sources, "kernel/src/task/scheduler.rs", broken_scheduler);
    assert!(validate_exit_sgi_is_teardown_only(&broken_sgi).is_err());

    let allocator = source(&sources, "kernel/src/memory/frame_allocator.rs");
    let fixture = source(&sources, "kernel/src/memory/frame_allocator_tests.rs");
    let process_memory = source(&sources, "kernel/src/memory/process_memory.rs");

    // R2: the mapper allocator and deleted escape hatches cannot be restored.
    let bypassed_recorder =
        process_memory.replacen("&mut TableRecorder(tables)", "&mut GlobalFrameAllocator", 1);
    let bypassed_recorder = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        bypassed_recorder,
    );
    assert!(validate_process_table_recorder(&bypassed_recorder).is_err());
    for restored_escape in [
        "pub fn mapper(&mut self) {}",
        "pub fn allocate_stack(&mut self) {}",
        "fn deep_copy_pml4_entry() {}",
        "fn deep_copy_l3_entry() {}",
        "fn deep_copy_l2_entry() {}",
    ] {
        let restored = with_replaced_source(
            &sources,
            "kernel/src/memory/process_memory.rs",
            format!("{process_memory}\n{restored_escape}"),
        );
        assert!(validate_process_table_recorder(&restored).is_err());
    }

    // R4: ProcessPageTable's Drop can count but can never return a frame.
    let freeing_drop = process_memory.replacen(
        "impl Drop for ProcessPageTable {\n    fn drop(&mut self) {",
        "impl Drop for ProcessPageTable {\n    fn drop(&mut self) { crate::memory::frame_allocator::deallocate_frame(self.level_4_frame);",
        1,
    );
    let freeing_drop = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        freeing_drop,
    );
    assert!(validate_process_page_table_drop_is_non_freeing(&freeing_drop).is_err());

    // R5: bare, block-wrapped, and qualified raw drops are all rejected.
    for raw_drop in [
        "drop(process.page_table.take());",
        "drop({ process.page_table.take() });",
        "core::mem::drop((process.page_table.take()));",
    ] {
        let process_task = source(&sources, "kernel/src/task/process_task.rs");
        let raw_drop = with_replaced_source(
            &sources,
            "kernel/src/task/process_task.rs",
            format!(
                "{process_task}\nfn synthetic_raw_drop(process: &mut Process) {{ {raw_drop} }}"
            ),
        );
        assert!(validate_process_page_table_dispositions(&raw_drop).is_err());
    }
    let process_task = source(&sources, "kernel/src/task/process_task.rs");
    let wrong_reason = process_task.replacen(
        "page_table.abandon(AbandonReason::AlreadyTerminated);",
        "page_table.abandon(AbandonReason::NoProofPipeline);",
        1,
    );
    let wrong_reason =
        with_replaced_source(&sources, "kernel/src/task/process_task.rs", wrong_reason);
    assert!(validate_process_page_table_dispositions(&wrong_reason).is_err());
    // The exec-supersede walk is gone: `cleanup_for_exec` is one arch-neutral
    // custody body, and the only frame returns left anywhere in
    // process_memory.rs are the two inside the boot-test custody fixture. These
    // five negatives pin that state through the two named validators that own
    // it, each with a spelling the production source does not use, so the
    // ratchet is proven to recognise spans and resolved calls rather than one
    // literal form.
    const CUSTODY_BODY: &str =
        "        self.release_mapped_leaves();\n        self.retire_bounded(pid, budget)";

    // 1. A raw frame return smuggled back into the custody body, block-wrapped.
    let exec_body_frame_return = process_memory.replacen(
        CUSTODY_BODY,
        "        self.release_mapped_leaves();\n        {\n            deallocate_frame(self.level_4_frame);\n        }\n        self.retire_bounded(pid, budget)",
        1,
    );
    assert_ne!(
        exec_body_frame_return, process_memory,
        "exec-body frame-return mutation must apply"
    );
    let exec_body_frame_return = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        exec_body_frame_return,
    );
    assert!(validate_frame_return_choke_point(&exec_body_frame_return).is_err());

    // 2. The same free moved to a third function in the same file: the choke
    //    point is file-scoped, so relocating the escape does not launder it.
    let relocated_frame_return = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        format!(
            "{process_memory}\nfn synthetic_exec_release(frame: PhysFrame) {{ deallocate_frame(frame); }}"
        ),
    );
    assert!(validate_frame_return_choke_point(&relocated_frame_return).is_err());

    // 3. The same free spelled as a fully qualified path: the census resolves
    //    call sites, not one import spelling.
    let qualified_frame_return = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        format!(
            "{process_memory}\nfn synthetic_qualified_release(frame: PhysFrame) {{ crate::memory::frame_allocator::deallocate_frame(frame); }}"
        ),
    );
    assert!(validate_frame_return_choke_point(&qualified_frame_return).is_err());

    // 4. A disposition stamp restored inside the custody body, braced so the
    //    spelling differs from the deleted walk's.
    let exec_body_disposition = process_memory.replacen(
        CUSTODY_BODY,
        "        self.release_mapped_leaves();\n        self.tables.disposition = { Disposition::Retired };\n        self.retire_bounded(pid, budget)",
        1,
    );
    assert_ne!(
        exec_body_disposition, process_memory,
        "exec-body disposition mutation must apply"
    );
    let exec_body_disposition = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        exec_body_disposition,
    );
    assert!(validate_process_page_table_dispositions(&exec_body_disposition).is_err());

    // 5. The body split back into two architecture-selected copies. The second
    //    copy is itself custody-shaped, so only the single-body clause can fire.
    let split_exec_bodies = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        format!(
            "{process_memory}\nimpl ProcessPageTable {{\n    #[cfg(target_arch = \"x86_64\")]\n    pub(crate) fn cleanup_for_exec(&mut self, pid: u64, budget: &mut u32) -> RetireProgress {{\n{CUSTODY_BODY}\n    }}\n}}"
        ),
    );
    assert!(validate_process_page_table_dispositions(&split_exec_bodies).is_err());
    let abandon_reason_moved_back = process_task
        .replacen(
            "page_table.abandon(AbandonReason::NoProofPipeline);",
            "page_table.abandon({ AbandonReason::NoArchPipeline });",
            1,
        )
        .replacen(
            "let reason = AbandonReason::NoArchPipeline;",
            "let reason = AbandonReason::NoProofPipeline;",
            1,
        );
    let abandon_reason_moved_back = with_replaced_source(
        &sources,
        "kernel/src/task/process_task.rs",
        abandon_reason_moved_back,
    );
    assert!(validate_process_page_table_dispositions(&abandon_reason_moved_back).is_err());

    // R3: a second retirement call, even with a different receiver spelling,
    // is outside the one proof-gated site and must fail exact site membership.
    let extra_retire = with_replaced_source(
        &sources,
        "kernel/src/task/process_task.rs",
        format!(
            "{process_task}\nfn synthetic_retire(page_table: &mut ProcessPageTable, pid: u64, budget: &mut u32) {{ let _ = ProcessPageTable::retire_bounded(page_table, pid, budget); }}"
        ),
    );
    assert!(validate_process_page_table_retire_site(&extra_retire).is_err());
    let extra_reclaim = with_replaced_source(
        &sources,
        "kernel/src/task/process_task.rs",
        format!(
            "{process_task}\nfn synthetic_reclaim(reclaim: &mut PendingProcessReclaim) {{ let _ = PendingProcessReclaim::reclaim_bounded(reclaim); }}"
        ),
    );
    assert!(validate_process_page_table_retire_site(&extra_reclaim).is_err());
    let process_manager = source(&sources, "kernel/src/process/manager.rs");
    let cross_file_retire = with_replaced_source(
        &sources,
        "kernel/src/process/manager.rs",
        format!(
            "{process_manager}\nfn synthetic_cross_file_retire(page_table: &mut ProcessPageTable, pid: u64, budget: &mut u32) {{ let _ = page_table.retire_bounded(pid, budget); }}"
        ),
    );
    assert!(validate_process_page_table_retire_site(&cross_file_retire).is_err());

    // Q1/Q2: x86 retirement may drain only in normal context, the IRQ-return
    // hook may only stamp its epoch, and cached root writes remain aarch64-only.
    let x86_context = source(&sources, "kernel/src/interrupts/context_switch.rs");
    let moved_drain = x86_context
        .replacen(
            "        crate::task::process_task::reclaim_deferred_process_resources();",
            "        core::hint::spin_loop();",
            1,
        )
        .replacen(
            "    crate::task::scheduler::note_scheduling_epoch(0);",
            "    crate::task::scheduler::note_scheduling_epoch(0);\n    process_task::reclaim_deferred_process_resources();",
            1,
        );
    let moved_drain = with_replaced_source(
        &sources,
        "kernel/src/interrupts/context_switch.rs",
        moved_drain,
    );
    assert!(validate_deferred_reclaim_drain_sites(&moved_drain).is_err());

    let second_cached_writer = with_replaced_source(
        &sources,
        "kernel/src/interrupts/context_switch.rs",
        format!(
            "{x86_context}\nfn synthetic_x86_cached_writer(candidate: &mut Thread, new_root: u64) {{ candidate.cached_ttbr0 = new_root; }}"
        ),
    );
    assert!(validate_cached_ttbr0_single_writer(&second_cached_writer).is_err());

    let logged_before_stamp = x86_context.replacen(
        "    crate::task::scheduler::note_scheduling_epoch(0);",
        "    log::trace!(\"epoch probe\");\n    crate::task::scheduler::note_scheduling_epoch(0);",
        1,
    );
    let logged_before_stamp = with_replaced_source(
        &sources,
        "kernel/src/interrupts/context_switch.rs",
        logged_before_stamp,
    );
    assert!(validate_x86_epoch_stamp_is_minimal(&logged_before_stamp).is_err());

    let locked_before_stamp = x86_context.replacen(
        "    crate::task::scheduler::note_scheduling_epoch(0);",
        "    let _unexpected = SCHEDULER.lock();\n    crate::task::scheduler::note_scheduling_epoch(0);",
        1,
    );
    let locked_before_stamp = with_replaced_source(
        &sources,
        "kernel/src/interrupts/context_switch.rs",
        locked_before_stamp,
    );
    assert!(validate_x86_epoch_stamp_is_minimal(&locked_before_stamp).is_err());

    let statement_before_stamp = x86_context.replacen(
        "    crate::task::scheduler::note_scheduling_epoch(0);",
        "    core::hint::spin_loop();\n    crate::task::scheduler::note_scheduling_epoch(0);",
        1,
    );
    let statement_before_stamp = with_replaced_source(
        &sources,
        "kernel/src/interrupts/context_switch.rs",
        statement_before_stamp,
    );
    assert!(validate_x86_epoch_stamp_is_minimal(&statement_before_stamp).is_err());

    let missing_saved_shadow = process_task.replacen(
        "crate::per_cpu::get_saved_process_cr3()",
        "crate::per_cpu::get_next_cr3()",
        1,
    );
    let missing_saved_shadow = with_replaced_source(
        &sources,
        "kernel/src/task/process_task.rs",
        missing_saved_shadow,
    );
    assert!(validate_root_proof_architecture_legs(&missing_saved_shadow).is_err());

    let missing_shadow_clear = process_task.replacen(
        "        clear_shadow_root(page_table.level_4_frame().start_address().as_u64());",
        "        core::hint::spin_loop();",
        1,
    );
    let missing_shadow_clear = with_replaced_source(
        &sources,
        "kernel/src/task/process_task.rs",
        missing_shadow_clear,
    );
    assert!(validate_root_proof_architecture_legs(&missing_shadow_clear).is_err());

    let locked_root_proof = process_task.replacen(
        "        if !snapshot.fence_elapsed(&self.after_epoch)",
        "        let _unexpected = crate::process::manager();\n        if !snapshot.fence_elapsed(&self.after_epoch)",
        1,
    );
    let locked_root_proof = with_replaced_source(
        &sources,
        "kernel/src/task/process_task.rs",
        locked_root_proof,
    );
    assert!(validate_root_proof_architecture_legs(&locked_root_proof).is_err());

    // R6: inventory membership and unconditional declarations are both pinned.
    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let missing_pt_reader = provider.replacen("    &PT_ROOT_DROPPED_UNDECIDED,\n", "", 1);
    let missing_pt_reader = with_replaced_source(
        &sources,
        "kernel/src/tracing/providers/teardown.rs",
        missing_pt_reader,
    );
    assert!(validate_process_page_table_counter_inventory(&missing_pt_reader).is_err());
    let conditional_pt_counter = provider.replacen(
        "counter!(PT_TABLE_FRAMES_RECORDED,",
        "#[cfg(target_arch = \"aarch64\")]\ncounter!(PT_TABLE_FRAMES_RECORDED,",
        1,
    );
    let conditional_pt_counter = with_replaced_source(
        &sources,
        "kernel/src/tracing/providers/teardown.rs",
        conditional_pt_counter,
    );
    assert!(validate_process_page_table_counter_inventory(&conditional_pt_counter).is_err());

    // PR-3: inherited root slots must remain fail-closed and classified from
    // allocation-derived ownership rather than population alone.
    let missing_root_refusal = process_memory.replacen(
        "return Err(\"Cannot map into an inherited root page-table slot\");",
        "",
        1,
    );
    let missing_root_refusal = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        missing_root_refusal,
    );
    assert!(validate_root_slot_custody(&missing_root_refusal).is_err());
    let unclassified_root = process_memory.replacen(
        "self.owned_root_slots.contains(slot)",
        "true",
        1,
    );
    let unclassified_root = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        unclassified_root,
    );
    assert!(validate_root_slot_custody(&unclassified_root).is_err());

    // R7: neither explicit disposition path may gain formatting or heap work.
    let logged_abandon = process_memory.replacen(
        "pub(crate) fn abandon(mut self, reason: AbandonReason) {",
        "pub(crate) fn abandon(mut self, reason: AbandonReason) { log::info!(\"abandon\");",
        1,
    );
    let logged_abandon = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        logged_abandon,
    );
    assert!(validate_process_page_table_exit_paths_are_minimal(&logged_abandon).is_err());
    let allocating_drop = process_memory.replacen(
        "impl Drop for ProcessPageTable {\n    fn drop(&mut self) {",
        "impl Drop for ProcessPageTable {\n    fn drop(&mut self) { let _unexpected = Vec::<u8>::new();",
        1,
    );
    let allocating_drop = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        allocating_drop,
    );
    assert!(validate_process_page_table_exit_paths_are_minimal(&allocating_drop).is_err());

    let logged_retire = process_memory.replacen(
        "pub(crate) fn retire_bounded",
        "pub(crate) fn retire_bounded",
        1,
    );
    let logged_retire = logged_retire.replacen(
        "self.tables.disposition = Disposition::Retiring;",
        "log::info!(\"retire\"); self.tables.disposition = Disposition::Retiring;",
        1,
    );
    let logged_retire = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        logged_retire,
    );
    assert!(validate_process_page_table_exit_paths_are_minimal(&logged_retire).is_err());

    // PR-2: virtual-page records precede descriptor publication, decref is
    // fail-closed, and a live leaf can never enter the reuse pool.
    let no_leaf_reserve = process_memory.replacen(".try_reserve(1)", ".try_reserve(0)", 1);
    let no_leaf_reserve = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        no_leaf_reserve,
    );
    assert!(validate_leaf_custody(&no_leaf_reserve).is_err());
    let post_publish_record = process_memory.replacen(
        "let mapping = match acquire_leaf_mapping(frame)",
        "let mapping = match classify_after_descriptor(frame)",
        1,
    );
    let post_publish_record = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        post_publish_record,
    );
    assert!(validate_leaf_custody(&post_publish_record).is_err());
    let fail_open_decref = allocator.replacen(
        "if refs == 0 {",
        "if refs == 0 { return true; } if false {",
        1,
    );
    let fail_open_decref = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        fail_open_decref,
    );
    assert!(validate_leaf_custody(&fail_open_decref).is_err());
    let live_leaf_return = allocator.replacen(
        "ST_ALLOCATED => {\n                if slot.leaf_refs.load(Ordering::Acquire) != 0 {",
        "ST_ALLOCATED => {\n                if slot.leaf_refs.load(Ordering::Acquire) != 0 && false {",
        1,
    );
    let live_leaf_return = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        live_leaf_return,
    );
    assert!(validate_leaf_custody(&live_leaf_return).is_err());
    let logged_leaf_release = process_memory.replacen(
        "fn release_leaf_record(record: LeafRecord, frame: PhysFrame) {",
        "fn release_leaf_record(record: LeafRecord, frame: PhysFrame) { log::info!(\"leaf\");",
        1,
    );
    let logged_leaf_release = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        logged_leaf_release,
    );
    assert!(validate_leaf_custody(&logged_leaf_release).is_err());
    let restored_fork_incref = with_replaced_source(
        &sources,
        "kernel/src/process/fork.rs",
        format!("{}\nfn synthetic_leaf_bypass(frame: PhysFrame) {{ frame_incref(frame); }}", source(&sources, "kernel/src/process/fork.rs")),
    );
    assert!(validate_leaf_custody(&restored_fork_incref).is_err());
    let early_exec_supersede = source(&sources, "kernel/src/process/manager.rs").replacen(
        "crate::memory::process_memory::UnpublishedPageTable::new(",
        "crate::memory::process_memory::ProcessPageTable::new_unchecked(",
        1,
    );
    let early_exec_supersede = with_replaced_source(
        &sources,
        "kernel/src/process/manager.rs",
        early_exec_supersede,
    );
    assert!(validate_leaf_custody(&early_exec_supersede).is_err());

    // O2/G-H: weaken either exact counter assertion and the named validator fails.
    for (needle, replacement) in [
        (
            "after_abandon[3] != start[3] + 1",
            "after_abandon[3] != start[3] + 1 && false",
        ),
        (
            "after_drop[4] != after_abandon[4] + 1",
            "after_drop[4] != after_abandon[4] + 1 && false",
        ),
    ] {
        let weakened = process_memory.replacen(needle, replacement, 1);
        let weakened =
            with_replaced_source(&sources, "kernel/src/memory/process_memory.rs", weakened);
        assert!(validate_process_page_table_runtime_oracle(&weakened).is_err());
    }
    let residual_x86_o3 = process_memory.replacen(
        "after_retire[2] != after_drop[2]",
        "after_retire[2] != after_drop[2] + 1",
        1,
    );
    let residual_x86_o3 = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        residual_x86_o3,
    );
    assert!(validate_process_page_table_runtime_oracle(&residual_x86_o3).is_err());

    // O1 + leak oracle: removing anti-vacuity mappings or weakening any exact
    // per-PID/baseline equality invalidates the named validator directly.
    let no_sentinels = provider.replacen(
        "map_retire_sentinels(child_page_table.as_mut())",
        "map_retire_sentinels_disabled(child_page_table.as_mut())",
        1,
    );
    let no_sentinels = with_replaced_source(
        &sources,
        "kernel/src/tracing/providers/teardown.rs",
        no_sentinels,
    );
    assert!(validate_pr1c_retirement_oracles(&no_sentinels).is_err());
    for (needle, replacement) in [
        (
            "allocator_used_after != allocator_used_before {",
            "allocator_used_after != allocator_used_before && false {",
        ),
        (
            "counts.roots_retired != 1 + pending_old_roots {",
            "counts.roots_retired != 1 + pending_old_roots && false {",
        ),
        (
            "counts.table_frames_recorded != expected_tables + pending_old_tables {",
            "counts.table_frames_recorded != expected_tables + pending_old_tables && false {",
        ),
        (
            "counts.table_frames_returned != counts.table_frames_recorded + counts.roots_retired {",
            "counts.table_frames_returned != counts.table_frames_recorded + counts.roots_retired && false {",
        ),
    ] {
        let weakened = provider.replacen(needle, replacement, 1);
        let weakened = with_replaced_source(
            &sources,
            "kernel/src/tracing/providers/teardown.rs",
            weakened,
        );
        assert!(validate_pr1c_retirement_oracles(&weakened).is_err());
    }
    let unbalanced_x86_marker = provider.replacen(
        "mid_retire={}:balance={}]",
        "mid_retire={}:balance=unchecked]",
        1,
    );
    let unbalanced_x86_marker = with_replaced_source(
        &sources,
        "kernel/src/tracing/providers/teardown.rs",
        unbalanced_x86_marker,
    );
    assert!(validate_pr1c_retirement_oracles(&unbalanced_x86_marker).is_err());
    for (needle, replacement) in [
        (
            "counts.roots_retired != 1 + pending_old_roots {",
            "counts.roots_retired != 1 + pending_old_roots && 1 == 0 {",
        ),
        (
            "counts.table_frames_recorded != expected_tables + pending_old_tables {",
            "counts.table_frames_recorded != expected_tables + pending_old_tables && 1 == 0 {",
        ),
        (
            "counts.table_frames_returned != counts.table_frames_recorded + counts.roots_retired {",
            "counts.table_frames_returned != counts.table_frames_recorded + counts.roots_retired && 1 == 0 {",
        ),
    ] {
        let weakened = provider.replacen(needle, replacement, 1);
        let weakened = with_replaced_source(
            &sources,
            "kernel/src/tracing/providers/teardown.rs",
            weakened,
        );
        assert!(validate_pr1c_retirement_oracles(&weakened).is_err());
    }

    // R1: seven syntactically distinct ways to bypass the return choke point.
    for insertion in [
        "free_list.push(frame);",
        "free_list.insert(0, frame);",
        "free_list /* trivia */ .insert(0, frame);",
        "free_list.push_within_capacity(frame);",
    ] {
        let broken = allocator.replacen(
            "if let Some(frame) = free_list.pop() {",
            &format!("if let Some(frame) = free_list.pop() {{ {insertion}"),
            1,
        );
        let broken = with_replaced_source(&sources, "kernel/src/memory/frame_allocator.rs", broken);
        assert!(validate_frame_return_choke_point(&broken).is_err());
    }
    let reborrowed = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { let list = &mut *free_list; list.insert(0, frame);",
        1,
    );
    let reborrowed =
        with_replaced_source(&sources, "kernel/src/memory/frame_allocator.rs", reborrowed);
    assert!(validate_frame_return_choke_point(&reborrowed).is_err());
    let typed_reborrow = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { let list: &mut Vec<PhysFrame> = &mut free_list; list.insert(0, frame);",
        1,
    );
    let typed_reborrow = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        typed_reborrow,
    );
    assert!(validate_frame_return_choke_point(&typed_reborrow).is_err());
    let parenthesized_reborrow = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { (*free_list).insert(0, frame);",
        1,
    );
    let parenthesized_reborrow = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        parenthesized_reborrow,
    );
    assert!(validate_frame_return_choke_point(&parenthesized_reborrow).is_err());
    let indexed_alias_store = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { if free_list.len() > 0 { free_list[0] = frame; }",
        1,
    );
    let indexed_alias_store = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        indexed_alias_store,
    );
    assert!(validate_frame_return_choke_point(&indexed_alias_store).is_err());
    let deref_alias_store = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { *free_list = alloc::vec![frame, frame];",
        1,
    );
    let deref_alias_store = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        deref_alias_store,
    );
    assert!(validate_frame_return_choke_point(&deref_alias_store).is_err());
    // R1: the same physical bypass through an indexed, sliced or branch-selected alias.
    let indexed_element_replace = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { core::mem::replace(&mut free_list[0], frame);",
        1,
    );
    let indexed_element_replace = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        indexed_element_replace,
    );
    assert!(validate_frame_return_choke_point(&indexed_element_replace).is_err());
    let indexed_element_swap = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { let mut spare = frame; core::mem::swap(&mut free_list[0], &mut spare);",
        1,
    );
    let indexed_element_swap = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        indexed_element_swap,
    );
    assert!(validate_frame_return_choke_point(&indexed_element_swap).is_err());
    let indexed_element_clone_from = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { free_list[0].clone_from(&frame);",
        1,
    );
    let indexed_element_clone_from = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        indexed_element_clone_from,
    );
    assert!(validate_frame_return_choke_point(&indexed_element_clone_from).is_err());
    let sliced_copy_from_slice = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { free_list[..].copy_from_slice(&[frame]);",
        1,
    );
    let sliced_copy_from_slice = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        sliced_copy_from_slice,
    );
    assert!(validate_frame_return_choke_point(&sliced_copy_from_slice).is_err());
    let sliced_fill = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { free_list[0..1].fill(frame);",
        1,
    );
    let sliced_fill = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        sliced_fill,
    );
    assert!(validate_frame_return_choke_point(&sliced_fill).is_err());
    let indexed_helper_export = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { publish_frame(&mut free_list[0], frame);",
        1,
    );
    let indexed_helper_export = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        indexed_helper_export,
    );
    assert!(validate_frame_return_choke_point(&indexed_helper_export).is_err());
    let conditional_branch_alias_store = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { let mut spare = alloc::vec![frame]; let dest = if free_list.len() > 1 { &mut spare } else { &mut *free_list }; *dest = alloc::vec![frame];",
        1,
    );
    let conditional_branch_alias_store = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        conditional_branch_alias_store,
    );
    assert!(validate_frame_return_choke_point(&conditional_branch_alias_store).is_err());
    let conditional_branch_alias_method = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { let mut spare = alloc::vec![frame]; let dest = if free_list.len() > 1 { &mut spare } else { &mut *free_list }; dest.push(frame);",
        1,
    );
    let conditional_branch_alias_method = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        conditional_branch_alias_method,
    );
    assert!(validate_frame_return_choke_point(&conditional_branch_alias_method).is_err());
    let helper_export = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { stash_frame(&mut free_list, frame);",
        1,
    );
    let helper_export = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        helper_export,
    );
    assert!(validate_frame_return_choke_point(&helper_export).is_err());
    let ufcs_export = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { Vec::insert(&mut free_list, 0, frame);",
        1,
    );
    let ufcs_export = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        ufcs_export,
    );
    assert!(validate_frame_return_choke_point(&ufcs_export).is_err());
    let renamed_alias = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { if let Some(mut aliased) = FREE_FRAMES.try_lock() { aliased.insert(0, frame); }",
        1,
    );
    let renamed_alias = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        renamed_alias,
    );
    assert!(validate_frame_return_choke_point(&renamed_alias).is_err());
    let matched_alias = allocator.replacen(
        "if let Some(frame) = free_list.pop() {",
        "if let Some(frame) = free_list.pop() { match FREE_FRAMES.try_lock() { Some(mut renamed) => renamed.insert(0, frame), None => {} }",
        1,
    );
    let matched_alias = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        matched_alias,
    );
    assert!(validate_frame_return_choke_point(&matched_alias).is_err());
    let fixture_escape = format!(
        "{fixture}\nfn rogue_fixture(frame: PhysFrame) {{ FREE_FRAMES.lock().append(&mut alloc::vec![frame]); FREE_FRAMES.lock().extend_from_slice(&[frame]); }}"
    );
    let fixture_escape = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        fixture_escape,
    );
    assert!(validate_frame_return_choke_point(&fixture_escape).is_err());
    let process_escape = format!(
        "{process_memory}\nfn rogue_return(frame: PhysFrame) {{ crate::memory::frame_allocator::deallocate_frame(frame); }}"
    );
    let process_escape = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        process_escape,
    );
    assert!(validate_frame_return_choke_point(&process_escape).is_err());
    let cross_file_return = with_replaced_source(
        &sources,
        "kernel/src/process/manager.rs",
        format!(
            "{process_manager}\n#[cfg(test)] fn synthetic_cross_file_return() {{ if let Some(lease) = crate::memory::frame_allocator::allocate_frame_leased() {{ let _ = crate::memory::frame_allocator::return_lease(lease); }} }}"
        ),
    );
    assert!(validate_frame_return_choke_point(&cross_file_return).is_err());
    let unseeded_bootstrap_push = allocator.replacen(
        "        bootstrap.len = 0;",
        "        free_list.push(PhysFrame::containing_address(PhysAddr::new(0x100000)));\n        bootstrap.len = 0;",
        1,
    );
    let unseeded_bootstrap_push = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        unseeded_bootstrap_push,
    );
    assert!(validate_frame_return_choke_point(&unseeded_bootstrap_push).is_err());

    // R7: direct and transitive logging plus hidden capacity growth.
    let logged_counter = allocator.replacen(
        "match outcome {",
        "match outcome { _ if false => { log::warn!(\"refused\"); },",
        1,
    );
    let logged_counter = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        logged_counter,
    );
    assert!(validate_frame_ledger_hot_paths(&logged_counter).is_err());
    let logged_return = allocator.replacen(
        "pub(crate) fn return_lease(lease: FrameLease) -> ReturnOutcome {",
        "pub(crate) fn return_lease(lease: FrameLease) -> ReturnOutcome { log::info!(\"return\");",
        1,
    );
    let logged_return = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        logged_return,
    );
    assert!(validate_frame_ledger_hot_paths(&logged_return).is_err());
    let growing_return = allocator.replacen(
        "pub(crate) fn return_lease(lease: FrameLease) -> ReturnOutcome {",
        "pub(crate) fn return_lease(lease: FrameLease) -> ReturnOutcome { let _ = FREE_FRAMES.lock().try_reserve(1);",
        1,
    );
    let growing_return = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        growing_return,
    );
    assert!(validate_frame_ledger_hot_paths(&growing_return).is_err());
    let logged_get = allocator.replacen(
        "fn get(&self, index: usize) -> Option<&FrameLedgerSlot> {",
        "fn get(&self, index: usize) -> Option<&FrameLedgerSlot> { log::warn!(\"ledger get {}\", index);",
        1,
    );
    let logged_get =
        with_replaced_source(&sources, "kernel/src/memory/frame_allocator.rs", logged_get);
    assert!(validate_frame_ledger_hot_paths(&logged_get).is_err());
    let transitive_helper_log = allocator.replacen(
        "pub(crate) fn return_lease(lease: FrameLease) -> ReturnOutcome {",
        "fn note_return(frame: PhysFrame) { log::info!(\"returned {:#x}\", frame.start_address().as_u64()); }\n\nfn return_lease(lease: FrameLease) -> ReturnOutcome { note_return(lease.frame);",
        1,
    );
    let transitive_helper_log = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        transitive_helper_log,
    );
    assert!(validate_frame_ledger_hot_paths(&transitive_helper_log).is_err());

    // R9: each ordering negative calls the ordering validator directly.
    let memory_mod = source(&sources, "kernel/src/memory/mod.rs");
    let late_x86 = memory_mod.replacen(
        "frame_allocator::init_frame_ledger();\n\n    // Initialize slab caches (must be after heap)\n    slab::init();",
        "// Initialize slab caches (must be after heap)\n    slab::init();\n    frame_allocator::init_frame_ledger();",
        1,
    );
    let late_x86 = with_replaced_source(&sources, "kernel/src/memory/mod.rs", late_x86);
    assert!(validate_frame_ledger_boot_order(&late_x86).is_err());
    let arm_main = source(&sources, "kernel/src/main_aarch64.rs");
    let late_arm = arm_main.replacen(
        "kernel::memory::frame_allocator::init_frame_ledger();\n    kernel::memory::kernel_stack::init();",
        "kernel::memory::kernel_stack::init();\n    kernel::memory::frame_allocator::init_frame_ledger();",
        1,
    );
    let late_arm = with_replaced_source(&sources, "kernel/src/main_aarch64.rs", late_arm);
    assert!(validate_frame_ledger_boot_order(&late_arm).is_err());
    let preledger_root = arm_main.replacen(
        "kernel::memory::frame_allocator::init_frame_ledger();",
        "let _p = kernel::memory::process_memory::ProcessPageTable::new();\n    kernel::memory::frame_allocator::init_frame_ledger();",
        1,
    );
    let preledger_root =
        with_replaced_source(&sources, "kernel/src/main_aarch64.rs", preledger_root);
    assert!(validate_frame_ledger_boot_order(&preledger_root).is_err());
    let indirect_constructor = with_synthetic_source(
        &sources,
        "kernel/src/memory/indirect_constructor.rs",
        "fn rogue() { let _shadow = ProcessPageTable::new().expect(\"root alloc\"); }",
    );
    assert!(validate_frame_ledger_init(&indirect_constructor).is_err());

    // Demand-backing and pre-publication preparation cannot regress.
    let eager_ledger = allocator.replacen(
        "let advertised_frames =",
        "let _eager: Vec<_> = (0..total_frames).collect();\n    let advertised_frames =",
        1,
    );
    let eager_ledger = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        eager_ledger,
    );
    assert!(validate_frame_ledger_bounded_boot_allocation(&eager_ledger).is_err());
    let whole_ram_reserve = allocator.replacen(
        "let advertised_frames =",
        "FREE_FRAMES.lock().reserve_exact(total_frames);\n    let advertised_frames =",
        1,
    );
    let whole_ram_reserve = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        whole_ram_reserve,
    );
    assert!(validate_frame_ledger_bounded_boot_allocation(&whole_ram_reserve).is_err());
    let post_publish_prepare = allocator.replacen(
        "match prepare_frame_for_allocation(current) {",
        "let _after_publish = NEXT_FREE_FRAME.compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst);\n            match prepare_frame_for_allocation(current) {",
        1,
    );
    let post_publish_prepare = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        post_publish_prepare,
    );
    assert!(validate_frame_ledger_bounded_boot_allocation(&post_publish_prepare).is_err());

    // O2 mechanisms and fixtures must remain mutation-sensitive.
    let double_push = allocator.replacen(
        "ST_FREE => return counted(ReturnOutcome::RefusedDoubleRelease),",
        "ST_FREE => { FREE_FRAMES.lock().push(lease.frame); return counted(ReturnOutcome::RefusedDoubleRelease); },",
        1,
    );
    let double_push = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        double_push,
    );
    assert!(validate_frame_ledger_runtime_oracles(&double_push).is_err());
    let stale_disabled = allocator.replacen(
        "if observed >> 2 != lease.generation {",
        "if observed >> 2 != lease.generation && false {",
        1,
    );
    let stale_disabled = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        stale_disabled,
    );
    assert!(validate_frame_ledger_runtime_oracles(&stale_disabled).is_err());
    let stale_miscounted = allocator.replacen(
        "ReturnOutcome::RefusedStale => teardown::FRAME_RETURN_REFUSED_STALE.increment()",
        "ReturnOutcome::RefusedStale => teardown::FRAME_RETURN_REFUSED_DOUBLE.increment()",
        1,
    );
    let stale_miscounted = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        stale_miscounted,
    );
    assert!(validate_frame_ledger_runtime_oracles(&stale_miscounted).is_err());
    let escaped_duplicate = allocator.replacen(
        "crate::tracing::providers::teardown::FRAME_DUPLICATE_ALLOC_REFUSED.increment();\n                return Err(ClaimError::Duplicate);",
        "crate::tracing::providers::teardown::FRAME_DUPLICATE_ALLOC_REFUSED.increment();\n                return Ok(None);",
        1,
    );
    let escaped_duplicate = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        escaped_duplicate,
    );
    assert!(validate_frame_ledger_runtime_oracles(&escaped_duplicate).is_err());
    let broken_bounds = allocator.replacen(
        "\n    None\n}\n\nfn frame_in_external_leaf_span",
        "\n    Some(0)\n}\n\nfn frame_in_external_leaf_span",
        1,
    );
    let broken_bounds = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        broken_bounds,
    );
    assert!(validate_frame_ledger_runtime_oracles(&broken_bounds).is_err());
    let synthetic_stale = fixture.replacen(
        "let current = take_free_frame(stale.frame)?;",
        "let current = FrameLease { frame: stale.frame, index: stale.index, generation: stale.generation + 1 };",
        1,
    );
    let synthetic_stale = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        synthetic_stale,
    );
    assert!(validate_frame_ledger_runtime_oracles(&synthetic_stale).is_err());
    let unchecked_stale = fixture.replacen(
        "if current.index != stale.index || current.generation == stale.generation {",
        "if false {",
        1,
    );
    let unchecked_stale = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        unchecked_stale,
    );
    assert!(validate_frame_ledger_runtime_oracles(&unchecked_stale).is_err());
    let hand_forged_untracked = fixture.replacen(
        "deallocate_frame(untracked);",
        "let forged = FrameLease { frame: untracked, index: u32::MAX, generation: 0 }; let _ = return_lease(forged);",
        1,
    );
    let hand_forged_untracked = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        hand_forged_untracked,
    );
    assert!(validate_frame_ledger_runtime_oracles(&hand_forged_untracked).is_err());
    let unreserved_never_allocated = fixture.replacen(
        "if NEXT_FREE_FRAME\n            .compare_exchange(index, index + 1, Ordering::SeqCst, Ordering::SeqCst)\n            .is_ok()",
        "if true",
        1,
    );
    let unreserved_never_allocated = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        unreserved_never_allocated,
    );
    assert!(validate_frame_ledger_runtime_oracles(&unreserved_never_allocated).is_err());
    let muted_live_owner_assertion = fixture.replacen(
        "|| live_after != live_before",
        "|| (live_after != live_before) && false",
        1,
    );
    let muted_live_owner_assertion = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        muted_live_owner_assertion,
    );
    assert!(validate_frame_ledger_runtime_oracles(&muted_live_owner_assertion).is_err());
    let conditional_duplicate_cleanup = fixture.replacen(
        "    remove_duplicate_candidates(live.frame);\n    let live_after =",
        "    if false { remove_duplicate_candidates(live.frame); }\n    let live_after =",
        1,
    );
    let conditional_duplicate_cleanup = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        conditional_duplicate_cleanup,
    );
    assert!(validate_frame_ledger_runtime_oracles(&conditional_duplicate_cleanup).is_err());
    let neutered_gate_precondition = fixture.replacen(
        "if start[..5] != [0; 5] {",
        "if start[..5] != [0; 5] && false {",
        1,
    );
    let neutered_gate_precondition = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_gate_precondition,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_gate_precondition).is_err());
    let neutered_untracked_guard = fixture.replacen(
        "if after_untracked[3] != before_untracked[3] + 1 || free_after_untracked != free_before {",
        "if after_untracked[3] != before_untracked[3] + 1 || free_after_untracked != free_before && false {",
        1,
    );
    let neutered_untracked_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_untracked_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_untracked_guard).is_err());
    let neutered_never_guard = fixture.replacen(
        "if counters()[2] != before_never[2] + 1\n        || FREE_FRAMES.lock().len() != free_before\n        || !healthy_round_trip()\n    {",
        "if counters()[2] != before_never[2] + 1\n        || FREE_FRAMES.lock().len() != free_before\n        || !healthy_round_trip() && false\n    {",
        1,
    );
    let neutered_never_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_never_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_never_guard).is_err());
    let neutered_contention_guard = fixture.replacen(
        "if outcome != ReturnOutcome::LostContended {",
        "if outcome != ReturnOutcome::LostContended && false {",
        1,
    );
    let neutered_contention_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_contention_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_contention_guard).is_err());
    let neutered_healthy_guard = fixture.replacen(
        "if counters()[..5] != [1, 1, 1, 1, 3] {",
        "if counters()[..5] != [1, 1, 1, 1, 3] && false {",
        1,
    );
    let neutered_healthy_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_healthy_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_healthy_guard).is_err());
    let neutered_stale_owner_guard = fixture.replacen(
        "|state| state & STATE_MASK != ST_ALLOCATED || state >> 2 != current.generation)",
        "|state| state & STATE_MASK != ST_ALLOCATED || state >> 2 != current.generation) && 1 == 2",
        1,
    );
    let neutered_stale_owner_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_stale_owner_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_stale_owner_guard).is_err());
    let neutered_stale_recovery_guard = fixture.replacen(
        "if return_lease(current) != ReturnOutcome::Returned || !healthy_round_trip() {",
        "if return_lease(current) != ReturnOutcome::Returned || !healthy_round_trip() && 1 == 2 {",
        1,
    );
    let neutered_stale_recovery_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_stale_recovery_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_stale_recovery_guard).is_err());
    let neutered_duplicate_fixture_guard = fixture.replacen(
        "if !inject_duplicate_candidates(live.frame, 3) {",
        "if !inject_duplicate_candidates(live.frame, 3) && 1 == 2 {",
        1,
    );
    let neutered_duplicate_fixture_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_duplicate_fixture_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_duplicate_fixture_guard).is_err());
    let neutered_duplicate_cleanup_guard = fixture.replacen(
        "if !replacement_returned || !live_returned {",
        "if !replacement_returned || !live_returned && 1 == 2 {",
        1,
    );
    let neutered_duplicate_cleanup_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_duplicate_cleanup_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_duplicate_cleanup_guard).is_err());
    let neutered_duplicate_recovery_guard = fixture.replacen(
        "if !healthy_round_trip() {",
        "if !healthy_round_trip() && 1 == 2 {",
        1,
    );
    let neutered_duplicate_recovery_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_duplicate_recovery_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_duplicate_recovery_guard).is_err());
    let neutered_contended_isolation_guard = fixture.replacen(
        "if lost_state.is_none_or(|state| state & STATE_MASK != ST_FREE)\n        || free_frame_count(contended.frame) != 0\n    {",
        "if lost_state.is_none_or(|state| state & STATE_MASK != ST_FREE)\n        || free_frame_count(contended.frame) != 0 && 1 == 2\n    {",
        1,
    );
    let neutered_contended_isolation_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_contended_isolation_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_contended_isolation_guard).is_err());
    let neutered_contended_recovery_guard = fixture.replacen(
        "if return_lease(repaired) != ReturnOutcome::Returned\n        || counters()[..5] != before_healthy[..5]\n        || !healthy_round_trip()\n    {",
        "if return_lease(repaired) != ReturnOutcome::Returned\n        || counters()[..5] != before_healthy[..5]\n        || !healthy_round_trip() && 1 == 2\n    {",
        1,
    );
    let neutered_contended_recovery_guard = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        neutered_contended_recovery_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&neutered_contended_recovery_guard).is_err());

    // Registration is keyed by the function identity, not display spelling.
    let registry = source(&sources, "kernel/src/test_framework/registry.rs");
    let any_arch = registry.replacen(
        "func: crate::memory::frame_allocator::frame_custody_refusal_gate_test,\n        arch: Arch::Aarch64,",
        "func: crate::memory::frame_allocator::frame_custody_refusal_gate_test,\n        arch: Arch::Any,",
        1,
    );
    let any_arch =
        with_replaced_source(&sources, "kernel/src/test_framework/registry.rs", any_arch);
    assert!(validate_frame_ledger_runtime_oracles(&any_arch).is_err());
    let aliased_registration = format!(
        "use crate::memory::frame_allocator::frame_custody_refusal_gate_test as aliased_frame_custody_gate;\n{registry}\nconst _: Option<fn() -> TestResult> = Some(aliased_frame_custody_gate);"
    );
    let aliased_registration = with_replaced_source(
        &sources,
        "kernel/src/test_framework/registry.rs",
        aliased_registration,
    );
    assert!(validate_frame_ledger_runtime_oracles(&aliased_registration).is_err());
    let duplicate_registration = registry.replacen(
        "    TestDef {\n        name: \"process_manager_init\",",
        "    TestDef { name: \"frame_custody_refusal_gate_x86\", func: crate::memory::frame_allocator::frame_custody_refusal_gate_test, arch: Arch::Any, timeout_ms: 5000, stage: TestStage::EarlyBoot },\n    TestDef {\n        name: \"process_manager_init\",",
        1,
    );
    let duplicate_registration = with_replaced_source(
        &sources,
        "kernel/src/test_framework/registry.rs",
        duplicate_registration,
    );
    assert!(validate_frame_ledger_runtime_oracles(&duplicate_registration).is_err());
    let deleted_healthy_guard = registry.replacen(
        "    TestDef {\n        name: \"frame_custody_healthy_counters\",\n        func: crate::memory::frame_allocator::frame_custody_healthy_counters_test,\n        arch: Arch::Aarch64,\n        timeout_ms: 5000,\n        stage: TestStage::ProcessContext,\n    },\n",
        "",
        1,
    );
    let deleted_healthy_guard = with_replaced_source(
        &sources,
        "kernel/src/test_framework/registry.rs",
        deleted_healthy_guard,
    );
    assert!(validate_frame_ledger_runtime_oracles(&deleted_healthy_guard).is_err());
    let parallel_gate = registry.replacen(
        "stage: TestStage::SerialBoot,",
        "stage: TestStage::EarlyBoot,",
        1,
    );
    let parallel_gate = with_replaced_source(
        &sources,
        "kernel/src/test_framework/registry.rs",
        parallel_gate,
    );
    assert!(validate_frame_ledger_runtime_oracles(&parallel_gate).is_err());
    let executor = source(&sources, "kernel/src/test_framework/executor.rs");
    let nonserial_join = executor.replacen(
        "if target_stage == TestStage::SerialBoot {",
        "if target_stage == TestStage::SerialBoot && false {",
        1,
    );
    let nonserial_join = with_replaced_source(
        &sources,
        "kernel/src/test_framework/executor.rs",
        nonserial_join,
    );
    assert!(validate_frame_ledger_runtime_oracles(&nonserial_join).is_err());
    let nested_false_join = executor.replacen(
        "                if target_stage == TestStage::SerialBoot {\n                    total_failed += join_test_thread(subsystem.id, handle);\n                } else {\n                    handles.push((subsystem.id, handle));\n                }",
        "                if target_stage == TestStage::SerialBoot {\n                    if false {\n                        total_failed += join_test_thread(subsystem.id, handle);\n                    } else {\n                        handles.push((subsystem.id, handle));\n                    }\n                } else {\n                    handles.push((subsystem.id, handle));\n                }",
        1,
    );
    let nested_false_join = with_replaced_source(
        &sources,
        "kernel/src/test_framework/executor.rs",
        nested_false_join,
    );
    assert!(validate_frame_ledger_runtime_oracles(&nested_false_join).is_err());
    let vacuous_timer = registry.replacen("test_timer_ticks()", "TestResult::Pass", 1);
    let vacuous_timer = with_replaced_source(
        &sources,
        "kernel/src/test_framework/registry.rs",
        vacuous_timer,
    );
    assert!(validate_frame_ledger_runtime_oracles(&vacuous_timer).is_err());
    let spin_budget_workqueue = registry.replacen(
        "    work.wait();\n",
        "    for _ in 0..1_000_000 {\n        core::hint::spin_loop();\n    }\n",
        1,
    );
    assert!(validate_workqueue_progress_wait(&spin_budget_workqueue).is_err());
    let unscored_workqueue = registry.replacen(
        "if WORK_RUNS.load(Ordering::SeqCst) != before.wrapping_add(1) {",
        "if WORK_RUNS.load(Ordering::SeqCst) != before.wrapping_add(1) && 1 == 2 {",
        1,
    );
    assert!(validate_workqueue_progress_wait(&unscored_workqueue).is_err());

    let vacuous_frame_tests = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        format!("{fixture}\nfn synthetic_vacuity() {{ if false {{}} }}"),
    );
    assert!(validate_no_vacuous_test_conditions(&vacuous_frame_tests).is_err());
    let vacuous_registry = with_replaced_source(
        &sources,
        "kernel/src/test_framework/registry.rs",
        format!("{registry}\nfn synthetic_vacuity() {{ if true {{}} }}"),
    );
    assert!(validate_no_vacuous_test_conditions(&vacuous_registry).is_err());
    let vacuous_executor = with_replaced_source(
        &sources,
        "kernel/src/test_framework/executor.rs",
        format!("{executor}\nfn synthetic_vacuity() {{ while false {{}} }}"),
    );
    assert!(validate_no_vacuous_test_conditions(&vacuous_executor).is_err());
    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let vacuous_provider = with_replaced_source(
        &sources,
        "kernel/src/tracing/providers/teardown.rs",
        format!("{provider}\n#[cfg(any())]\nfn synthetic_vacuity() {{}}"),
    );
    assert!(validate_no_vacuous_test_conditions(&vacuous_provider).is_err());
    let vacuous_and_false = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator_tests.rs",
        format!(
            "{fixture}\nfn synthetic_vacuity_operand() {{ if counters()[0] == 0 && false {{}} }}"
        ),
    );
    assert!(validate_no_vacuous_test_conditions(&vacuous_and_false).is_err());
    let vacuous_false_and = with_replaced_source(
        &sources,
        "kernel/src/test_framework/registry.rs",
        format!(
            "{registry}\nfn synthetic_vacuity_operand() {{ if false && counters_ready() {{}} }}"
        ),
    );
    assert!(validate_no_vacuous_test_conditions(&vacuous_false_and).is_err());

    let harness = repo_text("docker/qemu/run-x86-boot-tests.sh");
    assert!(validate_x86_frame_custody_harness(&harness.replace("-eq 1", "-ge 0")).is_err());
    assert!(validate_x86_frame_custody_harness(
        &harness.replace("\n    $passed\n", "\n    $passed || true\n")
    )
    .is_err());
    assert!(validate_x86_frame_custody_harness(
        &harness.replace("set -euo pipefail", "set -uo pipefail\nset +e")
    )
    .is_err());
    assert!(validate_x86_frame_custody_harness(
        &harness.replace("\n    $passed\n", "\n    exit 0\n    $passed\n")
    )
    .is_err());
    assert!(validate_x86_frame_custody_harness(
        &harness.replace(
            "recorded=11:no_proof=0:no_arch=0:terminated=1:undecided=1:retired=1:returned=10:lost=0:requeued=0",
            "recorded=3:no_proof=0:no_arch=1:terminated=1:undecided=1",
        )
    )
    .is_err());
    assert!(validate_x86_frame_custody_harness(
        &harness.replace(
            "[PT_RETIRE_COHORT:x86:children=64:retired=65:returned=642:recorded=577:lost=0:no_arch=0:undecided=0:mid_retire=0:balance=0]",
            "[PT_RETIRE_COHORT:x86:.*]",
        )
    )
    .is_err());
    let cohort_count_start = harness
        .find("grep -h -c '\\[TEST:process:x86_retire_cohort:PASS\\]'")
        .expect("x86 cohort count assertion");
    let cohort_equality = harness[cohort_count_start..]
        .find("-eq 1")
        .map(|offset| cohort_count_start + offset)
        .expect("x86 cohort exact count equality");
    let mut weakened_cohort_count = harness.clone();
    weakened_cohort_count.replace_range(cohort_equality..cohort_equality + 5, "-ge 0");
    assert!(validate_x86_frame_custody_harness(&weakened_cohort_count).is_err());

    let missing_counter_reader = provider.replacen("    &FRAME_RETURN_REFUSED_DOUBLE,\n", "", 1);
    assert!(validate_frame_ledger_counter_inventory(&missing_counter_reader).is_err());
    let unproduced_counter = provider.replacen(
        "counter!(FRAME_LOST_CONTENDED, \"Frame returns lost to contention\");",
        "counter!(FRAME_LOST_CONTENDED, \"Frame returns lost to contention\");\ncounter!(FRAME_RETURN_REFUSED_UNUSED, \"Unproduced frame refusal\");",
        1,
    );
    assert!(validate_frame_ledger_counter_inventory(&unproduced_counter).is_err());
}

/// The ARM64 `timer_delay` re-measurement may only be granted on evidence that
/// the vCPU itself stopped executing. This pins the leniency-granting predicates
/// so a future edit cannot quietly turn the screen into a retry-until-green loop.
fn check_timer_delay_starvation_ratchet(body: &str) -> Result<(), String> {
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let require = |needle: &str| {
        body.contains(needle)
            .then_some(())
            .ok_or_else(|| format!("missing timer_delay safety anchor: {needle}"))
    };

    // Pin the target and leniency threshold as well as the bound expressions.
    require("const TARGET_MS: u64 = 10;")?;
    require("const MIN_MS: u64 = TARGET_MS / 2;")?;
    require("const MAX_MS: u64 = TARGET_MS * 2;")?;
    require("const CONTAMINATION_STALL_MILLISECONDS: u64 = 1;")?;
    require("TestResult::Fail(\"delay too short on ARM64\")")?;
    require("TestResult::Fail(\"delay too long on ARM64\")")?;
    require("if in_band { return TestResult::Pass;")?;

    // The window uses short IRQ+FIQ-masked slices. Each open window remains
    // open until both premise counters are quiet, subject to a hard cap.
    require("const SLICE_MICROSECONDS: u64 = 100;")?;
    require("const DRAIN_QUIET_MICROSECONDS: u64 = 5;")?;
    require("const DRAIN_CAP_MICROSECONDS: u64 = 1_000;")?;
    require("msr daifset, #3")?;
    require("msr daifclr, #3")?;
    require("if may_open_windows {")?;
    require("timer::elapsed_ticks(now, quiet_start) >= quiet_ticks")?;
    require("timer::elapsed_ticks(now, drain_start) >= cap_ticks")?;
    require("open_window_ticks = open_window_ticks.saturating_add(drain.elapsed_ticks);")?;

    // Counter lookup is per attempt, after preemption is disabled, and an
    // unindexable IRQ or synchronous-exception counter fails closed.
    let preempt = body
        .find("let _preempt_guard = TimerDelayPreemptGuard::enter();")
        .ok_or_else(|| "missing preemption guard".to_owned())?;
    let cpu = body[preempt..]
        .find("Aarch64PerCpu::cpu_id() as usize;")
        .map(|offset| preempt + offset)
        .ok_or_else(|| "CPU id is not sampled after preemption is disabled".to_owned())?;
    if cpu <= preempt {
        return Err("CPU id must be sampled inside the guarded window".to_owned());
    }
    require("let Some(irq) = IRQ_TOTAL.per_cpu.get(cpu) else { return None;")?;
    require("let Some(sync) = SYNC_EXCEPTION_COUNT.get(cpu) else { return None;")?;
    require("_ => false,")?;

    // Only gaps between samples inside the timestamp reads can be credited.
    // The boundary-bracket bookkeeping that caused the review hard-stop must
    // not return under another accidental edit.
    for forbidden in [
        "pre_start_counter",
        "post_end_counter",
        "opening_gap",
        "closing_gap",
    ] {
        if body.contains(forbidden) {
            return Err(format!(
                "timestamp boundary bracket must remain uncredited: {forbidden}"
            ));
        }
    }
    if body
        .matches("host_stall_ticks = host_stall_ticks.saturating_add(slice_stall_ticks);")
        .count()
        != 2
    {
        return Err(
            "host stall credit must come from exactly the completed and final masked slices"
                .to_owned(),
        );
    }
    require("fn sample_counter(samples: &mut u64) -> u64")?;
    require("*samples = samples.saturating_add(1);")?;
    if body.contains("samples = samples.saturating_add(2)")
        || body.contains("samples = samples.saturating_add(3)")
    {
        return Err("sample diagnostics must count each CNTVCT sample at its call site".to_owned());
    }

    // The final partial slice is decided while masked, then pending work is
    // drained before the closing scored timestamp.
    let final_premise = body
        .find("let final_slice_counters = counter_snapshot(cpu);")
        .ok_or_else(|| "missing final masked-slice premise check".to_owned())?;
    let final_drain = body[final_premise..]
        .find("let final_drain = drain_interrupts(")
        .map(|offset| final_premise + offset)
        .ok_or_else(|| "missing final interrupt drain".to_owned())?;
    let closing_timestamp = body[final_drain..]
        .find("let end_ns = timer::nanoseconds_since_base().unwrap_or(0);")
        .map(|offset| final_drain + offset)
        .ok_or_else(|| "final drain must precede the closing timestamp".to_owned())?;
    if !(final_premise < final_drain && final_drain < closing_timestamp) {
        return Err("final premise, drain, and closing timestamp are out of order".to_owned());
    }

    // Only credited stall, large enough to explain the overrun, sends a window
    // back for re-measurement; everything else is a verdict on this attempt.
    require("let host_starved = screened")?;
    require("&& measurement.host_stall_ticks >= contamination_stall_ticks")?;
    require("&& measurement.elapsed_ms.saturating_sub(host_stall_ms) <= MAX_MS;")?;
    require("if !host_starved { return TestResult::Fail(\"delay too long on ARM64\");")?;

    // Re-measurement stays bounded by attempts and by wall clock, and running
    // out of either is a distinct failure rather than a pass.
    require("const MAX_MEASUREMENT_ATTEMPTS: u32 = 4;")?;
    require("const MEASUREMENT_BUDGET_MS: u64 = 400;")?;
    require("TestResult::Fail(\"timer delay never observed an unstarved window on ARM64\")")?;

    Ok(())
}

#[test]
fn timer_delay_retry_is_gated_on_proven_host_starvation() {
    let registry = repo_text("kernel/src/test_framework/registry.rs");
    let body = function_body(&registry, "test_timer_delay");
    check_timer_delay_starvation_ratchet(body).expect("timer_delay starvation safety ratchet");
}

#[test]
fn deliberately_broken_timer_delay_variants_fail_the_ratchet() {
    let registry = repo_text("kernel/src/test_framework/registry.rs");
    let body = function_body(&registry, "test_timer_delay");

    let widened_target = body.replacen(
        "const TARGET_MS: u64 = 10;",
        "const TARGET_MS: u64 = 40;",
        1,
    );
    assert!(check_timer_delay_starvation_ratchet(&widened_target).is_err());

    let skipped_final_drain = body.replacen(
        "let final_drain = drain_interrupts(",
        "let final_drain = skip_interrupt_drain(",
        1,
    );
    assert!(check_timer_delay_starvation_ratchet(&skipped_final_drain).is_err());
}

fn validate_x86_direct_teardown_gates(
    main: &str,
    process_task: &str,
    teardown: &str,
    harness: &str,
) -> Result<(), &'static str> {
    let kernel_main = function_body(main, "kernel_main_on_kernel_stack");
    let retirement_call = kernel_main
        .find("process_task::run_x86_retirement_fence_gate();")
        .ok_or("missing direct x86 retirement-fence call")?;
    let progress_call = kernel_main
        .find("process_task::run_x86_reclaim_progress_gate();")
        .ok_or("missing direct x86 reclaim-progress call")?;
    let cohort_call = kernel_main
        .find("teardown::run_x86_retire_cohort_gate();")
        .ok_or("missing direct x86 cohort call")?;
    let exec_cohort_call = kernel_main
        .find("teardown::run_x86_exec_cohort_gate();")
        .ok_or("missing direct x86 exec cohort call")?;
    if !(retirement_call < progress_call
        && progress_call < cohort_call
        && cohort_call < exec_cohort_call)
        || !kernel_main.contains("The state-free fence check runs first")
        || !kernel_main.contains("The retire cohort follows")
        || !kernel_main.contains("the exec cohort runs last because")
    {
        return Err("x86 teardown-gate ordering or rationale changed");
    }

    for (wrapper, body, marker) in [
        (
            "run_x86_retirement_fence_gate",
            "retirement_fence_gate_test",
            "retirement_fence_gate",
        ),
        (
            "run_x86_reclaim_progress_gate",
            "reclaim_progress_gate_test",
            "reclaim_progress_gate",
        ),
    ] {
        let wrapper_body = function_body(process_task, wrapper);
        for suffix in ["START", "PASS", "FAIL"] {
            if !wrapper_body.contains(&format!("[TEST:process:{marker}:{suffix}")) {
                return Err("direct x86 gate wrapper lost a required marker");
            }
        }
        if !wrapper_body.contains("assert!(result.is_pass()")
            || function_body(process_task, body).contains(&format!(
                "[TEST:process:{marker}:PASS]"
            ))
        {
            return Err("direct x86 gate is not fail-loud or has two PASS producers");
        }
        if harness.matches(&format!("TEST:process:{marker}:PASS")).count() != 2 {
            return Err("x86 harness does not poll and count the PASS marker");
        }
        let count_anchor = format!("grep -h -c '\\[TEST:process:{marker}:PASS\\]'");
        let count = harness
            .find(&count_anchor)
            .ok_or("x86 harness lost an exact PASS count")?;
        if !harness[count..].contains("-eq 1") {
            return Err("x86 harness weakened an exactly-once PASS count");
        }
    }

    let cohort_body = function_body(teardown, "fork_exit_defer_reclaim_pairing_test");
    let cohort_wrapper = function_body(teardown, "run_x86_retire_cohort_gate");
    if cohort_body
        .matches("[TEST:process:x86_retire_cohort:PASS]")
        .count()
        != 1
        || cohort_wrapper.contains("[TEST:process:x86_retire_cohort:PASS]")
    {
        return Err("cohort PASS producer is no longer body-only");
    }
    Ok(())
}

#[test]
fn x86_teardown_gates_are_direct_reachable_and_exactly_once() {
    assert_eq!(
        validate_x86_direct_teardown_gates(
            &repo_text("kernel/src/main.rs"),
            &repo_text("kernel/src/task/process_task.rs"),
            &repo_text("kernel/src/tracing/providers/teardown.rs"),
            &repo_text("docker/qemu/run-x86-boot-tests.sh"),
        ),
        Ok(())
    );
}

#[test]
fn direct_x86_teardown_gate_validator_rejects_dead_tail_and_duplicate_pass() {
    let main = r#"
        fn kernel_main_on_kernel_stack() {
            process_task::run_x86_retirement_fence_gate();
            process_task::run_x86_reclaim_progress_gate();
            teardown::run_x86_retire_cohort_gate();
        }
    "#;
    let process = r#"
        fn retirement_fence_gate_test() { serial_println!("[TEST:process:retirement_fence_gate:PASS]"); }
        fn reclaim_progress_gate_test() {}
        fn run_x86_retirement_fence_gate() {
            serial_println!("[TEST:process:retirement_fence_gate:START]");
            let result = retirement_fence_gate_test();
            serial_println!("[TEST:process:retirement_fence_gate:PASS]");
            serial_println!("[TEST:process:retirement_fence_gate:FAIL]");
            assert!(result.is_pass());
        }
        fn run_x86_reclaim_progress_gate() {
            serial_println!("[TEST:process:reclaim_progress_gate:START]");
            let result = reclaim_progress_gate_test();
            serial_println!("[TEST:process:reclaim_progress_gate:PASS]");
            serial_println!("[TEST:process:reclaim_progress_gate:FAIL]");
            assert!(result.is_pass());
        }
    "#;
    let teardown = r#"
        fn fork_exit_defer_reclaim_pairing_test() { serial_println!("[TEST:process:x86_retire_cohort:PASS]"); }
        fn run_x86_retire_cohort_gate() {}
    "#;
    let harness = "TEST:process:retirement_fence_gate:PASS TEST:process:retirement_fence_gate:PASS -eq 1 TEST:process:reclaim_progress_gate:PASS TEST:process:reclaim_progress_gate:PASS -eq 1";
    assert!(validate_x86_direct_teardown_gates(main, process, teardown, harness).is_err());
}

fn validate_reclaim_progress_topology_arms(process_task: &str) -> Result<(), &'static str> {
    let gate = function_body(process_task, "reclaim_progress_gate_test");
    let gate_mask = code_mask(gate);
    let multi_marker = "if scheduler::MAX_CPUS >= 2";
    let multi_start = code_offsets(gate, &gate_mask, multi_marker)
        .into_iter()
        .next()
        .ok_or("reclaim gate lost its multi-CPU discriminator")?;
    let multi_arm = braced_block(gate, &gate_mask, multi_start)
        .ok_or("reclaim gate multi-CPU arm is not brace balanced")?;
    let one_marker = "else if scheduler::MAX_CPUS == 1";
    let one_start = code_offsets(gate, &gate_mask, one_marker)
        .into_iter()
        .next()
        .ok_or("reclaim gate lost its one-CPU discriminator")?;
    let one_arm = braced_block(gate, &gate_mask, one_start)
        .ok_or("reclaim gate one-CPU arm is not brace balanced")?;
    if one_start <= multi_start + multi_arm.len() || one_arm.contains("target_arch") {
        return Err("reclaim gate topology arms are not selected solely by CPU count");
    }

    let compact = |fragment: &str| normalized_code(fragment).replace(' ', "");
    let multi = compact(multi_arm);
    for required in [
        "letage_advance_cpu=scheduler::MAX_CPUS.saturating_sub(2);",
        "letage_last_cpu=scheduler::MAX_CPUS.saturating_sub(1);",
        "letage_mask=(1<<age_advance_cpu)|(1<<age_last_cpu);",
        "boot_push_parked(age_pid,age_record);",
        "age_63.epochs[age_advance_cpu]=age_63.epochs[age_advance_cpu].wrapping_add(63);",
        "boot_reclaim_locations(age_pid)!=(false,true)",
        "age_64.epochs[age_advance_cpu]=age_64.epochs[age_advance_cpu].wrapping_add(1);",
        "RECLAIM_UNPARKED_AGE.aggregate().saturating_sub(age_before)!=1",
    ] {
        if !multi.contains(required) {
            return Err("reclaim gate multi-CPU age proposition was weakened");
        }
    }

    let one = compact(one_arm);
    for required in [
        "letepoch_cpu=0;",
        "boot_synthetic_park(1<<epoch_cpu,200);",
        "boot_push_parked(epoch_pid,epoch_record);",
        "epoch_advanced.epochs[epoch_cpu]=epoch_advanced.epochs[epoch_cpu].wrapping_add(1);",
        "unpark_sweep_with_snapshot(epoch_advanced,epoch_record.row_epoch_at_park);",
        "RECLAIM_UNPARKED_EPOCH.aggregate().saturating_sub(epoch_before)!=1",
        "RECLAIM_UNPARKED_AGE.aggregate()!=age_before",
        "boot_reclaim_locations(epoch_pid)!=(true,false)",
    ] {
        if !one.contains(required) {
            return Err("reclaim gate one-CPU epoch proposition was weakened or skipped");
        }
    }
    Ok(())
}

#[test]
fn reclaim_progress_park_unpark_arms_follow_cpu_topology() {
    assert_eq!(
        validate_reclaim_progress_topology_arms(&repo_text(
            "kernel/src/task/process_task.rs"
        )),
        Ok(())
    );
}

#[test]
fn reclaim_progress_topology_validator_rejects_arch_selection_and_a_skipped_counter() {
    let process_task = repo_text("kernel/src/task/process_task.rs");
    let arch_selected = process_task.replacen(
        "if scheduler::MAX_CPUS >= 2",
        "if cfg!(target_arch = \"aarch64\")",
        1,
    );
    assert!(validate_reclaim_progress_topology_arms(&arch_selected).is_err());

    let skipped_counter = process_task.replacen(
        "trace::RECLAIM_UNPARKED_EPOCH\n            .aggregate()\n            .saturating_sub(epoch_before)\n            != 1",
        "trace::RECLAIM_UNPARKED_EPOCH.aggregate() == epoch_before",
        2,
    );
    assert!(validate_reclaim_progress_topology_arms(&skipped_counter).is_err());
}

fn validate_single_gate_producer_per_arch(main: &str, registry: &str) -> Result<(), ()> {
    let normalized_registry = normalized_code(registry);
    for (function, direct_call) in [
        (
            "fork_exit_defer_reclaim_pairing_test",
            "teardown::run_x86_retire_cohort_gate();",
        ),
        (
            "retirement_fence_gate_test",
            "process_task::run_x86_retirement_fence_gate();",
        ),
        (
            "reclaim_progress_gate_test",
            "process_task::run_x86_reclaim_progress_gate();",
        ),
    ] {
        let registration = format!(
            "func: crate::{}",
            if function == "fork_exit_defer_reclaim_pairing_test" {
                format!("tracing::providers::teardown::{function}")
            } else {
                format!("task::process_task::{function}")
            }
        );
        if normalized_registry.matches(&registration).count() != 1 {
            return Err(());
        }
        let offset = normalized_registry.find(&registration).ok_or(())?;
        let entry_start = normalized_registry[..offset]
            .rfind("TestDef {")
            .ok_or(())?;
        let entry_end = normalized_registry[offset..]
            .find("},")
            .map(|end| offset + end)
            .ok_or(())?;
        let entry = &normalized_registry[entry_start..entry_end];
        if !entry.contains("arch: Arch::Aarch64")
            || entry.contains("arch: Arch::Any")
            || entry.contains("arch: Arch::X86_64")
            || !function_body(main, "kernel_main_on_kernel_stack").contains(direct_call)
        {
            return Err(());
        }
    }
    Ok(())
}

#[test]
fn direct_x86_teardown_gates_keep_only_aarch64_registry_producers() {
    assert_eq!(
        validate_single_gate_producer_per_arch(
            &repo_text("kernel/src/main.rs"),
            &repo_text("kernel/src/test_framework/registry.rs"),
        ),
        Ok(())
    );
}

#[test]
fn gate_producer_validator_rejects_arch_any_double_registration() {
    let main = r#"
        fn kernel_main_on_kernel_stack() {
            teardown::run_x86_retire_cohort_gate();
            process_task::run_x86_retirement_fence_gate();
            process_task::run_x86_reclaim_progress_gate();
        }
    "#;
    let registry = r#"
        TestDef { func: crate::tracing::providers::teardown::fork_exit_defer_reclaim_pairing_test, arch: Arch::Aarch64 },
        TestDef { func: crate::task::process_task::retirement_fence_gate_test, arch: Arch::Any },
        TestDef { func: crate::task::process_task::reclaim_progress_gate_test, arch: Arch::Aarch64 },
    "#;
    assert!(validate_single_gate_producer_per_arch(main, registry).is_err());
}

fn validate_nonowning_reclaim_queue_acquisitions(process_task: &str) -> Result<(), ()> {
    for name in [
        "push_pending_or_abandon",
        "park_reclaim",
        "unpark_sweep_with_snapshot",
    ] {
        let body = function_body(process_task, name);
        let code = normalized_code(body);
        if code.contains(".lock()") || !code.contains(".try_lock()") {
            return Err(());
        }
    }
    let push = function_body(process_task, "push_pending_or_abandon");
    let park = function_body(process_task, "park_reclaim");
    let unpark = function_body(process_task, "unpark_sweep_with_snapshot");
    (push.contains("abandon_unqueued_reclaim")
        && park.contains("push_pending_or_abandon")
        && unpark.contains("return false")
        && unpark.contains("abandon_unqueued_reclaim"))
    .then_some(())
    .ok_or(())
}

#[test]
fn nonowning_reclaim_queue_paths_are_bounded_and_fail_closed() {
    assert_eq!(
        validate_nonowning_reclaim_queue_acquisitions(&repo_text(
            "kernel/src/task/process_task.rs"
        )),
        Ok(())
    );
}

#[test]
fn reclaim_queue_validator_rejects_a_blocking_enqueue_lock() {
    let synthetic = r#"
        fn push_pending_or_abandon(reclaim: Reclaim) {
            let mut pending = PENDING_PROCESS_RECLAIMS.lock();
            abandon_unqueued_reclaim(reclaim);
        }
        fn park_reclaim(reclaim: Reclaim) {
            let Some(mut parked) = PARKED_PROCESS_RECLAIMS.try_lock() else { return; };
            push_pending_or_abandon(reclaim);
        }
        fn unpark_sweep_with_snapshot() {
            let Some(mut parked) = PARKED_PROCESS_RECLAIMS.try_lock() else { return false; };
            abandon_unqueued_reclaim(parked.pop());
        }
    "#;
    assert!(validate_nonowning_reclaim_queue_acquisitions(synthetic).is_err());
}

fn validate_reclaim_nesting_counter(process_task: &str, provider: &str) -> Result<(), ()> {
    if !provider.contains("counter!(\n    RECLAIM_DRAIN_NESTED_REFUSED,")
        || !provider.contains("&RECLAIM_DRAIN_NESTED_REFUSED,")
    {
        return Err(());
    }
    let body = function_body(process_task, "reclaim_deferred_process_resources");
    let violation = body.find("RECLAIM_CONTEXT_VIOLATIONS").ok_or(())?;
    let compare_exchange = body.find("RECLAIM_DRAIN_ACTIVE").ok_or(())?;
    let nesting = body.find("RECLAIM_DRAIN_NESTED_REFUSED").ok_or(())?;
    if !(violation < compare_exchange && compare_exchange < nesting)
        || body[nesting..].contains("RECLAIM_CONTEXT_VIOLATIONS")
        || !body.contains("the receipt is already queued; the owning drain will take it")
    {
        return Err(());
    }
    Ok(())
}

#[test]
fn benign_reclaim_nesting_has_a_distinct_counter() {
    assert_eq!(
        validate_reclaim_nesting_counter(
            &repo_text("kernel/src/task/process_task.rs"),
            &repo_text("kernel/src/tracing/providers/teardown.rs"),
        ),
        Ok(())
    );
}

#[test]
fn reclaim_nesting_validator_rejects_context_violation_conflation() {
    let process = r#"
        fn reclaim_deferred_process_resources() {
            trace_count!(RECLAIM_CONTEXT_VIOLATIONS);
            if RECLAIM_DRAIN_ACTIVE.compare_exchange(false, true).is_err() {
                trace_count!(RECLAIM_CONTEXT_VIOLATIONS);
            }
        }
    "#;
    let provider = r#"
        counter!(RECLAIM_DRAIN_NESTED_REFUSED, "nested");
        static COUNTERS: [&Counter; 1] = [&RECLAIM_DRAIN_NESTED_REFUSED,];
    "#;
    assert!(validate_reclaim_nesting_counter(process, provider).is_err());
}

fn validate_shadow_root_clear_counter(process_task: &str, provider: &str) -> Result<(), ()> {
    if !provider.contains("counter!(PT_SHADOW_ROOT_CLEARED,")
        || !provider.contains("&PT_SHADOW_ROOT_CLEARED,")
    {
        return Err(());
    }
    let body = function_body(process_task, "clear_shadow_root");
    let clear = body.find("crate::per_cpu::set_saved_process_cr3(0)").ok_or(())?;
    let count = body.find("PT_SHADOW_ROOT_CLEARED").ok_or(())?;
    (clear < count).then_some(()).ok_or(())
}

#[test]
fn shadow_root_clears_are_observable_in_production_counters() {
    assert_eq!(
        validate_shadow_root_clear_counter(
            &repo_text("kernel/src/task/process_task.rs"),
            &repo_text("kernel/src/tracing/providers/teardown.rs"),
        ),
        Ok(())
    );
}

#[test]
fn shadow_root_counter_validator_rejects_an_uncounted_clear() {
    let process = r#"
        fn clear_shadow_root(root: u64) {
            if roots_match(saved(), root) { crate::per_cpu::set_saved_process_cr3(0); }
        }
    "#;
    let provider = r#"
        counter!(PT_SHADOW_ROOT_CLEARED, "cleared");
        static COUNTERS: [&Counter; 1] = [&PT_SHADOW_ROOT_CLEARED,];
    "#;
    assert!(validate_shadow_root_clear_counter(process, provider).is_err());
}

fn validate_x86_leaf_timing_oracle_is_live(
    process_task: &str,
    teardown: &str,
    harness: &str,
) -> Result<(), ()> {
    let defer = function_body(process_task, "defer_process_resources");
    let reclaim = function_body(process_task, "reclaim_bounded");
    let cohort = function_body(teardown, "fork_exit_defer_reclaim_pairing_test");
    if defer.contains("drain_old_page_tables_counted(process)")
        || !defer.contains("core::mem::take(&mut process.pending_old_page_tables)")
        || !reclaim.contains("drain_old_page_tables_counted(")
        || !cohort.contains("if iteration == 0")
        || !cohort.contains("pending_old_page_tables.push(")
        || !cohort.contains("ProcessPageTable::new()")
        || !cohort.contains("TEARDOWN_MASKED_FRAMES_WALKED")
        || !cohort.contains("!= 0")
        || !harness.contains(
            "PT_COHORT_LITERAL='[PT_RETIRE_COHORT:x86:children=64:retired=65:returned=642:recorded=577:lost=0:no_arch=0:undecided=0:mid_retire=0:balance=0]'",
        )
    {
        return Err(());
    }
    Ok(())
}

#[test]
fn x86_leaf_timing_oracle_has_an_exec_root_producer_in_its_window() {
    assert_eq!(
        validate_x86_leaf_timing_oracle_is_live(
            &repo_text("kernel/src/task/process_task.rs"),
            &repo_text("kernel/src/tracing/providers/teardown.rs"),
            &repo_text("docker/qemu/run-x86-boot-tests.sh"),
        ),
        Ok(())
    );
}

#[test]
fn leaf_timing_oracle_validator_rejects_an_empty_old_root_fixture() {
    let process = r#"
        fn defer_process_resources(process: &mut Process) {
            let old_page_tables = core::mem::take(&mut process.pending_old_page_tables);
        }
        fn reclaim_bounded() { drain_old_page_tables_counted(); }
    "#;
    let teardown = r#"
        fn fork_exit_defer_reclaim_pairing_test() {
            if iteration == 0 { let old = ProcessPageTable::new(); }
            if TEARDOWN_MASKED_FRAMES_WALKED.aggregate() != 0 { fail(); }
        }
    "#;
    let harness = "PT_COHORT_LITERAL='[PT_RETIRE_COHORT:x86:children=64:retired=65:returned=642:recorded=577:lost=0:no_arch=0:undecided=0:mid_retire=0:balance=0]'";
    assert!(validate_x86_leaf_timing_oracle_is_live(process, teardown, harness).is_err());
}

fn validate_production_root_custody_summary(
    provider: &str,
    procfs_trace: &str,
    x86_main: &str,
    arm_main: &str,
) -> Result<(), ()> {
    let marker = "pub fn emit_root_custody_summary";
    let declaration = provider.find(marker).ok_or(())?;
    let prefix_start = provider[..declaration]
        .rfind('}')
        .map(|offset| offset + 1)
        .unwrap_or(0);
    if provider[prefix_start..declaration].contains("boot_tests") {
        return Err(());
    }
    let body = function_body(provider, "emit_root_custody_summary");
    for required in [
        "PT_ROOT_ABANDONED_NO_PROOF",
        "PT_ROOT_ABANDONED_NO_ARCH",
        "PT_ROOT_ABANDONED_TERMINATED",
        "PT_ROOT_DROPPED_UNDECIDED",
        "PT_ROOT_DROPPED_MID_RETIRE",
        "PT_ROOTS_RETIRED",
        "[PT_ROOT_CUSTODY:",
    ] {
        if !body.contains(required) {
            return Err(());
        }
    }
    if body.matches("serial_println!").count() != 1
        || !function_body(procfs_trace, "generate_counters")
            .contains("emit_root_custody_summary();")
        || !function_body(x86_main, "kernel_main_on_kernel_stack")
            .contains("emit_root_custody_summary();")
        || !function_body(arm_main, "kernel_main").contains("emit_root_custody_summary();")
    {
        return Err(());
    }
    Ok(())
}

#[test]
fn production_boot_and_heartbeat_emit_root_custody_summary() {
    assert_eq!(
        validate_production_root_custody_summary(
            &repo_text("kernel/src/tracing/providers/teardown.rs"),
            &repo_text("kernel/src/fs/procfs/trace.rs"),
            &repo_text("kernel/src/main.rs"),
            &repo_text("kernel/src/main_aarch64.rs"),
        ),
        Ok(())
    );
}

#[test]
fn root_custody_summary_validator_rejects_a_boot_tests_only_emitter() {
    let provider = r#"
        #[cfg(feature = "boot_tests")]
        pub fn emit_root_custody_summary() {
            serial_println!("[PT_ROOT_CUSTODY:{}:{}:{}:{}:{}:{}]",
                PT_ROOT_ABANDONED_NO_PROOF, PT_ROOT_ABANDONED_NO_ARCH,
                PT_ROOT_ABANDONED_TERMINATED, PT_ROOT_DROPPED_UNDECIDED,
                PT_ROOT_DROPPED_MID_RETIRE, PT_ROOTS_RETIRED);
        }
    "#;
    let caller = "fn generate_counters() { emit_root_custody_summary(); }";
    let main = "fn kernel_main_on_kernel_stack() { emit_root_custody_summary(); }";
    assert!(validate_production_root_custody_summary(provider, caller, main, main).is_err());
}

fn validate_qemu_accelerator_and_cpu_knobs(source: &str) -> Result<(), &'static str> {
    let main = function_body(source, "main");
    for required in [
        "env::var(\"BREENIX_QEMU_ACCEL\")",
        "Ok(\"tcg\") => \"tcg\"",
        "Ok(\"kvm\") => \"kvm\"",
        "Ok(\"hvf\") => \"hvf\"",
        "Ok(\"whpx\") => \"whpx\"",
        "_ => \"tcg\"",
        "format!(\"pc,accel={}\", qemu_accel)",
        "machine.as_str()",
    ] {
        if !main.contains(required) {
            return Err("qemu accelerator knob lost its allowlist or TCG fallback");
        }
    }
    for required in [
        "env::var(\"BREENIX_QEMU_CPU\")",
        "Ok(\"qemu64\") => \"qemu64\"",
        "Ok(\"host\") => \"host\"",
        "Ok(\"max\") => \"max\"",
        "_ => \"qemu64\"",
        "\"-cpu\"",
        "qemu_cpu",
    ] {
        if !main.contains(required) {
            return Err("qemu CPU knob lost its allowlist or qemu64 fallback");
        }
    }
    for variable in ["BREENIX_QEMU_ACCEL", "BREENIX_QEMU_CPU"] {
        if main.contains(&format!("env::var(\"{variable}\").unwrap_or")) {
            return Err("qemu knob accepts an unvalidated environment value");
        }
    }
    Ok(())
}

#[test]
fn qemu_accelerator_is_opt_in_allowlisted_and_defaults_to_tcg() {
    let source = repo_text("src/bin/qemu-uefi.rs");
    assert_eq!(validate_qemu_accelerator_and_cpu_knobs(&source), Ok(()));
}

#[test]
fn qemu_accelerator_validator_rejects_unvalidated_environment_input() {
    let unvalidated_accelerator = r#"
        fn main() {
            let qemu_accel = env::var("BREENIX_QEMU_ACCEL")
                .unwrap_or_else(|_| "tcg".to_string());
            let qemu_cpu = match env::var("BREENIX_QEMU_CPU").as_deref() {
                Ok("qemu64") => "qemu64",
                Ok("host") => "host",
                Ok("max") => "max",
                _ => "qemu64",
            };
            let machine = format!("pc,accel={}", qemu_accel);
            qemu.args(["-machine", machine.as_str(), "-cpu", qemu_cpu]);
        }
    "#;
    let unvalidated_cpu = r#"
        fn main() {
            let qemu_accel = match env::var("BREENIX_QEMU_ACCEL").as_deref() {
                Ok("tcg") => "tcg",
                Ok("kvm") => "kvm",
                Ok("hvf") => "hvf",
                Ok("whpx") => "whpx",
                _ => "tcg",
            };
            let qemu_cpu = env::var("BREENIX_QEMU_CPU")
                .unwrap_or_else(|_| "qemu64".to_string());
            let machine = format!("pc,accel={}", qemu_accel);
            qemu.args(["-machine", machine.as_str(), "-cpu", qemu_cpu.as_str()]);
        }
    "#;
    assert!(validate_qemu_accelerator_and_cpu_knobs(unvalidated_accelerator).is_err());
    assert!(validate_qemu_accelerator_and_cpu_knobs(unvalidated_cpu).is_err());
}

fn validate_rust_fork_library_override(build_script: &str, xtask: &str) -> Result<(), ()> {
    const ENV: &str = "BREENIX_RUST_FORK_LIBRARY";
    let shell_assignment = format!(
        r#"RUST_FORK_LIBRARY="${{{ENV}:-$PROJECT_ROOT/rust-fork/library}}""#
    );
    let rust_env_lookup = "std::env::var_os(RUST_FORK_LIBRARY_ENV)";
    if !build_script.contains(ENV)
        || !build_script.contains(&shell_assignment)
        || !build_script.contains("__CARGO_TESTS_ONLY_SRC_ROOT=\"$RUST_FORK_LIBRARY\"")
        || !build_script.contains("forked Rust library not found at $RUST_FORK_LIBRARY")
        || !build_script.contains(&format!("Set {ENV} to the forked Rust library path"))
        || !xtask.contains(&format!("const RUST_FORK_LIBRARY_ENV: &str = \"{ENV}\";"))
        || !xtask.contains(&rust_env_lookup)
        || !xtask.contains(".map(PathBuf::from)")
        || !xtask.contains(".join(\"rust-fork/library\")")
        || !xtask.contains(".env(\"__CARGO_TESTS_ONLY_SRC_ROOT\", &rust_fork_library)")
        || !xtask.contains("Forked Rust library not found at {}. Set {}")
        || !xtask.contains("RUST_FORK_LIBRARY_ENV")
    {
        return Err(());
    }
    Ok(())
}

#[test]
fn rust_fork_library_paths_are_overrideable_in_both_builders() {
    assert_eq!(
        validate_rust_fork_library_override(
            &repo_text("userspace/programs/build.sh"),
            &repo_text("xtask/src/main.rs"),
        ),
        Ok(())
    );
}

#[test]
fn rust_fork_library_override_validator_rejects_hardcoded_paths() {
    let hardcoded_build_script = r#"
        RUST_FORK_LIBRARY="$PROJECT_ROOT/rust-fork/library"
        __CARGO_TESTS_ONLY_SRC_ROOT="$RUST_FORK_LIBRARY"
        echo "forked Rust library not found at $RUST_FORK_LIBRARY"
    "#;
    let hardcoded_xtask = r#"
        const RUST_FORK_LIBRARY_ENV: &str = "BREENIX_RUST_FORK_LIBRARY";
        let rust_fork_library = std::env::current_dir()
            .unwrap_or_default()
            .join("rust-fork/library");
        .env("__CARGO_TESTS_ONLY_SRC_ROOT", &rust_fork_library)
        bail!("Forked Rust library not found at {}. Set {}", path, RUST_FORK_LIBRARY_ENV);
    "#;
    assert!(validate_rust_fork_library_override(hardcoded_build_script, hardcoded_xtask).is_err());
}
