//! #823 -- each of the 4 thread-side holds of the fd table's `Arc<Mutex<UdpSocket>>`
//! runs masked for its whole duration, through one production primitive,
//! `crate::socket::udp::with_locked_masked`.
//!
//! The defect these rules pin: `net/udp.rs::deliver_to_socket` (the NetRx IRQ
//! route) locks this exact `Mutex<UdpSocket>` from inside
//! `with_process_manager`, which masks interrupts on both architectures. Four
//! thread-side callers took the same lock without masking for the whole hold
//! -- `ipc/poll.rs`'s `poll_fd` (reached by `sys_poll`, `sys_select` and
//! `sys_epoll_wait`), and three sites in `syscall/socket.rs` (`sys_bind`,
//! `sys_sendto`, `sys_recvfrom`'s nonblocking-flag read). These 4 sites now route
//! through `with_locked_masked`, which masks interrupts for the whole
//! closure.
//!
//! This is a fixed, enumerated census (4 call sites plus the one primitive
//! and its one IRQ-side counterpart), not a naming-convention sweep like
//! #821/#822's -- there is no larger family of `_nonblock`-style twins here to
//! rediscover, so the rules name the sites directly and each carries its own
//! anti-vacuity mutation.

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
        body.contains("f(&mut socket.lock())") || body.contains("f(&mut socket . lock ())"),
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
        "Cpu::without_interrupts(|| f(&mut socket.lock()))",
        "f(&mut socket.lock())",
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

/// #823 rule 2: `poll_fd`'s `FdKind::UdpSocket` arm calls the primitive, not
/// a raw `.lock()` on the socket.
#[test]
fn poll_fd_udp_arm_uses_the_masked_primitive() {
    let source = repo_text(POLL_MODULE);
    let code = masked_text(&source);
    let start = code
        .find("FdKind::UdpSocket(socket) =>")
        .expect("#823 shape failure: poll_fd's FdKind::UdpSocket arm was renamed or removed");
    let end = code[start..]
        .find("FdKind::RegularFile")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let arm = &code[start..end];
    assert!(
        arm.contains(PRIMITIVE_CALL),
        "#823 shape failure: poll_fd's UdpSocket arm no longer calls \
         with_locked_masked -- it may be back to an unmasked socket.lock()"
    );
    assert!(
        !arm.contains("socket.lock()"),
        "#823 shape failure: poll_fd's UdpSocket arm still takes a raw, \
         unmasked socket.lock() alongside (or instead of) with_locked_masked"
    );
}

#[test]
fn poll_fd_udp_arm_rule_is_not_vacuous() {
    let source = repo_text(POLL_MODULE);
    let mutated = source.replacen(
        "crate::socket::udp::with_locked_masked(socket, |s| s.has_data())",
        "socket.lock().has_data()",
        1,
    );
    assert_ne!(
        source, mutated,
        "mutation text not found in kernel/src/ipc/poll.rs -- update this test's \
         literal to match the real poll_fd UdpSocket arm before trusting rule 2"
    );
    let code = masked_text(&mutated);
    let start = code.find("FdKind::UdpSocket(socket) =>").unwrap();
    let end = code[start..]
        .find("FdKind::RegularFile")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let arm = &code[start..end];
    assert!(
        !arm.contains(PRIMITIVE_CALL),
        "mutation did not remove with_locked_masked from the arm under test"
    );
}

/// #823 rule 3: `sys_bind`'s UDP arm calls the primitive around `bind(..)`.
#[test]
fn sys_bind_udp_arm_uses_the_masked_primitive() {
    let source = repo_text(SOCKET_SYSCALLS);
    let code = masked_text(&source);
    let start = code
        .find("FdKind::UdpSocket(s) => {")
        .expect("#823 shape failure: sys_bind's FdKind::UdpSocket arm was renamed or removed");
    let end = code[start..]
        .find("FdKind::TcpSocket(existing_port)")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let arm = &code[start..end];
    assert!(
        arm.contains(PRIMITIVE_CALL),
        "#823 shape failure: sys_bind's UdpSocket arm no longer calls \
         with_locked_masked around bind()"
    );
}

#[test]
fn sys_bind_udp_arm_rule_is_not_vacuous() {
    let source = repo_text(SOCKET_SYSCALLS);
    let mutated = source.replacen(
        "let socket_ref = s.clone();\n                    match crate::socket::udp::with_locked_masked(&socket_ref, |socket| {\n                        socket.bind(pid, addr.addr, addr.port_host())\n                    }) {",
        "let socket_ref = s.clone();\n                    let mut socket = socket_ref.lock();\n                    match socket.bind(pid, addr.addr, addr.port_host()) {",
        1,
    );
    assert_ne!(
        source, mutated,
        "mutation text not found in kernel/src/syscall/socket.rs -- update this test's \
         literal to match the real sys_bind UdpSocket arm before trusting rule 3"
    );
    let code = masked_text(&mutated);
    let start = code.find("FdKind::UdpSocket(s) => {").unwrap();
    let end = code[start..]
        .find("FdKind::TcpSocket(existing_port)")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let arm = &code[start..end];
    assert!(
        !arm.contains(PRIMITIVE_CALL),
        "mutation did not remove with_locked_masked from the arm under test"
    );
}

/// #823 rule 4: `sys_sendto`'s local-port lookup calls the primitive.
#[test]
fn sys_sendto_udp_arm_uses_the_masked_primitive() {
    let source = repo_text(SOCKET_SYSCALLS);
    let code = masked_text(&source);
    let start = code
        .find("pub fn sys_sendto(")
        .expect("#823 shape failure: sys_sendto was renamed or removed");
    let end = code[start..]
        .find("let udp_packet =")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let region = &code[start..end];
    assert!(
        region.contains(PRIMITIVE_CALL),
        "#823 shape failure: sys_sendto's UdpSocket local-port read no longer \
         calls with_locked_masked"
    );
}

#[test]
fn sys_sendto_udp_arm_rule_is_not_vacuous() {
    let source = repo_text(SOCKET_SYSCALLS);
    let mutated = source.replacen(
        "crate::socket::udp::with_locked_masked(s, |socket| socket.local_port().unwrap_or(0))",
        "s.lock().local_port().unwrap_or(0)",
        1,
    );
    assert_ne!(
        source, mutated,
        "mutation text not found in kernel/src/syscall/socket.rs -- update this test's \
         literal to match the real sys_sendto UdpSocket arm before trusting rule 4"
    );
    let code = masked_text(&mutated);
    let start = code.find("pub fn sys_sendto(").unwrap();
    let end = code[start..]
        .find("let udp_packet =")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let region = &code[start..end];
    assert!(
        !region.contains(PRIMITIVE_CALL),
        "mutation did not remove with_locked_masked from the region under test"
    );
}

/// #823 rule 5: `sys_recvfrom`'s nonblocking-flag read calls the primitive.
#[test]
fn sys_recvfrom_nonblocking_read_uses_the_masked_primitive() {
    let source = repo_text(SOCKET_SYSCALLS);
    let code = masked_text(&source);
    let function_start = code.find("pub fn sys_recvfrom(").expect("sys_recvfrom missing");
    let start = code[function_start..]
        .find("let nonblocking =")
        .map(|i| function_start + i)
        .expect("#823 shape failure: sys_recvfrom's nonblocking-flag read was removed or renamed");
    let end = (start + 400).min(code.len());
    let region = &code[start..end];
    assert!(
        region.contains(PRIMITIVE_CALL),
        "#823 shape failure: sys_recvfrom's nonblocking-flag read no longer \
         calls with_locked_masked"
    );
}

#[test]
fn sys_recvfrom_nonblocking_read_rule_is_not_vacuous() {
    let source = repo_text(SOCKET_SYSCALLS);
    let mutated = source.replacen(
        "crate::socket::udp::with_locked_masked(&socket, |s| s.nonblocking)",
        "socket.lock().nonblocking",
        1,
    );
    assert_ne!(
        source, mutated,
        "mutation text not found in kernel/src/syscall/socket.rs -- update this test's \
         literal to match the real sys_recvfrom nonblocking read before trusting rule 5"
    );
    let code = masked_text(&mutated);
    let function_start = code.find("pub fn sys_recvfrom(").unwrap();
    let start = function_start + code[function_start..].find("let nonblocking =").unwrap();
    let end = (start + 400).min(code.len());
    let region = &code[start..end];
    assert!(
        !region.contains(PRIMITIVE_CALL),
        "mutation did not remove with_locked_masked from the region under test"
    );
}

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

/// #823 rule 7: the oracle's own receive checker uses the masked primitive.
#[test]
fn udp_lock_received_uses_the_masked_primitive() {
    let source = repo_text(TEST_REGISTRY);
    let code = masked_text(&source);
    let start = code
        .find("fn udp_lock_received(")
        .expect("#823 shape failure: udp_lock_received was renamed or removed");
    let end = code[start..]
        .find("\n}\n")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let body = &code[start..end];
    assert!(
        body.contains(PRIMITIVE_CALL),
        "#823 shape failure: udp_lock_received no longer calls with_locked_masked"
    );
}

#[test]
fn udp_lock_received_rule_is_not_vacuous() {
    let source = repo_text(TEST_REGISTRY);
    let mutated = source.replacen(
        "fn udp_lock_received(open: &UdpLockSocket) -> u64 {\n    crate::socket::udp::with_locked_masked(&open.socket, |socket| {\n        let queue = socket.rx_queue.lock();\n        queue\n            .iter()\n            .filter(|packet| packet.data.as_slice() == UDP_LOCK_PAYLOAD)\n            .count() as u64\n    })\n}",
        "fn udp_lock_received(open: &UdpLockSocket) -> u64 {\n    let socket = open.socket.lock();\n    let queue = socket.rx_queue.lock();\n    queue\n        .iter()\n        .filter(|packet| packet.data.as_slice() == UDP_LOCK_PAYLOAD)\n        .count() as u64\n}",
        1,
    );
    assert_ne!(
        source, mutated,
        "mutation text not found in kernel/src/test_framework/registry.rs -- update this test's literal before trusting rule 7"
    );
    let code = masked_text(&mutated);
    let start = code.find("fn udp_lock_received(").unwrap();
    let end = code[start..]
        .find("\n}\n")
        .map(|i| start + i)
        .unwrap_or(code.len());
    let body = &code[start..end];
    assert!(
        !body.contains(PRIMITIVE_CALL),
        "mutation did not remove with_locked_masked from the body under test"
    );
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
    deliver_to_socket_still_locks_under_process_manager();
    udp_lock_received_uses_the_masked_primitive();
}
