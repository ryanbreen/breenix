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
