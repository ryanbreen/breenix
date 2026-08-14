use std::fs;
use std::path::{Path, PathBuf};

const NET_TABLES: [&str; 6] = [
    "TCP_CONNECTIONS",
    "TCP_LISTENERS",
    "SEQ_COUNTER",
    "ARP_CACHE",
    "ARP_PENDING_QUEUE",
    "NET_CONFIG",
];

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
        let Some(brace) = function_open_brace(scope, &mask, cursor) else {
            continue;
        };
        return braced_block(scope, &mask, brace);
    }
    None
}

#[derive(Debug)]
struct FunctionSpan {
    name: String,
    function: usize,
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
            function,
            open,
            close,
        });
    }

    spans
}

fn code_sequence_end(
    source: &str,
    mask: &[bool],
    mut cursor: usize,
    expected: &str,
) -> Option<usize> {
    for expected_byte in expected.bytes() {
        while cursor < source.len()
            && (!mask[cursor] || source.as_bytes()[cursor].is_ascii_whitespace())
        {
            cursor += 1;
        }
        if source.as_bytes().get(cursor) != Some(&expected_byte) {
            return None;
        }
        cursor += 1;
    }
    Some(cursor)
}

fn kernel_source_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap_or_else(|error| {
        panic!(
            "read kernel source directory {}: {error}",
            directory.display()
        )
    }) {
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

fn validate_net_table_locks(sources: &[(String, String)]) -> Result<(), String> {
    for (path, source) in sources {
        let mask = code_mask(source);
        let functions = function_spans(source);

        for table in NET_TABLES {
            for lock in identifier_offsets(source, &mask, table)
                .into_iter()
                .filter(|offset| {
                    code_sequence_end(source, &mask, offset + table.len(), ".lock()").is_some()
                })
            {
                let function = functions
                    .iter()
                    .filter(|function| function.open < lock && lock < function.close)
                    .min_by_key(|function| function.close - function.open)
                    .ok_or_else(|| format!("{path}: {table}.lock() is outside a function"))?;
                let body = &source[function.open..=function.close];
                let body_mask = code_mask(body);
                let local_lock = lock - function.open;
                let guarded = identifier_offsets(body, &body_mask, "net_lock_guard")
                    .into_iter()
                    .filter(|guard| {
                        code_sequence_end(body, &body_mask, guard + "net_lock_guard".len(), "()")
                            .is_some()
                    })
                    .any(|guard| guard < local_lock);
                if !guarded {
                    return Err(format!(
                        "{path}: {table}.lock() in fn {} lacks a preceding net_lock_guard()",
                        function.name
                    ));
                }
            }
        }
    }
    Ok(())
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

fn block_is_empty_or_bare_return(block: &str) -> bool {
    let normalized = normalized_code(block);
    let statements = normalized
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .unwrap_or(&normalized)
        .trim();
    statements.is_empty() || statements == "return" || statements == "return;"
}

fn net_lock_guard_drop_body(source: &str) -> Result<&str, String> {
    let implementation = source
        .find("impl Drop for NetLockGuard")
        .ok_or_else(|| "missing Drop implementation for NetLockGuard".to_string())?;
    function_body(&source[implementation..], "drop")
        .ok_or_else(|| "missing NetLockGuard::drop body".to_string())
}

fn validate_x86_bottom_half_guard(source: &str) -> Result<(), String> {
    let acquire = function_body(source, "net_lock_guard")
        .ok_or_else(|| "missing fn net_lock_guard".to_string())?;
    let acquire_x86 = cfg_arch_block(acquire, "x86_64")
        .ok_or_else(|| "missing x86_64 net_lock_guard arm".to_string())?;
    if block_is_empty_or_bare_return(acquire_x86) {
        return Err("x86_64 net_lock_guard acquire arm is a no-op".to_string());
    }
    let acquire_mask = code_mask(acquire_x86);
    if identifier_offsets(acquire_x86, &acquire_mask, "softirq_enter").is_empty() {
        return Err("x86_64 net_lock_guard acquire arm does not call softirq_enter".to_string());
    }

    let drop = net_lock_guard_drop_body(source)?;
    let drop_x86 = cfg_arch_block(drop, "x86_64")
        .ok_or_else(|| "missing x86_64 NetLockGuard::drop arm".to_string())?;
    if block_is_empty_or_bare_return(drop_x86) {
        return Err("x86_64 NetLockGuard::drop arm is a no-op".to_string());
    }
    let drop_mask = code_mask(drop_x86);
    if identifier_offsets(drop_x86, &drop_mask, "softirq_exit").is_empty() {
        return Err("x86_64 NetLockGuard::drop arm does not call softirq_exit".to_string());
    }

    Ok(())
}

fn validate_aarch64_daif_guard(source: &str) -> Result<(), String> {
    let acquire = function_body(source, "net_lock_guard")
        .ok_or_else(|| "missing fn net_lock_guard".to_string())?;
    let acquire_aarch64 = cfg_arch_block(acquire, "aarch64")
        .ok_or_else(|| "missing aarch64 net_lock_guard arm".to_string())?;
    if !acquire_aarch64.contains("\"mrs {}, daif\"") {
        return Err("aarch64 acquire arm no longer reads DAIF".to_string());
    }
    if !acquire_aarch64.contains("\"msr daifset, #2\"") {
        return Err("aarch64 acquire arm no longer masks DAIF.I".to_string());
    }

    let drop = net_lock_guard_drop_body(source)?;
    let drop_aarch64 = cfg_arch_block(drop, "aarch64")
        .ok_or_else(|| "missing aarch64 NetLockGuard::drop arm".to_string())?;
    if !drop_aarch64.contains("\"msr daif, {}\"") {
        return Err("aarch64 Drop arm no longer restores DAIF".to_string());
    }

    Ok(())
}

fn body_forbidden_net_reference(body: &str) -> Option<&'static str> {
    let mask = code_mask(body);
    for name in NET_TABLES.into_iter().chain([
        "with_tcp_connections",
        "with_tcp_listeners",
        "generate_isn",
        "update_cache",
    ]) {
        if !identifier_offsets(body, &mask, name).is_empty() {
            return Some(name);
        }
    }
    for net in identifier_offsets(body, &mask, "net") {
        let Some(end) = code_sequence_end(body, &mask, net + "net".len(), "::config") else {
            continue;
        };
        let next_code = (end..body.len()).find(|index| mask[*index]);
        if !next_code.is_some_and(|index| identifier_byte(body.as_bytes()[index])) {
            return Some("net::config");
        }
    }
    None
}

fn x86_interrupt_functions(source: &str) -> Vec<(&str, &str)> {
    let mask = code_mask(source);
    function_spans(source)
        .into_iter()
        .filter_map(|function| {
            let prefix_start = function.function.saturating_sub(96);
            let prefix = &source[prefix_start..function.function];
            let prefix_mask = &mask[prefix_start..function.function];
            let is_x86_interrupt = identifier_offsets(prefix, prefix_mask, "extern")
                .into_iter()
                .any(|extern_offset| prefix[extern_offset..].contains("\"x86-interrupt\""));
            is_x86_interrupt.then_some((
                &source[function.function + 2..function.open],
                &source[function.open..=function.close],
            ))
        })
        .collect()
}

fn validate_hardirq_surface(interrupts: &str, e1000: &str) -> Result<(), String> {
    let handlers = x86_interrupt_functions(interrupts);
    if handlers.is_empty() {
        return Err("interrupts.rs has no extern x86-interrupt handlers".to_string());
    }
    for (signature, body) in handlers {
        if let Some(reference) = body_forbidden_net_reference(body) {
            return Err(format!(
                "x86-interrupt handler {signature} touches forbidden network surface {reference}"
            ));
        }
    }

    let e1000_handler = function_spans(e1000)
        .into_iter()
        .filter(|function| function.name == "handle_interrupt")
        .find_map(|function| {
            let body = &e1000[function.open..=function.close];
            let mask = code_mask(body);
            (!identifier_offsets(body, &mask, "raise_softirq").is_empty()).then_some(body)
        })
        .ok_or_else(|| "missing module-level e1000::handle_interrupt wrapper".to_string())?;
    if let Some(reference) = body_forbidden_net_reference(e1000_handler) {
        return Err(format!(
            "e1000::handle_interrupt touches forbidden network surface {reference}"
        ));
    }

    Ok(())
}

fn validate_deleted_irq_primitives(sources: &[(String, String)]) -> Result<(), String> {
    for (path, source) in sources {
        let mask = code_mask(source);
        for name in ["irq_save", "irq_restore"] {
            for offset in identifier_offsets(source, &mask, name) {
                if code_sequence_end(source, &mask, offset + name.len(), "(").is_some() {
                    return Err(format!("{path}: resurrected {name}(...)"));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn every_net_table_lock_is_taken_under_the_net_lock_guard() {
    assert_eq!(validate_net_table_locks(&kernel_sources()), Ok(()));
}

#[test]
fn net_table_lock_validator_rejects_unguarded_lock() {
    let synthetic = vec![(
        "kernel/src/net/broken.rs".to_string(),
        "fn broken() { let connections = TCP_CONNECTIONS.lock(); consume(connections); }"
            .to_string(),
    )];
    assert!(validate_net_table_locks(&synthetic).is_err());
}

#[test]
fn net_lock_guard_x86_arm_is_a_real_bottom_half_disable() {
    assert_eq!(
        validate_x86_bottom_half_guard(&repo_text("kernel/src/net/mod.rs")),
        Ok(())
    );
}

#[test]
fn net_lock_guard_x86_validator_rejects_empty_arm() {
    let synthetic = r#"
        fn net_lock_guard() -> NetLockGuard {
            #[cfg(target_arch = "x86_64")]
            {}
            #[cfg(target_arch = "aarch64")]
            { mask_daif(); }
        }
        impl Drop for NetLockGuard {
            fn drop(&mut self) {
                #[cfg(target_arch = "x86_64")]
                { return; }
                #[cfg(target_arch = "aarch64")]
                { restore_daif(); }
            }
        }
    "#;
    assert!(validate_x86_bottom_half_guard(synthetic).is_err());
}

#[test]
fn net_lock_guard_aarch64_arm_still_masks_daif() {
    assert_eq!(
        validate_aarch64_daif_guard(&repo_text("kernel/src/net/mod.rs")),
        Ok(())
    );
}

#[test]
fn net_lock_guard_aarch64_validator_rejects_missing_daifset() {
    let synthetic = r#"
        fn net_lock_guard() -> NetLockGuard {
            #[cfg(target_arch = "aarch64")]
            {
                asm!("mrs {}, daif", out(reg) saved_daif);
            }
        }
        impl Drop for NetLockGuard {
            fn drop(&mut self) {
                #[cfg(target_arch = "aarch64")]
                unsafe { asm!("msr daif, {}", in(reg) self.saved_daif); }
            }
        }
    "#;
    assert!(validate_aarch64_daif_guard(synthetic).is_err());
}

#[test]
fn hardirq_surface_never_touches_the_net_tables() {
    // Bottom-half exclusion is sufficient only while hardirq handlers remain
    // outside the guarded network tables. A new hardirq table user must force
    // an explicit review of the guard's exclusion mechanism.
    assert_eq!(
        validate_hardirq_surface(
            &repo_text("kernel/src/interrupts.rs"),
            &repo_text("kernel/src/drivers/e1000/mod.rs"),
        ),
        Ok(())
    );
}

#[test]
fn hardirq_surface_validator_rejects_net_table_reference() {
    let interrupts = r#"
        extern "x86-interrupt" fn irq10_handler(frame: InterruptStackFrame) {
            let connections = TCP_CONNECTIONS.lock();
            acknowledge(frame, connections);
        }
    "#;
    let e1000 = r#"
        pub fn handle_interrupt() {
            if let Some(driver) = E1000_DRIVER.lock().as_mut() {
                driver.handle_interrupt();
            }
            raise_softirq(SoftirqType::NetRx);
        }
    "#;
    assert!(validate_hardirq_surface(interrupts, e1000).is_err());
}

#[test]
fn the_no_op_irq_primitives_stay_deleted() {
    assert_eq!(validate_deleted_irq_primitives(&kernel_sources()), Ok(()));
}

#[test]
fn irq_primitive_validator_rejects_resurrected_irq_save() {
    let synthetic = vec![(
        "kernel/src/net/mod.rs".to_string(),
        "fn irq_save() -> u64 { 0 }".to_string(),
    )];
    assert!(validate_deleted_irq_primitives(&synthetic).is_err());
}
