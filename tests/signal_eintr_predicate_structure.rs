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
                    return Some(&source[open..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn function_body<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    for function in identifier_offsets(source, &mask, "fn") {
        let mut cursor = function + 2;
        while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        if &source[name_start..cursor] != name {
            continue;
        }
        let brace = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{')?;
        let semicolon = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b';');
        if semicolon.is_some_and(|semicolon| semicolon < brace) {
            continue;
        }
        return braced_block(source, &mask, brace);
    }
    None
}

fn calls_identifier(source: &str, identifier: &str) -> bool {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    identifier_offsets(source, &mask, identifier)
        .into_iter()
        .any(|offset| {
            let mut cursor = offset + identifier.len();
            while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
                cursor += 1;
            }
            bytes.get(cursor) == Some(&b'(')
        })
}

fn validate_eintr_call_site(source: &str) -> Result<(), &'static str> {
    let body = function_body(source, "check_signals_for_eintr")
        .ok_or("missing check_signals_for_eintr")?;
    if !calls_identifier(body, "has_interrupting_signals") {
        return Err("EINTR check does not call has_interrupting_signals");
    }
    if calls_identifier(body, "has_deliverable_signals") {
        return Err("EINTR check still calls has_deliverable_signals");
    }
    Ok(())
}

fn validate_interrupting_predicate(source: &str) -> Result<(), &'static str> {
    let body = function_body(source, "has_interrupting_signals")
        .ok_or("missing SignalState::has_interrupting_signals")?;
    if !calls_identifier(body, "has_deliverable_signals") {
        return Err("EINTR and delivery must share the disposition predicate");
    }
    let delivery = function_body(source, "has_deliverable_signals").unwrap();
    if !delivery.contains("self.pending & !self.blocked & !self.ignored") {
        return Err("delivery must filter the cached ignored disposition mask");
    }
    let install = function_body(source, "set_handler").unwrap();
    for required in [
        "action.is_ignore()",
        "action.is_default()",
        "DEFAULT_IGNORED_SIGNALS",
        "self.ignored |= bit",
        "self.ignored &= !bit",
        "self.pending &= !bit",
    ] {
        if !install.contains(required) {
            return Err("disposition cache maintenance missing");
        }
    }
    Ok(())
}

fn validate_delivery_wrapper(source: &str) -> Result<(), &'static str> {
    let body = function_body(source, "has_interrupting_signals")
        .ok_or("missing delivery::has_interrupting_signals")?;
    if !calls_identifier(body, "has_interrupting_signals") {
        return Err("delivery wrapper does not call the SignalState predicate");
    }
    Ok(())
}

#[test]
fn syscall_eintr_uses_disposition_aware_signal_predicate() {
    let syscall = repo_text("kernel/src/syscall/mod.rs");
    let signal_types = repo_text("kernel/src/signal/types.rs");
    let delivery = repo_text("kernel/src/signal/delivery.rs");

    assert_eq!(validate_eintr_call_site(&syscall), Ok(()));
    assert_eq!(validate_interrupting_predicate(&signal_types), Ok(()));
    assert_eq!(validate_delivery_wrapper(&delivery), Ok(()));
}

#[test]
fn eintr_validator_rejects_deliverable_signal_call() {
    let synthetic = r#"
        fn check_signals_for_eintr() -> Option<i32> {
            if crate::signal::delivery::has_deliverable_signals(process) {
                return Some(errno::EINTR);
            }
            None
        }
    "#;

    assert!(validate_eintr_call_site(synthetic).is_err());
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

#[test]
fn disposition_mutation_is_rejected() {
    let source = repo_text("kernel/src/signal/types.rs");
    let mutant = source.replace(
        "self.pending & !self.blocked & !self.ignored",
        "self.pending & !self.blocked",
    );
    assert!(validate_interrupting_predicate(&mutant).is_err());
}

fn live_code(source: &str) -> String {
    source
        .bytes()
        .zip(code_mask(source))
        .filter_map(|(b, live)| (live && !b.is_ascii_whitespace()).then_some(b as char))
        .collect()
}

fn depth_at(source: &str, end: usize) -> i32 {
    source[..end].bytes().fold(0, |depth, byte| match byte {
        b'{' => depth + 1,
        b'}' => depth - 1,
        _ => depth,
    })
}

fn validate_child_barrier(source: &str) -> Result<(), &'static str> {
    let race = live_code(function_body(source, "run_race").ok_or("missing run_race")?);
    // Conservative grammar: this exact control-flow tail must be at function
    // scope. Strings/comments are masked on both sides. Only a successful reap
    // of this child can break the loop; status/errors/deadline cannot fall through.
    let tail = live_code(
        r#"
        let deadline = monotonic_ms().saturating_add(PROBE_DEADLINE_MS);
        loop {
            let mut status = 0;
            match process::waitpid(child.raw() as i32, &mut status, process::WNOHANG) {
                Ok(pid) if pid == child => {
                    if !process::wifexited(status) || process::wexitstatus(status) != 0 {
                        return Err(fail("child_status", format!("{}", status)));
                    }
                    break;
                }
                Ok(_) => {}
                Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EINTR)) => {}
                Err(e) => return Err(fail("child_wait", format!("{}", e))),
            }
            if monotonic_ms() >= deadline {
                return Err(fail("child_wait_timeout", "not_reaped".to_string()));
            }
            let _ = process::yield_now();
        }
        Ok(())
    }"#,
    );
    let offset = race.find(&tail).ok_or("missing mandatory reap tail")?;
    if depth_at(&race, offset) != 1 || !race.ends_with(&tail) || race[..offset].contains("Ok(())") {
        return Err("reap is bypassable");
    }
    let run = live_code(function_body(source, "run").ok_or("missing run")?);
    let calls: Vec<_> = run.match_indices("run_race(").collect();
    if calls.len() != 2 {
        return Err("must synchronize both stages");
    }
    let mut ends = Vec::new();
    for (start, _) in &calls {
        if depth_at(&run, *start) != 1 {
            return Err("race call is conditional");
        }
        let mut depth = 1;
        let open = *start + "run_race(".len();
        let end = run
            .bytes()
            .enumerate()
            .skip(open)
            .find_map(|(i, b)| {
                if b == b'(' {
                    depth += 1;
                }
                if b == b')' {
                    depth -= 1;
                }
                (depth == 0).then_some(i + 1)
            })
            .ok_or("unfinished call")?;
        if !run[end..].starts_with("?;") {
            return Err("reap errors are discarded");
        }
        ends.push(end + 2);
    }
    let install = run
        .find("letaction=Sigaction::new(sigchld_handler);")
        .ok_or("missing install")?;
    let reset = run
        .find("SIGCHLD_HANDLED.store(false,Ordering::SeqCst);")
        .ok_or("missing reset")?;
    let assertion = live_code(
        r#"if !SIGCHLD_HANDLED.load(Ordering::SeqCst) {
        return Err(fail("sig_handler_never_ran", "flag=0".to_string()));
    } Ok(()) }"#,
    );
    if !(ends[0] <= install && install < reset && reset < calls[1].0)
        || run[ends[1]..] != assertion
        || run[..ends[1]].contains("Ok(())")
        || run[..ends[1]].contains("SIGCHLD_HANDLED.load")
    {
        return Err("handler assertion must follow propagated second reap");
    }
    Ok(())
}

#[test]
fn child_barrier_precedes_handler_assertion() {
    assert_eq!(
        validate_child_barrier(&repo_text("userspace/programs/src/block_eintr_oracle.rs")),
        Ok(())
    );
}

#[test]
fn barrier_mutations_are_rejected() {
    let source = repo_text("userspace/programs/src/block_eintr_oracle.rs");
    for (old, new) in [
        ("pid == child", "pid != child"),
        ("Ok(_) => {}", "Ok(_) => { break; }"),
        (
            "let deadline = monotonic_ms()",
            "return Ok(()); let deadline = monotonic_ms()",
        ),
        (
            "loop {\n        let mut status",
            "if false { loop {\n        let mut status",
        ),
        ("})?;", "});"),
        (
            "if !SIGCHLD_HANDLED.load",
            "if false {} if !SIGCHLD_HANDLED.load",
        ),
        (
            "return Err(fail(\"child_wait_timeout\", \"not_reaped\".to_string()));",
            "break;",
        ),
        ("process::WNOHANG", "0"),
    ] {
        assert!(source.contains(old), "mutation anchor missing: {old}");
        assert!(
            validate_child_barrier(&source.replace(old, new)).is_err(),
            "accepted {new}"
        );
    }
    let race = function_body(&source, "run_race").unwrap();
    let spoof = source.replace(
        race,
        r#"{
        if false { process::waitpid(0, 0, 0); }
        let evidence = "pid == child child_wait_timeout";
        Ok(())
    }"#,
    );
    assert!(validate_child_barrier(&spoof).is_err());
    let run = function_body(&source, "run").unwrap();
    let assertion = "if !SIGCHLD_HANDLED.load(Ordering::SeqCst)";
    let moved = source.replace(
        run,
        &run.replacen(
            "    // Stage 1",
            &format!(
                "    {assertion} {{ return Err(fail(\"early\", String::new())); }}\n    // Stage 1"
            ),
            1,
        ),
    );
    // Any early load is prohibited as well as requiring the final assertion.
    assert!(validate_child_barrier(&moved).is_err());
}

fn has_output(source: &str) -> bool {
    let code = live_code(source);
    ["log::", "serial_print", "println!", "print!", "format!"]
        .iter()
        .any(|s| code.contains(s))
}

#[test]
fn disposition_capture_is_silent_and_reporter_is_off_syscall_path() {
    let oracle = repo_text("kernel/src/syscall/futex_oracle.rs");
    for name in ["disposition_inject", "disposition_record"] {
        let body = function_body(&oracle, name).unwrap();
        assert!(!has_output(body), "output in {name}");
        assert!(has_output(&body.replacen(
            '{',
            "{ crate::serial_println!(\"mutant\");",
            1
        )));
    }
    let futex = repo_text("kernel/src/syscall/futex.rs");
    assert!(!calls_identifier(&futex, "disposition_report"));
    assert!(calls_identifier(&futex, "disposition_record"));
    let sampler = repo_text("kernel/src/task/strand_oracle.rs");
    assert!(calls_identifier(
        function_body(&sampler, "report_strand").unwrap(),
        "disposition_report"
    ));
    assert!(
        live_code(function_body(&oracle, "disposition_record").unwrap())
            .contains("slot.store(record,Ordering::Release)")
    );
    assert!(
        live_code(function_body(&oracle, "disposition_report").unwrap())
            .contains("slot.swap(0,Ordering::AcqRel)")
    );
}

#[test]
fn signal_delivery_and_local_helpers_are_silent() {
    let source = repo_text("kernel/src/signal/delivery.rs");
    assert!(!has_output(&source));
    for injected in [
        "log::debug!(\"mutant\");",
        "crate::serial_println!(\"mutant\");",
    ] {
        let body = function_body(&source, "deliver_pending_signals").unwrap();
        assert!(has_output(&source.replace(
            body,
            &body.replacen('{', &format!("{{{injected}"), 1)
        )));
    }
}

#[test]
fn disposition_oracle_drives_real_wait_and_strict_scorer_requires_both_arms() {
    let futex = repo_text("kernel/src/syscall/futex.rs");
    let queued = futex.rfind("PrepareOutcome::Queued =>").unwrap();
    let inject = futex.find("disposition_inject(_val3, thread_id)").unwrap();
    let check = futex
        .find("crate::syscall::check_signals_for_eintr()")
        .unwrap();
    assert!(queued < inject && inject < check);
    assert!(futex.contains("disposition_record(_val3, disposition_armed, &result)"));
    let oracle = repo_text("kernel/src/syscall/futex_oracle.rs");
    assert!(oracle.contains("thread.state == crate::task::thread::ThreadState::BlockedOnIO"));
    assert!(oracle.contains("process.signals.pending |= sig_mask(SIGCHLD)"));
    let scorer = repo_text("docker/qemu/run-aarch64-boot-test-strict.sh");
    for arm in [
        "default:blocked=1:pending=1:errno=110:PASS]",
        "handler:blocked=1:pending=1:errno=4:PASS]",
    ] {
        assert!(scorer.contains(arm));
    }
    assert!(scorer.contains("Signal disposition oracle failed"));
}

#[test]
fn strict_disposition_scoring_rejects_missing_and_failed_arms() {
    let fixture = repo_text("tests/fixtures/udp-socket-lock-aarch64-serial.txt");
    let arms = [
        "[SIGNAL_DISPOSITION_ORACLE:arm=default:blocked=1:pending=1:errno=110:PASS]",
        "[SIGNAL_DISPOSITION_ORACLE:arm=handler:blocked=1:pending=1:errno=4:PASS]",
    ];
    let scratch = std::env::temp_dir().join(format!("disposition-score-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    for (index, (serial, expected)) in [
        (fixture.clone(), true),
        (fixture.replace(arms[0], ""), false),
        (fixture.replace(arms[1], ""), false),
        (
            format!(
                "{}\n[SIGNAL_DISPOSITION_ORACLE:arm=default:blocked=1:pending=1:errno=4:FAIL]\n",
                fixture
            ),
            false,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let path = scratch.join(format!("{index}.txt"));
        std::fs::write(&path, serial).unwrap();
        let output = std::process::Command::new("bash")
            .arg("docker/qemu/run-aarch64-boot-test-strict.sh")
            .env("BREENIX_STRICT_SCORE_ONLY", &path)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            expected,
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
