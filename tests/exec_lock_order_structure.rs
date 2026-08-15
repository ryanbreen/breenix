use std::collections::BTreeMap;
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

fn identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric() || !byte.is_ascii()
}

fn identifier_offsets(source: &str, mask: &[bool], identifier: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    code_offsets(source, mask, identifier)
        .into_iter()
        .filter(|offset| {
            let end = *offset + identifier.len();
            !offset
                .checked_sub(1)
                .and_then(|before| bytes.get(before))
                .is_some_and(|byte| identifier_byte(*byte))
                && !bytes.get(end).is_some_and(|byte| identifier_byte(*byte))
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

/// Every `fn NAME(` definition in a module, keyed by name. Names may repeat
/// (`cfg`-split, or same-named inherent methods on different types); every body
/// is kept so a check over a name covers all of them.
fn module_function_bodies(source: &str) -> BTreeMap<String, Vec<&str>> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let mut bodies: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for offset in identifier_offsets(source, &mask, "fn") {
        let mut cursor = offset + "fn".len();
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
        let name = &source[name_start..cursor];
        // A signature terminated by `;` (trait requirement, extern block) has no
        // body; taking the next brace would attribute a foreign body to it.
        let brace = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b'{');
        let semicolon = (cursor..bytes.len()).find(|index| mask[*index] && bytes[*index] == b';');
        let Some(brace) = brace else { continue };
        if semicolon.is_some_and(|semicolon| semicolon < brace) {
            continue;
        }
        let Some(body) = braced_block(source, &mask, brace) else {
            continue;
        };
        bodies.entry(name.to_owned()).or_default().push(body);
    }
    bodies
}

fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let plain_marker = format!("fn {name}(");
    let generic_marker = format!("fn {name}<");
    let mask = code_mask(source);
    let start = [
        code_offsets(source, &mask, &plain_marker)
            .into_iter()
            .next(),
        code_offsets(source, &mask, &generic_marker)
            .into_iter()
            .next(),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or_else(|| panic!("missing function {name}"));
    let bytes = source.as_bytes();
    let mut line_comment = false;
    let mut block_comment_depth = 0usize;
    let mut string = false;
    let mut character = false;
    let mut raw_string_hashes = None;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut open = None;
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];

        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            }
            index += 1;
            continue;
        }
        if block_comment_depth != 0 {
            if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
                block_comment_depth += 1;
                index += 2;
            } else if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                block_comment_depth -= 1;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(hashes) = raw_string_hashes {
            if byte == b'"'
                && bytes
                    .get(index + 1..index + 1 + hashes)
                    .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
            {
                raw_string_hashes = None;
                index += hashes + 1;
            }
            index += 1;
            continue;
        }
        if string || character {
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
            line_comment = true;
            index += 2;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
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
                raw_string_hashes = Some(quote - index - 1);
                index = quote + 1;
                continue;
            }
        }
        if byte == b'"' {
            string = true;
            index += 1;
            continue;
        }
        if byte == b'\'' {
            let plain_char = bytes.get(index + 2) == Some(&b'\'');
            let escaped_char =
                bytes.get(index + 1) == Some(&b'\\') && bytes.get(index + 3) == Some(&b'\'');
            if plain_char || escaped_char {
                character = true;
                index += 1;
                continue;
            }
        }

        match byte {
            b'{' => {
                open.get_or_insert(index);
                depth += 1;
            }
            b'}' if open.is_some() => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..index + 1];
                }
            }
            _ => {}
        }
        index += 1;
    }
    panic!("unterminated function {name}")
}

const SCHEDULER_LOCK_FAMILY: [&str; 6] = [
    "with_thread_mut",
    "with_scheduler",
    "lock_scheduler",
    "try_lock_scheduler",
    "SCHEDULER.lock",
    "SCHEDULER.try_lock",
];

fn arm64_exec_bodies(manager: &str) -> Vec<(&str, &str)> {
    let functions = module_function_bodies(manager);
    ["exec_process", "exec_process_with_argv"]
        .into_iter()
        .flat_map(|name| {
            functions
                .get(name)
                .into_iter()
                .flatten()
                .filter(|body| body.contains("[ARM64]"))
                .map(move |body| (name, *body))
        })
        .collect()
}

fn validate_arm64_exec_bodies_never_touch_scheduler_lock(manager: &str) -> Result<(), String> {
    let bodies = arm64_exec_bodies(manager);
    if bodies.len() != 2 {
        return Err(format!(
            "expected exactly two ARM64 exec bodies, found {}",
            bodies.len()
        ));
    }

    for (name, body) in bodies {
        let mask = code_mask(body);
        for needle in SCHEDULER_LOCK_FAMILY {
            let count = code_offsets(body, &mask, needle).len();
            if count != 0 {
                return Err(format!(
                    "ARM64 {name} body contains {count} code occurrence(s) of {needle}"
                ));
            }
        }
    }
    Ok(())
}

fn validate_arm64_exec_staging_order(manager: &str) -> Result<(), String> {
    let bodies = arm64_exec_bodies(manager);
    if bodies.len() != 2 {
        return Err(format!(
            "expected exactly two ARM64 exec bodies, found {}",
            bodies.len()
        ));
    }

    for (name, body) in bodies {
        let mask = code_mask(body);
        let mut stages = Vec::new();
        for needle in [
            "new_page_table.publish()",
            "thread.context.elr_el1 = new_entry_point",
            "thread.state = crate::task::thread::ThreadState::Ready",
            "let ctx = thread.context.clone()",
            "crate::task::scheduler::ExecSchedCommit::new(",
        ] {
            let offsets = code_offsets(body, &mask, needle);
            if offsets.len() != 1 {
                return Err(format!(
                    "ARM64 {name} body must contain exactly one `{needle}`, found {}",
                    offsets.len()
                ));
            }
            stages.push((needle, offsets[0]));
        }

        for pair in stages.windows(2) {
            if pair[0].1 >= pair[1].1 {
                return Err(format!(
                    "ARM64 {name} staging order violation: `{}` must precede `{}`",
                    pair[0].0, pair[1].0
                ));
            }
        }
    }
    Ok(())
}

fn validate_manager_module_has_no_scheduler_lock_acquisition(manager: &str) -> Result<(), String> {
    let mask = code_mask(manager);
    for needle in SCHEDULER_LOCK_FAMILY {
        let count = code_offsets(manager, &mask, needle).len();
        if count != 0 {
            return Err(format!(
                "manager.rs contains {count} code occurrence(s) of {needle}"
            ));
        }
    }
    Ok(())
}

fn validate_exec_sched_commit(scheduler: &str) -> Result<(), String> {
    let mask = code_mask(scheduler);
    let declarations = code_offsets(scheduler, &mask, "pub struct ExecSchedCommit");
    if declarations.len() != 1 {
        return Err(format!(
            "expected one ExecSchedCommit declaration, found {}",
            declarations.len()
        ));
    }

    let mut cursor = declarations[0];
    let mut must_use = false;
    loop {
        while cursor != 0 && scheduler.as_bytes()[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor == 0 || scheduler.as_bytes()[cursor - 1] != b']' {
            break;
        }
        let Some(attribute_start) = scheduler[..cursor].rfind("#[") else {
            break;
        };
        must_use |= scheduler[attribute_start..cursor].starts_with("#[must_use");
        cursor = attribute_start;
    }
    if !must_use {
        return Err("ExecSchedCommit is not directly annotated #[must_use]".to_owned());
    }

    let functions = module_function_bodies(scheduler);
    let apply_bodies = functions.get("apply").map(Vec::as_slice).unwrap_or(&[]);
    if apply_bodies.len() != 1 {
        return Err(format!(
            "expected one ExecSchedCommit::apply body, found {}",
            apply_bodies.len()
        ));
    }
    let apply = apply_bodies[0];
    let apply_mask = code_mask(apply);
    for needle in [
        "process_manager_held_on_current_cpu()",
        "EXEC_COMMIT_MISSING_THREAD",
        "SCHED_AFTER_PM_VIOLATIONS",
        "EXEC_COMMIT_UNPINNED",
        "EXEC_SCHED_COMMITS",
    ] {
        if code_offsets(apply, &apply_mask, needle).is_empty() {
            return Err(format!("ExecSchedCommit::apply is missing {needle}"));
        }
    }
    if !apply.contains("[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]") {
        return Err("ExecSchedCommit::apply is missing the PM-held violation marker".to_owned());
    }
    if !apply.contains("[EXEC_LOCK_ORDER:VIOLATION:NO_SCHED_THREAD]") {
        return Err(
            "ExecSchedCommit::apply is missing the no-scheduler-thread violation marker".to_owned(),
        );
    }
    for forbidden in ["log::", "serial_println!"] {
        if !code_offsets(apply, &apply_mask, forbidden).is_empty() {
            return Err(format!(
                "ExecSchedCommit::apply contains forbidden output path {forbidden}"
            ));
        }
    }
    Ok(())
}

fn validate_sys_exec_releases_process_manager(syscall_entry: &str) -> Result<(), String> {
    let body = function_body(syscall_entry, "sys_exec_aarch64");
    let mask = code_mask(body);
    let drops = code_offsets(body, &mask, "drop(manager_guard)");
    let applies = code_offsets(body, &mask, "commit.apply()");
    let saved_cr3 = code_offsets(body, &mask, "set_saved_process_cr3(");
    let ttbr0: Vec<usize> = body
        .match_indices("msr ttbr0_el1")
        .map(|(offset, _)| offset)
        .collect();

    for (label, count) in [
        ("drop(manager_guard)", drops.len()),
        ("commit.apply()", applies.len()),
        ("msr ttbr0_el1", ttbr0.len()),
        ("set_saved_process_cr3(", saved_cr3.len()),
    ] {
        if count != 1 {
            return Err(format!(
                "expected one {label} in sys_exec_aarch64, found {count}"
            ));
        }
    }

    let drop_offset = drops[0];
    let apply_offset = applies[0];
    if !(drop_offset < apply_offset && apply_offset < ttbr0[0] && apply_offset < saved_cr3[0]) {
        return Err(
            "sys_exec_aarch64 must drop PM before apply, TTBR0 write, and saved-CR3 update"
                .to_owned(),
        );
    }

    let after_drop = &body[drop_offset + "drop(manager_guard)".len()..];
    let after_drop_mask = code_mask(after_drop);
    if !code_offsets(after_drop, &after_drop_mask, "get_process(").is_empty() {
        return Err("sys_exec_aarch64 accesses the process manager after guard release".to_owned());
    }
    Ok(())
}

fn validate_boot_verdict_and_gate_scripts(
    executor: &str,
    full_test: &str,
    native_test: &str,
) -> Result<(), String> {
    let functions = module_function_bodies(executor);
    let emitters = functions
        .get("emit_exec_lock_order_counters")
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let counter_emitters: Vec<&&str> = emitters
        .iter()
        .filter(|body| body.contains("[EXEC_LOCK_ORDER:commits="))
        .collect();
    if counter_emitters.len() != 1 {
        return Err(format!(
            "expected one aarch64 exec counter emitter, found {}",
            counter_emitters.len()
        ));
    }
    let emitter = *counter_emitters[0];
    let emitter_mask = code_mask(emitter);
    for needle in [
        "EXEC_COMMIT_MISSING_THREAD",
        "SCHED_AFTER_PM_VIOLATIONS",
        "EXEC_COMMIT_UNPINNED",
        "EXEC_SCHED_COMMITS",
        "pm_held == 0 && unpinned == 0 && missing == 0",
    ] {
        if code_offsets(emitter, &emitter_mask, needle).is_empty() {
            return Err(format!("exec counter emitter is missing gate {needle}"));
        }
    }
    if !emitter.contains("[EXEC_LOCK_ORDER:commits={}:pm_held={}:unpinned={}:missing={}]") {
        return Err("exec counter emitter is missing the missing-thread field".to_owned());
    }

    for name in ["advance_stage_marker_only", "run_staged_tests"] {
        let body = function_body(executor, name);
        let body_mask = code_mask(body);
        if code_offsets(body, &body_mask, "emit_exec_lock_order_counters()").len() != 1 {
            return Err(format!(
                "{name} must call emit_exec_lock_order_counters exactly once"
            ));
        }
        if code_offsets(body, &body_mask, "failed == 0 && lock_order_clean").len() != 1 {
            return Err(format!("{name} does not gate PASS on clean exec counters"));
        }
        if !body.contains("[BOOT_TESTS:PASS]") || !body.contains("[BOOT_TESTS:FAIL:") {
            return Err(format!("{name} is missing a boot-test verdict"));
        }
    }

    for (path, script) in [
        ("docker/qemu/run-aarch64-full-test.sh", full_test),
        ("docker/qemu/run-aarch64-boot-test-native.sh", native_test),
    ] {
        if script.matches("\\[EXEC_LOCK_ORDER:VIOLATION").count() != 1 {
            return Err(format!(
                "{path} must grep once for exec lock-order violations"
            ));
        }
        if !script.contains("Exec lock-order violation") {
            return Err(format!("{path} is missing the exec violation diagnostic"));
        }
    }

    Ok(())
}

fn insert_in_arm64_exec(manager: &str, function_name: &str, insertion: &str) -> String {
    let functions = module_function_bodies(manager);
    let body = functions
        .get(function_name)
        .into_iter()
        .flatten()
        .find(|body| body.contains("[ARM64]"))
        .expect("ARM64 exec body for negative control");
    let body_start = body.as_ptr() as usize - manager.as_ptr() as usize;
    let insert_at = body_start + body.rfind('}').expect("ARM64 exec closing brace");
    let mut mutated = manager.to_owned();
    mutated.insert_str(insert_at, insertion);
    mutated
}

#[test]
fn arm64_exec_bodies_never_touch_the_scheduler_lock() {
    let manager = repo_text("kernel/src/process/manager.rs");
    validate_arm64_exec_bodies_never_touch_scheduler_lock(&manager).expect("T1 validation");
}

#[test]
fn manager_module_has_no_scheduler_lock_acquisition_anywhere() {
    let manager = repo_text("kernel/src/process/manager.rs");
    validate_manager_module_has_no_scheduler_lock_acquisition(&manager).expect("T2 validation");
}

#[test]
fn exec_sched_commit_is_a_must_use_receipt_applied_in_the_scheduler() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    validate_exec_sched_commit(&scheduler).expect("T3 validation");
}

#[test]
fn sys_exec_releases_the_process_manager_before_the_scheduler() {
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    validate_sys_exec_releases_process_manager(&syscall_entry).expect("T4 validation");
}

#[test]
fn boot_verdict_emits_and_gates_on_the_exec_lock_order_counters() {
    let executor = repo_text("kernel/src/test_framework/executor.rs");
    let full_test = repo_text("docker/qemu/run-aarch64-full-test.sh");
    let native_test = repo_text("docker/qemu/run-aarch64-boot-test-native.sh");
    validate_boot_verdict_and_gate_scripts(&executor, &full_test, &native_test)
        .expect("T5 validation");
}

#[test]
fn arm64_exec_bodies_stage_the_receipt_after_the_context_reset() {
    let manager = repo_text("kernel/src/process/manager.rs");
    validate_arm64_exec_staging_order(&manager).expect("T6 validation");
}

#[test]
fn negative_arm64_exec_scheduler_acquisition_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let mutated = insert_in_arm64_exec(
        &manager,
        "exec_process_with_argv",
        "\ncrate::task::scheduler::with_thread_mut(thread_id, |t| {\n    t.state = crate::task::thread::ThreadState::Ready;\n});\n",
    );
    assert!(validate_arm64_exec_bodies_never_touch_scheduler_lock(&mutated).is_err());
}

#[test]
fn negative_arm64_exec_snapshot_before_reset_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let snapshot = "        let ctx = thread.context.clone();\n";
    let (snapshot_start, aligned_stack_start) = {
        let body = arm64_exec_bodies(&manager)
            .into_iter()
            .find(|(name, _)| *name == "exec_process_with_argv")
            .map(|(_, body)| body)
            .expect("exec_process_with_argv ARM64 body");
        let body_start = body.as_ptr() as usize - manager.as_ptr() as usize;
        let snapshot_start = body_start
            + body
                .find(snapshot)
                .expect("exec_process_with_argv context snapshot");
        let aligned_stack_start = body_start
            + body
                .find("        let aligned_stack = ")
                .expect("exec_process_with_argv aligned stack reset");
        (snapshot_start, aligned_stack_start)
    };

    let mut mutated = manager.clone();
    mutated.replace_range(snapshot_start..snapshot_start + snapshot.len(), "");
    mutated.insert_str(aligned_stack_start, snapshot);
    assert_ne!(mutated, manager, "snapshot-before-reset mutation applied");
    assert!(validate_arm64_exec_staging_order(&mutated).is_err());
}

#[test]
fn negative_manager_module_scheduler_acquisition_is_rejected() {
    let manager = repo_text("kernel/src/process/manager.rs");
    let mut mutated = manager.clone();
    mutated.push_str(
        "\nfn synthetic_lock_inversion(thread_id: u64) {\n    crate::task::scheduler::with_thread_mut(thread_id, |t| {\n        t.state = crate::task::thread::ThreadState::Ready;\n    });\n}\n",
    );
    assert!(validate_manager_module_has_no_scheduler_lock_acquisition(&mutated).is_err());
}

#[test]
fn negative_exec_sched_commit_without_must_use_is_rejected() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let attribute_start = scheduler
        .find("#[must_use")
        .expect("ExecSchedCommit must-use attribute");
    let attribute_end = scheduler[attribute_start..]
        .find('\n')
        .map(|offset| attribute_start + offset + 1)
        .expect("must-use attribute newline");
    let mut mutated = scheduler.clone();
    mutated.replace_range(attribute_start..attribute_end, "");
    assert!(validate_exec_sched_commit(&mutated).is_err());
}

#[test]
fn negative_exec_sched_commit_without_pm_guard_is_rejected() {
    let scheduler = repo_text("kernel/src/task/scheduler.rs");
    let mutated = scheduler.replacen(
        "let pm_held = crate::process::process_manager_held_on_current_cpu();",
        "let pm_held = false;",
        1,
    );
    assert_ne!(mutated, scheduler, "PM guard mutation applied");
    assert!(validate_exec_sched_commit(&mutated).is_err());
}

#[test]
fn negative_sys_exec_apply_before_drop_is_rejected() {
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let mutated = syscall_entry.replacen(
        "        drop(manager_guard);\n\n        commit.apply();",
        "        commit.apply();\n\n        drop(manager_guard);",
        1,
    );
    assert_ne!(mutated, syscall_entry, "drop/apply swap mutation applied");
    assert!(validate_sys_exec_releases_process_manager(&mutated).is_err());
}

#[test]
fn negative_sys_exec_missing_drop_is_rejected() {
    let syscall_entry = repo_text("kernel/src/arch_impl/aarch64/syscall_entry.rs");
    let mutated = syscall_entry.replacen(
        "        drop(manager_guard);\n\n        commit.apply();",
        "        commit.apply();",
        1,
    );
    assert_ne!(mutated, syscall_entry, "drop deletion mutation applied");
    assert!(validate_sys_exec_releases_process_manager(&mutated).is_err());
}

#[test]
fn negative_full_gate_without_violation_grep_is_rejected() {
    let executor = repo_text("kernel/src/test_framework/executor.rs");
    let full_test = repo_text("docker/qemu/run-aarch64-full-test.sh");
    let native_test = repo_text("docker/qemu/run-aarch64-boot-test-native.sh");
    let mutated = full_test.replacen(
        "\\[EXEC_LOCK_ORDER:VIOLATION",
        "\\[EXEC_LOCK_ORDER:REMOVED",
        1,
    );
    assert_ne!(mutated, full_test, "violation grep mutation applied");
    assert!(validate_boot_verdict_and_gate_scripts(&executor, &mutated, &native_test).is_err());
}
