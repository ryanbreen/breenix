//! #823 -- every thread-side hold of a UDP socket's `Arc<Mutex<UdpSocket>>`
//! (`UdpSock::inner`) runs masked for its whole duration, through one
//! production primitive, `crate::socket::udp::with_locked_masked`.
//!
//! The defect these rules pin: NetRx delivery in `net/udp.rs` (the IRQ
//! route) locks this exact `Mutex<UdpSocket>` from inside
//! `with_process_manager`, which masks interrupts on both architectures.
//! Thread-side callers once took the same lock without masking for the whole
//! hold -- `ipc/poll.rs`'s `poll_fd` and three sites in `syscall/socket.rs`.
//! Those holds now live in the UDP adapter, `impl ProtoOps for UdpSock` in
//! `socket/udp.rs` (poll, bind, sendmsg, recvmsg, local_name), and the rules
//! below require each of them to go through `with_locked_masked` with no raw
//! `.lock()` beside it, and `ipc/poll.rs` and `syscall/socket.rs` to stay off
//! the lock entirely.
//!
//! This is a fixed, enumerated census (the adapter's holds plus the one
//! primitive and its one IRQ-side counterpart), not a naming-convention sweep;
//! the rules name the sites directly and each carries its own anti-vacuity
//! mutation.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

const UDP_MODULE: &str = "kernel/src/socket/udp.rs";
const POLL_MODULE: &str = "kernel/src/ipc/poll.rs";
const SOCKET_SYSCALLS: &str = "kernel/src/syscall/socket.rs";
const TEST_REGISTRY: &str = "kernel/src/test_framework/registry.rs";
const NET_UDP: &str = "kernel/src/net/udp.rs";

const PRIMITIVE_CALL: &str = "crate::socket::udp::with_locked_masked";

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

/// #823 rule 1: `with_locked_masked` exists in `socket/udp.rs` and masks
/// interrupts around the whole closure call.
#[test]
fn with_locked_masked_masks_the_whole_hold() {
    let source = repo_text(UDP_MODULE);
    let code = masked_text(&source);
    let start = code
        .find("fn with_locked_masked")
        .expect("#823 shape failure: with_locked_masked is missing from kernel/src/socket/udp.rs");
    // Take the function body up to the next top-level `\n}\n` after the
    // opening brace that follows the signature -- good enough for this one
    // small function, which is not nested inside another item.
    let body_start = code[start..]
        .find('{')
        .map(|i| start + i)
        .expect("with_locked_masked has no body");
    let body_end = code[body_start..]
        .find("\n}\n")
        .map(|i| body_start + i)
        .unwrap_or(code.len());
    let body = &code[body_start..body_end];
    assert!(
        body.contains("without_interrupts"),
        "#823 shape failure: with_locked_masked's body does not call without_interrupts -- \
         the mask on the shared UDP-socket-lock primitive is gone"
    );
    assert!(
        body.contains("f(&mut udp.inner.lock())"),
        "#823 shape failure: with_locked_masked no longer runs f() under the \
         acquired lock inside the masked closure"
    );
}

/// Anti-vacuity for rule 1: deleting `without_interrupts` from a private copy
/// of the function body must make the assertion fail.
#[test]
fn with_locked_masked_rule_is_not_vacuous() {
    let source = repo_text(UDP_MODULE);
    let mutated = source.replacen(
        "Cpu::without_interrupts(|| f(&mut udp.inner.lock()))",
        "f(&mut udp.inner.lock())",
        1,
    );
    assert_ne!(
        source, mutated,
        "mutation text not found in kernel/src/socket/udp.rs -- update this test's \
         literal to match the real with_locked_masked body before trusting rule 1"
    );
    let code = masked_text(&mutated);
    let start = code.find("fn with_locked_masked").unwrap();
    let body_start = code[start..].find('{').map(|i| start + i).unwrap();
    let body_end = code[body_start..]
        .find("\n}\n")
        .map(|i| body_start + i)
        .unwrap_or(code.len());
    let body = &code[body_start..body_end];
    assert!(
        !body.contains("without_interrupts"),
        "mutation did not remove without_interrupts from the body under test"
    );
}

/// The body of `fn <name>` inside `impl ProtoOps for UdpSock`, from masked
/// code: up to the next method or the end of the impl.
fn adapter_method(code: &str, name: &str) -> Option<String> {
    let impl_start = code.find("impl ProtoOps for UdpSock {")?;
    let impl_end = code[impl_start..]
        .find("\n}\n")
        .map(|i| impl_start + i)
        .unwrap_or(code.len());
    let block = &code[impl_start..impl_end];
    let start = block.find(&format!("fn {name}("))?;
    let end = block[start + 1..]
        .find("\n    fn ")
        .map(|i| start + 1 + i)
        .unwrap_or(block.len());
    Some(block[start..end].to_string())
}

/// Whether the adapter method `name` holds the UdpSocket lock only through
/// the masked primitive.
fn adapter_hold_is_masked(source: &str, name: &str) -> bool {
    let code = masked_text(source);
    let body = adapter_method(&code, name)
        .unwrap_or_else(|| panic!("#823 shape failure: UdpSock::{name} was renamed or removed"));
    body.contains(PRIMITIVE_CALL) && !body.contains(".lock()")
}

/// Apply `from` -> `to` to the live source, failing if the literal is gone,
/// and say whether the rule still holds on the result.
fn mutated_rule_holds(name: &str, from: &str, to: &str) -> bool {
    let source = repo_text(UDP_MODULE);
    let mutated = source.replacen(from, to, 1);
    assert_ne!(
        source, mutated,
        "mutation text not found in kernel/src/socket/udp.rs -- update this test's \
         literal to match UdpSock::{name} before trusting its rule"
    );
    adapter_hold_is_masked(&mutated, name)
}

/// #823 rule 2: readiness (`UdpSock::poll`, reached by poll, select and
/// epoll through `ipc/poll.rs`) reads the queue under the primitive.
#[test]
fn poll_fd_udp_arm_uses_the_masked_primitive() {
    assert!(
        adapter_hold_is_masked(&repo_text(UDP_MODULE), "poll"),
        "#823 shape failure: UdpSock::poll takes the socket lock outside with_locked_masked"
    );
    let poll = masked_text(&repo_text(POLL_MODULE));
    assert!(
        !poll.contains("inner.lock()") && !poll.contains("UdpSocket"),
        "#823 shape failure: ipc/poll.rs reaches the UDP socket lock directly again"
    );
}

#[test]
fn poll_fd_udp_arm_rule_is_not_vacuous() {
    assert!(!mutated_rule_holds(
        "poll",
        "crate::socket::udp::with_locked_masked(self, |socket| socket.has_data())",
        "self.inner.lock().has_data()",
    ));
}

/// #823 rule 3: `UdpSock::bind` holds the lock under the primitive around
/// `bind(..)`.
#[test]
fn sys_bind_udp_arm_uses_the_masked_primitive() {
    assert!(
        adapter_hold_is_masked(&repo_text(UDP_MODULE), "bind"),
        "#823 shape failure: UdpSock::bind takes the socket lock outside with_locked_masked"
    );
}

#[test]
fn sys_bind_udp_arm_rule_is_not_vacuous() {
    assert!(!mutated_rule_holds(
        "bind",
        "crate::socket::udp::with_locked_masked(self, |socket| socket.bind(pid, addr, port))",
        "self.inner.lock().bind(pid, addr, port)",
    ));
}

/// #823 rule 4: the send's local-port lookup (`UdpSock::sendmsg`) calls the
/// primitive.
#[test]
fn sys_sendto_udp_arm_uses_the_masked_primitive() {
    assert!(
        adapter_hold_is_masked(&repo_text(UDP_MODULE), "sendmsg"),
        "#823 shape failure: UdpSock::sendmsg takes the socket lock outside with_locked_masked"
    );
}

#[test]
fn sys_sendto_udp_arm_rule_is_not_vacuous() {
    assert!(!mutated_rule_holds(
        "sendmsg",
        "crate::socket::udp::with_locked_masked(self, |socket| socket.local_port().unwrap_or(0));",
        "self.inner.lock().local_port().unwrap_or(0);",
    ));
}

/// #823 rule 5: the receive's dequeue (`UdpSock::recvmsg`) calls the
/// primitive. The nonblocking-flag read this rule once pinned is gone: the
/// mode is the descriptor's O_NONBLOCK or the call's MSG_DONTWAIT.
#[test]
fn sys_recvfrom_nonblocking_read_uses_the_masked_primitive() {
    assert!(
        adapter_hold_is_masked(&repo_text(UDP_MODULE), "recvmsg"),
        "#823 shape failure: UdpSock::recvmsg takes the socket lock outside with_locked_masked"
    );
    assert!(
        adapter_hold_is_masked(&repo_text(UDP_MODULE), "local_name"),
        "#823 shape failure: UdpSock::local_name takes the socket lock outside with_locked_masked"
    );
    let syscalls = masked_text(&repo_text(SOCKET_SYSCALLS));
    assert!(
        !syscalls.contains("inner.lock()") && !syscalls.contains("UdpSocket"),
        "#823 shape failure: syscall/socket.rs reaches the UDP socket lock directly again"
    );
}

#[test]
fn sys_recvfrom_nonblocking_read_rule_is_not_vacuous() {
    assert!(!mutated_rule_holds(
        "recvmsg",
        "crate::socket::udp::with_locked_masked(self, |socket| socket.recv_from())",
        "self.inner.lock().recv_from()",
    ));
}

/// #823 rule 6 (regression guard): NetRx delivery, inlined into `handle_udp`
/// by #1384, still locks the socket inside `with_process_manager`'s masked
/// closure, through the masked primitive, before it enqueues -- so a future
/// edit cannot move the IRQ-side lock out from under the mask while the
/// thread side is being kept honest.
#[test]
fn udp_delivery_locks_the_socket_under_process_manager() {
    let code = masked_text(&repo_text(NET_UDP));
    let start = code
        .find("pub fn handle_udp(")
        .expect("#823 shape failure: net/udp.rs::handle_udp was renamed or removed");
    let end = code[start..].find("\n}\n").map_or(code.len(), |i| start + i);
    let body = &code[start..end];
    let pm = body
        .find("with_process_manager(")
        .expect("#823 shape failure: handle_udp no longer delivers under with_process_manager");
    let lock = body
        .find("with_locked_masked(udp,")
        .expect("#823 shape failure: handle_udp no longer locks the socket through with_locked_masked");
    let enqueue = body
        .find("enqueue_packet(")
        .expect("#823 shape failure: handle_udp no longer enqueues the packet");
    assert!(
        pm < lock && lock < enqueue,
        "#823 shape failure: handle_udp must lock the socket inside the process-manager \
         closure and enqueue while holding it"
    );
    assert!(
        !body.contains(".lock()"),
        "#823 shape failure: handle_udp takes a lock outside the masked primitive"
    );
}

/// #823 rule 7: the oracle's own receive checker uses the masked primitive:
/// `udp_lock_received` counts through `queued_matching`, which holds the
/// socket lock only through `with_locked_masked`.
fn checker_is_masked(registry: &str, udp: &str) -> bool {
    let code = masked_text(registry);
    let Some(start) = code.find("fn udp_lock_received(") else {
        return false;
    };
    let end = code[start..]
        .find("\n}\n")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let checker = &code[start..end];
    let code = masked_text(udp);
    let Some(start) = code.find("fn queued_matching(") else {
        return false;
    };
    let end = code[start..]
        .find("\n}\n")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let counter = &code[start..end];
    checker.contains("crate::socket::udp::queued_matching(")
        && !checker.contains(".lock()")
        && counter.contains("with_locked_masked(udp,")
        && !counter.contains("inner.lock()")
}

#[test]
fn udp_lock_received_uses_the_masked_primitive() {
    assert!(
        checker_is_masked(&repo_text(TEST_REGISTRY), &repo_text(UDP_MODULE)),
        "#823 shape failure: the oracle's receive checker no longer counts under with_locked_masked"
    );
}

#[test]
fn udp_lock_received_rule_is_not_vacuous() {
    let registry = repo_text(TEST_REGISTRY);
    let udp = repo_text(UDP_MODULE);
    let unmasked = udp.replacen("    with_locked_masked(udp, |socket| {\n        socket\n            .rx_queue", "    (|socket: &mut UdpSocket| {\n        socket\n            .rx_queue", 1);
    assert_ne!(
        udp, unmasked,
        "mutation text not found in kernel/src/socket/udp.rs -- update this test's literal before trusting rule 7"
    );
    assert!(!checker_is_masked(&registry, &unmasked));
    let direct = registry.replacen(
        "crate::socket::udp::queued_matching(udp_of(&open.socket), UDP_LOCK_PAYLOAD)",
        "udp_of(&open.socket).inner.lock().rx_queue.lock().len() as u64",
        1,
    );
    assert_ne!(
        registry, direct,
        "mutation text not found in kernel/src/test_framework/registry.rs -- update this test's literal before trusting rule 7"
    );
    assert!(!checker_is_masked(&direct, &udp));
}

/// Green control: the file as it stands on this branch passes each of the 7 rules
/// above. This test exists so a run of just this file with 0 failures is
/// itself evidence, not merely the absence of a crash.
#[test]
fn green_control_all_rules_hold_at_head() {
    with_locked_masked_masks_the_whole_hold();
    poll_fd_udp_arm_uses_the_masked_primitive();
    sys_bind_udp_arm_uses_the_masked_primitive();
    sys_sendto_udp_arm_uses_the_masked_primitive();
    sys_recvfrom_nonblocking_read_uses_the_masked_primitive();
    udp_delivery_locks_the_socket_under_process_manager();
    udp_lock_received_uses_the_masked_primitive();
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
