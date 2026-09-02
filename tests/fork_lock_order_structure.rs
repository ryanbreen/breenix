//! Structural, mutation-provable ratchet for #745's central fix: x86
//! `sys_fork_with_parent_context` must never re-wrap its PM-lock windows in
//! a hardware interrupt mask, must drain both deferred-reclaim passes
//! before consuming a fresh kernel-stack-pool slot with NO process-manager
//! guard live across either drain call, and must never install the child
//! into the scheduler while holding the process-manager lock.
//!
//! This is #745's version of `exec_lock_order_structure.rs`'s
//! `validate_sys_exec_releases_process_manager` -- proving the fix BY
//! CONSTRUCTION (a text-shape assertion that cannot pass without the fix
//! present) rather than hoping a race shows up in a boot sample. Every
//! assertion below carries its own delete-mutation proof, per this
//! project's standing anti-vacuity rule and #721 review finding M1 ("K13
//! met" was reported without ever actually reddening most of the new gate
//! assertions under mutation) -- not repeated in this arc's own PR.
//!
//! Host-side only: a text read of the tree, no kernel build or QEMU boot.
//! Run: `cargo test --test fork_lock_order_structure`.

use std::fs;
use std::path::PathBuf;

const HANDLERS_RS: &str = "kernel/src/syscall/handlers.rs";
const FORK_FN: &str = "sys_fork_with_parent_context";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

// ---------------------------------------------------------------------------
// Generic text-scanning helpers, identical in shape to the ones every other
// `*_structure.rs` ratchet in this tree carries (see
// `exec_lock_order_structure.rs`'s own copy) -- not exec-specific, so
// duplicated here rather than imported (these test binaries do not share a
// crate).
// ---------------------------------------------------------------------------

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

/// The body of `fn NAME(` (brace-matched), panicking if the function is
/// missing -- a missing function is exactly the kind of drift this ratchet
/// must not pass through silently.
fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let marker = format!("fn {name}(");
    let mask = code_mask(source);
    let start = code_offsets(source, &mask, &marker)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("missing function {name}"));
    braced_block(source, &mask, start).unwrap_or_else(|| panic!("unterminated function {name}"))
}

/// The nearest enclosing `{ ... }` block containing `offset`, i.e. the
/// smallest brace pair opened before `offset` whose match closes after it.
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

// ---------------------------------------------------------------------------
// The validator (#745 precheck §4d / C1 / C4)
// ---------------------------------------------------------------------------

fn validate_fork_lock_order(body: &str) -> Result<(), String> {
    let mask = code_mask(body);

    // 1. Zero occurrences of an interrupt mask anywhere in the function --
    // proves §3.1's fix by construction, not by hoping a race shows up in a
    // boot sample (#745 precheck §4d bullet 1, C1).
    for masker in ["arch_without_interrupts(", "without_interrupts("] {
        if !code_offsets(body, &mask, masker).is_empty() {
            return Err(format!(
                "{FORK_FN} re-wraps its body in {masker} -- reproduces the exact \
                 single-CPU deadlock anti-pattern aarch64 fork's own history already \
                 proved (see the function's own #745 doc comment)"
            ));
        }
    }

    // 2. Exactly one call each to the two reclaim passes, both preceding
    // ProcessPageTable::new( (§4d bullet 2).
    let deferred = code_offsets(body, &mask, "reclaim_deferred_process_resources(");
    let terminated = code_offsets(body, &mask, "reclaim_terminated_threads(");
    let page_table_new = code_offsets(body, &mask, "ProcessPageTable::new(");
    for (label, offsets) in [
        ("reclaim_deferred_process_resources(", &deferred),
        ("reclaim_terminated_threads(", &terminated),
        ("ProcessPageTable::new(", &page_table_new),
    ] {
        if offsets.len() != 1 {
            return Err(format!(
                "{FORK_FN} must call {label} exactly once, found {}",
                offsets.len()
            ));
        }
    }
    if !(deferred[0] < page_table_new[0] && terminated[0] < page_table_new[0]) {
        return Err(format!(
            "{FORK_FN} must drain both reclaim passes before ProcessPageTable::new("
        ));
    }

    // 3. Guard-liveness (#745 precheck C4): no process-manager guard may be
    // live across either reclaim call. `crate::process::manager()` must
    // occur exactly twice (Window 1: read parent info; Window 2: fork +
    // publish), and BOTH reclaim calls must sit strictly between Window 1's
    // own enclosing block closing and Window 2's acquisition -- i.e.
    // outside every manager-guard scope, by construction.
    let manager_calls = code_offsets(body, &mask, "crate::process::manager()");
    if manager_calls.len() != 2 {
        return Err(format!(
            "{FORK_FN} must acquire the process manager exactly twice (Window 1 + \
             Window 2), found {}",
            manager_calls.len()
        ));
    }
    let window_one_close = enclosing_block_close(body, &mask, manager_calls[0])
        .ok_or_else(|| format!("{FORK_FN} Window 1's process-manager binding has no scope"))?;
    let window_two_open = manager_calls[1];
    if !(window_one_close < deferred[0]
        && deferred[0] < terminated[0]
        && terminated[0] < window_two_open)
    {
        return Err(format!(
            "{FORK_FN} calls a reclaim pass while a process-manager guard could still \
             be live -- both drain calls must sit strictly between Window 1's guard \
             going out of scope and Window 2's guard being acquired"
        ));
    }

    // 4. The child is never installed into the scheduler while PM is held
    // (#745 precheck §4d bullet 1's third enclosure target, and the
    // creation-publication lock-order census, C5): `drop(manager_guard)`
    // must precede `scheduler::spawn_front(` on every path that reaches it.
    let spawn_front = code_offsets(body, &mask, "scheduler::spawn_front(");
    if spawn_front.len() != 1 {
        return Err(format!(
            "{FORK_FN} must call scheduler::spawn_front( exactly once, found {}",
            spawn_front.len()
        ));
    }
    let drops = code_offsets(body, &mask, "drop(manager_guard)");
    if !drops.iter().any(|&drop_offset| drop_offset < spawn_front[0]) {
        return Err(format!(
            "{FORK_FN} calls scheduler::spawn_front( without a preceding \
             drop(manager_guard) -- the process-manager guard may still be live"
        ));
    }

    Ok(())
}

#[test]
fn sys_fork_with_parent_context_has_the_required_lock_order() {
    let source = repo_text(HANDLERS_RS);
    let body = function_body(&source, FORK_FN);
    assert_eq!(validate_fork_lock_order(body), Ok(()));
}

// ---------------------------------------------------------------------------
// Delete-mutation proofs -- every assertion above reddens under the mutation
// it exists to catch (#721 review M1: do not report a ratchet "met" without
// actually reddening it).
// ---------------------------------------------------------------------------

#[test]
fn negative_reintroduced_interrupt_mask_is_rejected() {
    let source = repo_text(HANDLERS_RS);
    let body = function_body(&source, FORK_FN);
    let mutated = format!("{{ arch_without_interrupts(|| {{ {body} }}); }}");
    assert_ne!(mutated, body, "mutation did not apply");
    assert!(validate_fork_lock_order(&mutated).is_err());
}

#[test]
fn negative_dropped_deferred_reclaim_call_is_rejected() {
    let source = repo_text(HANDLERS_RS);
    let body = function_body(&source, FORK_FN);
    let mutated = body.replacen(
        "crate::task::process_task::reclaim_deferred_process_resources();\n",
        "",
        1,
    );
    assert_ne!(mutated, body, "mutation did not apply");
    assert!(validate_fork_lock_order(&mutated).is_err());
}

#[test]
fn negative_dropped_terminated_reclaim_call_is_rejected() {
    let source = repo_text(HANDLERS_RS);
    let body = function_body(&source, FORK_FN);
    let mutated = body.replacen(
        "crate::task::scheduler::reclaim_terminated_threads();\n",
        "",
        1,
    );
    assert_ne!(mutated, body, "mutation did not apply");
    assert!(validate_fork_lock_order(&mutated).is_err());
}

#[test]
fn negative_reordered_page_table_before_reclaim_is_rejected() {
    let source = repo_text(HANDLERS_RS);
    let body = function_body(&source, FORK_FN);
    // Move an equivalent ProcessPageTable::new( call to the very front of the
    // body (still exactly one call site the way the shape check reads it: the
    // original call text itself is deleted). This proves the *ordering*
    // assertion, not merely the *presence* count.
    let without_original = body.replacen(
        "let child_page_table = match crate::memory::process_memory::ProcessPageTable::new() {",
        "let child_page_table = match Ok::<(), ()>(()).map(|_| unreachable!()) {",
        1,
    );
    assert_ne!(without_original, body, "mutation did not apply (original call site)");
    let mutated = format!(
        "{{ let _early = crate::memory::process_memory::ProcessPageTable::new(); {without_original} }}"
    );
    assert!(validate_fork_lock_order(&mutated).is_err());
}

#[test]
fn negative_guard_live_across_reclaim_is_rejected() {
    let source = repo_text(HANDLERS_RS);
    let body = function_body(&source, FORK_FN);
    // Simulate Window 2's guard being acquired BEFORE the reclaim calls
    // instead of after, by inserting a second `crate::process::manager()`
    // call immediately before the first reclaim call -- this changes the
    // manager-call count from 2 to 3 (still caught) AND, independently,
    // pulls a manager() acquisition ahead of the drain calls.
    let mutated = body.replacen(
        "crate::task::process_task::reclaim_deferred_process_resources();",
        "let _early_guard = crate::process::manager(); \
         crate::task::process_task::reclaim_deferred_process_resources();",
        1,
    );
    assert_ne!(mutated, body, "mutation did not apply");
    assert!(validate_fork_lock_order(&mutated).is_err());
}

#[test]
fn negative_missing_drop_before_spawn_front_is_rejected() {
    let source = repo_text(HANDLERS_RS);
    let body = function_body(&source, FORK_FN);
    let mutated = body.replacen("drop(manager_guard);\n", "", 2);
    assert_ne!(mutated, body, "mutation did not apply");
    assert!(validate_fork_lock_order(&mutated).is_err());
}
