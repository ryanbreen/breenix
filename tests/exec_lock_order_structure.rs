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

fn block_statements(block: &str) -> Option<&str> {
    let mask = code_mask(block);
    let open = (0..block.len()).find(|index| mask[*index] && block.as_bytes()[*index] == b'{')?;
    let close = block.len().checked_sub(1)?;
    (block.as_bytes()[close] == b'}' && open < close).then(|| &block[open + 1..close])
}

fn compact_code(fragment: &str) -> String {
    fragment
        .bytes()
        .zip(code_mask(fragment))
        .filter_map(|(byte, code)| (code && !byte.is_ascii_whitespace()).then_some(byte as char))
        .collect()
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

/// The only `scheduler::` items the process manager may name. `ExecSchedCommit` is a receipt type:
/// constructing one takes no lock, and `apply()` — the sole SCHEDULER acquisition — lives in
/// scheduler.rs and runs with the process-manager lock released.
const EXEC_SCHED_COMMIT_ALLOWLIST: [&str; 1] = ["ExecSchedCommit"];

fn next_code_non_whitespace(source: &str, mask: &[bool], mut cursor: usize) -> Option<usize> {
    while cursor < source.len() {
        if mask[cursor] && !source.as_bytes()[cursor].is_ascii_whitespace() {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn previous_code_non_whitespace(source: &str, mask: &[bool], mut cursor: usize) -> Option<usize> {
    while cursor != 0 {
        cursor -= 1;
        if mask[cursor] && !source.as_bytes()[cursor].is_ascii_whitespace() {
            return Some(cursor);
        }
    }
    None
}

fn scheduler_group_item(
    source: &str,
    mask: &[bool],
    start: usize,
    end: usize,
) -> Option<(String, usize)> {
    let item_start = next_code_non_whitespace(source, mask, start)?;
    if item_start >= end {
        return None;
    }
    if source.as_bytes()[item_start] == b'*' {
        return Some(("*".to_owned(), item_start));
    }
    let mut item_end = item_start;
    while item_end < end && mask[item_end] && identifier_byte(source.as_bytes()[item_end]) {
        item_end += 1;
    }
    (item_end != item_start).then(|| (source[item_start..item_end].to_owned(), item_start))
}

/// Census every item reached through a `scheduler::` path, including braced
/// imports and glob imports. Offsets are relative to `source`.
fn scheduler_path_items(source: &str) -> Vec<(String, usize)> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut items = Vec::new();

    for scheduler_offset in identifier_offsets(source, &mask, "scheduler") {
        let Some(first_colon) =
            next_code_non_whitespace(source, &mask, scheduler_offset + "scheduler".len())
        else {
            continue;
        };
        if bytes.get(first_colon) != Some(&b':') || bytes.get(first_colon + 1) != Some(&b':') {
            continue;
        }
        let Some(item_start) = next_code_non_whitespace(source, &mask, first_colon + 2) else {
            continue;
        };

        if bytes[item_start] == b'{' {
            let mut depth = 1usize;
            let mut entry_start = item_start + 1;
            let mut cursor = item_start + 1;
            while cursor < bytes.len() {
                if !mask[cursor] {
                    cursor += 1;
                    continue;
                }
                match bytes[cursor] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            if let Some(item) =
                                scheduler_group_item(source, &mask, entry_start, cursor)
                            {
                                items.push(item);
                            }
                            break;
                        }
                    }
                    b',' if depth == 1 => {
                        if let Some(item) = scheduler_group_item(source, &mask, entry_start, cursor)
                        {
                            items.push(item);
                        }
                        entry_start = cursor + 1;
                    }
                    _ => {}
                }
                cursor += 1;
            }
        } else if bytes[item_start] == b'*' {
            items.push(("*".to_owned(), item_start));
        } else {
            let mut item_end = item_start;
            while item_end < bytes.len() && mask[item_end] && identifier_byte(bytes[item_end]) {
                item_end += 1;
            }
            if item_end != item_start {
                items.push((source[item_start..item_end].to_owned(), item_start));
            }
        }
    }

    items
}

fn validate_scheduler_paths(file: &str, source: &str, source_offset: usize) -> Result<(), String> {
    for (item, offset) in scheduler_path_items(source) {
        if !EXEC_SCHED_COMMIT_ALLOWLIST.contains(&item.as_str()) {
            return Err(format!(
                "{file} names disallowed scheduler item `{item}` at byte {}",
                source_offset + offset
            ));
        }
    }
    Ok(())
}

struct ModuleFunction<'a> {
    body: &'a str,
    is_public: bool,
}

/// Module-level function definitions only. Inherent methods, trait methods,
/// nested functions, and inline submodule functions all have nonzero brace
/// depth at their `fn` token and are deliberately excluded.
fn module_level_functions(source: &str) -> BTreeMap<String, Vec<ModuleFunction<'_>>> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut depth_at = vec![0usize; bytes.len()];
    let mut depth = 0usize;
    for index in 0..bytes.len() {
        depth_at[index] = depth;
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }

    let mut functions: BTreeMap<String, Vec<ModuleFunction<'_>>> = BTreeMap::new();
    for fn_offset in identifier_offsets(source, &mask, "fn") {
        if depth_at[fn_offset] != 0 {
            continue;
        }

        let mut cursor = fn_offset + "fn".len();
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

        let brace = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{');
        let semicolon = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b';');
        let Some(brace) = brace else { continue };
        if semicolon.is_some_and(|semicolon| semicolon < brace) {
            continue;
        }
        let Some(body) = braced_block(source, &mask, brace) else {
            continue;
        };

        let mut item_start = 0usize;
        let mut item_depth = 0usize;
        for index in 0..fn_offset {
            if !mask[index] {
                continue;
            }
            match bytes[index] {
                b'{' => item_depth += 1,
                b'}' => {
                    item_depth = item_depth.saturating_sub(1);
                    if item_depth == 0 {
                        item_start = index + 1;
                    }
                }
                b';' if item_depth == 0 => item_start = index + 1,
                _ => {}
            }
        }
        let visibility = &source[item_start..fn_offset];
        let visibility_mask = code_mask(visibility);
        let is_public = !identifier_offsets(visibility, &visibility_mask, "pub").is_empty();
        functions
            .entry(source[name_start..cursor].to_owned())
            .or_default()
            .push(ModuleFunction { body, is_public });
    }
    functions
}

fn scheduler_member_lock_root(body: &str, mask: &[bool]) -> bool {
    let bytes = body.as_bytes();
    identifier_offsets(body, mask, "SCHEDULER")
        .into_iter()
        .any(|scheduler_offset| {
            let Some(dot) =
                next_code_non_whitespace(body, mask, scheduler_offset + "SCHEDULER".len())
            else {
                return false;
            };
            if bytes[dot] != b'.' {
                return false;
            }
            let Some(member_start) = next_code_non_whitespace(body, mask, dot + 1) else {
                return false;
            };
            ["lock", "try_lock"].into_iter().any(|member| {
                body[member_start..].starts_with(member)
                    && !bytes
                        .get(member_start + member.len())
                        .is_some_and(|byte| identifier_byte(*byte))
            })
        })
}

fn body_has_scheduler_lock_root(body: &str) -> bool {
    let mask = code_mask(body);
    let bytes = body.as_bytes();
    let name_family_root = (0..bytes.len()).any(|offset| {
        mask[offset]
            && (offset == 0 || !identifier_byte(bytes[offset - 1]))
            && identifier_byte(bytes[offset])
            && {
                let mut end = offset + 1;
                while end < bytes.len() && mask[end] && identifier_byte(bytes[end]) {
                    end += 1;
                }
                body[offset..end].ends_with("lock_scheduler")
            }
    });
    name_family_root || scheduler_member_lock_root(body, &mask)
}

fn unqualified_call_offsets(source: &str, name: &str) -> Vec<usize> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    identifier_offsets(source, &mask, name)
        .into_iter()
        .filter(|offset| {
            let qualified = previous_code_non_whitespace(source, &mask, *offset)
                .and_then(|before| bytes.get(before))
                .is_some_and(|byte| matches!(byte, b'.' | b':'));
            if qualified {
                return false;
            }
            next_code_non_whitespace(source, &mask, *offset + name.len())
                .is_some_and(|after| bytes[after] == b'(')
        })
        .collect()
}

fn census_exported_scheduler_lock_family(scheduler: &str) -> Result<BTreeSet<String>, String> {
    let functions = module_level_functions(scheduler);
    let mut lock_taking = BTreeSet::new();

    for (name, definitions) in &functions {
        if definitions
            .iter()
            .any(|definition| body_has_scheduler_lock_root(definition.body))
        {
            lock_taking.insert(name.clone());
        }
    }

    loop {
        let mut changed = false;
        for (name, definitions) in &functions {
            if lock_taking.contains(name) {
                continue;
            }
            let calls_lock_taker = definitions.iter().any(|definition| {
                lock_taking
                    .iter()
                    .any(|callee| !unqualified_call_offsets(definition.body, callee).is_empty())
            });
            if calls_lock_taker {
                changed |= lock_taking.insert(name.clone());
            }
        }
        if !changed {
            break;
        }
    }

    let exported: BTreeSet<String> = lock_taking
        .into_iter()
        .filter(|name| {
            functions.get(name).is_some_and(|definitions| {
                definitions.iter().any(|definition| definition.is_public)
            })
        })
        .collect();
    for required in [
        "with_thread_mut",
        "with_scheduler",
        "current_thread_id",
        "set_current_thread",
        "spawn",
    ] {
        if !exported.contains(required) {
            return Err(format!(
                "scheduler lock-family census collapsed: missing required export `{required}`"
            ));
        }
    }
    if exported.is_empty() {
        return Err("scheduler lock-family census collapsed to an empty set".to_owned());
    }
    Ok(exported)
}

fn validate_unqualified_scheduler_calls(
    file: &str,
    source: &str,
    source_offset: usize,
    scheduler: &str,
) -> Result<(), String> {
    for name in census_exported_scheduler_lock_family(scheduler)? {
        if let Some(offset) = unqualified_call_offsets(source, &name).into_iter().next() {
            return Err(format!(
                "{file} calls scheduler lock-taking export `{name}` unqualified at byte {}",
                source_offset + offset
            ));
        }
    }
    Ok(())
}

fn arm64_exec_bodies(manager: &str) -> Vec<(&str, &str)> {
    let functions = module_function_bodies(manager);
    ["exec_process", "exec_process_with_argv"]
        .into_iter()
        .flat_map(|name| {
            functions
                .get(name)
                .into_iter()
                .flatten()
                .filter(|body| body.contains("[ARM64]"))
                .map(move |body| (name, *body))
        })
        .collect()
}

fn validate_arm64_exec_bodies_never_touch_scheduler_lock(
    manager: &str,
    scheduler: &str,
) -> Result<(), String> {
    let bodies = arm64_exec_bodies(manager);
    if bodies.len() != 2 {
        return Err(format!(
            "expected exactly two ARM64 exec bodies, found {}",
            bodies.len()
        ));
    }

    for (name, body) in bodies {
        let body_offset = body.as_ptr() as usize - manager.as_ptr() as usize;
        validate_scheduler_paths("kernel/src/process/manager.rs", body, body_offset)
            .map_err(|error| format!("ARM64 {name}: {error}"))?;
        validate_unqualified_scheduler_calls(
            "kernel/src/process/manager.rs",
            body,
            body_offset,
            scheduler,
        )
        .map_err(|error| format!("ARM64 {name}: {error}"))?;
    }
    Ok(())
}

fn validate_arm64_exec_staging_order(manager: &str) -> Result<(), String> {
    let bodies = arm64_exec_bodies(manager);
    if bodies.len() != 2 {
        return Err(format!(
            "expected exactly two ARM64 exec bodies, found {}",
            bodies.len()
        ));
    }

    for (name, body) in bodies {
        let mask = code_mask(body);
        let mut stages = Vec::new();
        for needle in [
            "new_page_table.publish()",
            "thread.context.elr_el1 = new_entry_point",
            "thread.state = crate::task::thread::ThreadState::Ready",
            "let ctx = thread.context.clone()",
            "crate::task::scheduler::ExecSchedCommit::new(",
        ] {
            let offsets = code_offsets(body, &mask, needle);
            if offsets.len() != 1 {
                return Err(format!(
                    "ARM64 {name} body must contain exactly one `{needle}`, found {}",
                    offsets.len()
                ));
            }
            stages.push((needle, offsets[0]));
        }

        for pair in stages.windows(2) {
            if pair[0].1 >= pair[1].1 {
                return Err(format!(
                    "ARM64 {name} staging order violation: `{}` must precede `{}`",
                    pair[0].0, pair[1].0
                ));
            }
        }
    }
    Ok(())
}

/// #721 K4/X4: the x86_64 analogue of `validate_arm64_exec_staging_order` — the receipt must
/// be snapshotted from `process.main_thread` only after the page table is published and the
/// context/state fields are already set, never recomputed separately from what the frame
/// patch in `handlers.rs` uses.
fn x86_64_exec_with_argv_body(manager: &str) -> &str {
    let functions = module_function_bodies(manager);
    functions
        .get("exec_process_with_argv")
        .into_iter()
        .flatten()
        .find(|body| !body.contains("[ARM64]"))
        .expect("x86_64 exec_process_with_argv body")
}

fn validate_x86_64_exec_staging_order(manager: &str) -> Result<(), String> {
    let body = x86_64_exec_with_argv_body(manager);
    let mask = code_mask(body);
    let mut stages = Vec::new();
    for needle in [
        "new_page_table.publish()",
        "thread.context.rip = new_entry_point",
        "thread.state = crate::task::thread::ThreadState::Ready",
        "let ctx = thread.context.clone()",
        "crate::task::scheduler::ExecSchedCommit::new(",
    ] {
        let offsets = code_offsets(body, &mask, needle);
        if offsets.len() != 1 {
            return Err(format!(
                "x86_64 exec_process_with_argv body must contain exactly one `{needle}`, found {}",
                offsets.len()
            ));
        }
        stages.push((needle, offsets[0]));
    }

    for pair in stages.windows(2) {
        if pair[0].1 >= pair[1].1 {
            return Err(format!(
                "x86_64 exec_process_with_argv staging order violation: `{}` must precede `{}`",
                pair[0].0, pair[1].0
            ));
        }
    }
    Ok(())
}

fn validate_manager_module_has_no_scheduler_lock_acquisition(
    manager: &str,
    scheduler: &str,
) -> Result<(), String> {
    validate_scheduler_paths("kernel/src/process/manager.rs", manager, 0)?;
    validate_unqualified_scheduler_calls("kernel/src/process/manager.rs", manager, 0, scheduler)
}

fn validate_exec_sched_commit(scheduler: &str) -> Result<(), String> {
    let mask = code_mask(scheduler);
    let declarations = code_offsets(scheduler, &mask, "pub struct ExecSchedCommit");
    if declarations.len() != 1 {
        return Err(format!(
            "expected one ExecSchedCommit declaration, found {}",
            declarations.len()
        ));
    }

    let mut cursor = declarations[0];
    let mut must_use = false;
    loop {
        while cursor != 0 && scheduler.as_bytes()[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor == 0 || scheduler.as_bytes()[cursor - 1] != b']' {
            break;
        }
        let Some(attribute_start) = scheduler[..cursor].rfind("#[") else {
            break;
        };
        must_use |= scheduler[attribute_start..cursor].starts_with("#[must_use");
        cursor = attribute_start;
    }
    if !must_use {
        return Err("ExecSchedCommit is not directly annotated #[must_use]".to_owned());
    }

    let functions = module_function_bodies(scheduler);
    let apply_bodies = functions.get("apply").map(Vec::as_slice).unwrap_or(&[]);
    if apply_bodies.len() != 1 {
        return Err(format!(
            "expected one ExecSchedCommit::apply body, found {}",
            apply_bodies.len()
        ));
    }
    let apply = apply_bodies[0];
    let apply_mask = code_mask(apply);
    for needle in [
        "process_manager_held_on_current_cpu()",
        "EXEC_COMMIT_MISSING_THREAD",
        "SCHED_AFTER_PM_VIOLATIONS",
        "EXEC_COMMIT_UNPINNED",
        "EXEC_SCHED_COMMITS",
    ] {
        if code_offsets(apply, &apply_mask, needle).is_empty() {
            return Err(format!("ExecSchedCommit::apply is missing {needle}"));
        }
    }
    if !apply.contains("[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]") {
        return Err("ExecSchedCommit::apply is missing the PM-held violation marker".to_owned());
    }
    if !apply.contains("[EXEC_LOCK_ORDER:VIOLATION:NO_SCHED_THREAD]") {
        return Err(
            "ExecSchedCommit::apply is missing the no-scheduler-thread violation marker".to_owned(),
        );
    }
    for forbidden in ["log::"] {
        if !code_offsets(apply, &apply_mask, forbidden).is_empty() {
            return Err(format!(
                "ExecSchedCommit::apply contains forbidden output path {forbidden}"
            ));
        }
    }
    if code_offsets(apply, &apply_mask, "crate::serial_println!").len() != 4 {
        return Err(
            "ExecSchedCommit::apply must emit all four gate markers through locked serial"
                .to_owned(),
        );
    }
    if !code_offsets(apply, &apply_mask, "raw_uart_str").is_empty() {
        return Err("ExecSchedCommit::apply still uses the tearable raw UART writer".to_owned());
    }
    for marker in [
        "[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]",
        "[EXEC_LOCK_ORDER:VIOLATION:UNPINNED]",
        "[EXEC_LOCK_ORDER:VIOLATION:NO_SCHED_THREAD]",
        "[EXEC_LOCK_ORDER:FIRST_COMMIT]",
    ] {
        if apply.matches(marker).count() != 1 {
            return Err(format!(
                "ExecSchedCommit::apply must preserve exactly one {marker} literal"
            ));
        }
    }
    let scheduler_lock_end = code_offsets(apply, &apply_mask, "scheduler_lock")
        .into_iter()
        .max()
        .ok_or_else(|| "ExecSchedCommit::apply has no scheduler guard".to_owned())?;
    let first_marker_write = code_offsets(apply, &apply_mask, "crate::serial_println!")
        .into_iter()
        .min()
        .ok_or_else(|| "ExecSchedCommit::apply has no locked marker write".to_owned())?;
    if scheduler_lock_end >= first_marker_write {
        return Err(
            "ExecSchedCommit::apply writes a gate marker before releasing its scheduler guard"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_sys_exec_releases_process_manager(syscall_entry: &str) -> Result<(), String> {
    let body = function_body(syscall_entry, "sys_exec_aarch64");
    let mask = code_mask(body);
    let drops = code_offsets(body, &mask, "drop(manager_guard)");
    let applies = code_offsets(body, &mask, "commit.apply()");
    let saved_cr3 = code_offsets(body, &mask, "set_saved_process_cr3(");
    let ttbr0: Vec<usize> = body
        .match_indices("msr ttbr0_el1")
        .map(|(offset, _)| offset)
        .collect();

    for (label, count) in [
        ("drop(manager_guard)", drops.len()),
        ("commit.apply()", applies.len()),
        ("msr ttbr0_el1", ttbr0.len()),
        ("set_saved_process_cr3(", saved_cr3.len()),
    ] {
        if count != 1 {
            return Err(format!(
                "expected one {label} in sys_exec_aarch64, found {count}"
            ));
        }
    }

    let drop_offset = drops[0];
    let apply_offset = applies[0];
    if !(drop_offset < apply_offset && apply_offset < ttbr0[0] && apply_offset < saved_cr3[0]) {
        return Err(
            "sys_exec_aarch64 must drop PM before apply, TTBR0 write, and saved-CR3 update"
                .to_owned(),
        );
    }

    let after_drop = &body[drop_offset + "drop(manager_guard)".len()..];
    let after_drop_mask = code_mask(after_drop);
    if !code_offsets(after_drop, &after_drop_mask, "get_process(").is_empty() {
        return Err("sys_exec_aarch64 accesses the process manager after guard release".to_owned());
    }
    Ok(())
}

/// #721 K3: the x86_64 analogue of `validate_sys_exec_releases_process_manager` above —
/// `sys_execv_with_frame` must carry the same drop-before-apply-before-CR3 shape as
/// `sys_exec_aarch64`, pinned structurally so the x86 handler's PM-then-SCHEDULER lock order
/// can't silently regress the same way C-c/K3 found the naive "reuse verbatim" instruction
/// would have shipped it. Unlike aarch64 (one function, one profile), `sys_execv_with_frame`
/// carries a testing-arm/production-arm split, and #721 gave both arms the identical shape —
/// so each marker is expected exactly TWICE (arm 0 = testing, arm 1 = production, in that
/// textual order), each pair independently ordered and PM-clean after its own drop.
fn validate_sys_execv_with_frame_releases_process_manager(handlers: &str) -> Result<(), String> {
    let body = function_body(handlers, "sys_execv_with_frame");
    let mask = code_mask(body);
    let drops = code_offsets(body, &mask, "drop(manager_guard)");
    let applies = code_offsets(body, &mask, "commit.apply()");
    let set_next_cr3 = code_offsets(body, &mask, "set_next_cr3(");

    for (label, count) in [
        ("drop(manager_guard)", drops.len()),
        ("commit.apply()", applies.len()),
        ("set_next_cr3(", set_next_cr3.len()),
    ] {
        if count != 2 {
            return Err(format!(
                "expected {label} exactly twice (testing + production arms) in sys_execv_with_frame, found {count}"
            ));
        }
    }

    for arm in 0..2 {
        if !(drops[arm] < applies[arm] && applies[arm] < set_next_cr3[arm]) {
            return Err(format!(
                "sys_execv_with_frame arm {arm} must drop PM before apply and before installing the new CR3"
            ));
        }
    }

    // #721 m4: bound each arm's post-drop scan to that arm's own remaining body,
    // not through to the *next* arm's drop. The naive `drops[arm + 1]` end bound
    // swept arm 0's segment straight through arm 1's own pre-drop code (which
    // still legitimately holds the PM guard) — a false span, not a false negative
    // today only because arm 1 happens not to call get_process() before its own
    // drop. The real boundary between the testing and production arms is the
    // production arm's own cfg attribute.
    let production_arm_start = body
        .find("#[cfg(not(feature = \"testing\"))]")
        .ok_or_else(|| {
            "sys_execv_with_frame missing #[cfg(not(feature = \"testing\"))] arm boundary"
                .to_string()
        })?;
    let arm_bounds = [production_arm_start, body.len()];
    for arm in 0..2 {
        let arm_end = arm_bounds[arm];
        if drops[arm] >= arm_end {
            return Err(format!(
                "sys_execv_with_frame arm {arm}'s drop(manager_guard) is not inside that arm"
            ));
        }
        let after_drop_start = drops[arm] + "drop(manager_guard)".len();
        let segment = &body[after_drop_start..arm_end];
        let segment_mask = code_mask(segment);
        if !code_offsets(segment, &segment_mask, "get_process(").is_empty() {
            return Err(format!(
                "sys_execv_with_frame arm {arm} accesses the process manager after guard release"
            ));
        }
    }
    Ok(())
}

fn validate_boot_verdict_and_gate_scripts(
    executor: &str,
    full_test: &str,
    native_test: &str,
) -> Result<(), String> {
    let functions = module_function_bodies(executor);
    let emitters = functions
        .get("emit_exec_lock_order_counters")
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let counter_emitters: Vec<&&str> = emitters
        .iter()
        .filter(|body| body.contains("[EXEC_LOCK_ORDER:commits="))
        .collect();
    if counter_emitters.len() != 1 {
        return Err(format!(
            "expected one aarch64 exec counter emitter, found {}",
            counter_emitters.len()
        ));
    }
    let emitter = *counter_emitters[0];
    let emitter_mask = code_mask(emitter);
    for needle in [
        "EXEC_COMMIT_MISSING_THREAD",
        "SCHED_AFTER_PM_VIOLATIONS",
        "EXEC_COMMIT_UNPINNED",
        "EXEC_SCHED_COMMITS",
        "pm_held == 0 && unpinned == 0 && missing == 0",
    ] {
        if code_offsets(emitter, &emitter_mask, needle).is_empty() {
            return Err(format!("exec counter emitter is missing gate {needle}"));
        }
    }
    if !emitter.contains("[EXEC_LOCK_ORDER:commits={}:pm_held={}:unpinned={}:missing={}]") {
        return Err("exec counter emitter is missing the missing-thread field".to_owned());
    }

    for name in ["advance_stage_marker_only", "run_staged_tests"] {
        let body = function_body(executor, name);
        let body_mask = code_mask(body);
        if code_offsets(body, &body_mask, "emit_exec_lock_order_counters()").len() != 1 {
            return Err(format!(
                "{name} must call emit_exec_lock_order_counters exactly once"
            ));
        }
        if code_offsets(body, &body_mask, "failed == 0 && lock_order_clean").len() != 1 {
            return Err(format!("{name} does not gate PASS on clean exec counters"));
        }
        if !body.contains("[BOOT_TESTS:PASS]") || !body.contains("[BOOT_TESTS:FAIL:") {
            return Err(format!("{name} is missing a boot-test verdict"));
        }
    }

    for (path, script) in [
        ("docker/qemu/run-aarch64-full-test.sh", full_test),
        ("docker/qemu/run-aarch64-boot-test-native.sh", native_test),
    ] {
        if script.matches("\\[EXEC_LOCK_ORDER:VIOLATION").count() != 1 {
            return Err(format!(
                "{path} must grep once for exec lock-order violations"
            ));
        }
        if !script.contains("Exec lock-order violation") {
            return Err(format!("{path} is missing the exec violation diagnostic"));
        }
        for marker in [
            "\\[EXEC_SMOKE:TARGET_OK\\]",
            "\\[EXEC_LOCK_ORDER:FIRST_COMMIT\\]",
        ] {
            let greps_for_marker = script
                .lines()
                .any(|line| line.contains("grep") && line.contains(marker));
            if !greps_for_marker {
                return Err(format!("{path} does not grep for {marker}"));
            }
        }
    }

    for extraction in [
        "EXEC_COUNTER_LINE=",
        "EXEC_COMMITS=",
        "EXEC_PM_HELD=",
        "EXEC_UNPINNED=",
        "EXEC_MISSING=",
    ] {
        if !full_test.contains(extraction) {
            return Err(format!(
                "docker/qemu/run-aarch64-full-test.sh is missing {extraction} extraction"
            ));
        }
    }
    if !full_test.contains("\"$EXEC_COMMITS\" -lt 1") {
        return Err(
            "docker/qemu/run-aarch64-full-test.sh is missing the commits >= 1 floor".to_owned(),
        );
    }
    for counter in ["EXEC_PM_HELD", "EXEC_UNPINNED", "EXEC_MISSING"] {
        if !full_test.contains(&format!("\"${counter}\" -ne 0")) {
            return Err(format!(
                "docker/qemu/run-aarch64-full-test.sh does not reject nonzero {counter}"
            ));
        }
    }

    Ok(())
}

fn validate_exec_smoke_is_wired(
    init_rs: &str,
    build_sh: &str,
    cargo_toml: &str,
    launcher_rs: &str,
    target_rs: &str,
    syscall_entry: &str,
) -> Result<(), String> {
    let main = function_body(init_rs, "main");
    let main_mask = code_mask(main);
    // #721 K2: run_exec_smoke is now called on both architectures, so the raw count is two,
    // not one. Each arch's own call site is then checked individually below so a delete of
    // either one — not just both — reddens this validator.
    let smoke_calls = code_offsets(main, &main_mask, "run_exec_smoke()");
    if smoke_calls.len() != 2 {
        return Err(format!(
            "init main must call run_exec_smoke exactly once per architecture, found {}",
            smoke_calls.len()
        ));
    }
    let aarch64_gate = "#[cfg(target_arch = \"aarch64\")]\n    run_exec_smoke();";
    let x86_64_gate = "#[cfg(target_arch = \"x86_64\")]\n    run_exec_smoke();";
    if !main.contains(aarch64_gate) {
        return Err("init main does not aarch64-gate a run_exec_smoke call".to_owned());
    }
    if !main.contains(x86_64_gate) {
        return Err("init main does not x86_64-gate a run_exec_smoke call".to_owned());
    }
    let aarch64_smoke = main
        .find(aarch64_gate)
        .ok_or_else(|| "aarch64 run_exec_smoke call site not found".to_owned())?
        + aarch64_gate.len();
    let x86_smoke = main
        .find(x86_64_gate)
        .ok_or_else(|| "x86_64 run_exec_smoke call site not found".to_owned())?
        + x86_64_gate.len();

    // aarch64 ordering: after the liveness service, before wait-stress (unchanged from the
    // original single-arch check).
    let liveness_calls = code_offsets(main, &main_mask, "start_liveness_service()");
    if liveness_calls.len() != 1 || liveness_calls[0] >= aarch64_smoke {
        return Err("init must spawn the liveness service before the aarch64 exec smoke".to_owned());
    }
    let wait_stress = code_offsets(main, &main_mask, "run_wait_stress_if_enabled()");
    if wait_stress.len() != 1 || aarch64_smoke >= wait_stress[0] {
        return Err("init must run the aarch64 exec smoke before wait stress".to_owned());
    }

    // x86_64 ordering: after x86's own tty oracle call (its existing last service before this
    // one), matching the position #713's run_spawn_smoke()/run_tty_oracle() already occupy.
    let x86_tty_gate = "#[cfg(target_arch = \"x86_64\")]\n    run_tty_oracle();";
    let x86_tty = main
        .find(x86_tty_gate)
        .ok_or_else(|| "x86_64 run_tty_oracle call site not found".to_owned())?;
    if x86_tty >= x86_smoke {
        return Err("init must run x86's tty oracle before the x86 exec smoke".to_owned());
    }

    // Both arches: exec smoke precedes the remaining shared boot services and the reap loop.
    // Init stalls in a later service spawn on the aarch64 QEMU gates, so anything after
    // run_boot_script() never executes there.
    let boot_services = code_offsets(main, &main_mask, "run_boot_script()");
    if boot_services.len() != 1
        || aarch64_smoke >= boot_services[0]
        || x86_smoke >= boot_services[0]
    {
        return Err("init must run both arches' exec smoke before the remaining boot services".to_owned());
    }
    let reap_loops = code_offsets(main, &main_mask, "loop {");
    if reap_loops.len() != 1 || aarch64_smoke >= reap_loops[0] || x86_smoke >= reap_loops[0] {
        return Err("init must run both arches' exec smoke before the reap loop".to_owned());
    }

    let liveness = function_body(init_rs, "start_liveness_service");
    if !liveness.contains("spawn(b\"/bin/heartbeat\\0\")") {
        return Err("start_liveness_service must spawn /bin/heartbeat".to_owned());
    }
    let boot_script = function_body(init_rs, "run_boot_script");
    if boot_script.contains("b\"/bin/heartbeat\\0\"") {
        return Err("run_boot_script must not spawn /bin/heartbeat".to_owned());
    }

    // #721 K2: run_exec_smoke's definition must now be arch-neutral — both arches call it
    // and its body (spawn + waitpid + print) has no arch-specific content. Reject a stray
    // target_arch gate directly above the definition, which would make one arch's call site
    // fail to compile.
    let smoke_fn_offset = init_rs
        .find("fn run_exec_smoke(")
        .ok_or_else(|| "init is missing run_exec_smoke".to_owned())?;
    let preceding = init_rs[..smoke_fn_offset].trim_end_matches(['\n', ' ']);
    let last_line = preceding.rsplit('\n').next().unwrap_or("");
    if last_line.trim_start().starts_with("#[cfg(target_arch") {
        return Err(
            "run_exec_smoke must be arch-neutral (both arches call it), but is gated to one architecture"
                .to_owned(),
        );
    }
    let smoke = function_body(init_rs, "run_exec_smoke");
    for required in [
        "spawn(b\"/bin/exec_smoke\\0\")",
        "waitpid(",
        "[EXEC_SMOKE:LAUNCHER_EXIT code={}]",
        "[EXEC_SMOKE:SPAWN_FAILED {}]",
    ] {
        if !smoke.contains(required) {
            return Err(format!("run_exec_smoke is missing {required}"));
        }
    }

    for binary in ["exec_smoke", "exec_smoke_target"] {
        let build_entry = format!("    \"{binary}\"");
        if build_sh.lines().filter(|line| *line == build_entry).count() != 1 {
            return Err(format!("build.sh must install {binary} exactly once"));
        }
        let cargo_entry = format!("[[bin]]\nname = \"{binary}\"\npath = \"src/{binary}.rs\"");
        if cargo_toml.matches(&cargo_entry).count() != 1 {
            return Err(format!("Cargo.toml must declare {binary} exactly once"));
        }
    }

    let launcher = function_body(launcher_rs, "main");
    if !launcher.contains("execv(path, argv.as_ptr())")
        || !launcher.contains("b\"/bin/exec_smoke_target\\0\"")
        || !launcher.contains("[EXEC_SMOKE:EXEC_FAILED]")
    {
        return Err("exec smoke launcher is not wired to execv the target".to_owned());
    }

    let target = function_body(target_rs, "main");
    let sleep = target
        .find("libbreenix::time::nanosleep(&ts)")
        .ok_or_else(|| "exec smoke target does not sleep".to_owned())?;
    let yield_call = target
        .find("yield_now()")
        .ok_or_else(|| "exec smoke target does not yield".to_owned())?;
    let ok_marker = target
        .find("[EXEC_SMOKE:TARGET_OK]")
        .ok_or_else(|| "exec smoke target is missing its success marker".to_owned())?;
    if sleep >= ok_marker || yield_call >= ok_marker {
        return Err("exec smoke target marker must follow its sleep and yield".to_owned());
    }

    let sys_exec = function_body(syscall_entry, "sys_exec_aarch64");
    let sys_exec_mask = code_mask(sys_exec);
    let emitters = code_offsets(
        sys_exec,
        &sys_exec_mask,
        "crate::test_framework::emit_exec_lock_order_counters()",
    );
    if emitters.len() != 1 {
        return Err(format!(
            "sys_exec_aarch64 must emit live counters exactly once, found {}",
            emitters.len()
        ));
    }
    let apply = code_offsets(sys_exec, &sys_exec_mask, "commit.apply()");
    if apply.len() != 1 || apply[0] >= emitters[0] {
        return Err("sys_exec_aarch64 must emit counters after commit.apply()".to_owned());
    }
    let cfg = sys_exec[..emitters[0]]
        .rfind("#[cfg(feature = \"boot_tests\")]")
        .ok_or_else(|| "sys_exec_aarch64 counter emission is not boot_tests-only".to_owned())?;
    let guarded = &sys_exec[cfg..emitters[0]];
    if !guarded.contains("if result == 0") || guarded.contains("commit.apply()") {
        return Err(
            "sys_exec_aarch64 counter emission is not in a post-commit boot_tests guard".to_owned(),
        );
    }

    Ok(())
}

fn insert_in_arm64_exec(manager: &str, function_name: &str, insertion: &str) -> String {
    let functions = module_function_bodies(manager);
    let body = functions
        .get(function_name)
        .into_iter()
        .flatten()
        .find(|body| body.contains("[ARM64]"))
        .expect("ARM64 exec body for negative control");
    let body_start = body.as_ptr() as usize - manager.as_ptr() as usize;
    let insert_at = body_start
        + body
            .rfind("        Ok(")
            .expect("ARM64 exec final Ok return");
    let mut mutated = manager.to_owned();
    mutated.insert_str(insert_at, insertion);
    mutated
}

fn validate_creation_publication_seams(scheduler: &str) -> Result<(), String> {
    let functions = module_function_bodies(scheduler);
    for name in ["spawn", "spawn_front", "spawn_as_current"] {
        let bodies = functions.get(name).map(Vec::as_slice).unwrap_or(&[]);
        if bodies.len() != 1 {
            return Err(format!(
                "scheduler publication entry `{name}` must be unique, found {}",
                bodies.len()
            ));
        }
        let body = bodies[0];
        let mask = code_mask(body);
        let notes = code_offsets(body, &mask, "note_scheduler_publication()");
        let critical_sections = code_offsets(body, &mask, "without_interrupts(");
        if notes.len() != 1 || critical_sections.len() != 1 || notes[0] >= critical_sections[0] {
            return Err(format!(
                "{name} must publish exactly once before entering without_interrupts"
            ));
        }
        let statements = block_statements(body)
            .ok_or_else(|| format!("{name} does not have a braced function body"))?;
        if !compact_code(statements).starts_with("note_scheduler_publication();") {
            return Err(format!(
                "note_scheduler_publication() is not the first statement of {name}"
            ));
        }
    }
    Ok(())
}

type PublicationCensus = BTreeMap<(String, String), usize>;

/// An empty expected set means no creation publication may occur while a
/// process-manager binding is live; the validator separately requires a
/// non-empty population of functions containing both mechanisms.
const CREATION_PUBLICATION_UNDER_PM_GUARD: &[(&str, &str, usize)] = &[];

fn enclosing_block_close(source: &str, mask: &[bool], offset: usize) -> Option<usize> {
    (0..offset)
        .filter(|open| mask[*open] && source.as_bytes()[*open] == b'{')
        .filter_map(|open| {
            let block = braced_block(source, mask, open)?;
            let close = open + block.len() - 1;
            (offset < close).then_some((open, close))
        })
        .max_by_key(|(open, _)| *open)
        .map(|(_, close)| close)
}

fn manager_binding_name(body: &str, manager_call: usize) -> Result<String, String> {
    let prefix = &body[..manager_call];
    let mask = code_mask(prefix);
    let statement_start = (0..prefix.len())
        .rev()
        .find(|offset| mask[*offset] && matches!(prefix.as_bytes()[*offset], b';' | b'{' | b'}'))
        .map(|offset| offset + 1)
        .unwrap_or(0);
    let let_offset = identifier_offsets(prefix, &mask, "let")
        .into_iter()
        .filter(|offset| *offset >= statement_start)
        .next_back()
        .ok_or_else(|| "crate::process::manager() is not bound by let".to_owned())?;
    let equals = (let_offset + "let".len()..manager_call)
        .find(|offset| mask[*offset] && prefix.as_bytes()[*offset] == b'=')
        .ok_or_else(|| "process-manager let binding has no assignment".to_owned())?;
    let pattern = &prefix[let_offset + "let".len()..equals];
    let pattern_mask = code_mask(pattern);
    let bytes = pattern.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !pattern_mask[cursor] || !(bytes[cursor] == b'_' || bytes[cursor].is_ascii_alphabetic())
        {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && pattern_mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        let candidate = &pattern[start..cursor];
        if !matches!(candidate, "mut" | "ref" | "let") {
            return Ok(candidate.to_owned());
        }
    }
    Err("process-manager let binding has no guard identifier".to_owned())
}

fn guard_is_explicitly_dropped(
    body: &str,
    mask: &[bool],
    guard: &str,
    after: usize,
    before: usize,
) -> bool {
    identifier_offsets(body, mask, "drop")
        .into_iter()
        .filter(|offset| *offset > after && *offset < before)
        .any(|offset| {
            let Some(open) = next_code_non_whitespace(body, mask, offset + "drop".len()) else {
                return false;
            };
            if body.as_bytes()[open] != b'(' {
                return false;
            }
            let Some(close) = matching_paren(body, mask, open) else {
                return false;
            };
            compact_code(&body[open + 1..close]) == guard
        })
}

fn validate_creation_publications_release_process_manager(
    sources: &[(String, String)],
) -> Result<(), String> {
    let mut offenders = PublicationCensus::new();
    let mut candidate_functions = 0usize;
    for (path, source) in sources {
        for (name, bodies) in module_function_bodies(source) {
            let duplicate = bodies.len() > 1;
            for body in bodies {
                let mask = code_mask(body);
                let manager_calls = code_offsets(body, &mask, "crate::process::manager()");
                let mut spawn_calls = Vec::new();
                for publication in [
                    "scheduler::spawn(",
                    "scheduler::spawn_front(",
                    "scheduler::spawn_as_current(",
                ] {
                    spawn_calls.extend(code_offsets(body, &mask, publication));
                }
                spawn_calls.sort_unstable();
                if manager_calls.is_empty() || spawn_calls.is_empty() {
                    continue;
                }
                candidate_functions += 1;
                let item = if duplicate {
                    format!("fn {name} [duplicate item path]")
                } else {
                    format!("fn {name}")
                };
                let mut bindings = Vec::new();
                for manager_call in manager_calls {
                    let guard = manager_binding_name(body, manager_call)
                        .map_err(|error| format!("{path} :: {item}: {error}"))?;
                    let scope_close = enclosing_block_close(body, &mask, manager_call)
                        .ok_or_else(|| format!("{path} :: {item}: manager binding has no scope"))?;
                    bindings.push((manager_call, scope_close, guard));
                }
                for spawn_call in spawn_calls {
                    let guard_live = bindings.iter().any(|(manager_call, scope_close, guard)| {
                        *manager_call < spawn_call
                            && spawn_call < *scope_close
                            && !guard_is_explicitly_dropped(
                                body,
                                &mask,
                                guard,
                                *manager_call,
                                spawn_call,
                            )
                    });
                    if guard_live {
                        *offenders.entry((path.clone(), item.clone())).or_default() += 1;
                    }
                }
            }
        }
    }
    if candidate_functions == 0 {
        return Err(
            "creation publication guard census became vacuous: no function contained both mechanisms"
                .to_owned(),
        );
    }

    let mut expected = PublicationCensus::new();
    for (path, item, count) in CREATION_PUBLICATION_UNDER_PM_GUARD {
        expected.insert(((*path).to_owned(), (*item).to_owned()), *count);
    }
    if offenders != expected {
        let details = offenders
            .into_iter()
            .map(|((path, item), count)| format!("{path} :: {item} ({count} offending calls)"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "creation publication occurred under a live process-manager guard\n{details}"
        ));
    }
    Ok(())
}

fn validate_creation_lock_order_marker(scheduler: &str, teardown: &str) -> Result<(), String> {
    const PRODUCTION_MARKER: &str = "[CREATION_LOCK_ORDER:VIOLATION:PM_HELD]";
    const INJECTION_MARKER: &str = "[CREATION_LOCK_ORDER:INJECTED:PM_HELD]";
    const EXEC_MARKER: &str = "[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]";
    const SHARED_HELPER: &str = "account_creation_publication_pm_held";
    const PM_HELD_PREDICATE: &str = "crate::process::process_manager_held_on_current_cpu()";
    if PRODUCTION_MARKER == INJECTION_MARKER || PRODUCTION_MARKER == EXEC_MARKER {
        return Err("creation lock-order markers are not distinct".to_owned());
    }
    if scheduler.matches(PRODUCTION_MARKER).count() != 1
        || scheduler.matches(INJECTION_MARKER).count() != 1
        || scheduler.matches(EXEC_MARKER).count() != 1
    {
        return Err(
            "creation injection, violation, or exec marker is not uniquely pinned".to_owned(),
        );
    }
    let functions = module_function_bodies(scheduler);
    let unique_body = |name: &str| -> Result<&str, String> {
        let bodies = functions.get(name).map(Vec::as_slice).unwrap_or(&[]);
        if bodies.len() != 1 {
            return Err(format!(
                "{name} must exist exactly once, found {}",
                bodies.len()
            ));
        }
        Ok(bodies[0])
    };
    let helper = unique_body(SHARED_HELPER)?;
    let helper_mask = code_mask(helper);
    if code_offsets(helper, &helper_mask, PM_HELD_PREDICATE).len() != 1
        || code_offsets(
            helper,
            &helper_mask,
            "CREATION_PUBLICATIONS_PM_HELD.fetch_add",
        )
        .len()
            != 1
    {
        return Err(
            "shared creation publication helper must own the predicate and PM-held accounting"
                .to_owned(),
        );
    }
    let helper_statements = block_statements(helper)
        .ok_or_else(|| "shared creation publication helper has no body".to_owned())?;
    if !compact_code(helper_statements).ends_with("pm_held") {
        return Err("shared creation publication helper does not return its predicate".to_owned());
    }

    let note = unique_body("note_scheduler_publication")?;
    let note_mask = code_mask(note);
    let probe = unique_body("probe_publication_lock_order_injection")?;
    let probe_mask = code_mask(probe);
    let helper_call = format!("{SHARED_HELPER}()");
    for (name, body, mask) in [
        ("note_scheduler_publication", note, note_mask.as_slice()),
        (
            "probe_publication_lock_order_injection",
            probe,
            probe_mask.as_slice(),
        ),
    ] {
        if code_offsets(body, mask, &helper_call).len() != 1 {
            return Err(format!(
                "{name} must call the shared creation helper exactly once"
            ));
        }
        if !code_offsets(body, mask, PM_HELD_PREDICATE).is_empty() {
            return Err(format!(
                "{name} duplicates the process-manager-held predicate"
            ));
        }
    }
    if note.matches(PRODUCTION_MARKER).count() != 1 || note.contains(INJECTION_MARKER) {
        return Err("creation PM-held marker left the publication seam".to_owned());
    }
    if probe.matches(INJECTION_MARKER).count() != 1 || probe.contains(PRODUCTION_MARKER) {
        return Err("injection probe does not own its distinct marker".to_owned());
    }
    if code_offsets(
        probe,
        &probe_mask,
        "CREATION_PUBLICATIONS_PM_HELD_INJECTED.fetch_add",
    )
    .len()
        != 1
    {
        return Err("injection probe does not account its injected PM-held event".to_owned());
    }
    let probe_statements = block_statements(probe)
        .ok_or_else(|| "creation publication injection probe has no body".to_owned())?;
    if !compact_code(probe_statements).ends_with("pm_held") {
        return Err(
            "creation publication injection probe does not return the predicate".to_owned(),
        );
    }
    unique_body("creation_lock_order_counters")?;

    let oracle = function_body(teardown, "kernel_stack_ownership_oracle_test");
    let oracle_mask = code_mask(oracle);
    if code_offsets(
        oracle,
        &oracle_mask,
        "crate::task::scheduler::creation_lock_order_counters()",
    )
    .len()
        != 2
    {
        return Err(
            "kernel-stack ownership oracle must read creation counters around the injection"
                .to_owned(),
        );
    }
    for needle in [
        "crate::task::scheduler::probe_publication_lock_order_injection()",
        "if !injection_saw_pm_held",
        "if injected_delta != 1",
        "if measurements.sched_pm_held_production != 0",
        "if measurements.sched_pm_held_injected != 1",
    ] {
        if code_offsets(oracle, &oracle_mask, needle).len() != 1 {
            return Err(format!(
                "kernel-stack ownership oracle does not uniquely pin `{needle}`"
            ));
        }
    }
    const ORACLE_FIELDS: &str =
        ":sched_publications={}:sched_pm_held_production={}:sched_pm_held_injected={}:reconciliation_diff={}:reconciliation_skew_bound={}:balance={}]";
    if oracle.matches(ORACLE_FIELDS).count() != 1 {
        return Err(
            "kernel-stack ownership oracle lock-order fields are not uniquely pinned".to_owned(),
        );
    }
    if code_offsets(oracle, &oracle_mask, "creation_counters.pm_held_injected").len() != 2 {
        return Err(
            "kernel-stack ownership oracle does not read the injected counter for production subtraction and reporting"
                .to_owned(),
        );
    }
    Ok(())
}

#[test]
fn arm64_exec_bodies_never_touch_the_scheduler_lock() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    validate_arm64_exec_bodies_never_touch_scheduler_lock(&manager, &scheduler)
        .expect("T1 validation");
}

#[test]
fn manager_module_has_no_scheduler_lock_acquisition_anywhere() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    validate_manager_module_has_no_scheduler_lock_acquisition(&manager, &scheduler)
        .expect("T2 validation");
}

#[test]
fn exec_sched_commit_is_a_must_use_receipt_applied_in_the_scheduler() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    validate_exec_sched_commit(&scheduler).expect("T3 validation");
}

#[test]
fn sys_exec_releases_the_process_manager_before_the_scheduler() {
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    validate_sys_exec_releases_process_manager(&syscall_entry).expect("T4 validation");
}

/// #721 K3: the x86_64 analogue of T4 above.
#[test]
fn sys_execv_with_frame_releases_the_process_manager_before_the_scheduler() {
    let handlers = repo_text("kernel/src/syscall/handlers.rs");
    validate_sys_execv_with_frame_releases_process_manager(&handlers).expect("#721 K3 validation");
}

#[test]
fn boot_verdict_emits_and_gates_on_the_exec_lock_order_counters() {
    let executor = repo_text("kernel/src/test_framework/executor.rs");
    let full_test = repo_text("docker/qemu/run-aarch64-full-test.sh");
    let native_test = repo_text("docker/qemu/run-aarch64-boot-test-native.sh");
    validate_boot_verdict_and_gate_scripts(&executor, &full_test, &native_test)
        .expect("T5 validation");
}

#[test]
fn arm64_exec_bodies_stage_the_receipt_after_the_context_reset() {
    let manager = repo_text("kernel/src/process/manager.rs");
    validate_arm64_exec_staging_order(&manager).expect("T6 validation");
}

/// #721 K4/X4: the x86_64 analogue of T6 above.
#[test]
fn x86_64_exec_body_stages_the_receipt_after_the_context_reset() {
    let manager = repo_text("kernel/src/process/manager.rs");
    validate_x86_64_exec_staging_order(&manager).expect("#721 K4 validation");
}

#[test]
fn exec_smoke_is_wired_into_both_boot_paths() {
    let init_rs = repo_text("userspace/programs/src/init.rs");
    let build_sh = repo_text("userspace/programs/build.sh");
    let cargo_toml = repo_text("userspace/programs/Cargo.toml");
    let launcher_rs = repo_text("userspace/programs/src/exec_smoke.rs");
    let target_rs = repo_text("userspace/programs/src/exec_smoke_target.rs");
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    validate_exec_smoke_is_wired(
        &init_rs,
        &build_sh,
        &cargo_toml,
        &launcher_rs,
        &target_rs,
        &syscall_entry,
    )
    .expect("T7 validation");
}

#[test]
fn negative_arm64_exec_scheduler_acquisition_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let mutated = insert_in_arm64_exec(
        &manager,
        "exec_process_with_argv",
        "\ncrate::task::scheduler::with_thread_mut(thread_id, |t| {\n    t.state = crate::task::thread::ThreadState::Ready;\n});\n",
    );
    assert!(validate_arm64_exec_bodies_never_touch_scheduler_lock(&mutated, &scheduler).is_err());
}

#[test]
fn negative_arm64_exec_scheduler_path_call_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let mutated = insert_in_arm64_exec(
        &manager,
        "exec_process_with_argv",
        "\nlet _peer = crate::task::scheduler::current_thread_id();\n",
    );
    assert!(validate_arm64_exec_bodies_never_touch_scheduler_lock(&mutated, &scheduler).is_err());
    assert!(
        validate_manager_module_has_no_scheduler_lock_acquisition(&mutated, &scheduler).is_err()
    );
}

#[test]
fn negative_arm64_exec_snapshot_before_reset_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let snapshot = "        let ctx = thread.context.clone();\n";
    let (snapshot_start, aligned_stack_start) = {
        let body = arm64_exec_bodies(&manager)
            .into_iter()
            .find(|(name, _)| *name == "exec_process_with_argv")
            .map(|(_, body)| body)
            .expect("exec_process_with_argv ARM64 body");
        let body_start = body.as_ptr() as usize - manager.as_ptr() as usize;
        let snapshot_start = body_start
            + body
                .find(snapshot)
                .expect("exec_process_with_argv context snapshot");
        let aligned_stack_start = body_start
            + body
                .find("        let aligned_stack = ")
                .expect("exec_process_with_argv aligned stack reset");
        (snapshot_start, aligned_stack_start)
    };

    let mut mutated = manager.clone();
    mutated.replace_range(snapshot_start..snapshot_start + snapshot.len(), "");
    mutated.insert_str(aligned_stack_start, snapshot);
    assert_ne!(mutated, manager, "snapshot-before-reset mutation applied");
    assert!(validate_arm64_exec_staging_order(&mutated).is_err());
}

/// #721 K4/K13: the x86_64 analogue of the ARM64 snapshot-before-reset negative test above.
#[test]
fn negative_x86_64_exec_snapshot_before_reset_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let snapshot = "        let ctx = thread.context.clone();\n";
    let reset_marker = "        thread.context.rip = new_entry_point;\n";
    let (snapshot_start, reset_start) = {
        let body = x86_64_exec_with_argv_body(&manager);
        let body_start = body.as_ptr() as usize - manager.as_ptr() as usize;
        let snapshot_start = body_start
            + body
                .find(snapshot)
                .expect("x86_64 exec_process_with_argv context snapshot");
        let reset_start = body_start
            + body
                .find(reset_marker)
                .expect("x86_64 exec_process_with_argv context reset");
        (snapshot_start, reset_start)
    };

    let mut mutated = manager.clone();
    mutated.replace_range(snapshot_start..snapshot_start + snapshot.len(), "");
    mutated.insert_str(reset_start, snapshot);
    assert_ne!(mutated, manager, "x86_64 snapshot-before-reset mutation applied");
    assert!(validate_x86_64_exec_staging_order(&mutated).is_err());
}

#[test]
fn negative_manager_module_scheduler_acquisition_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let mut mutated = manager.clone();
    mutated.push_str(
        "\nfn synthetic_lock_inversion(thread_id: u64) {\n    crate::task::scheduler::with_thread_mut(thread_id, |t| {\n        t.state = crate::task::thread::ThreadState::Ready;\n    });\n}\n",
    );
    assert!(
        validate_manager_module_has_no_scheduler_lock_acquisition(&mutated, &scheduler).is_err()
    );
}

#[test]
fn negative_manager_scheduler_use_import_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let mut mutated = manager.clone();
    mutated.push_str(
        "\nuse crate::task::scheduler::current_thread_id;\nfn synthetic_scheduler_call() {\n    let peer = current_thread_id();\n    core::mem::drop(peer);\n}\n",
    );
    assert!(
        validate_manager_module_has_no_scheduler_lock_acquisition(&mutated, &scheduler).is_err()
    );
}

#[test]
fn negative_scheduler_glob_import_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let mut mutated = manager.clone();
    mutated.push_str("\nuse crate::task::scheduler::*;\n");
    assert!(
        validate_manager_module_has_no_scheduler_lock_acquisition(&mutated, &scheduler).is_err()
    );
}

#[test]
fn negative_unqualified_scheduler_call_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let mut mutated = manager.clone();
    mutated.push_str("\nfn synthetic_scheduler_call() {\n    set_current_thread(0);\n}\n");
    assert!(
        validate_manager_module_has_no_scheduler_lock_acquisition(&mutated, &scheduler).is_err()
    );
}

#[test]
fn negative_collapsed_scheduler_census_is_rejected() {
    let scheduler = "pub fn harmless() {}\n";
    assert!(census_exported_scheduler_lock_family(scheduler).is_err());
}

#[test]
fn negative_exec_sched_commit_without_must_use_is_rejected() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let attribute_start = scheduler
        .find("#[must_use")
        .expect("ExecSchedCommit must-use attribute");
    let attribute_end = scheduler[attribute_start..]
        .find('\n')
        .map(|offset| attribute_start + offset + 1)
        .expect("must-use attribute newline");
    let mut mutated = scheduler.clone();
    mutated.replace_range(attribute_start..attribute_end, "");
    assert!(validate_exec_sched_commit(&mutated).is_err());
}

#[test]
fn negative_exec_sched_commit_without_pm_guard_is_rejected() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let predicate = "let pm_held = crate::process::process_manager_held_on_current_cpu();";
    let apply_start = scheduler
        .find("pub fn apply(self)")
        .expect("ExecSchedCommit::apply start");
    let predicate_start = apply_start
        + scheduler[apply_start..]
            .find(predicate)
            .expect("ExecSchedCommit::apply PM-held predicate");
    let mut mutated = scheduler.clone();
    mutated.replace_range(
        predicate_start..predicate_start + predicate.len(),
        "let pm_held = false;",
    );
    assert_ne!(mutated, scheduler, "PM guard mutation applied");
    assert!(validate_exec_sched_commit(&mutated).is_err());
}

#[test]
fn negative_sys_exec_apply_before_drop_is_rejected() {
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let mutated = syscall_entry.replacen(
        "        drop(manager_guard);\n\n        commit.apply();",
        "        commit.apply();\n\n        drop(manager_guard);",
        1,
    );
    assert_ne!(mutated, syscall_entry, "drop/apply swap mutation applied");
    assert!(validate_sys_exec_releases_process_manager(&mutated).is_err());
}

#[test]
fn negative_sys_exec_missing_drop_is_rejected() {
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let mutated = syscall_entry.replacen(
        "        drop(manager_guard);\n\n        commit.apply();",
        "        commit.apply();",
        1,
    );
    assert_ne!(mutated, syscall_entry, "drop deletion mutation applied");
    assert!(validate_sys_exec_releases_process_manager(&mutated).is_err());
}

/// #721 K3/K13: the x86_64 analogue of the two aarch64 negative tests above.
#[test]
fn negative_sys_execv_with_frame_apply_before_drop_is_rejected() {
    let handlers = repo_text("kernel/src/syscall/handlers.rs");
    let mutated = handlers.replacen(
        "            drop(manager_guard);\n\n            commit.apply();",
        "            commit.apply();\n\n            drop(manager_guard);",
        1,
    );
    assert_ne!(mutated, handlers, "drop/apply swap mutation applied");
    assert!(validate_sys_execv_with_frame_releases_process_manager(&mutated).is_err());
}

#[test]
fn negative_sys_execv_with_frame_missing_drop_is_rejected() {
    let handlers = repo_text("kernel/src/syscall/handlers.rs");
    let mutated = handlers.replacen(
        "            drop(manager_guard);\n\n            commit.apply();",
        "            commit.apply();",
        1,
    );
    assert_ne!(mutated, handlers, "drop deletion mutation applied");
    assert!(validate_sys_execv_with_frame_releases_process_manager(&mutated).is_err());
}

#[test]
fn negative_full_gate_without_violation_grep_is_rejected() {
    let executor = repo_text("kernel/src/test_framework/executor.rs");
    let full_test = repo_text("docker/qemu/run-aarch64-full-test.sh");
    let native_test = repo_text("docker/qemu/run-aarch64-boot-test-native.sh");
    let mutated = full_test.replacen(
        "\\[EXEC_LOCK_ORDER:VIOLATION",
        "\\[EXEC_LOCK_ORDER:REMOVED",
        1,
    );
    assert_ne!(mutated, full_test, "violation grep mutation applied");
    assert!(validate_boot_verdict_and_gate_scripts(&executor, &mutated, &native_test).is_err());
}

#[test]
fn negative_exec_smoke_init_spawn_deletion_is_rejected() {
    let init_rs = repo_text("userspace/programs/src/init.rs");
    let build_sh = repo_text("userspace/programs/build.sh");
    let cargo_toml = repo_text("userspace/programs/Cargo.toml");
    let launcher_rs = repo_text("userspace/programs/src/exec_smoke.rs");
    let target_rs = repo_text("userspace/programs/src/exec_smoke_target.rs");
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let mutated = init_rs.replacen(
        "spawn(b\"/bin/exec_smoke\\0\")",
        "spawn(b\"/bin/removed_exec_smoke\\0\")",
        1,
    );
    assert_ne!(mutated, init_rs, "init exec smoke spawn mutation applied");
    assert!(validate_exec_smoke_is_wired(
        &mutated,
        &build_sh,
        &cargo_toml,
        &launcher_rs,
        &target_rs,
        &syscall_entry,
    )
    .is_err());
}

#[test]
fn negative_exec_smoke_before_liveness_spawn_is_rejected() {
    let init_rs = repo_text("userspace/programs/src/init.rs");
    let build_sh = repo_text("userspace/programs/build.sh");
    let cargo_toml = repo_text("userspace/programs/Cargo.toml");
    let launcher_rs = repo_text("userspace/programs/src/exec_smoke.rs");
    let target_rs = repo_text("userspace/programs/src/exec_smoke_target.rs");
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let smoke_call = "    #[cfg(target_arch = \"aarch64\")]\n    run_exec_smoke();\n";
    let without_smoke = init_rs.replacen(smoke_call, "", 1);
    let liveness_call = "    #[cfg(target_arch = \"aarch64\")]\n    start_liveness_service();\n";
    let mutated = without_smoke.replacen(liveness_call, &format!("{smoke_call}{liveness_call}"), 1);
    assert_ne!(
        mutated, init_rs,
        "exec smoke before liveness mutation applied"
    );
    assert!(validate_exec_smoke_is_wired(
        &mutated,
        &build_sh,
        &cargo_toml,
        &launcher_rs,
        &target_rs,
        &syscall_entry,
    )
    .is_err());
}

#[test]
fn negative_exec_smoke_after_boot_services_is_rejected() {
    let init_rs = repo_text("userspace/programs/src/init.rs");
    let build_sh = repo_text("userspace/programs/build.sh");
    let cargo_toml = repo_text("userspace/programs/Cargo.toml");
    let launcher_rs = repo_text("userspace/programs/src/exec_smoke.rs");
    let target_rs = repo_text("userspace/programs/src/exec_smoke_target.rs");
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let smoke_call = "    #[cfg(target_arch = \"aarch64\")]\n    run_exec_smoke();\n";
    let without_smoke = init_rs.replacen(smoke_call, "", 1);
    let mutated = without_smoke.replacen(
        "    run_boot_script();\n",
        &format!("    run_boot_script();\n{smoke_call}"),
        1,
    );
    assert_ne!(
        mutated, init_rs,
        "exec smoke after boot services mutation applied"
    );
    assert!(validate_exec_smoke_is_wired(
        &mutated,
        &build_sh,
        &cargo_toml,
        &launcher_rs,
        &target_rs,
        &syscall_entry,
    )
    .is_err());
}

#[test]
fn negative_exec_smoke_after_reap_loop_is_rejected() {
    let init_rs = repo_text("userspace/programs/src/init.rs");
    let build_sh = repo_text("userspace/programs/build.sh");
    let cargo_toml = repo_text("userspace/programs/Cargo.toml");
    let launcher_rs = repo_text("userspace/programs/src/exec_smoke.rs");
    let target_rs = repo_text("userspace/programs/src/exec_smoke_target.rs");
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let smoke_call = "    #[cfg(target_arch = \"aarch64\")]\n    run_exec_smoke();\n";
    let without_smoke = init_rs.replacen(smoke_call, "", 1);
    let mutated = without_smoke.replacen(
        "    }\n}\n\n///",
        &format!("    }}\n{smoke_call}}}\n\n///"),
        1,
    );
    assert_ne!(
        mutated, init_rs,
        "exec smoke after reap loop mutation applied"
    );
    assert!(validate_exec_smoke_is_wired(
        &mutated,
        &build_sh,
        &cargo_toml,
        &launcher_rs,
        &target_rs,
        &syscall_entry,
    )
    .is_err());
}

/// #721 K2/K13: a deleted x86_64 call site must redden the validator too, not just the
/// pre-existing aarch64 one.
#[test]
fn negative_exec_smoke_x86_64_call_site_deletion_is_rejected() {
    let init_rs = repo_text("userspace/programs/src/init.rs");
    let build_sh = repo_text("userspace/programs/build.sh");
    let cargo_toml = repo_text("userspace/programs/Cargo.toml");
    let launcher_rs = repo_text("userspace/programs/src/exec_smoke.rs");
    let target_rs = repo_text("userspace/programs/src/exec_smoke_target.rs");
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let x86_smoke_call = "    #[cfg(target_arch = \"x86_64\")]\n    run_exec_smoke();\n";
    let mutated = init_rs.replacen(x86_smoke_call, "", 1);
    assert_ne!(mutated, init_rs, "x86_64 exec smoke call deletion applied");
    assert!(validate_exec_smoke_is_wired(
        &mutated,
        &build_sh,
        &cargo_toml,
        &launcher_rs,
        &target_rs,
        &syscall_entry,
    )
    .is_err());
}

/// #721 K2/K13: the x86_64 call site must be ordered after x86's own tty oracle, matching the
/// position #713's run_spawn_smoke()/run_tty_oracle() already occupy.
#[test]
fn negative_exec_smoke_x86_64_before_tty_oracle_is_rejected() {
    let init_rs = repo_text("userspace/programs/src/init.rs");
    let build_sh = repo_text("userspace/programs/build.sh");
    let cargo_toml = repo_text("userspace/programs/Cargo.toml");
    let launcher_rs = repo_text("userspace/programs/src/exec_smoke.rs");
    let target_rs = repo_text("userspace/programs/src/exec_smoke_target.rs");
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let x86_smoke_call = "    #[cfg(target_arch = \"x86_64\")]\n    run_exec_smoke();\n";
    let without_smoke = init_rs.replacen(x86_smoke_call, "", 1);
    let x86_spawn_smoke_call = "    #[cfg(target_arch = \"x86_64\")]\n    run_spawn_smoke();\n";
    let mutated = without_smoke.replacen(
        x86_spawn_smoke_call,
        &format!("{x86_smoke_call}{x86_spawn_smoke_call}"),
        1,
    );
    assert_ne!(mutated, init_rs, "x86_64 exec smoke reorder mutation applied");
    assert!(validate_exec_smoke_is_wired(
        &mutated,
        &build_sh,
        &cargo_toml,
        &launcher_rs,
        &target_rs,
        &syscall_entry,
    )
    .is_err());
}

/// #721 K2: re-gating the shared run_exec_smoke definition to a single architecture must
/// redden the validator, since the other architecture's call site would then fail to build.
#[test]
fn negative_exec_smoke_definition_regated_to_one_arch_is_rejected() {
    let init_rs = repo_text("userspace/programs/src/init.rs");
    let build_sh = repo_text("userspace/programs/build.sh");
    let cargo_toml = repo_text("userspace/programs/Cargo.toml");
    let launcher_rs = repo_text("userspace/programs/src/exec_smoke.rs");
    let target_rs = repo_text("userspace/programs/src/exec_smoke_target.rs");
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let mutated = init_rs.replacen(
        "fn run_exec_smoke() {",
        "#[cfg(target_arch = \"aarch64\")]\nfn run_exec_smoke() {",
        1,
    );
    assert_ne!(mutated, init_rs, "run_exec_smoke re-gate mutation applied");
    assert!(validate_exec_smoke_is_wired(
        &mutated,
        &build_sh,
        &cargo_toml,
        &launcher_rs,
        &target_rs,
        &syscall_entry,
    )
    .is_err());
}

#[test]
fn negative_full_gate_without_exec_smoke_target_grep_is_rejected() {
    let executor = repo_text("kernel/src/test_framework/executor.rs");
    let full_test = repo_text("docker/qemu/run-aarch64-full-test.sh");
    let native_test = repo_text("docker/qemu/run-aarch64-boot-test-native.sh");
    let mutated = full_test.replacen("\\[EXEC_SMOKE:TARGET_OK\\]", "\\[EXEC_SMOKE:REMOVED\\]", 1);
    assert_ne!(mutated, full_test, "exec smoke grep mutation applied");
    assert!(validate_boot_verdict_and_gate_scripts(&executor, &mutated, &native_test).is_err());
}

#[test]
fn creation_publication_seams_are_first_and_unique() {
    validate_creation_publication_seams(&repo_text("kernel/src/task/scheduler.rs"))
        .expect("creation publication seams");
}

#[test]
fn negative_creation_publication_after_without_interrupts_is_rejected() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let mutated = scheduler.replacen(
        "pub fn spawn(thread: Box<Thread>) {\n    note_scheduler_publication();",
        "pub fn spawn(thread: Box<Thread>) {\n    without_interrupts(|| {});\n    note_scheduler_publication();",
        1,
    );
    assert_ne!(mutated, scheduler, "publication-order mutation applied");
    assert!(validate_creation_publication_seams(&mutated).is_err());
}

#[test]
fn creation_paths_release_process_manager_before_publication() {
    validate_creation_publications_release_process_manager(&rust_sources_below("kernel/src"))
        .expect("creation publication process-manager guard census");
}

#[test]
fn negative_creation_publication_inside_process_manager_scope_is_rejected() {
    let sources = rust_sources_below("kernel/src");
    let mutated = with_synthetic_source(
        &sources,
        "kernel/src/synthetic_creation_inversion.rs",
        r#"
            fn publish_inside_guard(thread: Box<Thread>) {
                let mut manager_guard = crate::process::manager();
                if let Some(manager) = manager_guard.as_mut() {
                    manager.observe_creation();
                    crate::task::scheduler::spawn(thread);
                }
            }
        "#,
    );
    let error = validate_creation_publications_release_process_manager(&mutated)
        .expect_err("publication under a live process-manager guard escaped the census");
    assert!(
        error.contains("kernel/src/synthetic_creation_inversion.rs :: fn publish_inside_guard"),
        "offender failure did not name its file and item: {error}"
    );
}

#[test]
fn creation_lock_order_marker_and_oracle_reader_are_pinned() {
    validate_creation_lock_order_marker(
        &repo_text("kernel/src/task/scheduler.rs"),
        &repo_text("kernel/src/tracing/providers/teardown.rs"),
    )
    .expect("creation lock-order marker");
}

#[test]
fn negative_creation_marker_reusing_exec_marker_is_rejected() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let teardown = repo_text("kernel/src/tracing/providers/teardown.rs");
    let mutated = scheduler.replacen(
        "[CREATION_LOCK_ORDER:VIOLATION:PM_HELD]",
        "[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]",
        1,
    );
    assert_ne!(mutated, scheduler, "creation marker reuse mutation applied");
    assert!(validate_creation_lock_order_marker(&mutated, &teardown).is_err());
}

#[test]
fn negative_creation_injection_reusing_production_marker_is_rejected() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let teardown = repo_text("kernel/src/tracing/providers/teardown.rs");
    let mutated = scheduler.replacen(
        "[CREATION_LOCK_ORDER:INJECTED:PM_HELD]",
        "[CREATION_LOCK_ORDER:VIOLATION:PM_HELD]",
        1,
    );
    assert_ne!(
        mutated, scheduler,
        "injection marker reuse mutation applied"
    );
    assert!(validate_creation_lock_order_marker(&mutated, &teardown).is_err());
}

#[test]
fn negative_creation_injection_duplicating_predicate_is_rejected() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let teardown = repo_text("kernel/src/tracing/providers/teardown.rs");
    let mutated = scheduler.replacen(
        "let pm_held = account_creation_publication_pm_held();",
        "let pm_held = crate::process::process_manager_held_on_current_cpu();",
        1,
    );
    assert_ne!(mutated, scheduler, "duplicated predicate mutation applied");
    assert!(validate_creation_lock_order_marker(&mutated, &teardown).is_err());
}

#[test]
fn negative_creation_oracle_without_injected_assertion_is_rejected() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let teardown = repo_text("kernel/src/tracing/providers/teardown.rs");
    let mutated = teardown.replacen(
        "if measurements.sched_pm_held_injected != 1 {",
        "if measurements.sched_pm_held_production != 0 {",
        1,
    );
    assert_ne!(
        mutated, teardown,
        "injected assertion removal mutation applied"
    );
    assert!(validate_creation_lock_order_marker(&scheduler, &mutated).is_err());
}
