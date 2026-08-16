use std::collections::BTreeMap;
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
        || !compact_unpublished_arm.contains("process_memory::switch_to_kernel_page_table();")
        || !compact_unpublished_arm.contains("return;")
    {
        return Err("blocked-in-syscall unpublished-row recovery is not retry-only".to_string());
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
        || !compact_unpublished_arm.contains("return;")
    {
        return Err("normal-restore unpublished-row recovery is not retry-only".to_string());
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

fn validate_clone_publication_lifecycle() -> Result<(), String> {
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

    let clone = repo_text("kernel/src/syscall/clone.rs");
    let clone_body =
        function_body(&clone, "sys_clone").ok_or_else(|| "missing sys_clone body".to_string())?;
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
    if normalized_code(&clone_body[admission..insert])
        .replace(' ', "")
        .contains("drop(manager_guard)")
    {
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
    let spawn_calls = call_offsets(clone_body, &clone_mask, "spawn");
    let publication_steps = [
        ("attach_main_thread_unpublished", attach_calls.as_slice()),
        ("insert_process", insert_calls.as_slice()),
        ("set_ready", set_ready_calls.as_slice()),
        ("ThreadState::Ready", runnable_thread_writes.as_slice()),
        ("drop(manager_guard)", manager_guard_drops.as_slice()),
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

#[test]
fn clone_publication_lifecycle_is_closed() {
    assert_eq!(validate_clone_publication_lifecycle(), Ok(()));
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
