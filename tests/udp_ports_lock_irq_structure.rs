//! #908: private UDP registry holders mask once at the API boundary;
//! NetRx lookup refuses contention instead of blocking. Each defect rule has
//! an in-memory mutation of real source, with a checked replacement.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

const REGISTRY: &str = "kernel/src/socket/mod.rs";
const NET_UDP: &str = "kernel/src/net/udp.rs";

/// Strip line comments, block comments, and the contents of string/char
/// literals to `' '`, byte-for-byte, so a naive substring search cannot match
/// inside a comment or a string constant. Copied from
/// `tests/tty_irq_pm_structure.rs`'s `code_mask` -- keep it byte-identical to
/// that copy if you change either.
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

fn masked_text(source: &str) -> String {
    let mask = code_mask(source);
    source
        .bytes()
        .zip(mask.iter())
        .map(|(b, keep)| if *keep { b as char } else { ' ' })
        .collect()
}

/// Extract a balanced function body after masking comments and literals.
fn body(source: &str, name: &str) -> String {
    let code = masked_text(source);
    let start = code.find(&format!("fn {name}")).expect("function missing");
    let start = start + code[start..].find('{').expect("body missing");
    let mut depth = 0;
    for (offset, byte) in code.as_bytes()[start..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 { return code[start..=start + offset].to_owned(); }
            }
            _ => {}
        }
    }
    panic!("unclosed body")
}

fn check_with_udp_ports_masked_masks_the_whole_hold(source: &str) -> bool {
    let body = body(source, "with_udp_ports_masked");
    body.contains("Cpu::without_interrupts(|| f(&mut self.udp_ports.lock()))")
}

#[test]
fn with_udp_ports_masked_masks_the_whole_hold() {
    assert!(check_with_udp_ports_masked_masks_the_whole_hold(&repo_text(REGISTRY)), "#908: with_udp_ports_masked_masks_the_whole_hold");
}

#[test]
fn with_udp_ports_masked_masks_the_whole_hold_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Limit replacement to this function so bind and unbind cannot mutate each other.
    let start = source.find("fn with_udp_ports_masked").unwrap();
    let mutated = format!("{}{}", &source[..start], source[start..].replacen(
        "Cpu::without_interrupts(|| f(&mut self.udp_ports.lock()))", "f(&mut self.udp_ports.lock())", 1,
    ));
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(!check_with_udp_ports_masked_masks_the_whole_hold(&mutated), "mutation must redden the same rule");
}

fn check_bind_udp_uses_the_masked_primitive(source: &str) -> bool {
    let body = body(source, "bind_udp");
    body.contains("self.with_udp_ports_masked(") && !body.contains("self.udp_ports.lock()")
}

#[test]
fn bind_udp_uses_the_masked_primitive() {
    assert!(check_bind_udp_uses_the_masked_primitive(&repo_text(REGISTRY)), "#908: bind_udp_uses_the_masked_primitive");
}

#[test]
fn bind_udp_uses_the_masked_primitive_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Limit replacement to this function so bind and unbind cannot mutate each other.
    let start = source.find("fn bind_udp").unwrap();
    let mutated = format!("{}{}", &source[..start], source[start..].replacen(
        "self.with_udp_ports_masked(|ports| {", "{ let ports = &mut self.udp_ports.lock();", 1,
    ));
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(!check_bind_udp_uses_the_masked_primitive(&mutated), "mutation must redden the same rule");
}

fn check_unbind_udp_uses_the_masked_primitive(source: &str) -> bool {
    let body = body(source, "unbind_udp");
    body.contains("self.with_udp_ports_masked(") && !body.contains("self.udp_ports.lock()")
}

#[test]
fn unbind_udp_uses_the_masked_primitive() {
    assert!(check_unbind_udp_uses_the_masked_primitive(&repo_text(REGISTRY)), "#908: unbind_udp_uses_the_masked_primitive");
}

#[test]
fn unbind_udp_uses_the_masked_primitive_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Limit replacement to this function so bind and unbind cannot mutate each other.
    let start = source.find("fn unbind_udp").unwrap();
    let mutated = format!("{}{}", &source[..start], source[start..].replacen(
        "self.with_udp_ports_masked(|ports| {", "{ let ports = &mut self.udp_ports.lock();", 1,
    ));
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(!check_unbind_udp_uses_the_masked_primitive(&mutated), "mutation must redden the same rule");
}

fn check_try_lookup_udp_uses_try_lock_not_blocking_lock(source: &str) -> bool {
    let body = body(source, "try_lookup_udp");
    body.contains("self.udp_ports.try_lock()") && !body.contains("self.udp_ports.lock()")
}

#[test]
fn try_lookup_udp_uses_try_lock_not_blocking_lock() {
    assert!(check_try_lookup_udp_uses_try_lock_not_blocking_lock(&repo_text(REGISTRY)), "#908: try_lookup_udp_uses_try_lock_not_blocking_lock");
}

#[test]
fn try_lookup_udp_uses_try_lock_not_blocking_lock_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Limit replacement to this function so bind and unbind cannot mutate each other.
    let start = source.find("fn try_lookup_udp").unwrap();
    let mutated = format!("{}{}", &source[..start], source[start..].replacen(
        "self.udp_ports.try_lock()", "self.udp_ports.lock()", 1,
    ));
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(!check_try_lookup_udp_uses_try_lock_not_blocking_lock(&mutated), "mutation must redden the same rule");
}

fn check_try_lookup_udp_counts_refusals(source: &str) -> bool {
    let body = body(source, "try_lookup_udp");
    body.split("None => {").nth(1).is_some_and(|arm| arm.contains("UDP_PORTS_LOOKUP_REFUSED.fetch_add(1, Ordering::Relaxed)"))
}

#[test]
fn try_lookup_udp_counts_refusals() {
    assert!(check_try_lookup_udp_counts_refusals(&repo_text(REGISTRY)), "#908: try_lookup_udp_counts_refusals");
}

#[test]
fn try_lookup_udp_counts_refusals_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Limit replacement to this function so bind and unbind cannot mutate each other.
    let start = source.find("fn try_lookup_udp").unwrap();
    let mutated = format!("{}{}", &source[..start], source[start..].replacen(
        "UDP_PORTS_LOOKUP_REFUSED.fetch_add(1, Ordering::Relaxed);", "", 1,
    ));
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(!check_try_lookup_udp_counts_refusals(&mutated), "mutation must redden the same rule");
}

fn check_handle_udp_uses_try_lookup_udp_not_lookup_udp(source: &str) -> bool {
    let body = body(source, "handle_udp");
    body.contains("try_lookup_udp(") && !body.replace("try_lookup_udp(", "").contains("lookup_udp(")
}

#[test]
fn handle_udp_uses_try_lookup_udp_not_lookup_udp() {
    assert!(check_handle_udp_uses_try_lookup_udp_not_lookup_udp(&repo_text(NET_UDP)), "#908: handle_udp_uses_try_lookup_udp_not_lookup_udp");
}

#[test]
fn handle_udp_uses_try_lookup_udp_not_lookup_udp_rule_is_not_vacuous() {
    let source = repo_text(NET_UDP);
    // Limit replacement to this function so bind and unbind cannot mutate each other.
    let start = source.find("fn handle_udp").unwrap();
    let mutated = format!("{}{}", &source[..start], source[start..].replacen(
        "try_lookup_udp(header.dst_port)", "lookup_udp(header.dst_port)", 1,
    ));
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(!check_handle_udp_uses_try_lookup_udp_not_lookup_udp(&mutated), "mutation must redden the same rule");
}

fn check_handle_udp_contended_arm_has_no_logging(source: &str) -> bool {
    let body = body(source, "handle_udp");
    body.rsplit_once("None => {").is_some_and(|(_, arm)| {
        ["log::debug!", "log::warn!", "log::info!", "log::error!", "log::trace!", "serial_println!"]
            .iter().all(|logging| !arm.contains(logging))
    })
}

#[test]
fn handle_udp_contended_arm_has_no_logging() {
    assert!(check_handle_udp_contended_arm_has_no_logging(&repo_text(NET_UDP)), "#908: handle_udp_contended_arm_has_no_logging");
}

#[test]
fn handle_udp_contended_arm_has_no_logging_rule_is_not_vacuous() {
    let source = repo_text(NET_UDP);
    let start = source.find("fn handle_udp").unwrap();
    let end = source[start..].find("\n/// Deliver").map(|offset| start + offset).unwrap();
    let function = &source[start..end];
    let arm = function.rfind("None => {").unwrap();
    let mutated = format!("{}{}{}{}", &source[..start], &function[..arm], function[arm..].replacen(
        "None => {", "None => {\n            log::debug!(\n                \"UDP: udp_ports lock contended, dropping packet for port {}\",\n                header.dst_port\n            );", 1,
    ), &source[end..]);
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(!check_handle_udp_contended_arm_has_no_logging(&mutated), "mutation must redden the same rule");
}

/// Return only the contiguous doc lines immediately above the primitive.
fn udp_ports_masked_doc(source: &str) -> &str {
    let function = source.find("fn with_udp_ports_masked").expect("function missing");
    let end = source[..function].rfind('\n').map_or(0, |index| index + 1);
    let mut start = end;
    while start > 0 {
        let previous = source[..start - 1].rfind('\n').map_or(0, |index| index + 1);
        if !source[previous..start].trim_start().starts_with("///") {
            break;
        }
        start = previous;
    }
    &source[start..end]
}

fn check_with_udp_ports_masked_documents_removal_allocator_work(source: &str) -> bool {
    let doc = udp_ports_masked_doc(source);
    doc.contains("remove") && doc.contains("allocate") && doc.contains("heap.rs")
}

#[test]
fn with_udp_ports_masked_documents_removal_allocator_work() {
    assert!(check_with_udp_ports_masked_documents_removal_allocator_work(&repo_text(REGISTRY)), "#908: with_udp_ports_masked_documents_removal_allocator_work");
}

#[test]
fn with_udp_ports_masked_documents_removal_allocator_work_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    let doc = udp_ports_masked_doc(&source);
    let disclosure = concat!(
        "    /// `unbind_udp`'s map removal can also allocate/deallocate --\n",
        "    /// `BTreeMap::remove` may rebalance or merge nodes back to the\n",
        "    /// allocator. Neither is a deadlock risk: the global heap allocator\n",
        "    /// (`kernel/src/memory/heap.rs`) masks interrupts around its own lock\n",
        "    /// during alloc/dealloc, the same nested-mask pattern this\n",
        "    /// primitive already relies on.\n",
    );
    // An on-disk deletion already violates the production rule.
    if !doc.contains(disclosure) {
        assert!(!check_with_udp_ports_masked_documents_removal_allocator_work(&source), "missing disclosure must already redden production rule");
        return;
    }
    let start = source.find(doc).unwrap();
    let mutated = format!("{}{}{}", &source[..start], doc.replacen(disclosure, "", 1), &source[start + doc.len()..]);
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(!check_with_udp_ports_masked_documents_removal_allocator_work(&mutated), "mutation must redden the same rule");
}

/// Limitation: this copied #823 regression guard uses a whole-file PM
/// substring search, not a nesting-aware proof. It guards an unchanged path.
/// #823 rule 6 (regression guard): the NetRx IRQ-side counterpart is still
/// inside `with_process_manager`'s masked closure -- this rule does not pin
/// anything new, it stops a future edit from silently moving the IRQ-side
/// lock out from under the mask while the thread side is being kept honest.
#[test]
fn deliver_to_socket_still_locks_under_process_manager() {
    let source = repo_text(NET_UDP);
    let code = masked_text(&source);
    let start = code
        .find("fn deliver_to_socket")
        .expect("#823 shape failure: net/udp.rs::deliver_to_socket was renamed or removed");
    let with_pm = code[..start]
        .rfind("with_process_manager")
        .map(|_| true)
        .unwrap_or(false)
        || code[start..].contains("with_process_manager");
    assert!(
        with_pm,
        "#823 shape failure: deliver_to_socket no longer runs inside \
         with_process_manager -- the NetRx IRQ side may no longer be masked \
         while it locks the same Mutex<UdpSocket> the thread side holds"
    );
    let end = code[start..]
        .find("\nfn ")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let body = &code[start..end];
    assert!(
        body.contains("socket_ref.lock()"),
        "#823 shape failure: deliver_to_socket no longer locks socket_ref -- \
         update this test if the variable was renamed, but confirm the lock \
         it takes is still the outer Mutex<UdpSocket>"
    );
}

#[test]
fn green_control_all_rules_hold_at_head() {
    with_udp_ports_masked_masks_the_whole_hold();
    bind_udp_uses_the_masked_primitive();
    unbind_udp_uses_the_masked_primitive();
    try_lookup_udp_uses_try_lock_not_blocking_lock();
    try_lookup_udp_counts_refusals();
    handle_udp_uses_try_lookup_udp_not_lookup_udp();
    handle_udp_contended_arm_has_no_logging();
    handle_udp_contended_arm_has_no_logging_rule_is_not_vacuous();
    with_udp_ports_masked_documents_removal_allocator_work();
    with_udp_ports_masked_documents_removal_allocator_work_rule_is_not_vacuous();
    deliver_to_socket_still_locks_under_process_manager();
}
