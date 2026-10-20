//! #908: UDP registry holders mask at the API boundary. Receivers resolve
//! Weak socket bindings outside PM and refuse input across contention. Each rule has
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

const REGISTRY: &str = "kernel/src/socket/udp.rs";
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
            } else {
                index += 1;
            }
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
                if depth == 0 {
                    return code[start..=start + offset].to_owned();
                }
            }
            _ => {}
        }
    }
    panic!("unclosed body")
}

fn check_with_udp_ports_masked_masks_the_whole_hold(source: &str) -> bool {
    let body = body(source, "with_registry");
    body.contains("crate::arch_without_interrupts(|| f(&mut SOCKET_REGISTRY.lock()))")
}

#[test]
fn with_registry_masks_the_whole_hold() {
    assert!(
        check_with_udp_ports_masked_masks_the_whole_hold(&repo_text(REGISTRY)),
        "#908: with_udp_ports_masked_masks_the_whole_hold"
    );
}

#[test]
fn with_registry_masks_the_whole_hold_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Anchor the replacement to with_udp_ports_masked's body so the mutation targets this function specifically.
    let start = source.find("fn with_registry").unwrap();
    let mutated = format!(
        "{}{}",
        &source[..start],
        source[start..].replacen(
            "crate::arch_without_interrupts(|| f(&mut SOCKET_REGISTRY.lock()))",
            "f(&mut SOCKET_REGISTRY.lock())",
            1,
        )
    );
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(
        !check_with_udp_ports_masked_masks_the_whole_hold(&mutated),
        "mutation must redden the same rule"
    );
}

fn check_bind_udp_uses_the_masked_primitive(source: &str) -> bool {
    let body = body(source, "reserve");
    body.contains("with_registry(") && !body.contains("SOCKET_REGISTRY.lock()")
}

#[test]
fn reserve_uses_the_masked_primitive() {
    assert!(
        check_bind_udp_uses_the_masked_primitive(&repo_text(REGISTRY)),
        "#908: bind_udp_uses_the_masked_primitive"
    );
}

#[test]
fn reserve_uses_the_masked_primitive_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Limit replacement to this function so bind and unbind cannot mutate each other.
    let start = source.find("fn reserve").unwrap();
    let mutated = format!(
        "{}{}",
        &source[..start],
        source[start..].replacen(
            "with_registry(|registry| {",
            "{ let registry = &mut SOCKET_REGISTRY.lock();",
            1,
        )
    );
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(
        !check_bind_udp_uses_the_masked_primitive(&mutated),
        "mutation must redden the same rule"
    );
}

fn check_unbind_udp_uses_the_masked_primitive(source: &str) -> bool {
    let body = body(source, "release");
    body.contains("with_registry(") && !body.contains("SOCKET_REGISTRY.lock()")
}

#[test]
fn release_uses_the_masked_primitive() {
    assert!(
        check_unbind_udp_uses_the_masked_primitive(&repo_text(REGISTRY)),
        "#908: unbind_udp_uses_the_masked_primitive"
    );
}

#[test]
fn release_uses_the_masked_primitive_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Limit replacement to this function so bind and unbind cannot mutate each other.
    let start = source.find("fn release").unwrap();
    let mutated = format!(
        "{}{}",
        &source[..start],
        source[start..].replacen(
            "with_registry(|registry| {",
            "{ let registry = &mut SOCKET_REGISTRY.lock();",
            1,
        )
    );
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(
        !check_unbind_udp_uses_the_masked_primitive(&mutated),
        "mutation must redden the same rule"
    );
}

fn check_try_lookup_udp_uses_try_lock_not_blocking_lock(source: &str) -> bool {
    let body = body(source, "receivers");
    body.contains("crate::arch_without_interrupts(||")
        && body.contains("SOCKET_REGISTRY.try_lock()")
        && !body.contains("SOCKET_REGISTRY.lock()")
}

#[test]
fn receivers_refuses_contention_under_the_mask() {
    assert!(
        check_try_lookup_udp_uses_try_lock_not_blocking_lock(&repo_text(REGISTRY)),
        "#908: try_lookup_udp_uses_try_lock_not_blocking_lock"
    );
}

#[test]
fn receivers_refuses_contention_under_the_mask_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Anchor the replacement to try_lookup_udp's body so the mutation targets this function specifically.
    let start = source.find("fn receivers").unwrap();
    let mutated = format!(
        "{}{}",
        &source[..start],
        source[start..].replacen("crate::arch_without_interrupts(||", "(||", 1,)
    );
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(
        !check_try_lookup_udp_uses_try_lock_not_blocking_lock(&mutated),
        "mutation must redden the same rule"
    );
}

fn check_try_lookup_udp_counts_refusals(source: &str) -> bool {
    let body = body(source, "receivers");
    body.split("None => {").nth(1).is_some_and(|arm| {
        arm.contains("UDP_PORTS_LOOKUP_REFUSED.fetch_add(1, core::sync::atomic::Ordering::Relaxed)")
    })
}

#[test]
fn receivers_counts_refusals() {
    assert!(
        check_try_lookup_udp_counts_refusals(&repo_text(REGISTRY)),
        "#908: try_lookup_udp_counts_refusals"
    );
}

#[test]
fn receivers_counts_refusals_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    // Anchor the replacement to try_lookup_udp's body so the mutation targets this function specifically.
    let start = source.find("fn receivers").unwrap();
    let mutated = format!(
        "{}{}",
        &source[..start],
        source[start..].replacen(
            "UDP_PORTS_LOOKUP_REFUSED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);",
            "",
            1,
        )
    );
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(
        !check_try_lookup_udp_counts_refusals(&mutated),
        "mutation must redden the same rule"
    );
}

fn check_handle_udp_uses_try_lookup_udp_not_lookup_udp(source: &str) -> bool {
    let body = body(source, "handle_udp");
    body.contains("crate::socket::udp::deliver(") && !body.contains("with_process_manager(")
}

#[test]
fn handle_udp_uses_try_lookup_udp_not_lookup_udp() {
    assert!(
        check_handle_udp_uses_try_lookup_udp_not_lookup_udp(&repo_text(NET_UDP)),
        "#908: handle_udp_uses_try_lookup_udp_not_lookup_udp"
    );
}

#[test]
fn handle_udp_uses_try_lookup_udp_not_lookup_udp_rule_is_not_vacuous() {
    let source = repo_text(NET_UDP);
    // Anchor the replacement to handle_udp's body so the mutation targets this function specifically.
    let start = source.find("fn handle_udp").unwrap();
    let mutated = format!(
        "{}{}",
        &source[..start],
        source[start..].replacen("crate::socket::udp::deliver(", "with_process_manager(", 1,)
    );
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(
        !check_handle_udp_uses_try_lookup_udp_not_lookup_udp(&mutated),
        "mutation must redden the same rule"
    );
}

fn check_handle_udp_contended_arm_has_no_logging(source: &str) -> bool {
    let body = body(source, "handle_udp");
    [
        "log::debug!",
        "log::warn!",
        "log::info!",
        "log::error!",
        "log::trace!",
        "serial_println!",
    ]
    .iter()
    .all(|logging| !body.contains(logging))
}

#[test]
fn handle_udp_contended_arm_has_no_logging() {
    assert!(
        check_handle_udp_contended_arm_has_no_logging(&repo_text(NET_UDP)),
        "#908: handle_udp_contended_arm_has_no_logging"
    );
}

#[test]
fn handle_udp_contended_arm_has_no_logging_rule_is_not_vacuous() {
    let source = repo_text(NET_UDP);
    let start = source.find("pub fn handle_udp(").unwrap();
    let mutated = format!("{}{}", &source[..start], source[start..].replacen(
        "pub fn handle_udp(ip: &Ipv4Packet, data: &[u8]) -> Handled {",
        "pub fn handle_udp(ip: &Ipv4Packet, data: &[u8]) -> Handled {\n        log::debug!(\"UDP: contended\");",
        1,
    ));
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(
        !check_handle_udp_contended_arm_has_no_logging(&mutated),
        "mutation must redden the same rule"
    );
}

/// Return only the contiguous doc lines immediately above the primitive.
fn udp_ports_masked_doc(source: &str) -> &str {
    let function = source.find("fn with_registry").expect("function missing");
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
fn with_registry_documents_removal_allocator_work() {
    assert!(
        check_with_udp_ports_masked_documents_removal_allocator_work(&repo_text(REGISTRY)),
        "#908: with_udp_ports_masked_documents_removal_allocator_work"
    );
}

#[test]
fn with_registry_documents_removal_allocator_work_rule_is_not_vacuous() {
    let source = repo_text(REGISTRY);
    let doc = udp_ports_masked_doc(&source);
    let disclosure = "/// Registry insertions and remove operations may allocate/deallocate; memory/heap.rs\n/// masks its allocator lock. The outer IRQ mask covers the entire map hold.\n";
    // An on-disk deletion already violates the production rule.
    if !doc.contains(disclosure) {
        assert!(
            !check_with_udp_ports_masked_documents_removal_allocator_work(&source),
            "missing disclosure must already redden production rule"
        );
        return;
    }
    let start = source.find(doc).unwrap();
    let mutated = format!(
        "{}{}{}",
        &source[..start],
        doc.replacen(disclosure, "", 1),
        &source[start + doc.len()..]
    );
    assert_ne!(source, mutated, "mutation text must match real source");
    assert!(
        !check_with_udp_ports_masked_documents_removal_allocator_work(&mutated),
        "mutation must redden the same rule"
    );
}

#[test]
fn udp_delivery_uses_weak_sockets_with_masked_state() {
    let wire = masked_text(&repo_text(NET_UDP));
    assert!(wire.contains("crate::socket::udp::deliver("));
    assert!(!wire.contains("with_process_manager("));
    let registry = masked_text(&repo_text(REGISTRY));
    assert!(registry.contains("weak.upgrade()"));
    assert!(registry.contains("crate::socket::udp::with_locked_masked(udp,"));
    assert!(!registry.contains("with_process_manager("));
}

#[test]
fn green_control_all_rules_hold_at_head() {
    with_registry_masks_the_whole_hold();
    reserve_uses_the_masked_primitive();
    release_uses_the_masked_primitive();
    receivers_refuses_contention_under_the_mask();
    receivers_counts_refusals();
    handle_udp_uses_try_lookup_udp_not_lookup_udp();
    handle_udp_contended_arm_has_no_logging();
    handle_udp_contended_arm_has_no_logging_rule_is_not_vacuous();
    with_registry_documents_removal_allocator_work();
    with_registry_documents_removal_allocator_work_rule_is_not_vacuous();
    udp_delivery_uses_weak_sockets_with_masked_state();
}

#[test]
fn code_mask_raw_string_close_preserves_next_byte() {
    // Check scanner correctness with a consistent live-code token.
    // Hash counts 0, 1, 2, and 3 are exercised directly here, not just 0 and 1.
    for fixture in [
        r##"r"x"serial_println!"##,
        r##"r#"x"#serial_println!"##,
        r####"r##"x"##serial_println!"####,
        r#####"r###"x"###serial_println!"#####,
    ] {
        let mask = code_mask(fixture);
        let offset = fixture.find("serial_println!").unwrap();
        assert!(mask[offset], "raw-string close swallowed the next byte");
    }
    // A skipped ordinary identifier byte stays true in the default mask.
    // A skipped raw opener instead changes lexical state: the embedded quote
    // closes an ordinary string, hiding the real token after the raw close.
    // Exercised at a 0-then-1 hash boundary and again at a 1-then-2 hash
    // boundary, so the compound-skip fix is checked past the smallest counts too.
    for fixture in [
        r###"r"x"r#"a"b"#serial_println!"###,
        r######"r#"x"#r##"a"b"##serial_println!"######,
    ] {
        let mask = code_mask(fixture);
        let offset = fixture.find("serial_println!").unwrap();
        assert!(mask[offset], "raw-string close swallowed the next byte");
    }
}
