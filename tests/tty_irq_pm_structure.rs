//! #821 -- the TTY input IRQ entry takes no blocking `PROCESS_MANAGER`
//! acquisition, and the pid-dependent work it used to do there happens in
//! thread context instead.
//!
//! The defect these rules pin: `TtyDevice::input_char_nonblock`, the body both
//! architectures' input interrupts reach, called `crate::process::current_pid()`
//! -- which blocks in `manager()` -- while claiming interrupt safety in its own
//! doc comment. A thread-context holder of that lock can be running with
//! interrupts live (the x86_64 `manager()` arm performs 0 mask operations), so
//! the interrupt waits for a lock its own CPU may own, or waits out a remote
//! hold in interrupt context.
//!
//! The rules are CENSUSES, not lists of known names:
//!
//! * the IRQ-side roots inside `kernel/src/tty/driver.rs` are read out of that
//!   file as the functions whose names end in `_nonblock` -- its own naming
//!   convention for the interrupt-context twins, 6 of them today -- so a
//!   seventh one added later is swept in without an edit here;
//! * the IRQ handlers are read out of `kernel/src` as the functions that call
//!   `tty::push_char_nonblock`, 5 of them today, so a sixth input driver is
//!   swept in too.
//!
//! The call graph inside `driver.rs` is walked only through `self.` and `Self::`
//! receivers and unqualified calls. That restriction is deliberate and is the
//! IRQ-context lock census's own Appendix A lesson: a name-plus-receiver
//! resolver attributed `ldisc.input_char(..)` -- a `LineDiscipline` method -- to
//! the same-named `TtyDevice::input_char`, and produced a chain into
//! `send_signal_to_foreground` that does not exist. Both endpoints are still
//! covered: `input_char_nonblock` is a root by the naming census, and
//! `line_discipline.rs` is checked for 0 mentions of `crate::process`.
//!
//! The 6 rules are mutation-tested at the bottom of the file against in-memory
//! copies, with a green control, so a rule that had quietly stopped matching
//! cannot pass forever. The gate legs run the two aarch64 gates for real, in
//! scoring-only mode, over committed serials.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

const DRIVER: &str = "kernel/src/tty/driver.rs";
const LDISC: &str = "kernel/src/tty/line_discipline.rs";
const PROCESS: &str = "kernel/src/process/mod.rs";
const HANDLERS: &str = "kernel/src/syscall/handlers.rs";
const STRICT_GATE: &str = "docker/qemu/run-aarch64-boot-test-strict.sh";
const PROD_GATE: &str = "docker/qemu/run-aarch64-prod-profile-boot-test.sh";
const X86_BOOT_GATE: &str = "docker/qemu/run-x86-boot-tests.sh";
const X86_PROD_GATE: &str = "docker/qemu/run-x86-prod-profile-boot-test.sh";
const GREEN_SERIAL: &str =
    "docs/planning/green-program/irq-locks/serials/821/02-a64-green-repaired-serial.txt";
const PROD_SERIAL: &str =
    "docs/planning/green-program/irq-locks/serials/821/03-a64-prod-profile-serial.txt";
const ORACLE_MARKER: &str = "[TTY_IRQ_PM_ORACLE:";

/// The spellings that BLOCK on `PROCESS_MANAGER`. `try_manager` is absent on
/// purpose: it does not wait, and it masks around its hold, so the IRQ side is
/// allowed to use it -- `send_signal_to_process_nonblock` does.
const BLOCKING_PM_CALLS: [&str; 3] = ["manager", "current_pid", "with_process_manager"];
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


/// The Rust sources under `kernel/src`, as (repo-relative path, contents).
fn kernel_sources() -> Vec<(String, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).expect("read kernel source directory") {
            let path = entry.expect("read kernel source entry").path();
            if path.is_dir() {
                visit(root, &path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(root)
                    .expect("kernel source stays under the repository root")
                    .display()
                    .to_string();
                let text = fs::read_to_string(&path).expect("read kernel source");
                out.push((relative, text));
            }
        }
    }
    let root = repo_root();
    let mut out = Vec::new();
    visit(&root, &root.join("kernel/src"), &mut out);
    out.sort();
    out
}

/// Offsets in `body` at which one of the BLOCKING process-manager accessors is
/// CALLED. An occurrence that is not followed by `(` is a binding or a mention,
/// not a call, and `identifier_offsets` already refuses `try_manager` and
/// `with_process_manager` when it is asked for `manager`.
fn blocking_pm_calls(body: &str) -> Vec<&'static str> {
    let mask = code_mask(body);
    let bytes = body.as_bytes();
    let mut found = Vec::new();
    for call in BLOCKING_PM_CALLS {
        for offset in identifier_offsets(body, &mask, call) {
            let mut cursor = offset + call.len();
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'(') {
                found.push(call);
                break;
            }
        }
    }
    found
}

/// The functions in `driver.rs` that `body` calls through a `self.`, `Self::`
/// or unqualified call. See the module comment for why a call through any other
/// receiver is not an edge.
fn driver_edges<'a>(body: &str, defined: &[&'a str]) -> Vec<&'a str> {
    let mask = code_mask(body);
    let bytes = body.as_bytes();
    let mut edges = Vec::new();
    for name in defined {
        for offset in identifier_offsets(body, &mask, name) {
            let mut cursor = offset + name.len();
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if bytes.get(cursor) != Some(&b'(') {
                continue;
            }
            let qualified_by_self = body[..offset].ends_with("self.")
                || body[..offset].ends_with("Self::")
                || body[..offset].ends_with("self . ");
            let unqualified = !body[..offset].ends_with('.') && !body[..offset].ends_with("::");
            if qualified_by_self || unqualified {
                edges.push(*name);
                break;
            }
        }
    }
    edges
}

/// #821 rule 1. The functions reachable from a `driver.rs` interrupt-context
/// entry take 0 blocking `PROCESS_MANAGER` acquisitions between them.
fn validate_irq_entry_takes_no_blocking_pm(driver: &str) -> Result<(), String> {
    let spans = function_spans(driver);
    let defined: Vec<&str> = spans.iter().map(|span| span.name.as_str()).collect();
    let roots: Vec<&str> = defined
        .iter()
        .copied()
        .filter(|name| name.ends_with("_nonblock"))
        .collect();
    if roots.len() < 5 {
        return Err(format!(
            "driver.rs declares {} interrupt-context entries (names ending in _nonblock); this \
             rule was written against 6 and a census that shrank below 5 is a rule that stopped \
             looking",
            roots.len()
        ));
    }
    if !roots.contains(&"input_char_nonblock") {
        return Err("driver.rs no longer declares input_char_nonblock, the entry #821 is about"
            .to_string());
    }

    let mut queue: Vec<&str> = roots.clone();
    let mut seen: Vec<&str> = Vec::new();
    let mut failures = Vec::new();
    while let Some(name) = queue.pop() {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        let Some(span) = spans.iter().find(|span| span.name == name) else {
            continue;
        };
        let body = &driver[span.open..=span.close];
        for call in blocking_pm_calls(body) {
            failures.push(format!(
                "{name} is reachable from a TTY interrupt-context entry and calls {call}(), which \
                 blocks on PROCESS_MANAGER"
            ));
        }
        for edge in driver_edges(body, &defined) {
            queue.push(edge);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// #821 rule 2. The line discipline the entry drives carries 0 mentions of
/// `crate::process`, which is what lets rule 1 stop at the `driver.rs`
/// boundary.
fn validate_line_discipline_is_process_free(ldisc: &str) -> Result<(), String> {
    if normalized_code(ldisc).contains("crate::process") {
        return Err(
            "line_discipline.rs names crate::process, so rule 1 can no longer stop at the \
             driver.rs boundary"
                .to_string(),
        );
    }
    Ok(())
}

/// #821 rule 3. The entry records the deferral in the branch the pid lookup
/// used to sit in, and runs inside the scope that counts a reintroduction.
fn validate_deferred_arm(driver: &str) -> Result<(), String> {
    let entry = function_body(driver, "input_char_nonblock")
        .ok_or_else(|| "driver.rs declares no input_char_nonblock".to_string())?;
    let normalized = normalized_code(entry);
    if !normalized.contains("NoBlockingProcessManagerScope::enter()") {
        return Err(
            "input_char_nonblock does not enter a NoBlockingProcessManagerScope, so a blocking \
             PROCESS_MANAGER acquisition taken from it would be counted by nothing"
                .to_string(),
        );
    }
    let mask = code_mask(entry);
    let field = identifier_offsets(entry, &mask, "foreground_pgrp")
        .into_iter()
        .next()
        .ok_or_else(|| "input_char_nonblock no longer reads foreground_pgrp".to_string())?;
    let branch = braced_block(entry, &mask, field)
        .ok_or_else(|| "input_char_nonblock's foreground_pgrp test has no block".to_string())?;
    let branch = normalized_code(branch);
    if !branch.contains("adopt_pending") {
        return Err(
            "input_char_nonblock's unset-foreground-pgrp branch does not record that an adoption \
             is owed"
                .to_string(),
        );
    }
    if !branch.contains("TTY_IRQ_PM_DEFERRED") {
        return Err(
            "input_char_nonblock's unset-foreground-pgrp branch does not count the deferral"
                .to_string(),
        );
    }
    let consumer = function_body(driver, "adopt_deferred_foreground_pgrp")
        .ok_or_else(|| "driver.rs declares no adopt_deferred_foreground_pgrp".to_string())?;
    let consumer = normalized_code(consumer);
    if !consumer.contains("adopt_pending") || !consumer.contains("TTY_IRQ_PM_ADOPTED") {
        return Err(
            "adopt_deferred_foreground_pgrp does not take the recorded adoption and count it"
                .to_string(),
        );
    }
    if function_body(driver, "adopt_foreground_pgrp_from_reader").is_none() {
        return Err(
            "driver.rs declares no adopt_foreground_pgrp_from_reader, the entry point a reader \
             calls"
                .to_string(),
        );
    }
    Ok(())
}

/// #821 rule 4. The reader takes the adoption, with its own pid, after it has
/// let go of the process-manager lock.
fn validate_reader_takes_the_adoption(handlers: &str) -> Result<(), String> {
    let read = function_body(handlers, "sys_read")
        .ok_or_else(|| "handlers.rs declares no sys_read".to_string())?;
    let mask = code_mask(read);
    let call = identifier_offsets(read, &mask, "adopt_foreground_pgrp_from_reader")
        .into_iter()
        .next()
        .ok_or_else(|| {
            "sys_read never takes a deferred foreground-pgrp adoption, so the work #821 moved out \
             of the interrupt is done by nobody"
                .to_string()
        })?;
    if !normalized_code(read).contains("adopt_foreground_pgrp_from_reader(reader_pid)") {
        return Err(
            "sys_read does not hand the adoption the reading process's own pid".to_string()
        );
    }
    let released = identifier_offsets(read, &mask, "manager_guard")
        .into_iter()
        .filter(|offset| read[*offset..].starts_with("manager_guard)"))
        .next()
        .ok_or_else(|| "sys_read no longer drops its process-manager guard".to_string())?;
    if call < released {
        return Err(
            "sys_read takes the adoption while it still holds the process-manager guard".to_string(),
        );
    }
    Ok(())
}

/// #821 rule 5. Each blocking accessor of `PROCESS_MANAGER` counts itself when
/// a no-blocking scope is open; the non-blocking one deliberately does not.
fn validate_blocking_accessors_are_counted(process: &str) -> Result<(), String> {
    const NOTE: &str = "note_blocking_process_manager_acquisition()";
    let spans = function_spans(process);
    let blocking: Vec<&FunctionSpan> = spans
        .iter()
        .filter(|span| span.name == "manager" || span.name == "with_process_manager")
        .collect();
    if blocking.len() < 3 {
        return Err(format!(
            "process/mod.rs declares {} blocking PROCESS_MANAGER accessors (manager plus each \
             with_process_manager arm); this rule was written against 3",
            blocking.len()
        ));
    }
    let mut failures = Vec::new();
    for span in &blocking {
        let body = &process[span.open..=span.close];
        if !normalized_code(body).contains(NOTE) {
            failures.push(format!(
                "{} does not count itself against an open no-blocking scope",
                span.name
            ));
        }
    }
    let try_manager = function_body(process, "try_manager")
        .ok_or_else(|| "process/mod.rs declares no try_manager".to_string())?;
    if normalized_code(try_manager).contains(NOTE) {
        failures.push(
            "try_manager counts itself, which would make an IRQ-side non-blocking acquisition \
             read as the #821 defect"
                .to_string(),
        );
    }
    let scope_drop = spans
        .iter()
        .filter(|span| span.name == "drop")
        .map(|span| &process[span.open..=span.close])
        .find(|body| normalized_code(body).contains("NO_BLOCKING_PM_DEPTH"))
        .ok_or_else(|| {
            "no Drop body releases NO_BLOCKING_PM_DEPTH, so an entered scope would never close"
                .to_string()
        })?;
    if !normalized_code(scope_drop).contains("self.slot") {
        return Err(
            "the no-blocking scope's Drop does not release the slot it captured at entry, so a \
             region entered on one CPU and left on another would leave a depth behind"
                .to_string(),
        );
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// #821 rule 6. The 5 IRQ handlers that push a byte into the TTY take 0
/// blocking process-manager acquisitions between them.
///
/// `kernel/src/test_framework/` is excluded by name and on purpose: the #821
/// oracle drives the same entry while deliberately holding that lock, which is
/// what makes it an oracle.
fn validate_tty_irq_handlers(sources: &[(String, String)]) -> Result<(), String> {
    let mut callers = 0usize;
    let mut failures = Vec::new();
    for (path, text) in sources {
        if path.starts_with("kernel/src/test_framework/") || path == DRIVER {
            continue;
        }
        for span in function_spans(text) {
            let body = &text[span.open..=span.close];
            if !has_identifier(body, "push_char_nonblock") {
                continue;
            }
            callers += 1;
            for call in blocking_pm_calls(body) {
                failures.push(format!(
                    "{path}: {} pushes a byte into the TTY from interrupt context and calls \
                     {call}(), which blocks on PROCESS_MANAGER",
                    span.name
                ));
            }
        }
    }
    if callers < 4 {
        return Err(format!(
            "found {callers} interrupt-side callers of tty::push_char_nonblock; this rule was \
             written against 5 and a census that shrank below 4 is a rule that stopped looking"
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

// ---------------------------------------------------------------------------
// The rules, against the tree
// ---------------------------------------------------------------------------

#[test]
fn the_tty_input_irq_entry_takes_no_blocking_process_manager_acquisition() {
    validate_irq_entry_takes_no_blocking_pm(&repo_text(DRIVER)).expect("#821 rule 1");
}

#[test]
fn the_line_discipline_names_no_process_manager() {
    validate_line_discipline_is_process_free(&repo_text(LDISC)).expect("#821 rule 2");
}

#[test]
fn the_entry_defers_the_adoption_it_used_to_resolve_in_the_interrupt() {
    validate_deferred_arm(&repo_text(DRIVER)).expect("#821 rule 3");
}

#[test]
fn the_stdin_reader_takes_the_deferred_adoption() {
    validate_reader_takes_the_adoption(&repo_text(HANDLERS)).expect("#821 rule 4");
}

#[test]
fn each_blocking_process_manager_accessor_counts_itself() {
    validate_blocking_accessors_are_counted(&repo_text(PROCESS)).expect("#821 rule 5");
}

#[test]
fn every_tty_input_irq_handler_is_free_of_a_blocking_acquisition() {
    validate_tty_irq_handlers(&kernel_sources()).expect("#821 rule 6");
}

#[test]
fn both_boot_test_gates_pin_the_oracle_and_both_production_gates_pin_its_absence() {
    let strict = repo_text(STRICT_GATE);
    for required in [
        "TTY_IRQ_PM_ORACLE_PATTERN=",
        "tty_irq_pm_oracle_sample",
        "TTY input IRQ process-manager oracle marker missing or failed",
        "TTY input IRQ process-manager oracle reported failure",
        "'[TTY_IRQ_PM_ORACLE:'",
    ] {
        assert!(
            strict.contains(required),
            "{STRICT_GATE} no longer carries {required}, so the #821 oracle is unpinned on the \
             gate that decides kernel merges"
        );
    }
    // The selfcheck is what keeps the entry_us pin from being a `[0-9]+` that
    // accepts a boot which sat out the whole remote hold -- the reading #796's
    // review found missing on first_wait_us.
    assert!(
        strict.contains("entry_us=20022"),
        "{STRICT_GATE}'s selfcheck no longer proves the pattern rejects the unrepaired reading"
    );

    let prod = repo_text(PROD_GATE);
    for required in [
        "TTY_IRQ_PM_ORACLE_LITERAL=",
        "TTY_IRQ_PM_ORACLE_COUNT",
        "boot_tests-only TTY input IRQ oracle marker was present",
    ] {
        assert!(
            prod.contains(required),
            "{PROD_GATE} no longer asserts the #821 marker absent on the shipped profile"
        );
    }

    let x86 = repo_text(X86_BOOT_GATE);
    assert!(
        x86.contains("TTY_IRQ_PM_ORACLE_PATTERN="),
        "{X86_BOOT_GATE} no longer pins the #821 oracle line"
    );
    assert!(
        x86.contains("TTY_IRQ_PM_ORACLE_LINE"),
        "{X86_BOOT_GATE} no longer echoes the #821 oracle line it scored, so a preserved gate log \
         could not be read as a receipt for it"
    );
    assert!(
        x86.matches("$TTY_IRQ_PM_ORACLE_PATTERN").count() >= 2,
        "{X86_BOOT_GATE} references the #821 pattern fewer than twice, so either the wait chain \
         or the echo has lost it"
    );

    let x86_prod = repo_text(X86_PROD_GATE);
    assert!(
        x86_prod.contains("'[TTY_IRQ_PM_ORACLE:'"),
        "{X86_PROD_GATE} no longer lists the #821 marker among the test-only markers it forbids"
    );
}

// ---------------------------------------------------------------------------
// Anti-vacuity: each rule reddens on a deliberately broken copy of its input
// ---------------------------------------------------------------------------

fn inject_into_function(source: &str, name: &str, code: &str) -> String {
    let span = function_spans(source)
        .into_iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("find function {name} to mutate"));
    let mut mutated = source.to_string();
    mutated.insert_str(span.open + 1, code);
    mutated
}

fn replace_once(source: &str, from: &str, to: &str) -> String {
    assert!(
        source.contains(from),
        "the mutation's anchor is absent from the source it mutates: {from}"
    );
    source.replacen(from, to, 1)
}

#[test]
fn deliberately_broken_copies_redden_the_rules() {
    let driver = repo_text(DRIVER);
    let handlers = repo_text(HANDLERS);
    let process = repo_text(PROCESS);
    let ldisc = repo_text(LDISC);
    let sources = kernel_sources();

    // Leg A -- the green control. Without it the six failing legs below are
    // satisfied by a rule set that rejects everything.
    validate_irq_entry_takes_no_blocking_pm(&driver).expect("control: rule 1 is green");
    validate_line_discipline_is_process_free(&ldisc).expect("control: rule 2 is green");
    validate_deferred_arm(&driver).expect("control: rule 3 is green");
    validate_reader_takes_the_adoption(&handlers).expect("control: rule 4 is green");
    validate_blocking_accessors_are_counted(&process).expect("control: rule 5 is green");
    validate_tty_irq_handlers(&sources).expect("control: rule 6 is green");

    // Leg B -- the defect itself, put back: the blocking pid lookup returns to
    // the interrupt-context entry.
    let restored = replace_once(
        &driver,
        "            self.adopt_pending.store(true, Ordering::Release);",
        "            if let Some(current_pid) = crate::process::current_pid() {\n                \
         self.set_foreground_pgrp(current_pid.as_u64());\n            }\n            \
         self.adopt_pending.store(true, Ordering::Release);",
    );
    assert_ne!(restored, driver, "leg B's mutation must apply");
    let error = validate_irq_entry_takes_no_blocking_pm(&restored)
        .expect_err("leg B: the restored blocking pid lookup must redden rule 1");
    assert!(
        error.contains("current_pid"),
        "leg B reddened rule 1 for some other reason: {error}"
    );

    // Leg C -- the deferral count is deleted, so the branch records 0.
    let silent = replace_once(
        &driver,
        "TTY_IRQ_PM_DEFERRED.fetch_add(1, Ordering::Relaxed);",
        "",
    );
    assert_ne!(silent, driver, "leg C's mutation must apply");
    let error =
        validate_deferred_arm(&silent).expect_err("leg C: a deleted deferral must redden rule 3");
    assert!(
        error.contains("count the deferral"),
        "leg C reddened rule 3 for some other reason: {error}"
    );

    // Leg D -- the no-blocking scope is deleted from the entry.
    let unscoped = replace_once(
        &driver,
        "        let _no_blocking_pm = crate::process::NoBlockingProcessManagerScope::enter();",
        "",
    );
    assert_ne!(unscoped, driver, "leg D's mutation must apply");
    let error = validate_deferred_arm(&unscoped)
        .expect_err("leg D: a deleted no-blocking scope must redden rule 3");
    assert!(
        error.contains("NoBlockingProcessManagerScope"),
        "leg D reddened rule 3 for some other reason: {error}"
    );

    // Leg E -- the reader stops taking the adoption, so the work the entry
    // deferred is done by nobody.
    let unread = replace_once(
        &handlers,
        "crate::tty::driver::adopt_foreground_pgrp_from_reader(reader_pid);",
        "",
    );
    assert_ne!(unread, handlers, "leg E's mutation must apply");
    let error = validate_reader_takes_the_adoption(&unread)
        .expect_err("leg E: a reader that drops the adoption must redden rule 4");
    assert!(
        error.contains("done by nobody"),
        "leg E reddened rule 4 for some other reason: {error}"
    );

    // Leg F -- the reader takes it while it still holds the lock, which is the
    // ordering the repair exists to avoid.
    let early = replace_once(
        &handlers,
        "            // Drop the process manager lock before potentially blocking\n            drop(manager_guard);",
        "            crate::tty::driver::adopt_foreground_pgrp_from_reader(reader_pid);\n            drop(manager_guard);",
    );
    let early = replace_once(
        &early,
        "            // #821: take any foreground-pgrp adoption the input IRQ entry\n            // deferred. Deliberately after the guard is dropped, so this holds\n            // no PROCESS_MANAGER while it touches the TTY's own locks.\n            crate::tty::driver::adopt_foreground_pgrp_from_reader(reader_pid);\n",
        "",
    );
    assert_ne!(early, handlers, "leg F's mutation must apply");
    let error = validate_reader_takes_the_adoption(&early)
        .expect_err("leg F: taking the adoption under the guard must redden rule 4");
    assert!(
        error.contains("still holds"),
        "leg F reddened rule 4 for some other reason: {error}"
    );

    // Leg G -- the blocking accessor stops counting itself.
    let uncounted = replace_once(
        &process,
        "    note_blocking_process_manager_acquisition();\n    #[cfg(target_arch = \"aarch64\")]",
        "    #[cfg(target_arch = \"aarch64\")]",
    );
    assert_ne!(uncounted, process, "leg G's mutation must apply");
    let error = validate_blocking_accessors_are_counted(&uncounted)
        .expect_err("leg G: an uncounted manager() must redden rule 5");
    assert!(
        error.contains("does not count itself"),
        "leg G reddened rule 5 for some other reason: {error}"
    );

    // Leg H -- the non-blocking accessor starts counting itself, which would
    // report an allowed IRQ-side acquisition as the defect.
    let overcounted = replace_once(
        &process,
        "pub fn try_manager() -> Option<TryProcessManagerGuard> {",
        "pub fn try_manager() -> Option<TryProcessManagerGuard> {\n    note_blocking_process_manager_acquisition();",
    );
    assert_ne!(overcounted, process, "leg H's mutation must apply");
    let error = validate_blocking_accessors_are_counted(&overcounted)
        .expect_err("leg H: a counted try_manager() must redden rule 5");
    assert!(
        error.contains("try_manager counts itself"),
        "leg H reddened rule 5 for some other reason: {error}"
    );

    // Leg I -- an input IRQ handler acquires the lock itself, which is the same
    // defect one frame further out than where it was found.
    let handler_file = sources
        .iter()
        .find(|(path, text)| {
            path != DRIVER
                && !path.starts_with("kernel/src/test_framework/")
                && function_spans(text).into_iter().any(|span| {
                    has_identifier(&text[span.open..=span.close], "push_char_nonblock")
                })
        })
        .expect("at least one interrupt-side caller of push_char_nonblock");
    let handler_name = function_spans(&handler_file.1)
        .into_iter()
        .find(|span| has_identifier(&handler_file.1[span.open..=span.close], "push_char_nonblock"))
        .expect("the caller's enclosing function")
        .name;
    let mut mutated_sources = sources.clone();
    for entry in mutated_sources.iter_mut() {
        if entry.0 == handler_file.0 {
            entry.1 = inject_into_function(
                &entry.1,
                &handler_name,
                "\n    let _ = crate::process::manager();",
            );
        }
    }
    assert_ne!(mutated_sources, sources, "leg I's mutation must apply");
    let error = validate_tty_irq_handlers(&mutated_sources)
        .expect_err("leg I: a handler that blocks on the lock must redden rule 6");
    assert!(
        error.contains("blocks on PROCESS_MANAGER"),
        "leg I reddened rule 6 for some other reason: {error}"
    );

    // Leg J -- the line discipline reaches into process management, which is
    // what rule 1's stop at the driver.rs boundary depends on not happening.
    let reaching = replace_once(
        &ldisc,
        "    /// Get the number of bytes available for reading",
        "    fn peek_pid() -> Option<u64> {\n        crate::process::current_pid().map(|p| p.as_u64())\n    }\n\n    /// Get the number of bytes available for reading",
    );
    assert_ne!(reaching, ldisc, "leg J's mutation must apply");
    validate_line_discipline_is_process_free(&reaching)
        .expect_err("leg J: a process-aware line discipline must redden rule 2");
}

// ---------------------------------------------------------------------------
// The gates themselves, run over committed serials
// ---------------------------------------------------------------------------
//
// R157/ASID-01's lesson, applied here: a test that asserts a gate script
// CONTAINS a pattern string stays true of a script whose assertions were
// deleted and whose variable definitions remain -- which was demonstrated on
// that round. So the gates are RUN, in their scoring-only mode, over the
// serials this branch recorded them on and over mutations of those serials,
// and the exit status is the measurement.

fn score_with_gate(gate: &str, variable: &str, serial: &Path) -> (bool, String) {
    let script = repo_text(gate);
    assert!(
        script.contains(variable),
        "{gate} has no {variable} scoring-only entry point, so its verdict rules cannot be run \
         from a test -- and invoking it without one would boot QEMU"
    );
    let output = std::process::Command::new("bash")
        .arg(repo_root().join(gate))
        .env(variable, serial)
        .current_dir(repo_root())
        .output()
        .unwrap_or_else(|error| panic!("run {gate} in scoring-only mode: {error}"));
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

#[test]
fn the_aarch64_gates_score_the_821_oracle_rather_than_merely_naming_it() {
    let scratch = std::env::temp_dir().join(format!("breenix-821-gate-legs-{}", std::process::id()));
    fs::create_dir_all(&scratch).expect("create the scratch directory for the gate legs");
    let write = |name: &str, body: &str| -> PathBuf {
        let path = scratch.join(format!("{name}.txt"));
        fs::write(&path, body).expect("write a gate leg serial");
        path
    };

    // --- the boot-test gate, over the serial it was recorded green on -------
    let green = repo_text(GREEN_SERIAL);
    assert!(
        green.contains(ORACLE_MARKER),
        "{GREEN_SERIAL} is the green baseline for {STRICT_GATE} and has to carry the line it is \
         the baseline for"
    );

    // Leg A. Anti-vacuity for the 4 legs below: a gate that rejected each
    // serial handed to it would satisfy them without scoring a boot.
    let (passed, output) = score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("green", &green));
    assert!(
        passed,
        "{STRICT_GATE} has to pass the serial it was recorded green on, or the failing legs below \
         say nothing: {output}"
    );

    // Leg B. The oracle reported failure. That is the defect class, live.
    let failed_verdict = green.replace(":PASS:peer_hold]", ":FAIL:peer_hold]");
    assert_ne!(failed_verdict, green, "leg B's mutation must apply");
    let (passed, output) = score_with_gate(
        STRICT_GATE,
        "BREENIX_STRICT_SCORE_ONLY",
        &write("failed", &failed_verdict),
    );
    assert!(!passed, "{STRICT_GATE} passed a serial whose #821 oracle failed: {output}");
    assert!(
        output.contains("TTY input IRQ process-manager oracle"),
        "{STRICT_GATE} failed the serial, but for some other reason: {output}"
    );

    // Leg C. The oracle did not print at all. A gate that fails only on a bad
    // reading is satisfied by a kernel that stopped reporting.
    let deleted: String = green
        .lines()
        .filter(|line| !line.contains(ORACLE_MARKER))
        .map(|line| format!("{line}\n"))
        .collect();
    let (passed, output) =
        score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("deleted", &deleted));
    assert!(!passed, "{STRICT_GATE} passed a serial with no #821 oracle line at all: {output}");

    // Leg D. The entry completed, but only after waiting out the whole remote
    // hold -- the unrepaired reading this branch actually recorded.
    let waited = green.replace(":entry_us=2:", ":entry_us=20022:");
    assert_ne!(waited, green, "leg D's mutation must apply");
    let (passed, output) =
        score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("waited", &waited));
    assert!(
        !passed,
        "{STRICT_GATE} passed a serial whose input IRQ entry waited 20 ms for the lock: {output}"
    );

    // Leg E. The property itself: a blocking acquisition inside the scope.
    let acquired = green.replace("pm_blocking_acquires=0", "pm_blocking_acquires=1");
    assert_ne!(acquired, green, "leg E's mutation must apply");
    let (passed, output) =
        score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("acquired", &acquired));
    assert!(
        !passed,
        "{STRICT_GATE} passed a serial reporting a blocking PROCESS_MANAGER acquisition inside \
         the input IRQ entry: {output}"
    );

    // --- the production gate, over the serial it was recorded green on ------
    let prod = repo_text(PROD_SERIAL);
    assert!(
        !prod.contains(ORACLE_MARKER),
        "{PROD_SERIAL} is a shipped-profile boot and must not carry a boot_tests-only marker"
    );

    // Leg F. Anti-vacuity, as leg A.
    let (passed, output) =
        score_with_gate(PROD_GATE, "BREENIX_PROD_SCORE_ONLY", &write("prod-green", &prod));
    assert!(
        passed,
        "{PROD_GATE} has to pass the serial it was recorded green on: {output}"
    );

    // Leg G. The boot_tests-only oracle appeared on the shipped profile.
    let leaked = format!(
        "{prod}[TTY_IRQ_PM_ORACLE:aarch64:fg_unset_before=1:pm_blocking_acquires=0:deferred=2:\
         pgrp_set_by_entry=0:processed=2:buffered=2:irqs_enabled_before=1:holder_cpu=1:\
         pm_busy_probe=1:hold_us=20000:entry_us=2:joined=1:adopted=1:adopted_pgrp=821:restored=1:\
         PASS:peer_hold]\n"
    );
    let (passed, output) =
        score_with_gate(PROD_GATE, "BREENIX_PROD_SCORE_ONLY", &write("prod-leaked", &leaked));
    assert!(
        !passed,
        "{PROD_GATE} passed a shipped-profile serial carrying the boot_tests-only #821 marker: \
         {output}"
    );
    assert!(
        output.contains("TTY input IRQ oracle marker was present"),
        "{PROD_GATE} failed the leaked serial, but for some other reason: {output}"
    );

    fs::remove_dir_all(&scratch).ok();
}
