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
    let (open, close) = braced_block_span(source, mask, start)?;
    Some(&source[open..=close])
}

fn braced_block_span(source: &str, mask: &[bool], start: usize) -> Option<(usize, usize)> {
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
                    return Some((open, index));
                }
            }
            _ => {}
        }
    }
    None
}

fn function_open_brace(source: &str, mask: &[bool], start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    for index in start..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'(' => parentheses += 1,
            b')' => parentheses = parentheses.checked_sub(1)?,
            b'[' => brackets += 1,
            b']' => brackets = brackets.checked_sub(1)?,
            b'{' if parentheses == 0 && brackets == 0 => return Some(index),
            b';' if parentheses == 0 && brackets == 0 => return None,
            _ => {}
        }
    }
    None
}

#[derive(Debug)]
struct FunctionSpan {
    name: String,
    open: usize,
    close: usize,
}

fn function_spans(source: &str) -> Vec<FunctionSpan> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    for function in identifier_offsets(source, &mask, "fn") {
        let mut cursor = function + 2;
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
        let Some(brace) = function_open_brace(source, &mask, cursor) else {
            continue;
        };
        let Some((open, close)) = braced_block_span(source, &mask, brace) else {
            continue;
        };
        spans.push(FunctionSpan {
            name: source[name_start..cursor].to_string(),
            open,
            close,
        });
    }
    spans
}

fn function_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    function_spans(source)
        .into_iter()
        .find(|span| span.name == name)
        .map(|span| &source[span.open..=span.close])
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

fn has_identifier(source: &str, identifier: &str) -> bool {
    !identifier_offsets(source, &code_mask(source), identifier).is_empty()
}

fn code_text_offset(source: &str, text: &str) -> Option<usize> {
    let mask = code_mask(source);
    source.match_indices(text).find_map(|(offset, _)| {
        mask[offset..offset + text.len()]
            .iter()
            .all(|code| *code)
            .then_some(offset)
    })
}

fn cfg_arch_block<'a>(scope: &'a str, arch: &str) -> Option<&'a str> {
    let mask = code_mask(scope);
    let bytes = scope.as_bytes();
    for target_arch in identifier_offsets(scope, &mask, "target_arch") {
        let attribute_end =
            (target_arch..bytes.len()).find(|index| mask[*index] && bytes[*index] == b']')?;
        if !scope[target_arch..=attribute_end].contains(&format!("\"{arch}\"")) {
            continue;
        }
        let block_start =
            (attribute_end + 1..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{')?;
        return braced_block(scope, &mask, block_start);
    }
    None
}

fn test_def_block<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let name_offset = source.find(&format!("name: \"{name}\""))?;
    let mask = code_mask(source);
    let test_def = identifier_offsets(&source[..name_offset], &mask[..name_offset], "TestDef")
        .into_iter()
        .last()?;
    braced_block(source, &mask, test_def + "TestDef".len())
}

fn compact_code(fragment: &str) -> String {
    normalized_code(fragment)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn compact_whitespace(fragment: &str) -> String {
    fragment
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn loop_body_after_anchor<'a>(source: &'a str, anchor: &str) -> Option<&'a str> {
    let anchor_offset = source.find(anchor)?;
    let mask = code_mask(source);
    let loop_offset = identifier_offsets(source, &mask, "loop")
        .into_iter()
        .find(|offset| *offset > anchor_offset + anchor.len())?;
    braced_block(source, &mask, loop_offset + "loop".len())
}

fn remove_call_from_loop_after_anchor(source: &str, anchor: &str, call: &str) -> Option<String> {
    let anchor_offset = source.find(anchor)?;
    let mask = code_mask(source);
    let loop_offset = identifier_offsets(source, &mask, "loop")
        .into_iter()
        .find(|offset| *offset > anchor_offset + anchor.len())?;
    let (open, close) = braced_block_span(source, &mask, loop_offset + "loop".len())?;
    let call_offset = source[open..=close].find(call)? + open;
    let mut mutated = source.to_string();
    mutated.replace_range(call_offset..call_offset + call.len(), "");
    Some(mutated)
}

fn kernel_source_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read kernel source {}: {error}", directory.display()))
    {
        let path = entry.expect("read kernel source directory entry").path();
        if path.is_dir() {
            kernel_source_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn kernel_sources() -> Vec<(String, String)> {
    let root = repo_root();
    let mut paths = Vec::new();
    kernel_source_files(&root.join("kernel/src"), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .expect("kernel source remains under repository root")
                .display()
                .to_string();
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read kernel source {}: {error}", path.display()));
            (relative, source)
        })
        .collect()
}

fn joined_sources(sources: impl Iterator<Item = String>) -> String {
    sources.collect::<Vec<_>>().join("\n")
}

fn net_source_text() -> String {
    joined_sources(
        kernel_sources()
            .into_iter()
            .filter(|(path, _)| {
                path == "kernel/src/net/mod.rs" || path.starts_with("kernel/src/net/")
            })
            .map(|(_, source)| source),
    )
}

fn kernel_source_text() -> String {
    joined_sources(kernel_sources().into_iter().map(|(_, source)| source))
}

fn inject_into_function(source: &str, name: &str, code: &str) -> String {
    let span = function_spans(source)
        .into_iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("find function {name} in fixture"));
    let mut mutated = source.to_string();
    mutated.insert_str(span.open + 1, code);
    mutated
}

fn inject_into_block_after_identifier(
    source: &str,
    function_name: &str,
    identifier: &str,
    code: &str,
) -> String {
    let span = function_spans(source)
        .into_iter()
        .find(|span| span.name == function_name)
        .unwrap_or_else(|| panic!("find function {function_name} in fixture"));
    let body = &source[span.open..=span.close];
    let mask = code_mask(body);
    let identifier = identifier_offsets(body, &mask, identifier)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("find {identifier} in {function_name}"));
    let (open, _) = braced_block_span(body, &mask, identifier)
        .unwrap_or_else(|| panic!("find block after {identifier} in {function_name}"));
    let mut mutated = source.to_string();
    mutated.insert_str(span.open + open + 1, code);
    mutated
}

fn validate_pump_producer_seam_is_single(source: &str) -> Result<(), String> {
    let producers: Vec<_> = function_spans(source)
        .into_iter()
        .filter(|span| {
            let body = &source[span.open..=span.close];
            has_identifier(body, "LOOPBACK_QUEUE") && normalized_code(body).contains(".push(")
        })
        .collect();
    if producers.len() != 1 {
        return Err(format!(
            "expected one LOOPBACK_QUEUE producer, found {}",
            producers.len()
        ));
    }
    let body = &source[producers[0].open..=producers[0].close];
    if !normalized_code(body).contains("wake_loopback_pump()") {
        return Err("loopback producer does not wake kloopbackd".to_string());
    }
    Ok(())
}

fn validate_pump_wake_is_the_only_pump_wake_path(source: &str) -> Result<(), String> {
    let wake = function_body(source, "wake_loopback_pump")
        .ok_or_else(|| "missing wake_loopback_pump".to_string())?;
    if !has_identifier(wake, "wake_thread_any_context") {
        return Err("wake_loopback_pump does not call wake_thread_any_context".to_string());
    }
    for span in function_spans(source) {
        if span.name == "wake_loopback_pump" {
            continue;
        }
        let body = &source[span.open..=span.close];
        if has_identifier(body, "wake_thread_any_context")
            && has_identifier(body, "LOOPBACK_PUMP_TID")
        {
            return Err(format!("{} is a second pump wake path", span.name));
        }
    }
    Ok(())
}

fn validate_any_context_wake_never_takes_scheduler_lock_in_interrupt_context(
    source: &str,
) -> Result<(), String> {
    let wake = function_body(source, "wake_thread_any_context")
        .ok_or_else(|| "missing wake_thread_any_context".to_string())?;
    let mask = code_mask(wake);
    let interrupt = identifier_offsets(wake, &mask, "in_interrupt")
        .into_iter()
        .next()
        .ok_or_else(|| "wake_thread_any_context has no in_interrupt arm".to_string())?;
    let arm = braced_block(wake, &mask, interrupt)
        .ok_or_else(|| "cannot parse in_interrupt arm".to_string())?;
    for forbidden in ["with_scheduler", "lock_scheduler", "SCHEDULER"] {
        if has_identifier(arm, forbidden) {
            return Err(format!("interrupt wake arm references {forbidden}"));
        }
    }
    Ok(())
}

fn validate_wake_waiting_threads_is_arch_neutral(source: &str) -> Result<(), String> {
    let wake = function_body(source, "wake_waiting_threads")
        .ok_or_else(|| "missing wake_waiting_threads".to_string())?;
    if has_identifier(wake, "target_arch") {
        return Err("wake_waiting_threads contains target_arch cfg".to_string());
    }
    if !has_identifier(wake, "wake_thread_any_context") {
        return Err("wake_waiting_threads does not call wake_thread_any_context".to_string());
    }
    Ok(())
}

fn validate_waiter_lists_are_not_drained_before_the_wake(source: &str) -> Result<(), String> {
    for name in ["wake_connection_waiters", "wake_accept_waiters"] {
        let body = function_body(source, name).ok_or_else(|| format!("missing {name}"))?;
        if normalized_code(body).contains(".drain(") {
            return Err(format!("{name} drains its waiter list before wake"));
        }
    }
    Ok(())
}

fn validate_tcp_accept_inserts_before_pending_removal(source: &str) -> Result<(), String> {
    let accept =
        function_body(source, "tcp_accept").ok_or_else(|| "missing tcp_accept".to_string())?;
    let insert = code_text_offset(accept, "connections.insert(conn_id, conn)")
        .ok_or_else(|| "tcp_accept does not insert the accepted connection".to_string())?;
    let removal = code_text_offset(accept, "listener.pending.remove(position)")
        .ok_or_else(|| "tcp_accept does not remove the matched pending entry".to_string())?;

    if insert >= removal {
        return Err("tcp_accept removes the pending entry before connection insertion".to_string());
    }
    if code_text_offset(&accept[..insert], ".pop_front()").is_some() {
        return Err("tcp_accept pops a pending entry before connection insertion".to_string());
    }
    Ok(())
}

fn validate_tcp_accept_claims_pending_in_peek_closure(source: &str) -> Result<(), String> {
    let accept =
        function_body(source, "tcp_accept").ok_or_else(|| "missing tcp_accept".to_string())?;
    let selection_start = code_text_offset(accept, "let pending = with_tcp_listeners(|listeners|")
        .ok_or_else(|| "tcp_accept does not peek under the listeners lock".to_string())?;
    let mask = code_mask(&accept);
    let (selection_open, selection_close) = braced_block_span(&accept, &mask, selection_start)
        .ok_or_else(|| "cannot parse tcp_accept pending-selection closure".to_string())?;
    let selection = &accept[selection_open..=selection_close];
    let normalized = compact_code(selection);

    for required in [
        "listener.pending.iter_mut()",
        ".find(|pending|!pending.claimed)",
        "pending.claimed=true",
        "pending.clone()",
    ] {
        if !normalized.contains(required) {
            return Err(format!(
                "tcp_accept pending-selection closure is missing {required}"
            ));
        }
    }
    Ok(())
}

fn validate_general_idle_loops_drain_loopback(
    x86_source: &str,
    aarch64_source: &str,
) -> Result<(), String> {
    let x86_idle = function_body(x86_source, "idle_thread_fn")
        .ok_or_else(|| "missing x86 idle_thread_fn".to_string())?;
    if !has_identifier(x86_idle, "drain_loopback_from_idle") {
        return Err("x86 idle_thread_fn does not use the idle drain seam".to_string());
    }

    for (anchor, description) in [
        (
            r#"serial_print!("breenix> ");"#,
            "aarch64 testing boot-thread idle loop",
        ),
        (
            r#"serial_println!("[interactive] No userspace init — idling");"#,
            "aarch64 no-userspace boot-thread idle loop",
        ),
    ] {
        let body = loop_body_after_anchor(aarch64_source, anchor)
            .ok_or_else(|| format!("missing {description}"))?;
        if !body.contains(r#"core::arch::asm!("wfi", options(nomem, nostack))"#) {
            return Err(format!("{description} is not a wfi loop"));
        }
        if !has_identifier(body, "drain_loopback_from_idle") {
            return Err(format!("{description} does not use the idle drain seam"));
        }
    }
    Ok(())
}

fn validate_drain_exclusion_is_a_typed_guard(source: &str) -> Result<(), String> {
    let drain = function_body(source, "drain_loopback_rounds")
        .ok_or_else(|| "missing drain_loopback_rounds".to_string())?;
    if has_identifier(drain, "AtomicBool") {
        return Err("drain_loopback_rounds uses AtomicBool exclusion".to_string());
    }
    if code_text_offset(source, "struct LoopbackDrainGuard").is_none() {
        return Err("missing LoopbackDrainGuard type".to_string());
    }
    let implementation = code_text_offset(source, "impl Drop for LoopbackDrainGuard")
        .ok_or_else(|| "missing LoopbackDrainGuard Drop implementation".to_string())?;
    let drop_body = function_body(&source[implementation..], "drop")
        .ok_or_else(|| "missing LoopbackDrainGuard::drop".to_string())?;
    if !has_identifier(drop_body, "LOOPBACK_DRAIN_OWNER")
        || !has_identifier(drop_body, "compare_exchange")
        || !has_identifier(drop_body, "owner")
        || !has_identifier(drop_body, "Release")
    {
        return Err("LoopbackDrainGuard::drop does not release its own owner ticket".to_string());
    }
    Ok(())
}

fn validate_drain_guard_scope_excludes_delivery(source: &str) -> Result<(), String> {
    let take = function_body(source, "take_queued_loopback_packets")
        .ok_or_else(|| "missing take_queued_loopback_packets".to_string())?;
    for required in [
        "LoopbackDrainGuard",
        "LOOPBACK_QUEUE",
        "LOOPBACK_QUEUE_DEPTH",
        "take",
    ] {
        if !has_identifier(take, required) {
            return Err(format!("queue-take window is missing {required}"));
        }
    }
    for forbidden in ["handle_ipv4", "drain_deferred_tx", "tcp", "log"] {
        if has_identifier(take, forbidden) {
            return Err(format!(
                "queue-take window contains delivery/logging identifier {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_no_force_release_of_the_drain_owner(source: &str) -> Result<(), String> {
    const COMPARE: &str = "LOOPBACK_DRAIN_OWNER.compare_exchange(";
    const STORE: &str = "LOOPBACK_DRAIN_OWNER.store(";

    let implementation = code_text_offset(source, "impl Drop for LoopbackDrainGuard")
        .ok_or_else(|| "missing LoopbackDrainGuard Drop implementation".to_string())?;
    let approved_drop = function_spans(&source[implementation..])
        .into_iter()
        .find(|span| span.name == "drop")
        .ok_or_else(|| "missing LoopbackDrainGuard::drop".to_string())?;
    let approved_drop_open = implementation + approved_drop.open;

    for span in function_spans(source) {
        let body = &source[span.open..=span.close];
        if span.open == approved_drop_open {
            continue;
        }

        let compact = compact_code(body);
        let mut cursor = 0usize;
        while let Some(relative) = compact[cursor..].find(COMPARE) {
            let arguments_start = cursor + relative + COMPARE.len();
            let arguments_end = compact[arguments_start..]
                .find(')')
                .map(|end| arguments_start + end)
                .ok_or_else(|| format!("{} has an unclosed owner CAS", span.name))?;
            let arguments: Vec<_> = compact[arguments_start..arguments_end].split(',').collect();
            if arguments.get(1).copied() == Some("0") {
                return Err(format!("{} force-releases LOOPBACK_DRAIN_OWNER", span.name));
            }
            cursor = arguments_end + 1;
        }

        if let Some(store) = compact.find(STORE) {
            let value = &compact[store + STORE.len()..];
            if value.starts_with("0,") || value.starts_with("0)") {
                return Err(format!("{} stores zero to LOOPBACK_DRAIN_OWNER", span.name));
            }
        }
    }
    Ok(())
}

fn validate_pump_does_not_halt_while_work_remains(source: &str) -> Result<(), String> {
    let pump = function_body(source, "loopback_pump_fn")
        .ok_or_else(|| "missing loopback_pump_fn".to_string())?;
    let more = code_text_offset(pump, "if more")
        .ok_or_else(|| "loopback_pump_fn has no more-work branch".to_string())?;
    let branch = braced_block(pump, &code_mask(pump), more)
        .ok_or_else(|| "cannot parse loopback pump more-work branch".to_string())?;
    if has_identifier(branch, "arch_halt_with_interrupts") {
        return Err("loopback pump halts while work remains".to_string());
    }
    if !has_identifier(branch, "yield_current") || !has_identifier(branch, "continue") {
        return Err("loopback pump more-work branch must yield and continue".to_string());
    }
    Ok(())
}

fn validate_net_lock_guard_disables_bottom_halves_not_softirq_execution(
    source: &str,
) -> Result<(), String> {
    let acquire = function_body(source, "net_lock_guard")
        .ok_or_else(|| "missing net_lock_guard".to_string())?;
    let acquire_x86 = cfg_arch_block(acquire, "x86_64")
        .ok_or_else(|| "missing x86_64 net_lock_guard arm".to_string())?;
    if !has_identifier(acquire_x86, "bh_disable") || has_identifier(acquire_x86, "softirq_enter") {
        return Err("x86 net_lock_guard does not use pure BH disable".to_string());
    }

    let implementation = code_text_offset(source, "impl Drop for NetLockGuard")
        .ok_or_else(|| "missing NetLockGuard Drop implementation".to_string())?;
    let drop = function_body(&source[implementation..], "drop")
        .ok_or_else(|| "missing NetLockGuard::drop".to_string())?;
    let drop_x86 = cfg_arch_block(drop, "x86_64")
        .ok_or_else(|| "missing x86_64 NetLockGuard::drop arm".to_string())?;
    if !has_identifier(drop_x86, "bh_enable") || has_identifier(drop_x86, "softirq_exit") {
        return Err("x86 NetLockGuard::drop does not use pure BH enable".to_string());
    }
    Ok(())
}

fn validate_any_context_wake_dispatches_on_execution_context(source: &str) -> Result<(), String> {
    let wake = function_body(source, "wake_thread_any_context")
        .ok_or_else(|| "missing wake_thread_any_context".to_string())?;
    if !has_identifier(wake, "in_interrupt") {
        return Err("wake_thread_any_context does not use in_interrupt".to_string());
    }
    for forbidden in ["in_softirq", "softirq_count"] {
        if has_identifier(wake, forbidden) {
            return Err(format!(
                "wake_thread_any_context branches on wide predicate {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_generic_unblock_keeps_child_exit_dedicated(source: &str) -> Result<(), String> {
    let unblock =
        function_body(source, "unblock").ok_or_else(|| "missing Scheduler::unblock".to_string())?;
    for state in [
        "Blocked",
        "BlockedOnSignal",
        "BlockedOnTimer",
        "BlockedOnIO",
    ] {
        if !has_identifier(unblock, state) {
            return Err(format!("Scheduler::unblock does not handle {state}"));
        }
    }
    if has_identifier(unblock, "BlockedOnChildExit") {
        return Err("Scheduler::unblock wakes the dedicated child-exit state".to_string());
    }
    Ok(())
}

fn validate_io_wake_buffers_before_thread_context_overflow_fallback(
    source: &str,
) -> Result<(), String> {
    let io_wake = function_body(source, "isr_unblock_for_io")
        .ok_or_else(|| "missing isr_unblock_for_io".to_string())?;
    if compact_code(io_wake) != "{let_=buffer_isr_wakeup(tid);set_need_resched();}" {
        return Err(
            "isr_unblock_for_io must buffer and then request rescheduling unconditionally"
                .to_string(),
        );
    }

    let buffer = function_body(source, "buffer_isr_wakeup")
        .ok_or_else(|| "missing buffer_isr_wakeup".to_string())?;
    let full_offset = code_text_offset(buffer, "IsrWakePush::Full")
        .ok_or_else(|| "buffer helper has no full-buffer arm".to_string())?;
    let full = braced_block(buffer, &code_mask(buffer), full_offset)
        .ok_or_else(|| "cannot parse full-buffer arm".to_string())?;
    if !compact_code(full).contains("if!crate::per_cpu::in_interrupt()") {
        return Err("full-buffer fallback is not restricted to thread context".to_string());
    }

    let interrupt_offset = identifier_offsets(full, &code_mask(full), "in_interrupt")
        .into_iter()
        .next()
        .ok_or_else(|| "full-buffer arm does not inspect interrupt context".to_string())?;
    let thread_context = braced_block(full, &code_mask(full), interrupt_offset)
        .ok_or_else(|| "cannot parse thread-context overflow arm".to_string())?;
    for required in ["with_scheduler", "unblock", "Applied", "AlreadyRunnable"] {
        if !has_identifier(thread_context, required) {
            return Err(format!("thread-context overflow arm is missing {required}"));
        }
    }
    if has_identifier(thread_context, "ENQUEUE_ISR_BUFFER_FULL") {
        return Err("thread-context overflow is counted as an ISR drop".to_string());
    }

    let else_offset = code_text_offset(full, "else")
        .ok_or_else(|| "full-buffer arm has no real-interrupt rejection branch".to_string())?;
    let interrupt_context = braced_block(full, &code_mask(full), else_offset)
        .ok_or_else(|| "cannot parse real-interrupt overflow arm".to_string())?;
    for required in ["ENQUEUE_ISR_BUFFER_FULL", "Rejected"] {
        if !has_identifier(interrupt_context, required) {
            return Err(format!("real-interrupt overflow arm is missing {required}"));
        }
    }
    if has_identifier(interrupt_context, "with_scheduler") {
        return Err("real-interrupt overflow arm takes the scheduler lock".to_string());
    }
    Ok(())
}

fn validate_loopback_regression_tests_are_arch_neutral(source: &str) -> Result<(), String> {
    for name in [
        "loopback_recv_wake_when_idle",
        "loopback_recv_wake_under_load",
        "loopback_pump_does_not_busy_spin",
        "loopback_wake_loss_counters_are_zero",
        "tcp_final_ack_survives_accept_publish_race",
    ] {
        let definition =
            test_def_block(source, name).ok_or_else(|| format!("missing TestDef for {name}"))?;
        if !compact_code(definition).contains("arch:Arch::Any") {
            return Err(format!("{name} is not Arch::Any"));
        }
    }
    Ok(())
}

fn validate_x86_gate_requires_the_loopback_regression_tests(source: &str) -> Result<(), String> {
    if !source.contains("for _ in $(seq 1 900); do") {
        return Err("x86 gate does not allow 900 seconds for late registry tests".to_string());
    }
    for explanation in ["registry", "userspace programs", "slow-but-healthy"] {
        if !source[..source.find("set -euo pipefail").unwrap_or(0)].contains(explanation) {
            return Err(format!(
                "x86 gate header does not explain the 900-second bound ({explanation})"
            ));
        }
    }
    let required_name = "loopback_recv_wake";
    let required_marker = format!("\\[TEST:userspace:{required_name}:PASS\\]");
    if source.matches(&required_marker).count() != 2 {
        return Err(format!(
            "x86 gate does not poll and assert exactly-once evidence for {required_name}"
        ));
    }
    if !source.contains(&format!("grep -q '{required_marker}'")) {
        return Err(format!("x86 gate does not poll for {required_name}"));
    }
    let count_anchor = format!("grep -h -c '{required_marker}'");
    let count = source
        .find(&count_anchor)
        .ok_or_else(|| format!("x86 gate lost the exact PASS count for {required_name}"))?;
    let count_tail = &source[count..];
    let next_count = count_tail[1..]
        .find("test \"$(grep -h -c '")
        .map_or(count_tail.len(), |offset| offset + 1);
    if !count_tail[..next_count].contains("-eq 1") {
        return Err(format!(
            "x86 gate weakened the exactly-once PASS count for {required_name}"
        ));
    }
    if source.contains("\\[TEST:network:loopback_wake_loss_counters_are_zero:PASS\\]") {
        return Err("x86 gate still treats the boot-window counter marker as proof".to_string());
    }
    // Re-pinned per delta by P6a PR-2 (review finding F5): the tombstone join
    // oracle's fixture stages two rows that reach the real zombie state through
    // `terminate_minimal`, which is the tally's choke point, and the same
    // re-derivation closed a pre-existing one-row gap — the enumeration read 101
    // while the gate's own runs measured 102, because the RING3_SMOKE
    // `smoke_hello_time` process was never counted. 102 + 2 = 104.
    //
    // Re-pinned again to 105 by the #737 DF-oracle branch: `df_preempt_oracle`
    // joined the RING3_SMOKE roster as one more launched program that runs to
    // completion and exits once, which moves this floor by exactly 1. The gate
    // script carries the same derivation beside its own constant. This literal
    // and that one are the two halves of one pin, and when the branch moved the
    // script it left this half at 104, which is the finding this edit closes.
    // claim-lint:ok: 1 of 1 `readonly EXPECTED_USERSPACE_EXITS=` assignment in
    // docker/qemu/run-x86-boot-tests.sh reads 105 at this head, and the
    // mutation leg `x86_gate_validator_rejects_lower_userspace_exit_pin` below
    // reddens this validator by rewriting exactly that assignment. #737.
    if source
        .matches("readonly EXPECTED_USERSPACE_EXITS=105")
        .count()
        != 1
    {
        return Err("x86 gate does not pin the 105 expected userspace exits".to_string());
    }
    for rationale in [
        "B1",
        "bonus, not the proof",
        "before any user process exists",
        "three of its four counters are structurally zero",
        "cannot go red",
        "#545 regression",
        "mid-stream stall",
    ] {
        if !source.contains(rationale) {
            return Err(format!("x86 gate lost pinned rationale ({rationale})"));
        }
    }
    for name in [
        "loopback_recv_wake_when_idle",
        "loopback_recv_wake_under_load",
        "loopback_pump_does_not_busy_spin",
        "tcp_final_ack_survives_accept_publish_race",
    ] {
        let marker = format!("\\[TEST:network:{name}:PASS\\]");
        if source.contains(&marker) {
            return Err(format!("x86 gate requires deferred scheduling test {name}"));
        }
    }
    for explanation in [
        "loopback_recv_wake_when_idle",
        "loopback_recv_wake_under_load",
        "loopback_pump_does_not_busy_spin",
        "tcp_final_ack_survives_accept_publish_race",
        "#567",
    ] {
        if !source.contains(explanation) {
            return Err(format!(
                "x86 gate lost deferred-test rationale ({explanation})"
            ));
        }
    }
    for (failure_marker, description) in [
        ("\\[TEST:network:[^]]*:FAIL", "network"),
        ("\\[TEST:userspace:[^]]*:FAIL", "userspace"),
    ] {
        if source.matches(failure_marker).count() != 2 {
            return Err(format!(
                "x86 gate does not reject {description} test failures during and after polling"
            ));
        }
    }
    Ok(())
}

fn validate_x86_direct_loopback_gates(main: &str, registry: &str) -> Result<(), String> {
    let kernel_main = function_body(main, "kernel_main_on_kernel_stack")
        .ok_or_else(|| "missing kernel_main_on_kernel_stack".to_string())?;
    let cohort_call = kernel_main
        .find("teardown::run_x86_retire_cohort_gate();")
        .ok_or_else(|| "missing direct x86 retire-cohort gate call".to_string())?;
    let loopback_call = kernel_main
        .find("test_framework::registry::run_x86_loopback_gates();")
        .ok_or_else(|| "missing direct x86 loopback-gate call".to_string())?;
    if loopback_call <= cohort_call
        || !kernel_main.contains("only the non-scheduling loopback")
        || !kernel_main.contains("#567")
    {
        return Err("x86 loopback gates lost their direct-call order or rationale".to_string());
    }

    let declaration = registry
        .find("pub fn run_x86_loopback_gates")
        .ok_or_else(|| "missing run_x86_loopback_gates wrapper".to_string())?;
    let cfg_start = registry[..declaration]
        .rfind("#[cfg(")
        .ok_or_else(|| "x86 loopback wrapper has no cfg attribute".to_string())?;
    let cfg = registry[cfg_start..declaration]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    if cfg != "#[cfg(all(feature=\"boot_tests\",target_arch=\"x86_64\"))]" {
        return Err("x86 loopback wrapper is not boot_tests+x86_64 gated".to_string());
    }

    let wrapper = function_body(registry, "run_x86_loopback_gates")
        .ok_or_else(|| "cannot parse run_x86_loopback_gates wrapper".to_string())?;
    if wrapper.matches("let result = ").count() != 1 {
        return Err("x86 loopback wrapper does not contain exactly one gate".to_string());
    }
    let name = "loopback_wake_loss_counters_are_zero";
    let call = wrapper
        .find(&format!("let result = {name}();"))
        .ok_or_else(|| format!("x86 loopback wrapper does not call {name}"))?;
    let start = format!("[TEST:network:{name}:START]");
    let pass = format!("[TEST:network:{name}:PASS]");
    let fail = format!("[TEST:network:{name}:FAIL:");
    for marker in [&start, &pass, &fail] {
        if wrapper.matches(marker).count() != 1 || registry.matches(marker).count() != 1 {
            return Err(format!(
                "x86 loopback wrapper is not the sole producer of {marker}"
            ));
        }
    }
    let start_offset = wrapper
        .find(&start)
        .ok_or_else(|| "x86 loopback wrapper lost its START marker".to_string())?;
    let pass_offset = wrapper
        .find(&pass)
        .ok_or_else(|| "x86 loopback wrapper lost its PASS marker".to_string())?;
    let fail_offset = wrapper
        .find(&fail)
        .ok_or_else(|| "x86 loopback wrapper lost its FAIL marker".to_string())?;
    if start_offset >= call
        || call >= pass_offset.min(fail_offset)
        || !wrapper.contains("match result")
    {
        return Err("x86 loopback wrapper lost START then PASS-or-FAIL discipline".to_string());
    }
    if wrapper.contains("panic!(") || wrapper.contains(".unwrap()") || wrapper.contains(".expect(")
    {
        return Err("x86 loopback wrapper may panic instead of emitting FAIL".to_string());
    }

    let body = function_body(registry, name)
        .ok_or_else(|| format!("missing loopback gate body {name}"))?;
    if body.contains(&format!("[TEST:network:{name}:")) {
        return Err(format!("{name} body emits its own x86 marker"));
    }
    for name in [
        "loopback_recv_wake_when_idle",
        "loopback_recv_wake_under_load",
        "loopback_pump_does_not_busy_spin",
        "tcp_final_ack_survives_accept_publish_race",
    ] {
        if wrapper.contains(&format!("{name}()"))
            || wrapper.contains(&format!("[TEST:network:{name}:"))
        {
            return Err(format!("x86 loopback wrapper calls excluded gate {name}"));
        }
    }

    let wrapper_docs = &registry[declaration.saturating_sub(1200)..declaration];
    for rationale in [
        "Four `Arch::Any`",
        "loopback_recv_wake_when_idle",
        "INSTRUCTION_FETCH",
        "0x0",
        "loopback_recv_wake_under_load",
        "0x100002590aa",
        "loopback_pump_does_not_busy_spin",
        "data address",
        "tcp_final_ack_survives_accept_publish_race",
        "0x4444446053e6",
        "Any test that schedules in this window currently poisons the x86 boot",
        "does no scheduling",
        "#567",
    ] {
        if !wrapper_docs.contains(rationale) {
            return Err(format!(
                "x86 loopback wrapper docs lost rationale {rationale}"
            ));
        }
    }

    validate_loopback_regression_tests_are_arch_neutral(registry)
}

fn validate_x86_userspace_phase_launches_loopback_wake_test(source: &str) -> Result<(), String> {
    let kernel_main_continue = function_body(source, "kernel_main_continue")
        .ok_or_else(|| "missing kernel_main_continue".to_string())?;
    let compact = compact_whitespace(kernel_main_continue);
    let load = "kernel::userspace_test::get_test_binary(\"loopback_wake_test\")";
    if compact.matches(load).count() != 1 {
        return Err("kernel_main_continue must load loopback_wake_test exactly once".to_string());
    }
    let launch = "process::creation::create_user_process(String::from(\"loopback_wake_test\")";
    if compact.matches(launch).count() != 1 {
        return Err("kernel_main_continue must launch loopback_wake_test exactly once".to_string());
    }
    Ok(())
}

fn validate_schedule_rearms_a_blocked_pump(source: &str) -> Result<(), String> {
    let schedule = function_body(source, "schedule")
        .ok_or_else(|| "missing Scheduler::schedule".to_string())?;
    if !has_identifier(schedule, "loopback_queue_has_work") {
        return Err("Scheduler::schedule does not check for loopback work".to_string());
    }
    if !has_identifier(schedule, "loopback_pump_tid") {
        return Err("Scheduler::schedule does not load the loopback pump tid".to_string());
    }
    if !normalized_code(schedule).contains("self.unblock(pump_tid)") {
        return Err("Scheduler::schedule does not re-arm the blocked pump".to_string());
    }
    Ok(())
}

fn validate_wakeup_placement_is_bounded_by_online_cpus(source: &str) -> Result<(), String> {
    for name in ["find_target_cpu_for_wakeup", "least_loaded_cpu"] {
        let body = function_body(source, name).ok_or_else(|| format!("missing {name}"))?;
        let compact = compact_code(body);
        if !has_identifier(body, "online_cpu_count") {
            return Err(format!("{name} does not reference online_cpu_count"));
        }
        if !has_identifier(body, "cpu_accepts_wakeups") {
            return Err(format!("{name} does not consult CPU scheduling liveness"));
        }
        if !compact.contains(
            "(0..self.online_cpu_count()).filter(|&cpu|self.cpu_accepts_wakeups(cpu)).min_by_key(",
        ) {
            return Err(format!(
                "{name} does not select from scheduling CPUs in the online range"
            ));
        }
        if compact.contains("(0..MAX_CPUS).min_by_key(") {
            return Err(format!("{name} selects from all MAX_CPUS queues"));
        }
    }
    Ok(())
}

fn validate_scheduling_paths_reclaim_unschedulable_cpu_queues(source: &str) -> Result<(), String> {
    for name in ["schedule", "schedule_deferred_requeue"] {
        let body = function_body(source, name).ok_or_else(|| format!("missing {name}"))?;
        if !compact_code(body).contains("self.reclaim_unschedulable_cpu_queues()") {
            return Err(format!("{name} does not reclaim unschedulable CPU queues"));
        }
    }
    Ok(())
}

#[test]
fn pump_producer_seam_is_single() {
    validate_pump_producer_seam_is_single(&net_source_text()).expect("single producer seam");
}

#[test]
fn pump_producer_seam_validator_rejects_second_producer() {
    let source = net_source_text();
    let mutated = format!(
        "{source}\nfn rogue_producer() {{ let mut queue = LOOPBACK_QUEUE.lock(); queue.push(packet); }}"
    );
    assert!(validate_pump_producer_seam_is_single(&mutated).is_err());
}

#[test]
fn pump_wake_is_the_only_pump_wake_path() {
    validate_pump_wake_is_the_only_pump_wake_path(&kernel_source_text())
        .expect("single pump wake path");
}

#[test]
fn pump_wake_validator_rejects_second_wake_path() {
    let source = kernel_source_text();
    let mutated = format!(
        "{source}\nfn rogue_wake() {{ let tid = LOOPBACK_PUMP_TID.load(Ordering::Acquire); wake_thread_any_context(tid); }}"
    );
    assert!(validate_pump_wake_is_the_only_pump_wake_path(&mutated).is_err());
}

#[test]
fn any_context_wake_never_takes_the_scheduler_lock_in_interrupt_context() {
    validate_any_context_wake_never_takes_scheduler_lock_in_interrupt_context(&repo_text(
        "kernel/src/task/scheduler.rs",
    ))
    .expect("interrupt arm remains lock-free");
}

#[test]
fn any_context_wake_validator_rejects_interrupt_scheduler_lock() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    let mutated = inject_into_block_after_identifier(
        &source,
        "wake_thread_any_context",
        "in_interrupt",
        "with_scheduler(|scheduler| scheduler.unblock(tid));",
    );
    assert!(
        validate_any_context_wake_never_takes_scheduler_lock_in_interrupt_context(&mutated)
            .is_err()
    );
}

#[test]
fn wake_waiting_threads_is_arch_neutral() {
    validate_wake_waiting_threads_is_arch_neutral(&repo_text("kernel/src/net/tcp.rs"))
        .expect("TCP wake remains arch-neutral");
}

#[test]
fn wake_waiting_threads_validator_rejects_arch_cfg() {
    let source = repo_text("kernel/src/net/tcp.rs");
    let mutated = inject_into_function(
        &source,
        "wake_waiting_threads",
        "#[cfg(target_arch = \"aarch64\")] { return Vec::new(); }",
    );
    assert!(validate_wake_waiting_threads_is_arch_neutral(&mutated).is_err());
}

#[test]
fn waiter_lists_are_not_drained_before_the_wake() {
    validate_waiter_lists_are_not_drained_before_the_wake(&repo_text("kernel/src/net/tcp.rs"))
        .expect("waiters remain registered until wake acceptance");
}

#[test]
fn waiter_list_validator_rejects_pre_wake_drain() {
    let source = repo_text("kernel/src/net/tcp.rs");
    let mutated = inject_into_function(
        &source,
        "wake_connection_waiters",
        "conn.waiting_threads.lock().drain(..);",
    );
    assert!(validate_waiter_lists_are_not_drained_before_the_wake(&mutated).is_err());
}

#[test]
fn tcp_accept_inserts_before_pending_removal() {
    validate_tcp_accept_inserts_before_pending_removal(&repo_text("kernel/src/net/tcp.rs"))
        .expect("accepted connection is visible before its pending entry is removed");
}

#[test]
fn tcp_accept_order_validator_rejects_remove_before_insert() {
    let source = repo_text("kernel/src/net/tcp.rs");
    let accept = function_body(&source, "tcp_accept").expect("find tcp_accept fixture");
    let mask = code_mask(accept);

    let insert_start = code_text_offset(accept, "with_tcp_connections(|connections|")
        .expect("find connection insertion fixture");
    let (_, insert_close) =
        braced_block_span(accept, &mask, insert_start).expect("parse connection insertion fixture");
    let insert_end = insert_close
        + accept[insert_close..]
            .find(';')
            .expect("find connection insertion terminator")
        + 1;

    let removal_start = code_text_offset(accept, "let final_pending = with_tcp_listeners")
        .expect("find pending removal fixture");
    let (_, removal_close) =
        braced_block_span(accept, &mask, removal_start).expect("parse pending removal fixture");
    let removal_end = removal_close
        + accept[removal_close..]
            .find(';')
            .expect("find pending removal terminator")
        + 1;

    assert!(
        insert_end <= removal_start,
        "fixture must insert before removing"
    );
    let mutated_accept = format!(
        "{}{}{}{}{}",
        &accept[..insert_start],
        &accept[removal_start..removal_end],
        &accept[insert_end..removal_start],
        &accept[insert_start..insert_end],
        &accept[removal_end..]
    );
    let mutated = source.replacen(accept, &mutated_accept, 1);
    assert_ne!(mutated, source, "fixture mutation must apply");
    assert!(validate_tcp_accept_inserts_before_pending_removal(&mutated).is_err());
}

#[test]
fn tcp_accept_claims_pending_in_peek_closure() {
    validate_tcp_accept_claims_pending_in_peek_closure(&repo_text("kernel/src/net/tcp.rs")).expect(
        "tcp_accept claims an unclaimed pending entry while peeking under the listener lock",
    );
}

#[test]
fn tcp_accept_claim_validator_rejects_claim_outside_listener_lock() {
    let source = repo_text("kernel/src/net/tcp.rs");
    let without_claim = source.replacen("pending.claimed = true;", "", 1);
    assert_ne!(without_claim, source, "fixture mutation must apply");
    let marker = "let copied_early_data_len = pending.early_data.len();";
    let moved_claim = without_claim.replacen(
        marker,
        "let mut pending = pending;\n    pending.claimed = true;\n    let copied_early_data_len = pending.early_data.len();",
        1,
    );
    assert_ne!(moved_claim, without_claim, "fixture mutation must apply");
    assert!(validate_tcp_accept_claims_pending_in_peek_closure(&moved_claim).is_err());
}

#[test]
fn general_idle_loops_drain_loopback() {
    validate_general_idle_loops_drain_loopback(
        &repo_text("kernel/src/main.rs"),
        &repo_text("kernel/src/main_aarch64.rs"),
    )
    .expect("general idle backstops use the idle drain seam");
}

#[test]
fn general_idle_loop_validator_rejects_missing_x86_drain() {
    let x86_source = repo_text("kernel/src/main.rs");
    let aarch64_source = repo_text("kernel/src/main_aarch64.rs");
    let mutated = x86_source.replacen("crate::net::drain_loopback_from_idle();", "", 1);
    assert_ne!(mutated, x86_source, "fixture mutation must apply");
    assert!(validate_general_idle_loops_drain_loopback(&mutated, &aarch64_source).is_err());
}

#[test]
fn general_idle_loop_validator_rejects_missing_aarch64_testing_drain() {
    let x86_source = repo_text("kernel/src/main.rs");
    let aarch64_source = repo_text("kernel/src/main_aarch64.rs");
    let mutated = remove_call_from_loop_after_anchor(
        &aarch64_source,
        r#"serial_print!("breenix> ");"#,
        "kernel::net::drain_loopback_from_idle();",
    )
    .expect("fixture mutation must apply");
    assert!(validate_general_idle_loops_drain_loopback(&x86_source, &mutated).is_err());
}

#[test]
fn general_idle_loop_validator_rejects_missing_aarch64_no_userspace_drain() {
    let x86_source = repo_text("kernel/src/main.rs");
    let aarch64_source = repo_text("kernel/src/main_aarch64.rs");
    let mutated = remove_call_from_loop_after_anchor(
        &aarch64_source,
        r#"serial_println!("[interactive] No userspace init — idling");"#,
        "kernel::net::drain_loopback_from_idle();",
    )
    .expect("fixture mutation must apply");
    assert!(validate_general_idle_loops_drain_loopback(&x86_source, &mutated).is_err());
}

#[test]
fn drain_exclusion_is_a_typed_guard() {
    validate_drain_exclusion_is_a_typed_guard(&repo_text("kernel/src/net/mod.rs"))
        .expect("drain exclusion remains typed RAII");
}

#[test]
fn drain_exclusion_validator_rejects_atomic_bool() {
    let source = repo_text("kernel/src/net/mod.rs");
    let mutated = inject_into_function(
        &source,
        "drain_loopback_rounds",
        "let exclusion = AtomicBool::new(false); drop(exclusion);",
    );
    assert!(validate_drain_exclusion_is_a_typed_guard(&mutated).is_err());
}

#[test]
fn drain_guard_scope_excludes_delivery() {
    validate_drain_guard_scope_excludes_delivery(&repo_text("kernel/src/net/mod.rs"))
        .expect("drain guard remains limited to queue take");
}

#[test]
fn drain_guard_scope_validator_rejects_delivery_in_take_window() {
    let source = repo_text("kernel/src/net/mod.rs");
    let mutated = inject_into_function(
        &source,
        "take_queued_loopback_packets",
        "tcp::drain_deferred_tx();",
    );
    assert!(validate_drain_guard_scope_excludes_delivery(&mutated).is_err());
}

#[test]
fn no_force_release_of_the_drain_owner() {
    validate_no_force_release_of_the_drain_owner(&repo_text("kernel/src/net/mod.rs"))
        .expect("only LoopbackDrainGuard::drop releases its owner ticket");
}

#[test]
fn drain_owner_validator_rejects_force_release() {
    let source = repo_text("kernel/src/net/mod.rs");
    let mutated = inject_into_function(
        &source,
        "acquire",
        "LOOPBACK_DRAIN_OWNER.store(0, Ordering::Release);",
    );
    assert!(validate_no_force_release_of_the_drain_owner(&mutated).is_err());
}

#[test]
fn pump_does_not_halt_while_work_remains() {
    validate_pump_does_not_halt_while_work_remains(&repo_text("kernel/src/net/loopback_pump.rs"))
        .expect("pump yields and continues while work remains");
}

#[test]
fn pump_more_branch_validator_rejects_halt() {
    let source = repo_text("kernel/src/net/loopback_pump.rs");
    let mutated = inject_into_block_after_identifier(
        &source,
        "loopback_pump_fn",
        "more",
        "crate::arch_halt_with_interrupts();",
    );
    assert!(validate_pump_does_not_halt_while_work_remains(&mutated).is_err());
}

#[test]
fn net_lock_guard_disables_bottom_halves_not_softirq_execution() {
    validate_net_lock_guard_disables_bottom_halves_not_softirq_execution(&repo_text(
        "kernel/src/net/mod.rs",
    ))
    .expect("x86 net guard uses BH-disable accounting");
}

#[test]
fn net_lock_guard_context_validator_rejects_softirq_execution_entry() {
    let source = repo_text("kernel/src/net/mod.rs");
    let acquire = function_body(&source, "net_lock_guard").expect("find net_lock_guard fixture");
    let mutated_acquire = acquire.replacen(
        "crate::per_cpu::bh_disable()",
        "crate::per_cpu::softirq_enter()",
        1,
    );
    assert_ne!(mutated_acquire, acquire, "fixture mutation must apply");
    let mutated = source.replacen(acquire, &mutated_acquire, 1);
    assert!(
        validate_net_lock_guard_disables_bottom_halves_not_softirq_execution(&mutated).is_err()
    );
}

#[test]
fn any_context_wake_dispatches_on_execution_context() {
    validate_any_context_wake_dispatches_on_execution_context(&repo_text(
        "kernel/src/task/scheduler.rs",
    ))
    .expect("any-context wake uses the execution-context predicate");
}

#[test]
fn any_context_wake_dispatch_validator_rejects_wide_softirq_predicate() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    let wake = function_body(&source, "wake_thread_any_context")
        .expect("find wake_thread_any_context fixture");
    let mutated_wake = wake.replacen("in_interrupt", "in_softirq", 1);
    assert_ne!(mutated_wake, wake, "fixture mutation must apply");
    let mutated = source.replacen(wake, &mutated_wake, 1);
    assert!(validate_any_context_wake_dispatches_on_execution_context(&mutated).is_err());
}

#[test]
fn generic_unblock_keeps_child_exit_dedicated() {
    validate_generic_unblock_keeps_child_exit_dedicated(&repo_text("kernel/src/task/scheduler.rs"))
        .expect("generic unblock does not wake waitpid sleepers");
}

#[test]
fn generic_unblock_validator_rejects_child_exit_wake() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    let unblock = function_body(&source, "unblock").expect("find Scheduler::unblock fixture");
    let mutated_unblock = unblock.replacen(
        "|| thread.state == ThreadState::BlockedOnTimer",
        "|| thread.state == ThreadState::BlockedOnChildExit\n                || thread.state == ThreadState::BlockedOnTimer",
        1,
    );
    assert_ne!(mutated_unblock, unblock, "fixture mutation must apply");
    let mutated = source.replacen(unblock, &mutated_unblock, 1);
    assert!(validate_generic_unblock_keeps_child_exit_dedicated(&mutated).is_err());
}

#[test]
fn io_wake_buffers_before_thread_context_overflow_fallback() {
    validate_io_wake_buffers_before_thread_context_overflow_fallback(&repo_text(
        "kernel/src/task/scheduler.rs",
    ))
    .expect("I/O completion wake keeps the lock-free buffer dependency break");
}

#[test]
fn io_wake_validator_rejects_direct_any_context_routing() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    let mutated = source.replacen(
        "let _ = buffer_isr_wakeup(tid);\n    set_need_resched();",
        "let _ = wake_thread_any_context(tid);\n    set_need_resched();",
        1,
    );
    assert_ne!(mutated, source, "fixture mutation must apply");
    assert!(validate_io_wake_buffers_before_thread_context_overflow_fallback(&mutated).is_err());
}

#[test]
fn io_wake_validator_rejects_interrupt_context_inline_fallback() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    let mutated = source.replacen(
        "if !crate::per_cpu::in_interrupt()",
        "if crate::per_cpu::in_interrupt()",
        1,
    );
    assert_ne!(mutated, source, "fixture mutation must apply");
    assert!(validate_io_wake_buffers_before_thread_context_overflow_fallback(&mutated).is_err());
}

#[test]
fn loopback_regression_tests_are_arch_neutral() {
    validate_loopback_regression_tests_are_arch_neutral(&repo_text(
        "kernel/src/test_framework/registry.rs",
    ))
    .expect("all loopback regression TestDefs remain Arch::Any");
}

#[test]
fn loopback_arch_validator_rejects_arch_specific_test() {
    let source = repo_text("kernel/src/test_framework/registry.rs");
    let definition = test_def_block(&source, "loopback_recv_wake_when_idle")
        .expect("find loopback recv wake TestDef fixture");
    let mutated_definition = definition.replacen("Arch::Any", "Arch::X86_64", 1);
    assert_ne!(
        mutated_definition, definition,
        "fixture mutation must apply"
    );
    let mutated = source.replacen(definition, &mutated_definition, 1);
    assert!(validate_loopback_regression_tests_are_arch_neutral(&mutated).is_err());
}

#[test]
fn x86_loopback_gates_are_direct_reachable_and_have_one_marker_producer() {
    validate_x86_direct_loopback_gates(
        &repo_text("kernel/src/main.rs"),
        &repo_text("kernel/src/test_framework/registry.rs"),
    )
    .expect("x86 loopback gates remain directly reachable and exactly marked");
}

#[test]
fn deliberately_broken_x86_loopback_gate_variants_fail_the_validator() {
    let main = repo_text("kernel/src/main.rs");
    let registry = repo_text("kernel/src/test_framework/registry.rs");

    let missing_call = main.replacen(
        "kernel::test_framework::registry::run_x86_loopback_gates();",
        "",
        1,
    );
    assert_ne!(missing_call, main, "direct-call mutation must apply");
    assert!(validate_x86_direct_loopback_gates(&missing_call, &registry).is_err());

    let weak_cfg = registry.replacen(
        "#[cfg(all(feature = \"boot_tests\", target_arch = \"x86_64\"))]\n\
         pub fn run_x86_loopback_gates",
        "#[cfg(feature = \"boot_tests\")]\n\
         pub fn run_x86_loopback_gates",
        1,
    );
    assert_ne!(weak_cfg, registry, "cfg-gate mutation must apply");
    assert!(validate_x86_direct_loopback_gates(&main, &weak_cfg).is_err());

    let duplicate_marker = registry.replacen(
        "pub fn run_x86_loopback_gates() {",
        "pub fn run_x86_loopback_gates() {\n\
             crate::serial_println!(\"[TEST:network:loopback_wake_loss_counters_are_zero:PASS]\");",
        1,
    );
    assert_ne!(
        duplicate_marker, registry,
        "marker-producer mutation must apply"
    );
    assert!(validate_x86_direct_loopback_gates(&main, &duplicate_marker).is_err());

    let deferred_gate = registry.replacen(
        "pub fn run_x86_loopback_gates() {",
        "pub fn run_x86_loopback_gates() {\n\
             let _ = loopback_recv_wake_when_idle();",
        1,
    );
    assert_ne!(deferred_gate, registry, "deferred-gate mutation must apply");
    assert!(validate_x86_direct_loopback_gates(&main, &deferred_gate).is_err());
}

#[test]
fn x86_userspace_phase_launches_loopback_wake_test() {
    validate_x86_userspace_phase_launches_loopback_wake_test(&repo_text("kernel/src/main.rs"))
        .expect("kernel_main_continue loads and launches loopback_wake_test exactly once");
}

#[test]
fn x86_gate_requires_the_loopback_regression_tests() {
    validate_x86_gate_requires_the_loopback_regression_tests(&repo_text(
        "docker/qemu/run-x86-boot-tests.sh",
    ))
    .expect("x86 gate requires exactly the executable loopback regression markers");
}

#[test]
fn x86_gate_validator_rejects_missing_loopback_marker() {
    let source = repo_text("docker/qemu/run-x86-boot-tests.sh");
    let marker = "\\[TEST:userspace:loopback_recv_wake:PASS\\]";
    let mutated = source.replace(marker, "");
    assert_ne!(mutated, source, "fixture mutation must apply");
    assert!(validate_x86_gate_requires_the_loopback_regression_tests(&mutated).is_err());
}

#[test]
fn x86_gate_validator_rejects_weak_loopback_marker_count() {
    let source = repo_text("docker/qemu/run-x86-boot-tests.sh");
    let marker = "\\[TEST:userspace:loopback_recv_wake:PASS\\]";
    let count_anchor = format!("grep -h -c '{marker}'");
    let count = source
        .find(&count_anchor)
        .expect("find userspace loopback marker count fixture");
    let weak_count_tail = source[count..].replacen("-eq 1", "-ge 1", 1);
    let mutated = format!("{}{weak_count_tail}", &source[..count]);
    assert_ne!(mutated, source, "exact-count mutation must apply");
    assert!(validate_x86_gate_requires_the_loopback_regression_tests(&mutated).is_err());
}

#[test]
fn x86_gate_validator_rejects_missing_userspace_failure_trap() {
    let source = repo_text("docker/qemu/run-x86-boot-tests.sh");
    let failure_marker = "\\[TEST:userspace:[^]]*:FAIL";
    let mutated = source.replace(failure_marker, "");
    assert_ne!(mutated, source, "userspace FAIL trap mutation must apply");
    assert!(validate_x86_gate_requires_the_loopback_regression_tests(&mutated).is_err());
}

#[test]
fn x86_gate_validator_rejects_lower_userspace_exit_pin() {
    let source = repo_text("docker/qemu/run-x86-boot-tests.sh");
    let mutated = source.replacen(
        "readonly EXPECTED_USERSPACE_EXITS=105",
        "readonly EXPECTED_USERSPACE_EXITS=104",
        1,
    );
    assert_ne!(mutated, source, "userspace exit-pin mutation must apply");
    assert!(validate_x86_gate_requires_the_loopback_regression_tests(&mutated).is_err());
}

#[test]
fn x86_gate_validator_rejects_missing_http_deadline_tally_disclosure() {
    let source = repo_text("docker/qemu/run-x86-boot-tests.sh");
    let mutated = source.replacen("mid-stream stall", "", 1);
    assert_ne!(
        mutated, source,
        "http deadline disclosure mutation must apply"
    );
    assert!(validate_x86_gate_requires_the_loopback_regression_tests(&mutated).is_err());
}

#[test]
fn x86_userspace_launch_validator_rejects_missing_loopback_binary_load() {
    let source = repo_text("kernel/src/main.rs");
    let mutated = source.replacen(
        "kernel::userspace_test::get_test_binary(\"loopback_wake_test\")",
        "kernel::userspace_test::get_test_binary(\"loopback_wake_test_removed\")",
        1,
    );
    assert_ne!(mutated, source, "loopback binary-load mutation must apply");
    assert!(validate_x86_userspace_phase_launches_loopback_wake_test(&mutated).is_err());
}

#[test]
fn x86_userspace_launch_validator_rejects_missing_loopback_process_launch() {
    let source = repo_text("kernel/src/main.rs");
    let launch = "process::creation::create_user_process(\n                    String::from(\"loopback_wake_test\")";
    let mutated = source.replacen(
        launch,
        "process::creation::create_user_process_removed(\n                    String::from(\"loopback_wake_test\")",
        1,
    );
    assert_ne!(
        mutated, source,
        "loopback process-launch mutation must apply"
    );
    assert!(validate_x86_userspace_phase_launches_loopback_wake_test(&mutated).is_err());
}

#[test]
fn x86_gate_validator_rejects_deferred_loopback_requirement() {
    let source = repo_text("docker/qemu/run-x86-boot-tests.sh");
    let marker = "\\[TEST:network:loopback_recv_wake_under_load:PASS\\]";
    let mutated = format!("{source}\ntest \"$(grep -h -c '{marker}' /tmp/serial.txt)\" -eq 1\n");
    assert_ne!(mutated, source, "deferred-marker mutation must apply");
    assert!(validate_x86_gate_requires_the_loopback_regression_tests(&mutated).is_err());
}

#[test]
fn x86_gate_validator_rejects_short_registry_poll_bound() {
    let source = repo_text("docker/qemu/run-x86-boot-tests.sh");
    let mutated = source.replacen("for _ in $(seq 1 900); do", "for _ in $(seq 1 300); do", 1);
    assert_ne!(mutated, source, "fixture mutation must apply");
    assert!(validate_x86_gate_requires_the_loopback_regression_tests(&mutated).is_err());
}

#[test]
fn schedule_rearms_a_blocked_pump() {
    validate_schedule_rearms_a_blocked_pump(&repo_text("kernel/src/task/scheduler.rs"))
        .expect("Scheduler::schedule re-arms a blocked pump when loopback work remains");
}

#[test]
fn schedule_rearm_validator_rejects_missing_unblock() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    let schedule = function_body(&source, "schedule").expect("find Scheduler::schedule fixture");
    let call = "self.unblock(pump_tid)";
    assert!(
        schedule.contains(call),
        "fixture mutation target must exist"
    );
    let mutated_schedule = schedule.replacen(call, "false", 1);
    let mutated = source.replacen(schedule, &mutated_schedule, 1);
    assert!(validate_schedule_rearms_a_blocked_pump(&mutated).is_err());
}

#[test]
fn wakeup_placement_is_bounded_by_online_cpus() {
    validate_wakeup_placement_is_bounded_by_online_cpus(&repo_text("kernel/src/task/scheduler.rs"))
        .expect("wakeup and spawn placement remain online-bounded");
}

#[test]
fn wakeup_placement_validator_rejects_max_cpu_selection() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    let wakeup = function_body(&source, "find_target_cpu_for_wakeup")
        .expect("find wakeup placement fixture");
    let mutated_wakeup = wakeup.replacen("self.online_cpu_count()", "MAX_CPUS", 1);
    assert_ne!(mutated_wakeup, wakeup, "fixture mutation must apply");
    let mutated = source.replacen(wakeup, &mutated_wakeup, 1);
    assert!(validate_wakeup_placement_is_bounded_by_online_cpus(&mutated).is_err());
}

#[test]
fn wakeup_placement_validator_rejects_missing_scheduling_liveness_check() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    let least_loaded =
        function_body(&source, "least_loaded_cpu").expect("find spawn placement fixture");
    let mutated_least_loaded = least_loaded.replacen("self.cpu_accepts_wakeups(cpu)", "true", 1);
    assert_ne!(
        mutated_least_loaded, least_loaded,
        "fixture mutation must apply"
    );
    let mutated = source.replacen(least_loaded, &mutated_least_loaded, 1);
    assert!(validate_wakeup_placement_is_bounded_by_online_cpus(&mutated).is_err());
}

#[test]
fn scheduling_paths_reclaim_unschedulable_cpu_queues() {
    validate_scheduling_paths_reclaim_unschedulable_cpu_queues(&repo_text(
        "kernel/src/task/scheduler.rs",
    ))
    .expect("both scheduling paths reclaim unschedulable CPU queues");
}

#[test]
fn unschedulable_reclaim_validator_rejects_missing_deferred_reclaim() {
    let source = repo_text("kernel/src/task/scheduler.rs");
    let deferred = function_body(&source, "schedule_deferred_requeue")
        .expect("find deferred scheduling fixture");
    let mutated_deferred = deferred.replacen("self.reclaim_unschedulable_cpu_queues();", "", 1);
    assert_ne!(mutated_deferred, deferred, "fixture mutation must apply");
    let mutated = source.replacen(deferred, &mutated_deferred, 1);
    assert!(validate_scheduling_paths_reclaim_unschedulable_cpu_queues(&mutated).is_err());
}

/// Every loopback enqueue must schedule its own delivery.
///
/// Census-shaped on purpose: the check counts the functions in `kernel/src/net/mod.rs`
/// that push onto `LOOPBACK_QUEUE` and requires every one of them to raise the
/// softirq that owns delivery. A new enqueue site added later without a kick
/// changes the census and reddens this, which a named list of today's one site
/// would not.
fn validate_loopback_enqueue_owns_its_delivery(source: &str) -> Result<(), String> {
    let mut pushers = Vec::new();
    let mut kickers = 0usize;
    for span in function_spans(source) {
        let body = normalized_code(&source[span.open..=span.close]);
        // Skip enclosing scopes that merely contain a pushing function.
        if !body.contains("queue.push(LoopbackPacket") {
            continue;
        }
        let inner = function_spans(&source[span.open..=span.close])
            .into_iter()
            .any(|nested| {
                normalized_code(&source[span.open..=span.close][nested.open..=nested.close])
                    .contains("queue.push(LoopbackPacket")
            });
        if inner {
            continue;
        }
        pushers.push(span.name.clone());
        if body.contains("kick_loopback_delivery()") {
            kickers += 1;
        }
    }

    if pushers.is_empty() {
        return Err("no loopback enqueue site found in kernel/src/net/mod.rs".to_string());
    }
    if kickers != pushers.len() {
        return Err(format!(
            "{} of {} loopback enqueue sites raise the delivery softirq: {:?}",
            kickers,
            pushers.len(),
            pushers
        ));
    }

    let kick = function_body(source, "kick_loopback_delivery")
        .ok_or_else(|| "kick_loopback_delivery is not defined".to_string())?;
    if !normalized_code(kick).contains("raise_softirq(SoftirqType::NetRx)") {
        return Err("kick_loopback_delivery does not raise the NetRx softirq".to_string());
    }

    let handler = function_body(source, "net_rx_softirq_handler")
        .ok_or_else(|| "net_rx_softirq_handler is not defined".to_string())?;
    if !normalized_code(handler).contains("drain_loopback_rounds(MAX_DRAIN_ROUNDS, LoopbackDrainSource::Softirq)")
    {
        return Err("the NetRx softirq does not drain the loopback queue".to_string());
    }

    Ok(())
}

/// No exit from the loopback drain may leave queued work without an owner.
///
/// Census-shaped: it counts the `return` statements in `drain_loopback_rounds`
/// and requires each one to be preceded, within its own statement group, by the
/// deferred-TX flush; and it requires every early return that can leave packets
/// queued to hand them back to the softirq. Adding a new early return reddens
/// this until that return is given the same discharge.
fn validate_loopback_drain_exits_own_their_leftovers(source: &str) -> Result<(), String> {
    let body = function_body(source, "drain_loopback_rounds")
        .ok_or_else(|| "drain_loopback_rounds is not defined".to_string())?;
    let code = normalized_code(body);

    let returns = code.matches("return ").count();
    let flushes = code.matches("tcp::drain_deferred_tx();").count();
    if returns == 0 {
        return Err("drain_loopback_rounds has no return statements to audit".to_string());
    }
    if flushes < returns {
        return Err(format!(
            "drain_loopback_rounds has {returns} returns but only {flushes} deferred-TX flushes"
        ));
    }
    if !code.contains("LoopbackTake::Empty => { tcp::drain_deferred_tx(); return false; }") {
        return Err("the empty-queue exit does not flush deferred TX".to_string());
    }
    if !code.contains("LoopbackTake::Contended => { tcp::drain_deferred_tx(); return rearm_if_work_remains(); }")
    {
        return Err("the contended exit does not re-arm delivery".to_string());
    }
    if !code.ends_with("rearm_if_work_remains() }") {
        return Err("the exhausted-budget exit does not re-arm delivery".to_string());
    }

    let rearm = function_body(source, "rearm_if_work_remains")
        .ok_or_else(|| "rearm_if_work_remains is not defined".to_string())?;
    let rearm_code = normalized_code(rearm);
    if !rearm_code.contains("kick_loopback_delivery()") {
        return Err("rearm_if_work_remains does not kick delivery".to_string());
    }

    Ok(())
}

#[test]
fn loopback_enqueue_owns_its_delivery() {
    validate_loopback_enqueue_owns_its_delivery(&repo_text("kernel/src/net/mod.rs"))
        .expect("every loopback enqueue raises the softirq that delivers it");
}

#[test]
fn loopback_enqueue_validator_rejects_a_missing_kick() {
    let source = repo_text("kernel/src/net/mod.rs");
    let call = "kick_loopback_delivery();\n        crate::net::loopback_pump::wake_loopback_pump();";
    assert!(source.contains(call), "fixture mutation target must exist");
    let mutated = source.replacen(call, "crate::net::loopback_pump::wake_loopback_pump();", 1);
    assert_ne!(mutated, source, "fixture mutation must apply");
    assert!(validate_loopback_enqueue_owns_its_delivery(&mutated).is_err());
}

#[test]
fn loopback_enqueue_validator_rejects_a_softirq_that_does_not_drain() {
    let source = repo_text("kernel/src/net/mod.rs");
    let handler =
        function_body(&source, "net_rx_softirq_handler").expect("find the NetRx handler fixture");
    let drain = "let _ = drain_loopback_rounds(MAX_DRAIN_ROUNDS, LoopbackDrainSource::Softirq);";
    assert!(handler.contains(drain), "fixture mutation target must exist");
    let mutated_handler = handler.replacen(drain, "", 1);
    let mutated = source.replacen(handler, &mutated_handler, 1);
    assert!(validate_loopback_enqueue_owns_its_delivery(&mutated).is_err());
}

#[test]
fn loopback_drain_exits_own_their_leftovers() {
    validate_loopback_drain_exits_own_their_leftovers(&repo_text("kernel/src/net/mod.rs"))
        .expect("every loopback drain exit flushes deferred TX and re-arms remaining work");
}

#[test]
fn loopback_drain_validator_rejects_an_unflushed_empty_exit() {
    let source = repo_text("kernel/src/net/mod.rs");
    let body = function_body(&source, "drain_loopback_rounds").expect("find the drain fixture");
    let arm = "LoopbackTake::Empty => {\n                tcp::drain_deferred_tx();\n                return false;\n            }";
    assert!(body.contains(arm), "fixture mutation target must exist");
    let mutated_body = body.replacen(arm, "LoopbackTake::Empty => return false,", 1);
    let mutated = source.replacen(body, &mutated_body, 1);
    assert!(validate_loopback_drain_exits_own_their_leftovers(&mutated).is_err());
}

#[test]
fn loopback_drain_validator_rejects_a_dropped_rearm() {
    let source = repo_text("kernel/src/net/mod.rs");
    let rearm = function_body(&source, "rearm_if_work_remains").expect("find the re-arm fixture");
    let mutated_rearm = rearm.replacen("kick_loopback_delivery();", "", 1);
    assert_ne!(mutated_rearm, rearm, "fixture mutation must apply");
    let mutated = source.replacen(rearm, &mutated_rearm, 1);
    assert!(validate_loopback_drain_exits_own_their_leftovers(&mutated).is_err());
}
