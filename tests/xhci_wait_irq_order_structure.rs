//! #482 -- each of the two synchronous xHCI waits arms its transient
//! completion IRQ BEFORE the corresponding request is published to the
//! device (enqueued and doorbell-rung), in that order and not the reverse.
//!
//! The defect this pins: `submit_command_and_wait` and `control_transfer`
//! (EP0) both used to enqueue their TRB(s) and ring the doorbell FIRST, then
//! call `enable_irq_for_wait`, which clears the SPI's pending bit before
//! re-enabling it. If the controller executed the command/transfer and
//! raised its MSI in the window between publication and that clear, the
//! clear discards the legitimate pending edge and the wait times out despite
//! the request having actually completed -- see docs/planning/green-program/
//! input-usb/482-XHCI-ARM-BEFORE-KICK-2026-09-06.md for the full RCA. The
//! fix reorders both synchronous paths so arming precedes publication.
//!
//! The rules below are CENSUSES, not lists of known names:
//!
//! * the direct doorbell callers are read out of `xhci.rs` as each function
//!   whose body contains a call to `ring_doorbell(` -- a fifth caller added
//!   later is swept in automatically and must be classified (synchronous,
//!   arming before publication, or an explicit asynchronous exception) or
//!   the census fails;
//! * the raw MMIO write that actually rings the doorbell (`write32(...
//!   db_base...)`) is pinned to live inside `ring_doorbell` itself, so a new
//!   call site cannot ring the doorbell by writing `db_base` directly and
//!   sidestep the census above.
//!
//! Two call sites are synchronous and ordering-checked:
//! `submit_command_and_wait` (the command ring) and `control_transfer`
//! (EP0, via `prepare_transfer_wait`/`wait_for_prepared_transfer`). Two are
//! explicit asynchronous exceptions that must not call a synchronous wait:
//! `queue_hid_transfer` (interrupt IN polling, requeued from inside the ISR)
//! and `queue_msi_probe_noop` (the one-shot activation probe), though the
//! function that queues that probe, `activate_msi_if_ready_locked`, is still
//! checked to arm the SPI before it queues it.
//!
//! Each ordering rule is read from the SOURCE TEXT at byte-offset
//! granularity, through the same comment/string-stripping mask used
//! elsewhere in this repository's structure tests (see
//! `tests/tty_irq_fg_structure.rs`), so a comment claiming the right order
//! cannot satisfy a check that only looks at code, and `deliberately_broken_
//! copies_redden_the_rules` demonstrates that at the bottom of this file by
//! mutating in-memory copies back to each pre-fix ordering (plus one comment
//! that lies about it) and asserting each rule reddens (8 of 8 mutations).
//!
//! This is a narrow structural invariant over one file's call-site ordering,
//! not hardware-race emulation or a general Rust control-flow checker.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

// =============================================================================
// Generic source-reading helpers (byte-offset comment/string masking,
// function-span extraction) -- the same technique tests/tty_irq_fg_structure.rs
// uses, reimplemented here so this file compiles standalone via `rustc --test`.
// =============================================================================

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

/// `true` at each byte index that is ordinary code -- not inside a line
/// comment, a block comment, a string literal, a raw string literal, or a
/// char literal.
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
            // Distinguish a char literal from a lifetime the same way
            // tests/tty_irq_fg_structure.rs's code_mask does: a plain char
            // literal closes two bytes later (`'x'`), an escaped one closes
            // three bytes later (`'\\n'`, `'\\''`). Neither pattern matches a
            // lifetime (`'static`, `'a`), so this quote is left as ordinary
            // code and the lifetime's identifier is read normally below.
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

/// The first offset at which `needle` appears with each byte of the match
/// inside code (`mask[..]` true across the whole span, not a comment or a
/// string), scanning left to right. Unlike `identifier_offsets`, `needle`
/// need not be a single identifier -- it can be a call expression like
/// `enqueue_command(trb)`.
fn first_code_occurrence(source: &str, mask: &[bool], needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    source
        .match_indices(needle)
        .find(|(offset, _)| mask[*offset..*offset + needle.len()].iter().all(|code| *code))
        .map(|(offset, _)| offset)
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

/// Byte offset of the `)` matching the `(` at `open` (`source.as_bytes()
/// [open] == b'('`), skipping parens inside comments/strings via `mask`.
fn matching_paren(source: &str, mask: &[bool], open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0i32;
    for index in open..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

#[derive(Debug, Clone)]
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

/// The innermost function span (from a whole-file `function_spans` call)
/// whose brace-delimited body contains byte `offset`.
fn enclosing_function(spans: &[FunctionSpan], offset: usize) -> Option<&FunctionSpan> {
    spans
        .iter()
        .filter(|span| span.open <= offset && offset <= span.close)
        .min_by_key(|span| span.close - span.open)
}

// =============================================================================
// #482-specific constants and censuses
// =============================================================================

const XHCI: &str = "kernel/src/drivers/usb/xhci.rs";

/// Each function whose body directly calls `ring_doorbell(`. A name here
/// that is not in `SYNCHRONOUS_ORDERING_CHECKED` below must be one of the two
/// explicit asynchronous exceptions.
const EXPECTED_DOORBELL_CALLERS: [&str; 4] = [
    "submit_command_and_wait",
    "control_transfer",
    "queue_hid_transfer",
    "queue_msi_probe_noop",
];

/// The two synchronous doorbell callers whose wait must arm its transient
/// IRQ before the request is published.
const SYNCHRONOUS_ORDERING_CHECKED: [&str; 2] = ["submit_command_and_wait", "control_transfer"];

/// The two doorbell callers that must stay asynchronous: they ring a
/// doorbell without calling a synchronous wait themselves.
const ASYNC_DOORBELL_EXCEPTIONS: [&str; 2] = ["queue_hid_transfer", "queue_msi_probe_noop"];

/// Each offset in `source` at which `ring_doorbell(` is CALLED (not
/// defined), paired with the name of the function whose body contains the
/// call site.
fn doorbell_call_callers(source: &str) -> Result<Vec<(usize, String)>, String> {
    let mask = code_mask(source);
    let spans = function_spans(source);
    let bytes = source.as_bytes();
    let mut callers = Vec::new();

    for offset in identifier_offsets(source, &mask, "ring_doorbell") {
        let mut cursor = offset;
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if source[..cursor].ends_with("fn") {
            // The definition site: `fn ring_doorbell(...) { ... }`.
            continue;
        }
        let containing = enclosing_function(&spans, offset).ok_or_else(|| {
            format!("ring_doorbell call at byte {offset} is not inside any function")
        })?;
        callers.push((offset, containing.name.clone()));
    }
    Ok(callers)
}

/// Each offset of a `write32(` call whose argument list mentions the
/// `db_base` identifier -- i.e. a raw doorbell-register write, wherever it
/// lives.
fn write32_calls_targeting_db_base(source: &str) -> Result<Vec<usize>, String> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut hits = Vec::new();
    for offset in identifier_offsets(source, &mask, "write32") {
        let mut cursor = offset + "write32".len();
        while cursor < bytes.len() && mask[cursor] && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'(') {
            continue;
        }
        let close = matching_paren(source, &mask, cursor)
            .ok_or_else(|| format!("write32( at byte {offset} has no matching close paren"))?;
        let args = &source[cursor..=close];
        if !identifier_offsets(args, &code_mask(args), "db_base").is_empty() {
            hits.push(offset);
        }
    }
    Ok(hits)
}

// =============================================================================
// Rule 1: doorbell-caller census
// =============================================================================

fn validate_doorbell_census(source: &str) -> Result<(), String> {
    let callers = doorbell_call_callers(source)?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, name) in &callers {
        *counts.entry(name.clone()).or_insert(0) += 1;
    }
    for name in EXPECTED_DOORBELL_CALLERS {
        match counts.get(name) {
            Some(1) => {}
            Some(n) => {
                return Err(format!(
                    "{name} calls ring_doorbell {n} time(s), expected exactly 1 \
                     (duplicate doorbell ring)"
                ))
            }
            None => return Err(format!("expected doorbell caller {name} is missing its call")),
        }
    }
    for name in counts.keys() {
        if !EXPECTED_DOORBELL_CALLERS.contains(&name.as_str()) {
            return Err(format!(
                "unclassified doorbell caller {name} -- the census must classify every \
                 direct doorbell caller as synchronous (arm-before-publish) or an explicit \
                 asynchronous exception"
            ));
        }
    }
    Ok(())
}

// =============================================================================
// Rule 2: the raw db_base MMIO write is pinned to ring_doorbell
// =============================================================================

fn validate_db_base_write_pinned_to_ring_doorbell(source: &str) -> Result<(), String> {
    let spans = function_spans(source);
    let hits = write32_calls_targeting_db_base(source)?;
    if hits.is_empty() {
        return Err(
            "no write32(...db_base...) call found -- ring_doorbell's own MMIO write went missing"
                .to_string(),
        );
    }
    for offset in hits {
        let containing = enclosing_function(&spans, offset).ok_or_else(|| {
            format!("write32(...db_base...) at byte {offset} is not inside any function")
        })?;
        if containing.name != "ring_doorbell" {
            return Err(format!(
                "raw write32(...db_base...) found in {} -- doorbell writes must go through \
                 ring_doorbell() so the census above cannot be evaded",
                containing.name
            ));
        }
    }
    Ok(())
}

// =============================================================================
// Rule 3: submit_command_and_wait arms before it publishes the command
// =============================================================================

fn validate_command_arms_before_publish(source: &str) -> Result<(), String> {
    let body = function_body(source, "submit_command_and_wait")
        .ok_or_else(|| "submit_command_and_wait not found in xhci.rs".to_string())?;
    let mask = code_mask(body);

    let markers = [
        ("WAITING=true", "XHCI_COMMAND_WAITING.store(true"),
        ("IRQ arm", "enable_irq_for_wait(state)"),
        ("enqueue_command", "enqueue_command(trb)"),
        ("doorbell", "ring_doorbell(state, 0, 0)"),
        ("wait", "wait_timeout_uninterruptible("),
        ("WAITING=false", "XHCI_COMMAND_WAITING.store(false"),
        (
            "transient cleanup",
            "disable_transient_irq_after_wait(state, transient_irq)",
        ),
    ];

    let mut previous: Option<(&str, usize)> = None;
    for (label, needle) in markers {
        let offset = first_code_occurrence(body, &mask, needle).ok_or_else(|| {
            format!("submit_command_and_wait: expected `{needle}` ({label}) not found")
        })?;
        if let Some((prev_label, prev_offset)) = previous {
            if prev_offset >= offset {
                return Err(format!(
                    "submit_command_and_wait: expected {prev_label} (byte {prev_offset}) \
                     before {label} (byte {offset}) -- the completion IRQ must be armed \
                     before the command is published (enqueued and doorbell-rung), not after"
                ));
            }
        }
        previous = Some((label, offset));
    }
    Ok(())
}

// =============================================================================
// Rule 4: prepare_transfer_wait arms after software prep, and returns the flag
// =============================================================================

fn validate_prepare_transfer_wait_arms_after_software_prep(source: &str) -> Result<(), String> {
    let body = function_body(source, "prepare_transfer_wait")
        .ok_or_else(|| "prepare_transfer_wait not found in xhci.rs".to_string())?;
    let mask = code_mask(body);

    let irq_reject = first_code_occurrence(body, &mask, "state.irq == 0")
        .ok_or_else(|| "prepare_transfer_wait: expected `state.irq == 0` rejection not found".to_string())?;
    let waiting_true = first_code_occurrence(body, &mask, "XHCI_TRANSFER_WAITING.store(true")
        .ok_or_else(|| "prepare_transfer_wait: XHCI_TRANSFER_WAITING.store(true not found".to_string())?;
    let arm = first_code_occurrence(body, &mask, "enable_irq_for_wait(state)")
        .ok_or_else(|| "prepare_transfer_wait: enable_irq_for_wait(state) call not found".to_string())?;

    if !(irq_reject < waiting_true && waiting_true < arm) {
        return Err(format!(
            "prepare_transfer_wait: expected order irq-rejection ({irq_reject}) < software \
             prep / WAITING=true ({waiting_true}) < IRQ arm ({arm})"
        ));
    }

    if first_code_occurrence(body, &mask, "Ok(enable_irq_for_wait(state))").is_none() {
        return Err(
            "prepare_transfer_wait: does not return the armed transient-enable flag via \
             `Ok(enable_irq_for_wait(state))` -- the caller cannot pass ownership to the wait"
                .to_string(),
        );
    }
    Ok(())
}

// =============================================================================
// Rule 5: control_transfer arms before it publishes any EP0 TRB
// =============================================================================

fn validate_control_transfer_arms_before_publish(source: &str) -> Result<(), String> {
    let body = function_body(source, "control_transfer")
        .ok_or_else(|| "control_transfer not found in xhci.rs".to_string())?;
    let mask = code_mask(body);

    let markers = [
        ("prepare_transfer_wait call", "prepare_transfer_wait(state, slot_id, 1)"),
        ("first enqueue_transfer", "enqueue_transfer("),
        ("doorbell", "ring_doorbell(state, slot_id, 1)"),
        ("wait_for_prepared_transfer call", "wait_for_prepared_transfer(state, transient_irq)"),
    ];

    let mut previous: Option<(&str, usize)> = None;
    for (label, needle) in markers {
        let offset = first_code_occurrence(body, &mask, needle)
            .ok_or_else(|| format!("control_transfer: expected `{needle}` ({label}) not found"))?;
        if let Some((prev_label, prev_offset)) = previous {
            if prev_offset >= offset {
                return Err(format!(
                    "control_transfer: expected {prev_label} (byte {prev_offset}) before \
                     {label} (byte {offset}) -- the completion IRQ must be armed before any \
                     EP0 TRB is enqueued, not merely before the doorbell"
                ));
            }
        }
        previous = Some((label, offset));
    }
    Ok(())
}

// =============================================================================
// Rule 6: wait_for_prepared_transfer performs no GIC enable/clear of its own
// =============================================================================

fn validate_wait_for_prepared_transfer_has_no_gic_arming(source: &str) -> Result<(), String> {
    let body = function_body(source, "wait_for_prepared_transfer")
        .ok_or_else(|| "wait_for_prepared_transfer not found in xhci.rs".to_string())?;
    let mask = code_mask(body);
    for forbidden in ["enable_irq_for_wait(", "clear_spi_pending(", "gic::enable_spi("] {
        if first_code_occurrence(body, &mask, forbidden).is_some() {
            return Err(format!(
                "wait_for_prepared_transfer: must not call `{forbidden}` -- GIC arming belongs \
                 in prepare_transfer_wait, before publication, not in the wait"
            ));
        }
    }
    Ok(())
}

// =============================================================================
// Rule 7: the two asynchronous exceptions must not call a synchronous wait
// =============================================================================

const SYNCHRONOUS_WAIT_CALLS: [&str; 4] = [
    "wait_for_prepared_transfer(",
    "wait_timeout_uninterruptible(",
    "prepare_transfer_wait(",
    "submit_command_and_wait(",
];

fn validate_async_exception_has_no_synchronous_wait(source: &str, name: &str) -> Result<(), String> {
    let body = function_body(source, name)
        .ok_or_else(|| format!("{name} not found in xhci.rs"))?;
    let mask = code_mask(body);
    if first_code_occurrence(body, &mask, "ring_doorbell(").is_none() {
        return Err(format!(
            "{name}: expected asynchronous doorbell caller no longer rings a doorbell"
        ));
    }
    for forbidden in SYNCHRONOUS_WAIT_CALLS {
        if first_code_occurrence(body, &mask, forbidden).is_some() {
            return Err(format!(
                "{name}: must stay asynchronous (requeued from ISR context or a one-shot \
                 probe) -- must not call `{forbidden}`"
            ));
        }
    }
    Ok(())
}

// =============================================================================
// Rule 8: activation arms the SPI before it queues its probe NOOP
// =============================================================================

fn validate_activation_arms_before_probe(source: &str) -> Result<(), String> {
    let body = function_body(source, "activate_msi_if_ready_locked")
        .ok_or_else(|| "activate_msi_if_ready_locked not found in xhci.rs".to_string())?;
    let mask = code_mask(body);
    let enable = first_code_occurrence(body, &mask, "gic::enable_spi(state.irq)").ok_or_else(|| {
        "activate_msi_if_ready_locked: enable_spi(state.irq) call not found".to_string()
    })?;
    let probe = first_code_occurrence(body, &mask, "queue_msi_probe_noop(state)").ok_or_else(|| {
        "activate_msi_if_ready_locked: queue_msi_probe_noop(state) call not found".to_string()
    })?;
    if enable >= probe {
        return Err(format!(
            "activate_msi_if_ready_locked: expected SPI enable (byte {enable}) before \
             queue_msi_probe_noop (byte {probe})"
        ));
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[test]
fn every_direct_doorbell_caller_is_classified() {
    let source = repo_text(XHCI);
    if let Err(message) = validate_doorbell_census(&source) {
        panic!("{message}");
    }
}

#[test]
fn the_synchronous_and_async_censuses_agree() {
    // Sanity: the two lists above partition EXPECTED_DOORBELL_CALLERS.
    for name in SYNCHRONOUS_ORDERING_CHECKED {
        assert!(
            EXPECTED_DOORBELL_CALLERS.contains(&name),
            "{name} is in SYNCHRONOUS_ORDERING_CHECKED but not EXPECTED_DOORBELL_CALLERS"
        );
        assert!(
            !ASYNC_DOORBELL_EXCEPTIONS.contains(&name),
            "{name} is listed as both synchronous and an async exception"
        );
    }
    for name in ASYNC_DOORBELL_EXCEPTIONS {
        assert!(
            EXPECTED_DOORBELL_CALLERS.contains(&name),
            "{name} is in ASYNC_DOORBELL_EXCEPTIONS but not EXPECTED_DOORBELL_CALLERS"
        );
    }
    assert_eq!(
        SYNCHRONOUS_ORDERING_CHECKED.len() + ASYNC_DOORBELL_EXCEPTIONS.len(),
        EXPECTED_DOORBELL_CALLERS.len(),
        "every expected doorbell caller must be exactly one of synchronous or async-exception"
    );
}

#[test]
fn the_doorbell_mmio_write_is_pinned_to_ring_doorbell() {
    let source = repo_text(XHCI);
    if let Err(message) = validate_db_base_write_pinned_to_ring_doorbell(&source) {
        panic!("{message}");
    }
}

#[test]
fn submit_command_and_wait_arms_before_it_publishes() {
    let source = repo_text(XHCI);
    if let Err(message) = validate_command_arms_before_publish(&source) {
        panic!("{message}");
    }
}

#[test]
fn prepare_transfer_wait_arms_after_software_prep_and_returns_the_flag() {
    let source = repo_text(XHCI);
    if let Err(message) = validate_prepare_transfer_wait_arms_after_software_prep(&source) {
        panic!("{message}");
    }
}

#[test]
fn control_transfer_arms_before_it_publishes_any_ep0_trb() {
    let source = repo_text(XHCI);
    if let Err(message) = validate_control_transfer_arms_before_publish(&source) {
        panic!("{message}");
    }
}

#[test]
fn wait_for_prepared_transfer_does_no_gic_arming_of_its_own() {
    let source = repo_text(XHCI);
    if let Err(message) = validate_wait_for_prepared_transfer_has_no_gic_arming(&source) {
        panic!("{message}");
    }
}

#[test]
fn hid_transfer_and_msi_probe_stay_asynchronous() {
    let source = repo_text(XHCI);
    for name in ASYNC_DOORBELL_EXCEPTIONS {
        if let Err(message) = validate_async_exception_has_no_synchronous_wait(&source, name) {
            panic!("{message}");
        }
    }
}

#[test]
fn activation_arms_the_spi_before_queueing_its_probe() {
    let source = repo_text(XHCI);
    if let Err(message) = validate_activation_arms_before_probe(&source) {
        panic!("{message}");
    }
}

// =============================================================================
// Mutation legs -- each rule above reddens on the mutation it exists to
// catch (8 of 8 below), and a comment alone cannot satisfy a positional check.
// =============================================================================

fn replace_once(source: &str, from: &str, to: &str) -> String {
    let count = source.matches(from).count();
    assert_eq!(count, 1, "expected exactly one occurrence of {from:?}, found {count}");
    source.replacen(from, to, 1)
}

#[test]
fn deliberately_broken_copies_redden_the_rules() {
    let fixed = repo_text(XHCI);

    // Green control: the real, unmutated source passes each rule below.
    validate_doorbell_census(&fixed).expect("control: doorbell census");
    validate_db_base_write_pinned_to_ring_doorbell(&fixed).expect("control: db_base pin");
    validate_command_arms_before_publish(&fixed).expect("control: command ordering");
    validate_prepare_transfer_wait_arms_after_software_prep(&fixed)
        .expect("control: prepare_transfer_wait ordering");
    validate_control_transfer_arms_before_publish(&fixed).expect("control: control_transfer ordering");
    validate_wait_for_prepared_transfer_has_no_gic_arming(&fixed)
        .expect("control: wait_for_prepared_transfer has no GIC arming");
    for name in ASYNC_DOORBELL_EXCEPTIONS {
        validate_async_exception_has_no_synchronous_wait(&fixed, name)
            .unwrap_or_else(|message| panic!("control: {message}"));
    }
    validate_activation_arms_before_probe(&fixed).expect("control: activation ordering");

    // --- Mutation 1: restore the pre-#482 command ordering (arm AFTER the
    // doorbell, not before enqueue). ---
    let command_reverted = replace_once(
        &fixed,
        "    // Arm the transient completion IRQ before this command is published to\n    \
         // the device. `enqueue_command`'s cache-clean and the doorbell write are\n    \
         // the earliest points a running command ring can observe and act on the\n    \
         // TRB, so the SPI must already be enabled and its pending bit cleared\n    \
         // before either happens: this SPI is edge-triggered, so a completion\n    \
         // that lands between publication and arming has its GIC-pending edge\n    \
         // cleared by `enable_irq_for_wait` before it can be redelivered (#482).\n    \
         let transient_irq = enable_irq_for_wait(state);\n\n    \
         enqueue_command(trb);\n    \
         ring_doorbell(state, 0, 0);\n",
        "    enqueue_command(trb);\n    \
         ring_doorbell(state, 0, 0);\n\n    \
         let transient_irq = enable_irq_for_wait(state);\n",
    );
    assert!(
        validate_command_arms_before_publish(&command_reverted).is_err(),
        "command-ordering check must redden when arming is moved back after the doorbell"
    );
    // Everything else must still be internally consistent on this mutant --
    // only the targeted rule should move.
    assert!(validate_doorbell_census(&command_reverted).is_ok());

    // --- Mutation 1b: same revert, but with a comment right above the old
    // (wrong) arm line falsely claiming it already arms before publication.
    // The rule must still redden -- it reads code positions, not comments. ---
    let command_reverted_with_lying_comment = replace_once(
        &command_reverted,
        "    let transient_irq = enable_irq_for_wait(state);\n",
        "    // This already arms before publication, honest.\n    \
         let transient_irq = enable_irq_for_wait(state);\n",
    );
    assert!(
        validate_command_arms_before_publish(&command_reverted_with_lying_comment).is_err(),
        "a comment asserting the correct order must not satisfy a positional check"
    );

    // --- Mutation 2: restore the pre-#482 EP0 ordering (prepare_transfer_wait
    // called after all three TRBs are enqueued, right before the doorbell,
    // with its old two-argument signature and no returned flag). ---
    let ep0_reverted = replace_once(
        &fixed,
        "    // Arm the IRQ-side EP0 completion before this transfer is enqueued or\n    \
         // published to the device via any doorbell write below (#482).\n    \
         let transient_irq = prepare_transfer_wait(state, slot_id, 1)?;\n\n    \
         // Setup Stage TRB",
        "    // Setup Stage TRB",
    );
    let ep0_reverted = replace_once(
        &ep0_reverted,
        "    enqueue_transfer(slot_idx, status_trb);\n\n    \
         // Ring doorbell for EP0 (DCI = 1 for the default control endpoint)\n    \
         ring_doorbell(state, slot_id, 1);\n\n    \
         let event = wait_for_prepared_transfer(state, transient_irq)?;",
        "    enqueue_transfer(slot_idx, status_trb);\n\n    \
         // Arm the IRQ-side EP0 completion before ringing the doorbell so a fast\n    \
         // Transfer Event cannot race ahead of the waiter.\n    \
         prepare_transfer_wait(slot_id, 1);\n\n    \
         // Ring doorbell for EP0 (DCI = 1 for the default control endpoint)\n    \
         ring_doorbell(state, slot_id, 1);\n\n    \
         let event = wait_for_prepared_transfer(state)?;",
    );
    assert!(
        validate_control_transfer_arms_before_publish(&ep0_reverted).is_err(),
        "control_transfer ordering check must redden when prepare_transfer_wait moves back \
         to after the TRB enqueues"
    );

    // --- Mutation 3: reintroduce GIC arming inside wait_for_prepared_transfer. ---
    let wait_rearms = replace_once(
        &fixed,
        "fn wait_for_prepared_transfer(\n    state: &XhciState,\n    transient_irq: bool,\n) -> Result<Trb, &'static str> {\n    \
         // The transfer is already published",
        "fn wait_for_prepared_transfer(\n    state: &XhciState,\n    transient_irq: bool,\n) -> Result<Trb, &'static str> {\n    \
         let _reintroduced = enable_irq_for_wait(state);\n    \
         // The transfer is already published",
    );
    assert!(
        validate_wait_for_prepared_transfer_has_no_gic_arming(&wait_rearms).is_err(),
        "must redden when wait_for_prepared_transfer regains a GIC-arming call"
    );

    // --- Mutation 4: give queue_hid_transfer a synchronous wait, breaking
    // the asynchronous-exception rule. ---
    let hid_gains_wait = replace_once(
        &fixed,
        "    // Ring the doorbell for this endpoint\n    ring_doorbell(state, slot_id, dci);\n\n    Ok(())\n}",
        "    // Ring the doorbell for this endpoint\n    ring_doorbell(state, slot_id, dci);\n\n    \
         let _ = wait_for_prepared_transfer(state, false);\n\n    Ok(())\n}",
    );
    assert!(
        validate_async_exception_has_no_synchronous_wait(&hid_gains_wait, "queue_hid_transfer").is_err(),
        "must redden when queue_hid_transfer gains a synchronous wait call"
    );
    // The other exception is untouched by this mutation.
    assert!(
        validate_async_exception_has_no_synchronous_wait(&hid_gains_wait, "queue_msi_probe_noop").is_ok()
    );

    // --- Mutation 5: add an unclassified fifth doorbell caller. ---
    let rogue_caller = format!(
        "{fixed}\n\nfn rogue_doorbell_caller_482(state: &XhciState) {{\n    ring_doorbell(state, 9, 9);\n}}\n"
    );
    assert!(
        validate_doorbell_census(&rogue_caller).is_err(),
        "must redden when a new function rings the doorbell without being classified"
    );

    // --- Mutation 6: duplicate the doorbell ring inside an already-expected
    // caller (submit_command_and_wait). ---
    let duplicate_ring = replace_once(
        &fixed,
        "    let transient_irq = enable_irq_for_wait(state);\n\n    \
         enqueue_command(trb);\n    ring_doorbell(state, 0, 0);\n",
        "    let transient_irq = enable_irq_for_wait(state);\n\n    \
         enqueue_command(trb);\n    ring_doorbell(state, 0, 0);\n    ring_doorbell(state, 0, 0);\n",
    );
    assert!(
        validate_doorbell_census(&duplicate_ring).is_err(),
        "must redden on a duplicate doorbell ring inside an already-classified caller"
    );

    // --- Mutation 7: a raw db_base write outside ring_doorbell evades the
    // per-function-name census by construction, which is exactly why Rule 2
    // exists; prove it catches what Rule 1 alone cannot. ---
    let evading_write = format!(
        "{fixed}\n\nfn evading_doorbell_write_482(state: &XhciState) {{\n    write32(state.db_base, 0);\n}}\n"
    );
    assert!(
        validate_doorbell_census(&evading_write).is_ok(),
        "the evading function does not call ring_doorbell(, so the plain census alone \
         must NOT catch it -- this asserts Rule 2 is the one doing the work below"
    );
    assert!(
        validate_db_base_write_pinned_to_ring_doorbell(&evading_write).is_err(),
        "must redden when a function writes db_base directly instead of going through \
         ring_doorbell()"
    );

    // --- Mutation 8: activation queues its probe before arming the SPI. ---
    let activation_reordered = replace_once(
        &fixed,
        "    crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);\n    \
         crate::arch_impl::aarch64::gic::enable_spi(state.irq);\n    \
         let count = DIAG_SPI_ENABLE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;\n    \
         queue_msi_probe_noop(state);",
        "    queue_msi_probe_noop(state);\n    \
         crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);\n    \
         crate::arch_impl::aarch64::gic::enable_spi(state.irq);\n    \
         let count = DIAG_SPI_ENABLE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;",
    );
    assert!(
        validate_activation_arms_before_probe(&activation_reordered).is_err(),
        "must redden when activation queues its probe before enabling the SPI"
    );
}
