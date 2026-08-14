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
