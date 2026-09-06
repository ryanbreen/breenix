//! #822 -- no TTY interrupt entry touches the console's `foreground_pgrp`
//! mutex, and the value it used to read there is published as a snapshot the
//! interrupt side can read without a lock.
//!
//! The defect these rules pin: the console `TtyDevice`'s
//! `foreground_pgrp: Mutex<Option<u64>>` is a plain spin lock with no mask
//! operation of its own, so each of the 5 acquisitions of it in `driver.rs` --
//! reached from `tcsetpgrp` through `tty/ioctl.rs`, from TIOCGPGRP, and from
//! `process/creation.rs` at process creation -- runs with interrupts unmasked,
//! on both architectures. 2 interrupt-side paths read it anyway:
//! `input_char_nonblock` and
//! `send_signal_to_foreground_nonblock`. Both used `try_lock`, so neither
//! waited; what they did instead was lose the keystroke's meaning when the
//! lock was busy -- the second one DROPPED the Ctrl+C it was resolving, and
//! announced the drop through `serial_println!`, a second lock taken from
//! interrupt context.
//!
//! The rules are CENSUSES, not lists of known names:
//!
//! * the interrupt-side roots inside `kernel/src/tty/driver.rs` are read out
//!   of that file as the functions whose names end in `_nonblock` -- its own
//!   naming convention for the interrupt-context twins -- so a seventh one
//!   added later is swept in without an edit here;
//! * the acquisition sites are read out of `driver.rs` as the `.lock()` and
//!   `.try_lock()` calls on the `foreground_pgrp` field itself, so a sixth one
//!   added later has to carry the counter with it.
//!
//! The call graph inside `driver.rs` is walked only through `self.`, `Self::`
//! and unqualified calls, for the reason #821's file records: a
//! name-plus-receiver resolver once attributed `ldisc.input_char(..)` -- a
//! `LineDiscipline` method -- to `TtyDevice::input_char`.
//!
//! Each of the 6 rules is mutation-tested at the bottom of the file against
//! in-memory copies, with a green control, so a rule that had quietly stopped
//! matching cannot pass forever. 7 further legs run the aarch64 strict gate for
//! real, in its scoring-only mode, over committed serials.

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

const DRIVER: &str = "kernel/src/tty/driver.rs";
const IOCTL: &str = "kernel/src/tty/ioctl.rs";
const PROCESS: &str = "kernel/src/process/mod.rs";
const STRICT_GATE: &str = "docker/qemu/run-aarch64-boot-test-strict.sh";
const PROD_GATE: &str = "docker/qemu/run-aarch64-prod-profile-boot-test.sh";
const X86_BOOT_GATE: &str = "docker/qemu/run-x86-boot-tests.sh";
const X86_PROD_GATE: &str = "docker/qemu/run-x86-prod-profile-boot-test.sh";
const GREEN_SERIAL: &str =
    "docs/planning/green-program/irq-locks/serials/822/02-a64-green-repaired-serial.txt";
const ORACLE_MARKER: &str = "[TTY_IRQ_FG_ORACLE:";

/// The field whose mutex this round takes off the interrupt path.
const FIELD: &str = "foreground_pgrp";
/// The counter call each of the 5 acquisitions of it carries.
const NOTE: &str = "note_foreground_pgrp_acquisition(";
/// The lock-free reader that replaced those acquisitions on the interrupt side.
const SNAPSHOT: &str = "foreground_pgrp_snapshot";

/// The acquisition spellings. `try_lock` is here BECAUSE it does not wait: a
/// `try_lock` on this mutex from interrupt context cannot wedge, and that is
/// exactly why it was the wrong instrument -- it answered "no value" and the
/// caller degraded, dropping a signal. Both spellings belong off the interrupt
/// path.
const ACQUISITIONS: [&str; 2] = ["lock", "try_lock"];

/// Offsets in `body` at which the `foreground_pgrp` FIELD is acquired, as
/// (offset, spelling). A mention that is not `<field> . lock (` or
/// `<field> . try_lock (` is not an acquisition, and `identifier_offsets`
/// already refuses to match `foreground_pgrp_snapshot` when asked for
/// `foreground_pgrp`.
fn foreground_pgrp_acquisitions(body: &str) -> Vec<(usize, &'static str)> {
    let mask = code_mask(body);
    let bytes = body.as_bytes();
    let mut found = Vec::new();
    for offset in identifier_offsets(body, &mask, FIELD) {
        let mut cursor = offset + FIELD.len();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'.') {
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        for spelling in ACQUISITIONS {
            if !body[cursor..].starts_with(spelling) {
                continue;
            }
            let mut after = cursor + spelling.len();
            while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                after += 1;
            }
            if bytes.get(after) == Some(&b'(') {
                found.push((offset, spelling));
                break;
            }
        }
    }
    found
}

/// The `driver.rs` functions the interrupt handlers enter, plus everything
/// reachable from them through this file's own calls.
fn interrupt_reachable(driver: &str) -> Result<Vec<String>, String> {
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
        return Err(
            "driver.rs no longer declares input_char_nonblock, the entry both architectures' \
             input interrupts reach"
                .to_string(),
        );
    }
    if !roots.contains(&"send_signal_to_foreground_nonblock") {
        return Err(
            "driver.rs no longer declares send_signal_to_foreground_nonblock, the interrupt-side \
             signal path #822 is about"
                .to_string(),
        );
    }

    let mut queue: Vec<&str> = roots.clone();
    let mut seen: Vec<String> = Vec::new();
    while let Some(name) = queue.pop() {
        if seen.iter().any(|already| already == name) {
            continue;
        }
        seen.push(name.to_string());
        let Some(span) = spans.iter().find(|span| span.name == name) else {
            continue;
        };
        for edge in driver_edges(&driver[span.open..=span.close], &defined) {
            queue.push(edge);
        }
    }
    Ok(seen)
}

/// #822 rule 1. The functions reachable from a `driver.rs` interrupt-context
/// entry take 0 acquisitions of the `foreground_pgrp` mutex between them, by
/// either of the 2 spellings.
fn validate_irq_entry_takes_no_foreground_pgrp_lock(driver: &str) -> Result<(), String> {
    let reachable = interrupt_reachable(driver)?;
    let spans = function_spans(driver);
    let mut failures = Vec::new();
    for name in &reachable {
        let Some(span) = spans.iter().find(|span| &span.name == name) else {
            continue;
        };
        for (_, spelling) in foreground_pgrp_acquisitions(&driver[span.open..=span.close]) {
            failures.push(format!(
                "{name} is reachable from a TTY interrupt-context entry and calls \
                 foreground_pgrp.{spelling}(), on a mutex a thread holds with interrupts unmasked"
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// #822 rule 2. Each of the 5 acquisitions of the field in `driver.rs` is
/// counted, so the boot-test detector reads a reintroduction rather than
/// missing it.
fn validate_every_acquisition_is_counted(driver: &str) -> Result<(), String> {
    let spans = function_spans(driver);
    let mut sites = 0usize;
    let mut failures = Vec::new();
    for span in &spans {
        let body = &driver[span.open..=span.close];
        let here = foreground_pgrp_acquisitions(body);
        if here.is_empty() {
            continue;
        }
        sites += here.len();
        if !normalized_code(body).contains(NOTE) {
            failures.push(format!(
                "{} acquires foreground_pgrp {} time(s) and does not count itself, so the \
                 boot-test detector would read 0 while the interrupt path waited",
                span.name,
                here.len()
            ));
        }
    }
    if sites < 5 {
        return Err(format!(
            "found {sites} acquisitions of foreground_pgrp in driver.rs; this rule was written \
             against 5, and a census that fell below that is a rule that stopped looking"
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// #822 rule 3. The snapshot is published inside the mutex and after the field
/// is written, and by the file's only writer of that field.
fn validate_snapshot_is_published_under_the_mutex(driver: &str) -> Result<(), String> {
    let writer = function_body(driver, "store_foreground_pgrp")
        .ok_or_else(|| "driver.rs declares no store_foreground_pgrp".to_string())?;
    let mask = code_mask(writer);
    let acquisitions = foreground_pgrp_acquisitions(writer);
    if acquisitions.len() != 1 || acquisitions[0].1 != "lock" {
        return Err(format!(
            "store_foreground_pgrp takes {} acquisition(s) of foreground_pgrp; it has to take \
             exactly one, blocking, and hold it across the publication",
            acquisitions.len()
        ));
    }
    let field_write = identifier_offsets(writer, &mask, "guard")
        .into_iter()
        .find(|offset| writer[*offset..].starts_with("guard = "))
        .ok_or_else(|| {
            "store_foreground_pgrp does not write the field through the guard it took, so the \
             mutex is not what serialises its writers"
                .to_string()
        })?;
    let publication = identifier_offsets(writer, &mask, SNAPSHOT)
        .into_iter()
        .next()
        .ok_or_else(|| "store_foreground_pgrp does not publish the snapshot".to_string())?;
    let release = identifier_offsets(writer, &mask, "drop")
        .into_iter()
        .find(|offset| writer[*offset..].starts_with("drop(guard)"))
        .ok_or_else(|| {
            "store_foreground_pgrp does not release the guard by name, so the publication cannot \
             be shown to happen inside the critical section"
                .to_string()
        })?;
    if !(field_write < publication && publication < release) {
        return Err(
            "store_foreground_pgrp does not write the field, then publish the snapshot, then \
             release the mutex, in that order -- so the snapshot can lead the field"
                .to_string(),
        );
    }

    // The setters go through it, so there is one writer and not three.
    for setter in ["set_foreground_pgrp", "set_foreground_pgrp_raw_for_test"] {
        let body = function_body(driver, setter)
            .ok_or_else(|| format!("driver.rs declares no {setter}"))?;
        if !normalized_code(body).contains("store_foreground_pgrp(") {
            return Err(format!(
                "{setter} writes the foreground pgrp without going through \
                 store_foreground_pgrp, so it can leave the snapshot behind"
            ));
        }
    }
    Ok(())
}

/// #822 rule 4. Each interrupt-side reader reads the snapshot.
fn validate_irq_readers_use_the_snapshot(driver: &str) -> Result<(), String> {
    let mut failures = Vec::new();
    for reader in ["input_char_nonblock", "send_signal_to_foreground_nonblock"] {
        let body = function_body(driver, reader)
            .ok_or_else(|| format!("driver.rs declares no {reader}"))?;
        if !normalized_code(body).contains("foreground_pgrp_snapshot()") {
            failures.push(format!(
                "{reader} does not read the foreground pgrp from the snapshot, so it either \
                 dropped the question or went back to the mutex"
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// #822 rule 5. TIOCSPGRP still refuses a negative pgrp.
///
/// This is what keeps `u64::MAX` -- the snapshot's "unset" sentinel -- out of
/// the field. `pgrp as u64` on a negative `i32` is exactly that value, so
/// without this refusal a `tcsetpgrp(-1)` would publish a foreground pgrp the
/// interrupt side reads back as "no foreground pgrp".
fn validate_the_sentinel_stays_unreachable(ioctl: &str) -> Result<(), String> {
    let body = function_body(ioctl, "handle_tiocspgrp")
        .ok_or_else(|| "ioctl.rs declares no handle_tiocspgrp".to_string())?;
    let normalized = normalized_code(body);
    if !normalized.contains("if pgrp < 0 { return Err(EINVAL); }") {
        return Err(
            "handle_tiocspgrp no longer refuses a negative pgrp, so `pgrp as u64` can publish \
             u64::MAX -- the snapshot's own unset sentinel"
                .to_string(),
        );
    }
    if !normalized.contains("set_foreground_pgrp(pgrp as u64)") {
        return Err(
            "handle_tiocspgrp no longer sets the foreground pgrp through the checked path this \
             rule reads"
                .to_string(),
        );
    }
    Ok(())
}

/// #822 rule 6. The detector exists, reads the per-CPU scope depth, and is
/// what `driver.rs` consults.
fn validate_the_detector_is_wired(driver: &str, process: &str) -> Result<(), String> {
    let predicate = function_body(process, "in_no_blocking_process_manager_scope").ok_or_else(
        || {
            "process/mod.rs declares no in_no_blocking_process_manager_scope, so an acquisition \
             taken inside a TTY interrupt entry would be counted by nothing"
                .to_string()
        },
    )?;
    if !normalized_code(predicate).contains("NO_BLOCKING_PM_DEPTH") {
        return Err(
            "in_no_blocking_process_manager_scope does not read the per-CPU scope depth, so it \
             cannot say whether this CPU is inside a TTY interrupt entry"
                .to_string(),
        );
    }
    let note = function_body(driver, "note_foreground_pgrp_acquisition")
        .ok_or_else(|| "driver.rs declares no note_foreground_pgrp_acquisition".to_string())?;
    let note = normalized_code(note);
    if !note.contains("in_no_blocking_process_manager_scope()") {
        return Err(
            "note_foreground_pgrp_acquisition does not consult the scope, so it would count \
             every thread-context acquisition too and read above 0 forever"
                .to_string(),
        );
    }
    for counter in ["TTY_IRQ_FG_LOCK_TOUCHES", "TTY_IRQ_FG_BLOCKING_ACQUIRES"] {
        if !note.contains(counter) {
            return Err(format!(
                "note_foreground_pgrp_acquisition does not bump {counter}, which the oracle reads \
                 as its property"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The rules, against the tree
// ---------------------------------------------------------------------------

#[test]
fn no_tty_interrupt_entry_acquires_the_foreground_pgrp_mutex() {
    validate_irq_entry_takes_no_foreground_pgrp_lock(&repo_text(DRIVER)).expect("#822 rule 1");
}

#[test]
fn every_foreground_pgrp_acquisition_counts_itself() {
    validate_every_acquisition_is_counted(&repo_text(DRIVER)).expect("#822 rule 2");
}

#[test]
fn the_snapshot_is_published_inside_the_mutex() {
    validate_snapshot_is_published_under_the_mutex(&repo_text(DRIVER)).expect("#822 rule 3");
}

#[test]
fn each_interrupt_side_reader_reads_the_snapshot() {
    validate_irq_readers_use_the_snapshot(&repo_text(DRIVER)).expect("#822 rule 4");
}

#[test]
fn tiocspgrp_still_refuses_the_value_the_sentinel_uses() {
    validate_the_sentinel_stays_unreachable(&repo_text(IOCTL)).expect("#822 rule 5");
}

#[test]
fn the_foreground_pgrp_detector_is_wired_to_the_scope() {
    validate_the_detector_is_wired(&repo_text(DRIVER), &repo_text(PROCESS)).expect("#822 rule 6");
}

#[test]
fn both_boot_test_gates_pin_the_oracle_and_both_production_gates_pin_its_absence() {
    let strict = repo_text(STRICT_GATE);
    for required in [
        "TTY_IRQ_FG_ORACLE_PATTERN=",
        "tty_irq_fg_oracle_sample",
        "TTY input IRQ foreground-pgrp oracle marker missing or failed",
        "TTY input IRQ foreground-pgrp oracle reported failure",
        "'[TTY_IRQ_FG_ORACLE:'",
    ] {
        assert!(
            strict.contains(required),
            "{STRICT_GATE} no longer carries {required}, so the #822 oracle is unpinned on the \
             gate that decides kernel merges"
        );
    }
    // The selfcheck is what keeps the entry_us pin from being a `[0-9]+` that
    // accepts a boot which sat out the whole hold.
    assert!(
        strict.contains("entry_us=20022"),
        "{STRICT_GATE}'s selfcheck no longer proves a pattern rejects the unrepaired reading"
    );

    let prod = repo_text(PROD_GATE);
    for required in [
        "TTY_IRQ_FG_ORACLE_LITERAL=",
        "TTY_IRQ_FG_ORACLE_COUNT",
        "boot_tests-only TTY foreground-pgrp oracle marker was present",
    ] {
        assert!(
            prod.contains(required),
            "{PROD_GATE} no longer asserts the #822 marker absent on the shipped profile"
        );
    }

    let x86 = repo_text(X86_BOOT_GATE);
    assert!(
        x86.contains("TTY_IRQ_FG_ORACLE_PATTERN="),
        "{X86_BOOT_GATE} no longer pins the #822 oracle line"
    );
    assert!(
        x86.contains("TTY_IRQ_FG_ORACLE_LINE"),
        "{X86_BOOT_GATE} no longer echoes the #822 oracle line it scored, so a preserved gate log \
         could not be read as a receipt for it"
    );
    assert!(
        x86.matches("$TTY_IRQ_FG_ORACLE_PATTERN").count() >= 2,
        "{X86_BOOT_GATE} references the #822 pattern fewer than twice, so either the wait chain \
         or the echo has lost it"
    );

    let x86_prod = repo_text(X86_PROD_GATE);
    assert!(
        x86_prod.contains("'[TTY_IRQ_FG_ORACLE:'"),
        "{X86_PROD_GATE} no longer lists the #822 marker among the test-only markers it forbids"
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
    let ioctl = repo_text(IOCTL);
    let process = repo_text(PROCESS);

    // Leg A -- the green control. Without it the failing legs below are
    // satisfied by a rule that rejects everything.
    validate_irq_entry_takes_no_foreground_pgrp_lock(&driver).expect("control: rule 1 is green");
    validate_every_acquisition_is_counted(&driver).expect("control: rule 2 is green");
    validate_snapshot_is_published_under_the_mutex(&driver).expect("control: rule 3 is green");
    validate_irq_readers_use_the_snapshot(&driver).expect("control: rule 4 is green");
    validate_the_sentinel_stays_unreachable(&ioctl).expect("control: rule 5 is green");
    validate_the_detector_is_wired(&driver, &process).expect("control: rule 6 is green");

    // Leg B -- the blocking acquisition restored in the interrupt-side signal
    // path. This is the census's own F4 shape, and the mutation this branch
    // measured at entry_us=20048 on a live boot.
    let blocking_restored = inject_into_function(
        &driver,
        "send_signal_to_foreground_nonblock",
        "\n        let _fg = *self.foreground_pgrp.lock();\n",
    );
    assert_ne!(blocking_restored, driver, "leg B's mutation must apply");
    let error = validate_irq_entry_takes_no_foreground_pgrp_lock(&blocking_restored)
        .expect_err("leg B: rule 1 has to redden on a restored blocking acquisition");
    assert!(
        error.contains("send_signal_to_foreground_nonblock") && error.contains("foreground_pgrp.lock()"),
        "leg B reddened for the wrong reason: {error}"
    );

    // Leg C -- the try_lock restored in the input entry. It cannot wedge, and
    // it is still the defect: a busy lock answers "no value" and the caller
    // degrades.
    let try_restored = inject_into_function(
        &driver,
        "input_char_nonblock",
        "\n        let _fg = self.foreground_pgrp.try_lock();\n",
    );
    assert_ne!(try_restored, driver, "leg C's mutation must apply");
    let error = validate_irq_entry_takes_no_foreground_pgrp_lock(&try_restored)
        .expect_err("leg C: rule 1 has to redden on a restored try_lock");
    assert!(
        error.contains("input_char_nonblock") && error.contains("foreground_pgrp.try_lock()"),
        "leg C reddened for the wrong reason: {error}"
    );

    // Leg D -- an acquisition that does not count itself.
    let uncounted = replace_once(
        &driver,
        "        note_foreground_pgrp_acquisition(true);\n        *self.foreground_pgrp.lock()\n",
        "        *self.foreground_pgrp.lock()\n",
    );
    assert_ne!(uncounted, driver, "leg D's mutation must apply");
    let error = validate_every_acquisition_is_counted(&uncounted)
        .expect_err("leg D: rule 2 has to redden on an uncounted acquisition");
    assert!(
        error.contains("get_foreground_pgrp") && error.contains("does not count itself"),
        "leg D reddened for the wrong reason: {error}"
    );

    // Leg E -- the publication moved out of the critical section.
    let published_late = replace_once(
        &driver,
        "        self.foreground_pgrp_snapshot\n            .store(pgrp.unwrap_or(FOREGROUND_PGRP_UNSET), Ordering::Release);\n        drop(guard);\n",
        "        drop(guard);\n        self.foreground_pgrp_snapshot\n            .store(pgrp.unwrap_or(FOREGROUND_PGRP_UNSET), Ordering::Release);\n",
    );
    assert_ne!(published_late, driver, "leg E's mutation must apply");
    let error = validate_snapshot_is_published_under_the_mutex(&published_late)
        .expect_err("leg E: rule 3 has to redden when the publication leaves the mutex");
    assert!(
        error.contains("in that order"),
        "leg E reddened for the wrong reason: {error}"
    );

    // Leg F -- a setter that writes the field without the publisher.
    let unpublished_setter = replace_once(
        &driver,
        "    pub fn set_foreground_pgrp(&self, pgrp: u64) {\n        self.store_foreground_pgrp(Some(pgrp));\n    }",
        "    pub fn set_foreground_pgrp(&self, pgrp: u64) {\n        note_foreground_pgrp_acquisition(true);\n        *self.foreground_pgrp.lock() = Some(pgrp);\n    }",
    );
    assert_ne!(unpublished_setter, driver, "leg F's mutation must apply");
    let error = validate_snapshot_is_published_under_the_mutex(&unpublished_setter)
        .expect_err("leg F: rule 3 has to redden on a setter that skips the publisher");
    assert!(
        error.contains("set_foreground_pgrp") && error.contains("leave the snapshot behind"),
        "leg F reddened for the wrong reason: {error}"
    );

    // Leg G -- the interrupt-side signal path stops reading the snapshot.
    let reader_dropped = replace_once(
        &driver,
        "        if let Some(pgrp) = self.foreground_pgrp_snapshot() {\n            let pid = ProcessId::new(pgrp);",
        "        if let Some(pgrp) = None::<u64> {\n            let pid = ProcessId::new(pgrp);",
    );
    assert_ne!(reader_dropped, driver, "leg G's mutation must apply");
    let error = validate_irq_readers_use_the_snapshot(&reader_dropped)
        .expect_err("leg G: rule 4 has to redden when a reader stops reading the snapshot");
    assert!(
        error.contains("send_signal_to_foreground_nonblock"),
        "leg G reddened for the wrong reason: {error}"
    );

    // Leg H -- TIOCSPGRP stops refusing a negative pgrp, which is what keeps
    // the sentinel unreachable.
    let sentinel_reachable = replace_once(
        &ioctl,
        "    if pgrp < 0 {\n        return Err(EINVAL);\n    }\n",
        "",
    );
    assert_ne!(sentinel_reachable, ioctl, "leg H's mutation must apply");
    let error = validate_the_sentinel_stays_unreachable(&sentinel_reachable)
        .expect_err("leg H: rule 5 has to redden when the negative pgrp is accepted");
    assert!(
        error.contains("u64::MAX"),
        "leg H reddened for the wrong reason: {error}"
    );

    // Leg I -- the detector stops consulting the scope, so it would count each
    // of the 5 thread-context acquisitions too and read above 0 forever.
    let detector_unscoped = replace_once(
        &driver,
        "    if crate::process::in_no_blocking_process_manager_scope() {",
        "    if true {",
    );
    assert_ne!(detector_unscoped, driver, "leg I's mutation must apply");
    let error = validate_the_detector_is_wired(&detector_unscoped, &process)
        .expect_err("leg I: rule 6 has to redden when the detector stops consulting the scope");
    assert!(
        error.contains("does not consult the scope"),
        "leg I reddened for the wrong reason: {error}"
    );

    // Leg J -- the census itself shrinks. A rule that stopped looking is a
    // rule that passes forever.
    let census_gone = replace_once(
        &driver,
        "    pub fn input_char_nonblock(&self, c: u8) -> bool {",
        "    pub fn input_char_blocking_renamed(&self, c: u8) -> bool {",
    );
    assert_ne!(census_gone, driver, "leg J's mutation must apply");
    let error = validate_irq_entry_takes_no_foreground_pgrp_lock(&census_gone)
        .expect_err("leg J: rule 1 has to redden when its own census loses the entry");
    assert!(
        error.contains("input_char_nonblock"),
        "leg J reddened for the wrong reason: {error}"
    );
}

// ---------------------------------------------------------------------------
// Gate legs: the strict gate is RUN, in scoring-only mode, over real serials
// ---------------------------------------------------------------------------

/// Rewrite `from` to `to`, but only on the lines that carry this round's
/// oracle marker.
///
/// #821's own gate-mutation table was written without this and reddened the
/// gate through a DIFFERENT oracle's line: 3 of these fields -- `:PASS:`,
/// `processed=`, `restored=` -- are spelled the same way on #796's, #812's and
/// #821's verdict lines. A leg that reddens the gate through someone else's
/// marker measures 0 facts about this one.
fn mutate_oracle_line(serial: &str, from: &str, to: &str) -> String {
    serial
        .lines()
        .map(|line| {
            if line.contains(ORACLE_MARKER) {
                line.replace(from, to)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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
fn the_strict_gate_scores_the_822_oracle_rather_than_merely_naming_it() {
    let scratch = std::env::temp_dir().join(format!("breenix-822-gate-legs-{}", std::process::id()));
    fs::create_dir_all(&scratch).expect("create the scratch directory for the gate legs");
    let write = |name: &str, body: &str| -> PathBuf {
        let path = scratch.join(format!("{name}.txt"));
        fs::write(&path, body).expect("write a gate leg serial");
        path
    };

    let green = repo_text(GREEN_SERIAL);
    assert!(
        green.contains(ORACLE_MARKER),
        "{GREEN_SERIAL} is the green baseline for {STRICT_GATE} and has to carry the line it is \
         the baseline for"
    );

    // Leg A. Anti-vacuity for the 6 legs below: a gate that rejected each
    // serial handed to it would satisfy them without scoring a boot.
    let (passed, output) =
        score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("green", &green));
    assert!(
        passed,
        "{STRICT_GATE} has to pass the serial it was recorded green on, or the failing legs below \
         say nothing: {output}"
    );

    // Leg B. The oracle reported failure.
    let failed_verdict = mutate_oracle_line(&green, ":PASS:peer_hold]", ":FAIL:peer_hold]");
    assert_ne!(failed_verdict, green, "leg B's mutation must apply");
    let (passed, output) = score_with_gate(
        STRICT_GATE,
        "BREENIX_STRICT_SCORE_ONLY",
        &write("failed", &failed_verdict),
    );
    assert!(!passed, "{STRICT_GATE} passed a serial whose #822 oracle failed: {output}");
    assert!(
        output.contains("TTY input IRQ foreground-pgrp oracle"),
        "{STRICT_GATE} failed the serial, but for some other reason: {output}"
    );

    // Leg C. The oracle's line is absent from the serial.
    let deleted: String = green
        .lines()
        .filter(|line| !line.contains("TTY_IRQ_FG_ORACLE"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_ne!(deleted, green, "leg C's mutation must apply");
    let (passed, output) =
        score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("deleted", &deleted));
    assert!(!passed, "{STRICT_GATE} passed a serial with no #822 oracle line: {output}");
    assert!(
        output.contains("TTY input IRQ foreground-pgrp oracle marker missing or failed"),
        "{STRICT_GATE} failed the serial, but for some other reason: {output}"
    );

    // Leg D. The entry sat out the whole hold. This is the reading the gate's
    // own selfcheck exists for. The reading is read out of the serial rather
    // than written here, so a re-recorded fixture cannot silently turn this leg
    // into a no-op the way a hard-coded microsecond count would.
    let recorded_entry_us = green
        .lines()
        .find(|line| line.contains(ORACLE_MARKER))
        .and_then(|line| line.split(":entry_us=").nth(1))
        .and_then(|rest| rest.split(':').next())
        .expect("the green serial's oracle line carries an entry_us reading")
        .to_string();
    let waited = mutate_oracle_line(
        &green,
        &format!(":entry_us={recorded_entry_us}:"),
        ":entry_us=20022:",
    );
    assert_ne!(waited, green, "leg D's mutation must apply");
    let (passed, output) =
        score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("waited", &waited));
    assert!(
        !passed,
        "{STRICT_GATE} passed a serial whose input IRQ entry waited 20 ms: {output}"
    );

    // Leg E. The entry touched the mutex.
    let touched = mutate_oracle_line(&green, ":fg_lock_touches=0:", ":fg_lock_touches=1:");
    assert_ne!(touched, green, "leg E's mutation must apply");
    let (passed, output) =
        score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("touched", &touched));
    assert!(
        !passed,
        "{STRICT_GATE} passed a serial whose input IRQ entry acquired foreground_pgrp: {output}"
    );

    // Leg F. The Ctrl+C was dropped under the hold -- the defect's own
    // consequence, independent of any counter.
    let dropped = mutate_oracle_line(&green, ":sig_calls=1:", ":sig_calls=0:");
    assert_ne!(dropped, green, "leg F's mutation must apply");
    let (passed, output) =
        score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("dropped", &dropped));
    assert!(
        !passed,
        "{STRICT_GATE} passed a serial whose injected SIGINT never reached the signal path: \
         {output}"
    );

    // Leg G. The lock was not actually held at the instant of the injection, so
    // the measurement carries 0 information about a held lock.
    let unheld = mutate_oracle_line(&green, ":fg_busy_probe=1:", ":fg_busy_probe=0:");
    assert_ne!(unheld, green, "leg G's mutation must apply");
    let (passed, output) =
        score_with_gate(STRICT_GATE, "BREENIX_STRICT_SCORE_ONLY", &write("unheld", &unheld));
    assert!(
        !passed,
        "{STRICT_GATE} passed a serial whose oracle never had the lock held: {output}"
    );

    let _ = fs::remove_dir_all(&scratch);
}
