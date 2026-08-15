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
    for forbidden in ["log::", "serial_println!"] {
        if !code_offsets(apply, &apply_mask, forbidden).is_empty() {
            return Err(format!(
                "ExecSchedCommit::apply contains forbidden output path {forbidden}"
            ));
        }
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
    let smoke_calls = code_offsets(main, &main_mask, "run_exec_smoke()");
    if smoke_calls.len() != 1 {
        return Err(format!(
            "init main must call run_exec_smoke exactly once, found {}",
            smoke_calls.len()
        ));
    }
    let wait_stress = code_offsets(main, &main_mask, "run_wait_stress_if_enabled()");
    if wait_stress.len() != 1 || smoke_calls[0] >= wait_stress[0] {
        return Err("init must run the exec smoke before wait stress".to_owned());
    }
    if !main.contains("#[cfg(target_arch = \"aarch64\")]\n    run_exec_smoke();") {
        return Err("init main does not aarch64-gate run_exec_smoke".to_owned());
    }

    let smoke_fn_offset = init_rs
        .find("fn run_exec_smoke(")
        .ok_or_else(|| "init is missing run_exec_smoke".to_owned())?;
    let smoke_cfg = init_rs[..smoke_fn_offset]
        .rfind("#[cfg(target_arch = \"aarch64\")]")
        .ok_or_else(|| "run_exec_smoke is not aarch64-only".to_owned())?;
    if init_rs[smoke_cfg..smoke_fn_offset].contains("fn ") {
        return Err("run_exec_smoke is not directly guarded for aarch64".to_owned());
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

#[test]
fn exec_smoke_is_wired_into_the_aarch64_boot_path() {
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
    let mutated = scheduler.replacen(
        "let pm_held = crate::process::process_manager_held_on_current_cpu();",
        "let pm_held = false;",
        1,
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
fn negative_full_gate_without_exec_smoke_target_grep_is_rejected() {
    let executor = repo_text("kernel/src/test_framework/executor.rs");
    let full_test = repo_text("docker/qemu/run-aarch64-full-test.sh");
    let native_test = repo_text("docker/qemu/run-aarch64-boot-test-native.sh");
    let mutated = full_test.replacen("\\[EXEC_SMOKE:TARGET_OK\\]", "\\[EXEC_SMOKE:REMOVED\\]", 1);
    assert_ne!(mutated, full_test, "exec smoke grep mutation applied");
    assert!(validate_boot_verdict_and_gate_scripts(&executor, &mutated, &native_test).is_err());
}
