use std::fs;
use std::path::PathBuf;

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

fn masked_code(source: &str) -> String {
    let bytes: Vec<u8> = source
        .bytes()
        .zip(code_mask(source))
        .map(|(byte, code)| if code { byte } else { b' ' })
        .collect();
    String::from_utf8(bytes).expect("masked Rust source remains UTF-8")
}

fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let masked = masked_code(source);
    let needle = format!("fn {name}(");
    let function = masked
        .find(&needle)
        .unwrap_or_else(|| panic!("find function {name}"));
    let open = masked[function..]
        .find('{')
        .map(|offset| function + offset)
        .unwrap_or_else(|| panic!("find opening brace for function {name}"));

    let bytes = masked.as_bytes();
    let mut depth = 0usize;
    for index in open..bytes.len() {
        match bytes[index] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth
                    .checked_sub(1)
                    .unwrap_or_else(|| panic!("match braces for function {name}"));
                if depth == 0 {
                    return &source[open..=index];
                }
            }
            _ => {}
        }
    }

    panic!("find closing brace for function {name}")
}

fn assert_irq_masked_queue_access(body: &str, function: &str, queue_lock: &str) {
    let body = masked_code(body);
    let save = body
        .find("irq_save()")
        .unwrap_or_else(|| panic!("{function} no longer masks IRQs before taking {queue_lock}"));
    let lock = body.find(queue_lock).unwrap_or_else(|| {
        panic!("{function} no longer accesses {queue_lock}, so the #545 lock invariant is untested")
    });
    let restore = body.find("irq_restore(").unwrap_or_else(|| {
        panic!("{function} no longer restores IRQ state after releasing {queue_lock}")
    });

    assert!(
        save < lock && lock < restore,
        "{function} must take {queue_lock} between irq_save and irq_restore to prevent same-CPU softirq deadlock"
    );
}

// #545 invariant: loopback enqueue is IRQ-safe and always raises its dedicated pump.
#[test]
fn loopback_enqueue_raises_dedicated_softirq() {
    let source = repo_text("kernel/src/net/mod.rs");
    let body = function_body(&source, "send_ipv4");
    let code = masked_code(body);

    assert_irq_masked_queue_access(body, "send_ipv4", "LOOPBACK_QUEUE.lock()");
    let push = code
        .find("queue.push(LoopbackPacket")
        .expect("send_ipv4 no longer pushes loopback packets onto LOOPBACK_QUEUE");
    let lock = code
        .find("LOOPBACK_QUEUE.lock()")
        .expect("send_ipv4 no longer locks LOOPBACK_QUEUE before enqueue");
    let restore = code
        .find("irq_restore(")
        .expect("send_ipv4 no longer restores IRQs after loopback enqueue");
    let raise = code.find("raise_softirq(SoftirqType::Loopback)").expect(
        "send_ipv4 no longer raises the Loopback softirq, so a blocked receiver can starve",
    );
    assert!(
        lock < push && push < restore && restore < raise,
        "send_ipv4 must enqueue under the IRQ-masked LOOPBACK_QUEUE lock, then restore IRQs before raising the pump"
    );
}

// #545 invariant: the loopback drain defers under PM ownership and is single-entry and IRQ-safe.
#[test]
fn loopback_drain_has_deadlock_guards() {
    let source = repo_text("kernel/src/net/mod.rs");
    let body = function_body(&source, "drain_loopback_queue");
    let code = masked_code(body);

    assert!(
        code.contains("process_manager_held_on_current_cpu()"),
        "drain_loopback_queue must defer while this CPU owns the process-manager lock or UDP delivery can deadlock"
    );
    assert!(
        code.contains("LOOPBACK_DRAINING") && code.contains(".compare_exchange("),
        "drain_loopback_queue lost its LOOPBACK_DRAINING compare_exchange guard, allowing re-entrant drains"
    );
    assert_irq_masked_queue_access(body, "drain_loopback_queue", "LOOPBACK_QUEUE.lock()");
}

// #545 invariant: network softirq registration restores the dedicated loopback handler.
#[test]
fn network_registration_installs_loopback_handler() {
    let source = repo_text("kernel/src/net/mod.rs");
    let body = masked_code(function_body(&source, "register_net_softirq"));

    assert!(
        body.contains(
            "register_softirq_handler(SoftirqType::Loopback, loopback_softirq_handler)"
        ),
        "register_net_softirq must install the Loopback handler so tests and network init cannot leave the pump disconnected"
    );
}

// #545 invariant: the loopback handler drains only local backlog and never polls the NIC.
#[test]
fn loopback_handler_never_polls_the_device() {
    let source = repo_text("kernel/src/net/mod.rs");
    let body = masked_code(function_body(&source, "loopback_softirq_handler"));

    assert!(
        body.contains("drain_loopback_queue()"),
        "loopback_softirq_handler must drain LOOPBACK_QUEUE or blocked TCP receivers will not wake"
    );
    for forbidden in ["process_rx", "process_rx_budgeted", "e1000"] {
        assert!(
            !body.contains(forbidden),
            "loopback_softirq_handler mentions {forbidden}; loopback softirq context must never poll the NIC or contend for its driver lock"
        );
    }
}

// #545 invariant: deferred TCP TX queue access is IRQ-masked in producer and softirq drain paths.
#[test]
fn deferred_tcp_tx_queue_is_irq_masked() {
    let source = repo_text("kernel/src/net/tcp.rs");

    for function in ["queue_deferred_tx_with_mac", "drain_deferred_tx"] {
        assert_irq_masked_queue_access(
            function_body(&source, function),
            function,
            "DEFERRED_TX_QUEUE.lock()",
        );
    }
}
