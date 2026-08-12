use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

type Site = (String, usize);

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

fn is_code(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.starts_with("//") && !trimmed.starts_with("/*") && !trimmed.starts_with('*')
}

fn sites_matching<F>(sources: &[(String, String)], mut predicate: F) -> BTreeSet<Site>
where
    F: FnMut(&str) -> bool,
{
    let mut sites = BTreeSet::new();
    for (path, source) in sources {
        for (index, line) in source.lines().enumerate() {
            if is_code(line) && predicate(line) {
                sites.insert((path.clone(), index + 1));
            }
        }
    }
    sites
}

fn expected(sites: &[(&str, usize)]) -> BTreeSet<Site> {
    sites
        .iter()
        .map(|(path, line)| ((*path).to_owned(), *line))
        .collect()
}

fn assert_exact(actual: BTreeSet<Site>, expected_sites: &[(&str, usize)], label: &str) {
    assert_eq!(actual, expected(expected_sites), "{label} changed");
}

fn validate_exact(actual: &BTreeSet<Site>, expected_sites: &[(&str, usize)]) -> Result<(), ()> {
    (*actual == expected(expected_sites))
        .then_some(())
        .ok_or(())
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

fn code_sites(sources: &[(String, String)], needle: &str) -> BTreeSet<Site> {
    let mut sites = BTreeSet::new();
    for (path, source) in sources {
        let mask = code_mask(source);
        for offset in code_offsets(source, &mask, needle) {
            let line = source.as_bytes()[..offset]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
                + 1;
            sites.insert((path.clone(), line));
        }
    }
    sites
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

const TERMINATE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/interrupts/context_switch.rs", 1017),
    ("kernel/src/process/manager.rs", 1172),
    ("kernel/src/signal/delivery.rs", 225),
    ("kernel/src/signal/delivery.rs", 260),
];
const TERMINATE_MINIMAL_CALLS: &[(&str, usize)] = &[("kernel/src/task/process_task.rs", 506)];
const PRODUCTION_INIT_PID_SITES: &[(&str, usize)] = &[
    ("kernel/src/process/manager.rs", 1189),
    ("kernel/src/task/process_task.rs", 466),
    ("kernel/src/task/process_task.rs", 538),
];
const TEST_INIT_PID_SITES: &[(&str, usize)] = &[
    ("kernel/src/test_userspace.rs", 84),
    ("kernel/src/test_userspace.rs", 203),
    ("kernel/src/test_userspace.rs", 292),
];
const QUARANTINE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/exception.rs", 815),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1177),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1271),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1369),
    ("kernel/src/syscall/signal.rs", 163),
];
const KERNEL_STACK_MUTATIONS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/syscall_entry.rs", 961),
    ("kernel/src/process/manager.rs", 1858),
    ("kernel/src/syscall/clone.rs", 252),
];
const RECLAIM_ENQUEUE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/process/mod.rs", 53),
    ("kernel/src/process/mod.rs", 280),
    ("kernel/src/task/process_task.rs", 580),
];
const EXIT_PROCESS_AND_RETIRE_CALLS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/exception.rs", 824),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1187),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1273),
    ("kernel/src/arch_impl/aarch64/exception.rs", 1371),
    ("kernel/src/interrupts.rs", 1440),
    ("kernel/src/interrupts.rs", 1751),
    ("kernel/src/process/mod.rs", 413),
    ("kernel/src/syscall/signal.rs", 172),
];
const EXIT_PROCESS_LOCKED_CALLS: &[(&str, usize)] = &[("kernel/src/process/mod.rs", 265)];
const EXIT_PROCESS_BY_PID_CALLS: &[(&str, usize)] = &[
    ("kernel/src/process/mod.rs", 406),
    ("kernel/src/process/mod.rs", 418),
];
const EXIT_PROCESS_FOR_TEARDOWN_TEST_CALLS: &[(&str, usize)] =
    &[("kernel/src/tracing/providers/teardown.rs", 1095)];
const BLOCKING_PRIMITIVES: &[(&str, usize)] = &[
    ("kernel/src/task/scheduler.rs", 1973),
    ("kernel/src/task/scheduler.rs", 2187),
    ("kernel/src/task/scheduler.rs", 2206),
    ("kernel/src/task/scheduler.rs", 2355),
    ("kernel/src/task/scheduler.rs", 2443),
    ("kernel/src/task/scheduler.rs", 2508),
    ("kernel/src/task/scheduler.rs", 2517),
    ("kernel/src/task/scheduler.rs", 2676),
    ("kernel/src/task/waitqueue.rs", 52),
];
const RAW_SCHEDULER_LOCK_SITES: &[(&str, usize)] = &[
    ("kernel/src/task/scheduler.rs", 281),
    ("kernel/src/task/scheduler.rs", 288),
];
const PROCESS_MEMORY_FRAME_RETURNS: &[(&str, usize)] = &[
    ("kernel/src/memory/process_memory.rs", 1617),
    ("kernel/src/memory/process_memory.rs", 1656),
    ("kernel/src/memory/process_memory.rs", 1693),
    ("kernel/src/memory/process_memory.rs", 1705),
    ("kernel/src/memory/process_memory.rs", 1709),
    ("kernel/src/memory/process_memory.rs", 1713),
    ("kernel/src/memory/process_memory.rs", 1718),
    ("kernel/src/memory/process_memory.rs", 1785),
    ("kernel/src/memory/process_memory.rs", 1813),
    ("kernel/src/memory/process_memory.rs", 1840),
    ("kernel/src/memory/process_memory.rs", 1852),
    ("kernel/src/memory/process_memory.rs", 1856),
    ("kernel/src/memory/process_memory.rs", 1860),
    ("kernel/src/memory/process_memory.rs", 1865),
];
const TABLE_RECORDER_SITES: &[(&str, usize)] = &[
    ("kernel/src/memory/process_memory.rs", 1117),
    ("kernel/src/memory/process_memory.rs", 1228),
];
const PROCESS_PAGE_TABLE_ABANDON_SITES: &[(&str, usize, &str)] = &[
    (
        "kernel/src/task/process_task.rs",
        188,
        "AbandonReason::NoProofPipeline",
    ),
    (
        "kernel/src/task/process_task.rs",
        296,
        "AbandonReason::NoProofPipeline",
    ),
    (
        "kernel/src/task/process_task.rs",
        298,
        "AbandonReason::NoArchPipeline",
    ),
    (
        "kernel/src/task/process_task.rs",
        480,
        "AbandonReason::AlreadyTerminated",
    ),
    (
        "kernel/src/process/manager.rs",
        1151,
        "AbandonReason::AlreadyTerminated",
    ),
];
const FRAME_LEDGER_INIT_CALLS: &[(&str, usize)] = &[
    ("kernel/src/main_aarch64.rs", 493),
    ("kernel/src/memory/mod.rs", 137),
];
const PROCESS_PAGE_TABLE_CONSTRUCTORS: &[(&str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/syscall_entry.rs", 937),
    ("kernel/src/process/manager.rs", 144),
    ("kernel/src/process/manager.rs", 399),
    ("kernel/src/process/manager.rs", 623),
    ("kernel/src/process/manager.rs", 2242),
    ("kernel/src/process/manager.rs", 2508),
    ("kernel/src/process/manager.rs", 2836),
    ("kernel/src/process/manager.rs", 3112),
    ("kernel/src/process/manager.rs", 3393),
    ("kernel/src/syscall/handlers.rs", 1822),
    ("kernel/src/tracing/providers/teardown.rs", 991),
    ("kernel/src/tracing/providers/teardown.rs", 1051),
    ("kernel/src/tracing/providers/teardown.rs", 1112),
    ("kernel/src/memory/process_memory.rs", 1914),
    ("kernel/src/memory/process_memory.rs", 1931),
];

const BLOCKING_NAMES: &[&str] = &[
    "block_current(",
    "block_current_for_signal(",
    "block_current_for_signal_with_context(",
    "block_current_for_child_exit(",
    "block_current_for_timer(",
    "block_current_for_io(",
    "block_current_for_io_with_timeout(",
    "block_current_for_compositor(",
    "prepare_to_wait(",
];

fn validate_reclaim_enqueue_callers(sources: &[(String, String)]) -> Result<(), ()> {
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("enqueue_process_reclaim(")
                && !line.contains("fn enqueue_process_reclaim")
        }),
        RECLAIM_ENQUEUE_CALLS,
    )
}

fn validate_exit_process_entry_points(sources: &[(String, String)]) -> Result<(), ()> {
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("exit_process_and_retire(")
                && !line.contains("fn exit_process_and_retire")
        }),
        EXIT_PROCESS_AND_RETIRE_CALLS,
    )?;
    validate_exact(
        &sites_matching(sources, |line| line.contains(".exit_process_locked(")),
        EXIT_PROCESS_LOCKED_CALLS,
    )?;
    if !sites_matching(sources, |line| line.contains(".exit_process(")).is_empty() {
        return Err(());
    }
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("exit_process_by_pid(") && !line.contains("fn exit_process_by_pid")
        }),
        EXIT_PROCESS_BY_PID_CALLS,
    )?;
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("exit_process_for_teardown_test(")
                && !line.contains("fn exit_process_for_teardown_test")
        }),
        EXIT_PROCESS_FOR_TEARDOWN_TEST_CALLS,
    )
}

fn validate_blocking_primitives(sources: &[(String, String)]) -> Result<(), ()> {
    validate_exact(
        &sites_matching(sources, |line| {
            line.contains("pub fn ") && BLOCKING_NAMES.iter().any(|name| line.contains(name))
        }),
        BLOCKING_PRIMITIVES,
    )
}

fn validate_group_writes(sources: &[(String, String)]) -> Result<(), ()> {
    validate_exact(
        &sites_matching(sources, |line| line.contains("thread_group_id = Some(")),
        &[("kernel/src/syscall/clone.rs", 210)],
    )
}

fn validate_exit_sgi_is_teardown_only(sources: &[(String, String)]) -> Result<(), ()> {
    let scheduler = source(sources, "kernel/src/task/scheduler.rs");
    (function_body(scheduler, "send_exit_expedite_sgi").contains("EXIT_SGI_SENT")
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

fn validate_frame_return_choke_point(sources: &[(String, String)]) -> Result<(), ()> {
    let allocator = source(sources, "kernel/src/memory/frame_allocator.rs");
    let init = function_body(allocator, "init_frame_ledger");
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
    )?;
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
    )?;
    let init_methods = alias_method_calls(init);
    if init_methods.iter().filter(|method| *method == "push").count() != 1
        || !init.contains(
            "if seed_free_frame(&ledger, frame) {\n                free_list.push(frame);\n            }",
        )
    {
        eprintln!("bootstrap free-list insertion escaped its seeded-frame span");
        return Err(());
    }
    validate_alias_methods(
        function_body(allocator, "ensure_free_frame_capacity"),
        &["try_lock", "capacity", "len", "try_reserve", "is_err"],
        &[],
    )?;
    validate_alias_methods(
        function_body(allocator, "allocate_candidate"),
        &["try_lock", "pop", "len"],
        &[],
    )?;
    validate_alias_methods(
        function_body(allocator, "return_lease"),
        &["try_lock", "len", "capacity", "push"],
        &[],
    )?;
    validate_alias_methods(
        function_body(allocator, "memory_stats"),
        &["try_lock", "len"],
        &[],
    )?;

    let fixture = source(sources, "kernel/src/memory/frame_allocator_tests.rs");
    validate_free_frame_capabilities(
        "kernel/src/memory/frame_allocator_tests.rs",
        fixture,
        &[
            "inject_duplicate_candidates",
            "remove_duplicate_candidates",
            "republish_lost_frame",
            "free_frame_count",
            "free_list_len_for_gate",
            "take_free_frame",
            "frame_custody_refusal_gate_test",
        ],
    )?;

    for (path, module) in sources.iter().filter(|(path, _)| {
        path != "kernel/src/memory/frame_allocator.rs"
            && path != "kernel/src/memory/frame_allocator_tests.rs"
    }) {
        let mask = code_mask(module);
        if !identifier_offsets(module, &mask, "FREE_FRAMES").is_empty() {
            eprintln!("unexpected FREE_FRAMES capability in {path}");
            return Err(());
        }
    }

    validate_exact(
        &sites_matching(sources, |line| {
            (line.contains("deallocate_frame(") || line.contains("return_lease("))
                && !line.contains("fn deallocate_frame")
                && !line.contains("fn return_lease")
        })
        .into_iter()
        .filter(|(path, _)| path == "kernel/src/memory/process_memory.rs")
        .collect(),
        PROCESS_MEMORY_FRAME_RETURNS,
    )
}

fn validate_frame_ledger_hot_paths(sources: &[(String, String)]) -> Result<(), ()> {
    let allocator = source(sources, "kernel/src/memory/frame_allocator.rs");
    const ROOTS: [&str; 5] = [
        "frame_ordinal",
        "get",
        "claim_frame",
        "counted",
        "return_lease",
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
        && init.contains("NEXT_FREE_FRAME.load(Ordering::Acquire),\n        frontier_snapshot")
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

fn validate_frame_ledger_init(sources: &[(String, String)]) -> Result<(), ()> {
    validate_exact(
        &code_sites(sources, "init_frame_ledger();"),
        FRAME_LEDGER_INIT_CALLS,
    )?;
    validate_exact(
        &code_sites(sources, "ProcessPageTable::new("),
        PROCESS_PAGE_TABLE_CONSTRUCTORS,
    )?;
    validate_frame_ledger_boot_order(sources)
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
        && claim.contains("return Err(ClaimError::Duplicate);")
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
        && script.contains("FRAME_CUSTODY_COUNTERS:x86:double=1:stale=1:never=1:untracked=1:duplicate=3:contended=[1-9][0-9]*")
        && script.contains("page_table_custody_disposition_gate:PASS")
        && script.contains("recorded=2:no_proof=0:no_arch=0:terminated=1:undecided=1:exec_unreturned=0")
        && script.contains("-eq 1")
        && script.contains("x86 frame-custody gate run")
        && script.contains("BOOT_TESTS:FAIL|KERNEL PANIC|panic!"))
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
    const EXPECTED: [&str; 6] = [
        "FRAME_RETURN_REFUSED_DOUBLE",
        "FRAME_RETURN_REFUSED_STALE",
        "FRAME_RETURN_REFUSED_NEVER_ALLOCATED",
        "FRAME_RETURN_REFUSED_UNTRACKED",
        "FRAME_DUPLICATE_ALLOC_REFUSED",
        "FRAME_LOST_CONTENDED",
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
        && provider.contains("pub const COUNTER_COUNT: usize = 59;"))
    .then_some(())
    .ok_or(())
}

fn validate_process_table_recorder(sources: &[(String, String)]) -> Result<(), ()> {
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
            return Err(());
        }
    }

    validate_exact(
        &code_sites(sources, "TableRecorder(tables)")
            .into_iter()
            .filter(|(path, _)| path == "kernel/src/memory/process_memory.rs")
            .collect(),
        TABLE_RECORDER_SITES,
    )?;

    let recorder = function_body(process_memory, "allocate_frame");
    (recorder.contains("let lease = allocate_frame_leased()?")
        && recorder.contains("self.0.record(lease);")
        && recorder.contains("Some(frame)")
        && !recorder.contains("deallocate_frame")
        && !recorder.contains("return_lease"))
        .then_some(())
        .ok_or(())
}

fn validate_process_page_table_drop_is_non_freeing(
    sources: &[(String, String)],
) -> Result<(), ()> {
    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let drop_body = function_body(process_memory, "drop");
    (drop_body.contains("Disposition::Undecided")
        && drop_body.contains("trace_count!")
        && drop_body.contains("PT_ROOT_DROPPED_UNDECIDED")
        && ["deallocate_frame", "return_lease", "retire_bounded"]
            .iter()
            .all(|forbidden| !drop_body.contains(forbidden)))
        .then_some(())
        .ok_or(())
}

fn validate_process_page_table_dispositions(sources: &[(String, String)]) -> Result<(), ()> {
    let adapted_paths = [
        "kernel/src/task/process_task.rs",
        "kernel/src/process/manager.rs",
    ];
    let actual = sites_matching(sources, |line| line.contains(".abandon(AbandonReason::"))
        .into_iter()
        .filter(|(path, _)| adapted_paths.contains(&path.as_str()))
        .collect();
    let expected_sites = PROCESS_PAGE_TABLE_ABANDON_SITES
        .iter()
        .map(|(path, line, _)| (*path, *line))
        .collect::<Vec<_>>();
    validate_exact(&actual, &expected_sites)?;
    for (path, line, reason) in PROCESS_PAGE_TABLE_ABANDON_SITES {
        let source_line = source(sources, path).lines().nth(line - 1).ok_or(())?;
        if !source_line.contains(reason) {
            eprintln!("R5 abandon reason changed at {path}:{line}");
            return Err(());
        }
    }

    for path in adapted_paths {
        let module = source(sources, path);
        if !call_sites_with_argument(module, "drop", "page_table.take()").is_empty() {
            eprintln!("R5 raw page-table drop restored in {path}");
            return Err(());
        }
    }

    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let bodies = module_function_bodies(process_memory);
    let cleanup_bodies = bodies.get("cleanup_for_exec").ok_or(())?;
    (cleanup_bodies.len() == 2
        && cleanup_bodies.iter().all(|body| {
            body.matches("Disposition::RetiredByExecWalk").count() == 1
                && body.matches("PT_EXEC_WALK_LEASES_UNRETURNED").count() == 1
        }))
    .then_some(())
    .ok_or(())
}

fn validate_process_page_table_counter_inventory(
    sources: &[(String, String)],
) -> Result<(), ()> {
    const EXPECTED: [&str; 6] = [
        "PT_TABLE_FRAMES_RECORDED",
        "PT_ROOT_ABANDONED_NO_PROOF",
        "PT_ROOT_ABANDONED_NO_ARCH",
        "PT_ROOT_ABANDONED_TERMINATED",
        "PT_ROOT_DROPPED_UNDECIDED",
        "PT_EXEC_WALK_LEASES_UNRETURNED",
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
        || !provider.contains("pub const COUNTER_COUNT: usize = 59;")
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
    let drop_body = function_body(process_memory, "drop");
    let cleanup_bodies = module_function_bodies(process_memory)
        .remove("cleanup_for_exec")
        .ok_or(())?;
    (record.contains("PT_TABLE_FRAMES_RECORDED")
        && abandon.contains("PT_ROOT_ABANDONED_NO_PROOF")
        && abandon.contains("PT_ROOT_ABANDONED_NO_ARCH")
        && abandon.contains("PT_ROOT_ABANDONED_TERMINATED")
        && drop_body.contains("PT_ROOT_DROPPED_UNDECIDED")
        && cleanup_bodies.len() == 2
        && cleanup_bodies
            .iter()
            .all(|body| body.contains("PT_EXEC_WALK_LEASES_UNRETURNED")))
    .then_some(())
    .ok_or(())
}

fn validate_process_page_table_exit_paths_are_minimal(
    sources: &[(String, String)],
) -> Result<(), ()> {
    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    for body in [
        function_body(process_memory, "abandon"),
        function_body(process_memory, "drop"),
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

fn validate_process_page_table_runtime_oracle(sources: &[(String, String)]) -> Result<(), ()> {
    let process_memory = source(sources, "kernel/src/memory/process_memory.rs");
    let gate = function_body(process_memory, "page_table_custody_disposition_gate_test");
    if !gate.contains("terminated.abandon(AbandonReason::AlreadyTerminated);")
        || !gate.contains("drop(undecided);")
        || !gate.contains("after_abandon[3] != start[3] + 1")
        || !gate.contains("after_drop[4] != after_abandon[4] + 1")
        || gate.matches("free_list_len_for_gate()").count() != 4
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
    if registrations.len() != 1
        || registry.contains("page_table_custody_disposition_gate_test as")
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
    let harness = repo_text("docker/qemu/run-x86-boot-tests.sh");
    (harness.contains("page_table_custody_disposition_gate:PASS")
        && harness.contains("recorded=2:no_proof=0:no_arch=0:terminated=1:undecided=1:exec_unreturned=0")
        && harness.matches("page_table_custody_disposition_gate:PASS").count() == 2)
        .then_some(())
        .ok_or(())
}

#[test]
fn process_page_table_custody_ratchets_are_exact() {
    let sources = rust_sources_below("kernel/src");
    validate_process_table_recorder(&sources).expect("R2 process mapper recorder was bypassed");
    validate_process_page_table_drop_is_non_freeing(&sources)
        .expect("R4 ProcessPageTable Drop gained a freeing path");
    validate_process_page_table_dispositions(&sources)
        .expect("R5 process page-table disposition set changed");
    validate_process_page_table_counter_inventory(&sources)
        .expect("R6 process page-table counter inventory changed");
    validate_process_page_table_exit_paths_are_minimal(&sources)
        .expect("R7 process page-table exit path gained log/format/heap work");
    validate_process_page_table_runtime_oracle(&sources)
        .expect("O2/G-H process page-table runtime oracle became vacuous");
}

#[test]
fn frame_ledger_return_and_initialization_ratchets_are_exact() {
    let sources = rust_sources_below("kernel/src");
    validate_frame_ledger_hot_paths(&sources)
        .expect("R7 frame-ledger hot path gained log/format/heap work");
    validate_frame_ledger_bounded_boot_allocation(&sources)
        .expect("frame ledger regained eager or post-publication allocation");
    validate_frame_return_choke_point(&sources).expect("R1 frame-return choke point changed");
    validate_frame_ledger_boot_order(&sources).expect("R9 ARM/x86 frame-ledger boot order changed");
    validate_frame_ledger_init(&sources).expect("R9 frame-ledger initialization moved");
    validate_frame_ledger_runtime_oracles(&sources)
        .expect("frame-custody runtime oracle became vacuous");
    validate_no_vacuous_test_conditions(&sources)
        .expect("boot-test oracle gained an always-true/always-false vacuity shape");
    validate_x86_frame_custody_harness(&repo_text("docker/qemu/run-x86-boot-tests.sh"))
        .expect("x86 frame-custody harness became vacuous");
    validate_workqueue_progress_wait(source(&sources, "kernel/src/test_framework/registry.rs"))
        .expect("workqueue probe lost its progress key or regained a timing-based wait");

    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    validate_frame_ledger_counter_inventory(provider)
        .expect("frame-ledger counter inventory changed");
    for counter in [
        "FRAME_RETURN_REFUSED_DOUBLE",
        "FRAME_RETURN_REFUSED_STALE",
        "FRAME_RETURN_REFUSED_NEVER_ALLOCATED",
        "FRAME_RETURN_REFUSED_UNTRACKED",
        "FRAME_DUPLICATE_ALLOC_REFUSED",
        "FRAME_LOST_CONTENDED",
    ] {
        assert_eq!(
            provider.matches(&format!("counter!({counter},")).count()
                + provider
                    .matches(&format!("counter!(\n    {counter},"))
                    .count(),
            1,
            "counter declaration changed: {counter}"
        );
        let declaration = provider
            .find(counter)
            .unwrap_or_else(|| panic!("missing counter {counter}"));
        let prefix = &provider[declaration.saturating_sub(80)..declaration];
        assert!(
            !prefix.contains("#[cfg("),
            "counter became conditional: {counter}"
        );
    }
    assert!(provider.contains("pub const COUNTER_COUNT: usize = 59;"));
}

#[test]
fn current_teardown_bypass_surface_is_exact() {
    let sources = rust_sources_below("kernel/src");

    assert_exact(
        sites_matching(&sources, |line| line.contains(".terminate(")),
        TERMINATE_CALLS,
        "Process::terminate callers",
    );
    assert_exact(
        sites_matching(&sources, |line| line.contains(".terminate_minimal(")),
        TERMINATE_MINIMAL_CALLS,
        "Process::terminate_minimal callers",
    );

    let init_sites = sites_matching(&sources, |line| line.contains("ProcessId::new(1)"));
    let test_sites: BTreeSet<_> = init_sites
        .iter()
        .filter(|(path, _)| path == "kernel/src/test_userspace.rs")
        .cloned()
        .collect();
    let production_sites: BTreeSet<_> = init_sites.difference(&test_sites).cloned().collect();
    assert_exact(
        production_sites,
        PRODUCTION_INIT_PID_SITES,
        "production PID-1 literals",
    );
    assert_exact(
        test_sites,
        TEST_INIT_PID_SITES,
        "test_minimal_userspace PID-1 allowlist",
    );
    let test_userspace = source(&sources, "kernel/src/test_userspace.rs");
    assert_eq!(
        test_userspace
            .matches("pub fn test_minimal_userspace()")
            .count(),
        1,
        "test_minimal_userspace must remain uniquely nameable"
    );
    assert_eq!(
        function_body(test_userspace, "test_minimal_userspace")
            .matches("ProcessId::new(1)")
            .count(),
        3,
        "the three test PID-1 sites must remain in test_minimal_userspace"
    );

    assert_exact(
        sites_matching(&sources, |line| {
            line.contains(".terminate_process_threads(")
        }),
        QUARANTINE_CALLS,
        "terminate_process_threads callers",
    );
    assert_exact(
        sites_matching(&sources, |line| line.contains(".kernel_stack_allocation =")),
        KERNEL_STACK_MUTATIONS,
        "kernel_stack_allocation ownership mutations",
    );
    validate_reclaim_enqueue_callers(&sources)
        .expect("enqueue_process_reclaim caller ratchet changed");
}

#[test]
fn v3_structural_closures_are_exact() {
    let sources = rust_sources_below("kernel/src");
    validate_exit_process_entry_points(&sources).expect("process-exit entry-point ratchet changed");
    validate_blocking_primitives(&sources).expect("the nine P0 blocking primitives changed");
    validate_group_writes(&sources).expect("thread_group_id production writers changed");
    assert_exact(
        sites_matching(&sources, |line| {
            line.contains("SCHEDULER.lock()") || line.contains("SCHEDULER.try_lock()")
        }),
        RAW_SCHEDULER_LOCK_SITES,
        "raw scheduler-lock acquisitions outside the instrumented wrappers",
    );
    assert_exact(
        sites_matching(&sources, |line| line.contains("btrt::on_process_exit(")),
        &[("kernel/src/task/process_task.rs", 607)],
        "btrt::on_process_exit callers",
    );

    let provider = source(&sources, "kernel/src/tracing/providers/teardown.rs");
    let scheduler = source(&sources, "kernel/src/task/scheduler.rs");
    assert_eq!(provider.matches("counter!(EXIT_SGI_SENT,").count(), 1);
    assert_eq!(provider.matches("counter!(EXIT_KICK_PUBLISHED,").count(), 1);
    validate_exit_sgi_is_teardown_only(&sources)
        .expect("EXIT_SGI_SENT escaped the teardown-only producer");
    let expedite = function_body(scheduler, "send_exit_expedite_sgi");
    assert_eq!(expedite.matches("EXIT_SGI_SENT").count(), 1);
    assert_eq!(
        expedite
            .matches("trace_count!(EXIT_KICK_PUBLISHED)")
            .count(),
        1
    );
    assert!(expedite.find("slot.publish(").unwrap() < expedite.find("gic::send_sgi(").unwrap());
    assert!(!expedite.contains("current_thread"));
    assert_eq!(scheduler.matches("send_exit_expedite_sgi(").count(), 1);
    assert!(provider.contains("struct KickSlot"));
    assert!(provider.contains("pub(crate) pid: AtomicU64"));
    assert!(provider.contains("pub(crate) at: AtomicU64"));
    assert!(provider.contains("pub(crate) state: AtomicU64"));
    assert!(!provider.contains("trace_count!(EXIT_SGI_SENT"));
    assert!(!provider.contains("trace_count!(EXIT_KICK_PUBLISHED"));

    let process_mod = source(&sources, "kernel/src/process/mod.rs");
    assert!(process_mod.contains("pub(crate) struct RetirementReceipt"));
    assert!(!process_mod.contains("pub struct RetirementReceipt"));
    assert!(!process_mod.contains("pub fn from_reclaim"));
    assert!(function_body(process_mod, "drop").contains("enqueue_process_reclaim("));

    let process = source(&sources, "kernel/src/process/process.rs");
    for state in ["Absent", "Pending", "Claimed", "Completed"] {
        assert!(process.contains(state));
    }
    assert!(!process.contains("report_marker"));
    assert!(!process.contains("claim_exit_slot"));
    assert!(!process.contains("record_exit"));
}

#[test]
fn phase_one_retirement_fence_and_lock_domains_are_structural() {
    let sources = rust_sources_below("kernel/src");
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
    assert!(lock_free.contains("local_ttbr0_root()"));
    assert!(lock_free.contains("is_ttbr0_root_live_in_mask"));
    assert!(!lock_free.contains("with_scheduler"));
    assert!(!lock_free.contains("process::manager"));

    let park = function_body(process, "park_reclaim");
    assert!(park.contains("let snapshot_at_park = scheduler::RetirementSnapshot::capture();"));
    assert!(park.contains("let fence_at_park = snapshot_at_park.as_fence();"));
    assert!(!park.contains("reclaim.after_epoch"));
    let unpark = function_body(process, "unpark_sweep_with_snapshot");
    assert!(
        unpark.find("PARKED_PROCESS_RECLAIMS.lock()").unwrap()
            < unpark.find("PENDING_PROCESS_RECLAIMS.lock()").unwrap()
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

    assert_exact(
        sites_matching(&sources, |line| {
            line.contains("note_process_row_removed()")
                && !line.contains("fn note_process_row_removed")
        }),
        &[("kernel/src/process/manager.rs", 1104)],
        "ROW_REMOVAL_EPOCH bump sites",
    );
    assert!(function_body(manager, "remove_process").contains("self.processes.remove(&pid)"));
    assert!(ttbr0.contains("core::arch::asm!(\"mrs {}, ttbr0_el1\""));

    for counter in [
        "ROOT_PROOF_BLOCKED_EPOCH",
        "ROOT_PROOF_BLOCKED_HW",
        "ROOT_PROOF_BLOCKED_SHADOW",
        "ROOT_PROOF_BLOCKED_CACHED",
        "ROOT_PROOF_BLOCKED_LIVE_ROW",
        "RETIRE_EMPTY_ONLINE_MASK",
    ] {
        assert!(provider.contains(counter));
    }
    let declaration_only = provider
        .split("// Declaration-only until the phase named in PLAN.md.")
        .nth(1)
        .expect("declaration-only counter boundary")
        .split("pub const COUNTER_COUNT")
        .next()
        .expect("declaration-only counter terminator");
    assert!(!declaration_only.contains("RECLAIM_PASS_SKIPPED"));
    assert!(!declaration_only.contains("RETIRE_EMPTY_ONLINE_MASK"));

    let registry = source(&sources, "kernel/src/test_framework/registry.rs");
    assert!(registry.contains("name: \"retirement_fence_gate\""));
    assert!(registry.contains("name: \"reclaim_progress_gate\""));
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
    assert_eq!(declarations.len(), 59);
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
    let bypassed_recorder = process_memory.replacen(
        "&mut TableRecorder(tables)",
        "&mut GlobalFrameAllocator",
        1,
    );
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
        "fn drop(&mut self) {",
        "fn drop(&mut self) { crate::memory::frame_allocator::deallocate_frame(self.level_4_frame);",
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
            format!("{process_task}\nfn synthetic_raw_drop(process: &mut Process) {{ {raw_drop} }}"),
        );
        assert!(validate_process_page_table_dispositions(&raw_drop).is_err());
    }
    let process_task = source(&sources, "kernel/src/task/process_task.rs");
    let wrong_reason = process_task.replacen(
        "page_table.abandon(AbandonReason::AlreadyTerminated);",
        "page_table.abandon(AbandonReason::NoProofPipeline);",
        1,
    );
    let wrong_reason = with_replaced_source(
        &sources,
        "kernel/src/task/process_task.rs",
        wrong_reason,
    );
    assert!(validate_process_page_table_dispositions(&wrong_reason).is_err());
    let missing_exec_disposition = process_memory.replacen(
        "self.tables.disposition = Disposition::RetiredByExecWalk;",
        "self.tables.disposition = Disposition::Undecided;",
        1,
    );
    let missing_exec_disposition = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        missing_exec_disposition,
    );
    assert!(validate_process_page_table_dispositions(&missing_exec_disposition).is_err());

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
        "fn drop(&mut self) {",
        "fn drop(&mut self) { let _unexpected = Vec::<u8>::new();",
        1,
    );
    let allocating_drop = with_replaced_source(
        &sources,
        "kernel/src/memory/process_memory.rs",
        allocating_drop,
    );
    assert!(validate_process_page_table_exit_paths_are_minimal(&allocating_drop).is_err());

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
        let weakened = with_replaced_source(
            &sources,
            "kernel/src/memory/process_memory.rs",
            weakened,
        );
        assert!(validate_process_page_table_runtime_oracle(&weakened).is_err());
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
        "fn return_lease(lease: FrameLease) -> ReturnOutcome {",
        "fn return_lease(lease: FrameLease) -> ReturnOutcome { log::info!(\"return\");",
        1,
    );
    let logged_return = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        logged_return,
    );
    assert!(validate_frame_ledger_hot_paths(&logged_return).is_err());
    let growing_return = allocator.replacen(
        "fn return_lease(lease: FrameLease) -> ReturnOutcome {",
        "fn return_lease(lease: FrameLease) -> ReturnOutcome { let _ = FREE_FRAMES.lock().try_reserve(1);",
        1,
    );
    let growing_return = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        growing_return,
    );
    assert!(validate_frame_ledger_hot_paths(&growing_return).is_err());
    let logged_get = allocator.replacen(
        "fn get(&self, index: usize) -> Option<&AtomicU32> {",
        "fn get(&self, index: usize) -> Option<&AtomicU32> { log::warn!(\"ledger get {}\", index);",
        1,
    );
    let logged_get =
        with_replaced_source(&sources, "kernel/src/memory/frame_allocator.rs", logged_get);
    assert!(validate_frame_ledger_hot_paths(&logged_get).is_err());
    let transitive_helper_log = allocator.replacen(
        "fn return_lease(lease: FrameLease) -> ReturnOutcome {",
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
    let escaped_duplicate =
        allocator.replacen("return Err(ClaimError::Duplicate);", "return Ok(None);", 1);
    let escaped_duplicate = with_replaced_source(
        &sources,
        "kernel/src/memory/frame_allocator.rs",
        escaped_duplicate,
    );
    assert!(validate_frame_ledger_runtime_oracles(&escaped_duplicate).is_err());
    let broken_bounds = allocator.replacen(
        "\n    None\n}\n\nfn seed_free_frame",
        "\n    Some(0)\n}\n\nfn seed_free_frame",
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
