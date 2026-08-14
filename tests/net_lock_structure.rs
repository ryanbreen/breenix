use std::fs;
use std::path::{Path, PathBuf};

const THREAD_CONTEXT_ONLY: &[(&str, &str)] = &[];
const LOCK_OPERATIONS: [&str; 4] = ["lock", "try_lock", "read", "write"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct NetStatic {
    path: String,
    name: String,
    is_public: bool,
}

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

fn is_net_source(path: &str) -> bool {
    path == "kernel/src/net/mod.rs" || path.starts_with("kernel/src/net/")
}

fn next_code_byte(source: &str, mask: &[bool], mut cursor: usize) -> Option<usize> {
    while cursor < source.len() {
        if mask[cursor] && !source.as_bytes()[cursor].is_ascii_whitespace() {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn declaration_is_public(source: &str, mask: &[bool], declaration: usize) -> bool {
    let start = (0..declaration)
        .rev()
        .find(|index| mask[*index] && matches!(source.as_bytes()[*index], b';' | b'{' | b'}'))
        .map_or(0, |index| index + 1);
    let prefix = &source[start..declaration];
    let prefix_mask = &mask[start..declaration];
    !identifier_offsets(prefix, prefix_mask, "pub").is_empty()
}

fn static_type_end(source: &str, mask: &[bool], start: usize) -> Option<usize> {
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    let mut angles = 0usize;

    for index in start..source.len() {
        if !mask[index] {
            continue;
        }
        match source.as_bytes()[index] {
            b'(' => parentheses += 1,
            b')' => parentheses = parentheses.checked_sub(1)?,
            b'[' => brackets += 1,
            b']' => brackets = brackets.checked_sub(1)?,
            b'{' => braces += 1,
            b'}' => braces = braces.checked_sub(1)?,
            b'<' => angles += 1,
            b'>' if angles != 0 => angles -= 1,
            b'=' if parentheses == 0 && brackets == 0 && braces == 0 && angles == 0 => {
                return Some(index)
            }
            _ => {}
        }
    }
    None
}

/// Discover spin-locking statics declared under `kernel/src/net/` only. Mutex statics elsewhere
/// that the net path also touches (for example, `E1000_DRIVER` in the e1000 driver) are outside
/// this gate's scope and tracked separately. Atomics are intentionally excluded because they are
/// lock-free and cannot participate in the same spin-deadlock class.
fn discover_net_statics(sources: &[(String, String)]) -> Result<Vec<NetStatic>, String> {
    let mut statics = Vec::new();

    for (path, source) in sources.iter().filter(|(path, _)| is_net_source(path)) {
        let mask = code_mask(source);
        for declaration in identifier_offsets(source, &mask, "static") {
            if declaration
                .checked_sub(1)
                .is_some_and(|before| source.as_bytes()[before] == b'\'')
            {
                continue;
            }
            let Some(mut cursor) = next_code_byte(source, &mask, declaration + "static".len())
            else {
                return Err(format!("{path}: static declaration has no name"));
            };
            if source[cursor..].starts_with("mut")
                && identifier_offsets(&source[cursor..], &mask[cursor..], "mut")
                    .first()
                    .is_some_and(|offset| *offset == 0)
            {
                cursor = next_code_byte(source, &mask, cursor + "mut".len())
                    .ok_or_else(|| format!("{path}: static mut declaration has no name"))?;
            }

            let name_start = cursor;
            while cursor < source.len()
                && mask[cursor]
                && identifier_byte(source.as_bytes()[cursor])
            {
                cursor += 1;
            }
            if cursor == name_start {
                return Err(format!("{path}: static declaration has an invalid name"));
            }
            let name = &source[name_start..cursor];
            let colon = next_code_byte(source, &mask, cursor)
                .filter(|index| source.as_bytes()[*index] == b':')
                .ok_or_else(|| format!("{path}: static {name} declaration has no type"))?;
            let type_end = static_type_end(source, &mask, colon + 1)
                .ok_or_else(|| format!("{path}: static {name} declaration has no initializer"))?;
            let type_text = &source[colon + 1..type_end];
            let type_mask = &mask[colon + 1..type_end];
            let is_lock = ["Mutex", "RwLock"]
                .into_iter()
                .any(|lock| !identifier_offsets(type_text, type_mask, lock).is_empty());
            if is_lock {
                statics.push(NetStatic {
                    path: path.clone(),
                    name: name.to_string(),
                    is_public: declaration_is_public(source, &mask, declaration),
                });
            }
        }
    }

    statics.sort_by(|left, right| (&left.path, &left.name).cmp(&(&right.path, &right.name)));
    Ok(statics)
}

fn guard_is_bound_before(body: &str, body_mask: &[bool], lock: usize) -> bool {
    identifier_offsets(body, body_mask, "net_lock_guard")
        .into_iter()
        .filter(|guard| *guard < lock)
        .filter(|guard| {
            code_sequence_end(body, body_mask, guard + "net_lock_guard".len(), "()").is_some()
        })
        .any(|guard| {
            identifier_offsets(&body[..guard], &body_mask[..guard], "let")
                .into_iter()
                .rev()
                .take_while(|binding| {
                    !body[*binding..guard]
                        .bytes()
                        .zip(&body_mask[*binding..guard])
                        .any(|(byte, code)| *code && byte == b';')
                })
                .find_map(|binding| {
                    let equals = (binding + "let".len()..guard)
                        .find(|index| body_mask[*index] && body.as_bytes()[*index] == b'=')?;
                    let pattern = normalized_code(&body[binding + "let".len()..equals]);
                    Some(!pattern.is_empty() && pattern != "_" && !pattern.starts_with("_ :"))
                })
                .unwrap_or(false)
        })
}

fn validate_net_static_locks(
    sources: &[(String, String)],
    thread_context_only: &[(&str, &str)],
) -> Result<Vec<NetStatic>, String> {
    for (name, reason) in thread_context_only {
        if reason.trim().is_empty() {
            return Err(format!(
                "<allowlist>: {name} in fn <classification> has an empty thread-context-only reason"
            ));
        }
    }

    let statics = discover_net_statics(sources)?;
    for (name, _) in thread_context_only {
        if !statics.iter().any(|net_static| net_static.name == *name) {
            return Err(format!(
                "<allowlist>: {name} in fn <classification> does not name a discovered net static"
            ));
        }
    }

    for net_static in &statics {
        let classification_count = thread_context_only
            .iter()
            .filter(|(name, _)| *name == net_static.name)
            .count();
        if classification_count > 1 {
            return Err(format!(
                "{}: {} in fn <classification> has duplicate thread-context-only entries",
                net_static.path, net_static.name
            ));
        }

        let mut unguarded = None;
        for (path, source) in sources {
            let mask = code_mask(source);
            let functions = function_spans(source);

            for lock in identifier_offsets(source, &mask, &net_static.name) {
                let Some(operation) = LOCK_OPERATIONS.into_iter().find(|operation| {
                    code_sequence_end(
                        source,
                        &mask,
                        lock + net_static.name.len(),
                        &format!(".{operation}()"),
                    )
                    .is_some()
                }) else {
                    continue;
                };

                let Some(function) = functions
                    .iter()
                    .filter(|function| function.open < lock && lock < function.close)
                    .min_by_key(|function| function.close - function.open)
                else {
                    unguarded = Some(format!(
                        "{path}: {}.{operation}() in fn <none> is outside a function",
                        net_static.name
                    ));
                    break;
                };
                let body = &source[function.open..=function.close];
                let body_mask = code_mask(body);
                let local_lock = lock - function.open;
                if !guard_is_bound_before(body, &body_mask, local_lock) {
                    unguarded = Some(format!(
                        "{path}: {}.{operation}() in fn {} lacks a preceding bound net_lock_guard()",
                        net_static.name, function.name
                    ));
                    break;
                }
            }
            if unguarded.is_some() {
                break;
            }
        }

        let guarded = !net_static.is_public && unguarded.is_none();
        let declared_thread_only = classification_count == 1;
        match (guarded, declared_thread_only) {
            (true, false) | (false, true) => {}
            (true, true) => {
                return Err(format!(
                    "{}: {} in fn <classification> is both guarded and declared thread-context-only",
                    net_static.path, net_static.name
                ));
            }
            (false, false) if net_static.is_public => {
                return Err(format!(
                    "{}: {} in fn <declaration> is public, so external raw locks are possible",
                    net_static.path, net_static.name
                ));
            }
            (false, false) => return Err(unguarded.expect("unguarded static has a diagnostic")),
        }
    }

    Ok(statics)
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
    if identifier_offsets(acquire_x86, &acquire_mask, "bh_disable").is_empty() {
        return Err("x86_64 net_lock_guard acquire arm does not call bh_disable".to_string());
    }
    if !identifier_offsets(acquire_x86, &acquire_mask, "softirq_enter").is_empty() {
        return Err("x86_64 net_lock_guard acquire arm enters softirq execution".to_string());
    }

    let drop = net_lock_guard_drop_body(source)?;
    let drop_x86 = cfg_arch_block(drop, "x86_64")
        .ok_or_else(|| "missing x86_64 NetLockGuard::drop arm".to_string())?;
    if block_is_empty_or_bare_return(drop_x86) {
        return Err("x86_64 NetLockGuard::drop arm is a no-op".to_string());
    }
    let drop_mask = code_mask(drop_x86);
    if identifier_offsets(drop_x86, &drop_mask, "bh_enable").is_empty() {
        return Err("x86_64 NetLockGuard::drop arm does not call bh_enable".to_string());
    }
    if !identifier_offsets(drop_x86, &drop_mask, "softirq_exit").is_empty() {
        return Err("x86_64 NetLockGuard::drop arm exits softirq execution".to_string());
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

fn body_forbidden_net_reference(body: &str, net_statics: &[NetStatic]) -> Option<String> {
    let mask = code_mask(body);
    for name in net_statics
        .iter()
        .map(|net_static| net_static.name.as_str())
        .chain([
            "with_tcp_connections",
            "with_tcp_listeners",
            "generate_isn",
            "update_cache",
        ])
    {
        if !identifier_offsets(body, &mask, name).is_empty() {
            return Some(name.to_string());
        }
    }
    for net in identifier_offsets(body, &mask, "net") {
        let Some(end) = code_sequence_end(body, &mask, net + "net".len(), "::config") else {
            continue;
        };
        let next_code = (end..body.len()).find(|index| mask[*index]);
        if !next_code.is_some_and(|index| identifier_byte(body.as_bytes()[index])) {
            return Some("net::config".to_string());
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

fn validate_hardirq_surface(
    interrupts: &str,
    e1000: &str,
    net_sources: &[(String, String)],
) -> Result<(), String> {
    let net_statics = discover_net_statics(net_sources)?;
    let handlers = x86_interrupt_functions(interrupts);
    if handlers.is_empty() {
        return Err("interrupts.rs has no extern x86-interrupt handlers".to_string());
    }
    for (signature, body) in handlers {
        if let Some(reference) = body_forbidden_net_reference(body, &net_statics) {
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
    if let Some(reference) = body_forbidden_net_reference(e1000_handler, &net_statics) {
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
fn every_cross_context_net_static_is_guarded_or_declared_thread_only() {
    let statics = validate_net_static_locks(&kernel_sources(), THREAD_CONTEXT_ONLY)
        .unwrap_or_else(|error| panic!("{error}"));
    let names: Vec<_> = statics
        .iter()
        .map(|net_static| net_static.name.as_str())
        .collect();

    // This floor list is only a canary that discovery still sees the known declarations; it is
    // not the protected set. The protected set is enumerated from source, so adding a new net
    // static needs no edit here. Update this floor only when a listed static is legitimately
    // deleted.
    const DISCOVERY_FLOOR: &[&str] = &[
        "TCP_CONNECTIONS",
        "TCP_LISTENERS",
        "SEQ_COUNTER",
        "DEFERRED_TX_QUEUE",
        "ARP_CACHE",
        "NET_CONFIG",
        "LOOPBACK_QUEUE",
        "ARP_PENDING_QUEUE",
        "CURRENT_PACKET_SRC_MAC",
    ];
    for expected in DISCOVERY_FLOOR {
        assert!(
            names.contains(expected),
            "known net static {expected} was not discovered; discovery may have gone blind"
        );
    }

    eprintln!("discovered guarded net statics: {}", names.join(", "));
}

#[test]
fn net_static_validator_rejects_discovered_unguarded_static() {
    for (lock_type, initializer, operation) in [
        ("Mutex<u8>", "Mutex::new(0)", "lock"),
        ("Mutex<u8>", "Mutex::new(0)", "try_lock"),
        ("RwLock<u8>", "RwLock::new(0)", "read"),
        ("RwLock<u8>", "RwLock::new(0)", "write"),
    ] {
        let synthetic = vec![(
            "kernel/src/net/broken.rs".to_string(),
            format!(
                "static NEW_TABLE: {lock_type} = {initializer};\n\
                 fn broken() {{ let table = NEW_TABLE.{operation}(); consume(table); }}"
            ),
        )];
        let error = validate_net_static_locks(&synthetic, &[]).unwrap_err();
        assert!(error.contains("kernel/src/net/broken.rs"));
        assert!(error.contains("NEW_TABLE"));
        assert!(error.contains("fn broken"));
    }
}

#[test]
fn net_static_validator_rejects_public_mutex_static() {
    let synthetic = vec![(
        "kernel/src/net/broken.rs".to_string(),
        r#"
            pub static PUBLIC_TABLE: Mutex<u8> = Mutex::new(0);
            fn guarded() {
                let _guard = net_lock_guard();
                let table = PUBLIC_TABLE.lock();
                consume(table);
            }
        "#
        .to_string(),
    )];
    let error = validate_net_static_locks(&synthetic, &[]).unwrap_err();
    assert!(error.contains("kernel/src/net/broken.rs"));
    assert!(error.contains("PUBLIC_TABLE"));
    assert!(error.contains("fn <declaration>"));
}

#[test]
fn net_static_validator_rejects_immediately_dropped_guard() {
    let synthetic = vec![(
        "kernel/src/net/broken.rs".to_string(),
        r#"
            static WILDCARD_TABLE: Mutex<u8> = Mutex::new(0);
            fn broken() {
                let _ = net_lock_guard();
                let table = WILDCARD_TABLE.lock();
                consume(table);
            }
        "#
        .to_string(),
    )];
    let error = validate_net_static_locks(&synthetic, &[]).unwrap_err();
    assert!(error.contains("kernel/src/net/broken.rs"));
    assert!(error.contains("WILDCARD_TABLE"));
    assert!(error.contains("fn broken"));
}

#[test]
fn net_static_validator_accepts_declared_thread_context_only_static() {
    let synthetic = vec![(
        "kernel/src/net/thread_only.rs".to_string(),
        r#"
            static THREAD_ONLY_TABLE: Mutex<u8> = Mutex::new(0);
            fn thread_only() { let table = THREAD_ONLY_TABLE.lock(); consume(table); }
        "#
        .to_string(),
    )];
    assert!(validate_net_static_locks(
        &synthetic,
        &[(
            "THREAD_ONLY_TABLE",
            "Only the boot thread accesses this table before interrupts are enabled",
        )],
    )
    .is_ok());
}

#[test]
fn net_static_validator_requires_exactly_one_classification() {
    let synthetic = vec![(
        "kernel/src/net/overclassified.rs".to_string(),
        r#"
            static GUARDED_TABLE: Mutex<u8> = Mutex::new(0);
            fn guarded() {
                let _guard = net_lock_guard();
                let table = GUARDED_TABLE.lock();
                consume(table);
            }
        "#
        .to_string(),
    )];
    assert!(validate_net_static_locks(
        &synthetic,
        &[("GUARDED_TABLE", "Incorrectly classified as thread-only")],
    )
    .is_err());
}

#[test]
fn net_static_validator_rejects_empty_thread_context_only_reason() {
    let synthetic = vec![(
        "kernel/src/net/thread_only.rs".to_string(),
        r#"
            static THREAD_ONLY_TABLE: Mutex<u8> = Mutex::new(0);
            fn thread_only() { let table = THREAD_ONLY_TABLE.lock(); consume(table); }
        "#
        .to_string(),
    )];
    assert!(validate_net_static_locks(&synthetic, &[("THREAD_ONLY_TABLE", "")]).is_err());
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
    // outside the source-enumerated net statics and their guarded accessors. A
    // new hardirq user must force an explicit review of the exclusion mechanism.
    assert_eq!(
        validate_hardirq_surface(
            &repo_text("kernel/src/interrupts.rs"),
            &repo_text("kernel/src/drivers/e1000/mod.rs"),
            &kernel_sources(),
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
    let net_sources = vec![(
        "kernel/src/net/tcp.rs".to_string(),
        "static TCP_CONNECTIONS: Mutex<u8> = Mutex::new(0);".to_string(),
    )];
    assert!(validate_hardirq_surface(interrupts, e1000, &net_sources).is_err());
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
