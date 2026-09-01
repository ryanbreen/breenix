//! #728 ext2 lock-discipline structural ratchet (review finding B5).
//!
//! Census-shaped, not line-pinned: every check below re-derives its target
//! text from the live source (function bodies, cfg arms, `impl Drop`
//! blocks) via a small comment/string-aware scanner, the same technique
//! `tests/exec_lock_order_structure.rs` uses, so a rename or reformat does
//! not silently defeat the ratchet the way a byte/line-offset pin would.
//! Every positive property below has a matching `negative_*` test proving
//! the validator actually reddens when the property it protects is
//! violated — mutation-proven, not merely "runs and returns Ok on main".
//!
//! Properties pinned here, each an explicit condition from the #728 review
//! round 2 (`B5`) or the precheck it closes:
//!   1. The two park-capable acquisition paths (`ext2_acquire`,
//!      `ext2_acquire_write`) are gated by `ext2_lock_can_sleep()` and keep
//!      a spin fallback for when it returns false (C1).
//!   2. `ext2_lock_can_sleep()`'s x86 arm never regains an
//!      `interrupts_enabled()` conjunct (the exact regression review B1
//!      found: that conjunct makes the park path unreachable from every
//!      x86 syscall), while the aarch64 arm keeps it (C3).
//!   3. `Ext2ReadGuard`/`Ext2WriteGuard::drop` release the inner guard
//!      before waking waiters — `EXT2_STATE` released before `WAITQUEUE`
//!      is touched (C9).
//!   4. The acquisition paths use only `prepare_to_wait_checked`, never the
//!      untimed `prepare_to_wait` (C6).
//!   5. `is_mounted`/`is_home_mounted`/`home_mount_id` are routed through
//!      the park-capable accessors, not a raw `ROOT_EXT2`/`HOME_EXT2`
//!      `.read()` (review finding M1).

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

// ---------------------------------------------------------------------------
// Comment/string-aware source scanning (trimmed from
// tests/exec_lock_order_structure.rs's proven helpers).
// ---------------------------------------------------------------------------

fn code_mask(source: &str) -> Vec<bool> {
    let bytes = source.as_bytes();
    let mut mask = vec![true; bytes.len()];
    let mut line_comment = false;
    let mut block_comment_depth = 0usize;
    let mut string = false;
    let mut character = false;
    let mut raw_string_hashes: Option<usize> = None;
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

/// Extract a top-level `fn NAME(...) { ... }` body (brace-depth aware, so
/// nested braces inside the body don't truncate it early).
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
    braced_block(source, &mask, start).unwrap_or_else(|| panic!("unterminated function {name}"))
}

/// The braced block immediately following the first occurrence of `marker`
/// (an `impl ... for ...` header or a `#[cfg(...)]` attribute) in `source`.
fn block_after<'a>(source: &'a str, marker: &str) -> &'a str {
    let mask = code_mask(source);
    let start = code_offsets(source, &mask, marker)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("missing {marker}"));
    braced_block(source, &mask, start).unwrap_or_else(|| panic!("unterminated block after {marker}"))
}

// ---------------------------------------------------------------------------
// Validators — each takes an isolated source fragment and a Result, so the
// negative tests below can exercise them directly against hand-mutated
// fragments without having to splice a mutation back into the whole file.
// ---------------------------------------------------------------------------

/// C1: a park-capable acquisition body must consult `ext2_lock_can_sleep()`
/// and retain a call to its spin fallback for when that gate is false.
fn validate_park_capable_with_spin_fallback(body: &str, spin_marker: &str) -> Result<(), String> {
    let mask = code_mask(body);
    if identifier_offsets(body, &mask, "ext2_lock_can_sleep").is_empty() {
        return Err("missing the ext2_lock_can_sleep() gate".to_string());
    }
    if code_offsets(body, &mask, spin_marker).is_empty() {
        return Err(format!("missing the spin fallback ({spin_marker})"));
    }
    Ok(())
}

/// B1/C3: `ext2_lock_can_sleep()`'s aarch64 arm must keep its
/// `interrupts_enabled()` gate (load-bearing for C3's IRQ-masked site); its
/// x86 arm must never regain one (that conjunct is unconditionally false in
/// every x86 syscall, which is exactly the regression the review's B1
/// finding identified as making the whole fix a no-op on x86).
fn validate_can_sleep_arch_split(fn_body: &str) -> Result<(), String> {
    let aarch64_block = block_after(fn_body, "#[cfg(target_arch = \"aarch64\")]");
    let x86_block = block_after(fn_body, "#[cfg(not(target_arch = \"aarch64\"))]");

    let aarch64_mask = code_mask(aarch64_block);
    if identifier_offsets(aarch64_block, &aarch64_mask, "interrupts_enabled").is_empty() {
        return Err(
            "aarch64 arm lost its interrupts_enabled() gate — the C3 IRQ-masked no-park site \
             (load_test_binaries_from_ext2) is no longer provably refused"
                .to_string(),
        );
    }

    let x86_mask = code_mask(x86_block);
    if !identifier_offsets(x86_block, &x86_mask, "interrupts_enabled").is_empty() {
        return Err(
            "non-aarch64 arm regained an interrupts_enabled() conjunct — RFLAGS.IF is \
             unconditionally 0 for the duration of every x86 syscall (syscall/entry.asm's cli, \
             no sti before rust_syscall_handler), so this makes the park path unreachable from \
             every x86 syscall site again (#728 review round-2 B1)"
                .to_string(),
        );
    }

    Ok(())
}

/// C9: the guard's `Drop` must release the inner lock guard before it wakes
/// any parked waiter — `EXT2_STATE` must not be held across the
/// `WAITQUEUE` touch.
fn validate_release_before_wake(drop_body: &str) -> Result<(), String> {
    let mask = code_mask(drop_body);
    let release = code_offsets(drop_body, &mask, "self.inner = None")
        .into_iter()
        .next()
        .ok_or_else(|| "missing `self.inner = None` release in Drop".to_string())?;
    let wake = identifier_offsets(drop_body, &mask, "wake_up")
        .into_iter()
        .next()
        .ok_or_else(|| "missing a wake_up() call in Drop".to_string())?;
    if release < wake {
        Ok(())
    } else {
        Err(
            "wake_up() is reachable before `self.inner = None` releases the guard — a parked \
             waiter could observe EXT2_STATE held across the wake (C9's \
             EXT2_STATE -> WAITQUEUE -> SCHEDULER order)"
                .to_string(),
        )
    }
}

/// C6: only the timed, condition-checked `prepare_to_wait_checked` may be
/// used; the untimed `prepare_to_wait` is a lost-wake permanent hang on a
/// brand-new lock in a kernel with this project's own lost-wake history
/// (#584/#586/#589).
fn validate_checked_wait_used_exclusively(body: &str) -> Result<(), String> {
    let mask = code_mask(body);
    if identifier_offsets(body, &mask, "prepare_to_wait_checked").is_empty() {
        return Err("prepare_to_wait_checked() is never called".to_string());
    }
    // `identifier_offsets` requires both boundaries to be non-identifier
    // bytes, so a needle of "prepare_to_wait" does NOT match inside
    // "prepare_to_wait_checked" (the character following the needle there
    // is '_', an identifier byte) -- only a genuinely bare call matches.
    let bare = identifier_offsets(body, &mask, "prepare_to_wait");
    if bare.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} bare prepare_to_wait() call(s) alongside prepare_to_wait_checked() — only the \
             checked, timed variant may be used (C6, no untimed park)",
            bare.len()
        ))
    }
}

/// M1: `is_mounted`/`is_home_mounted`/`home_mount_id` must be routed
/// through the park-capable accessor, not a raw `spin::RwLock::read()` on
/// `ROOT_EXT2`/`HOME_EXT2` — a raw `.read()` spins non-yieldingly whenever
/// a writer holds the upgradeable slot (`spin`'s `try_read()` rejects new
/// readers while UPGRADED is set), invisibly to the gate because it
/// bypasses `ext2_spin_wait` entirely.
fn validate_routed_through_accessor(body: &str, accessor_call: &str) -> Result<(), String> {
    let mask = code_mask(body);
    if code_offsets(body, &mask, accessor_call).is_empty() {
        return Err(format!("does not call {accessor_call}"));
    }
    if !code_offsets(body, &mask, "_EXT2.read(").is_empty() {
        return Err(
            "still calls a raw spin::RwLock .read() directly on ROOT_EXT2/HOME_EXT2 instead of \
             routing through the park-capable accessor (#728 review M1)"
                .to_string(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Positive tests — the real source satisfies every property.
// ---------------------------------------------------------------------------

#[test]
fn ext2_acquire_is_can_sleep_gated_with_a_spin_fallback() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    let body = function_body(&source, "ext2_acquire");
    validate_park_capable_with_spin_fallback(body, "spin_fallback").unwrap();
}

#[test]
fn ext2_acquire_write_is_can_sleep_gated_with_a_spin_fallback() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    let body = function_body(&source, "ext2_acquire_write");
    validate_park_capable_with_spin_fallback(body, "ext2_spin_wait_upgrade").unwrap();
}

#[test]
fn can_sleep_predicate_arch_split_is_correct() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    let body = function_body(&source, "ext2_lock_can_sleep");
    validate_can_sleep_arch_split(body).unwrap();
}

#[test]
fn read_guard_drop_releases_before_waking() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    let impl_block = block_after(&source, "impl Drop for Ext2ReadGuard");
    let drop_body = function_body(impl_block, "drop");
    validate_release_before_wake(drop_body).unwrap();
}

#[test]
fn write_guard_drop_releases_before_waking() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    let impl_block = block_after(&source, "impl Drop for Ext2WriteGuard");
    let drop_body = function_body(impl_block, "drop");
    validate_release_before_wake(drop_body).unwrap();
}

#[test]
fn acquisition_paths_use_only_the_checked_timed_wait() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    let combined = format!(
        "{}\n{}",
        function_body(&source, "ext2_acquire"),
        function_body(&source, "ext2_acquire_write")
    );
    validate_checked_wait_used_exclusively(&combined).unwrap();
}

#[test]
fn is_mounted_is_home_mounted_and_home_mount_id_route_through_the_accessor() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    validate_routed_through_accessor(function_body(&source, "is_mounted"), "root_fs_read()")
        .unwrap();
    validate_routed_through_accessor(
        function_body(&source, "is_home_mounted"),
        "home_fs_read()",
    )
    .unwrap();
    validate_routed_through_accessor(function_body(&source, "home_mount_id"), "home_fs_read()")
        .unwrap();
}

// ---------------------------------------------------------------------------
// Negative tests — mutation-prove each validator actually reddens.
// ---------------------------------------------------------------------------

#[test]
fn negative_missing_can_sleep_gate_is_rejected() {
    let regressed = "{ if condition_a() { return spin_fallback(); } spin_fallback() }";
    let err = validate_park_capable_with_spin_fallback(regressed, "spin_fallback").unwrap_err();
    assert!(err.contains("ext2_lock_can_sleep"), "unexpected message: {err}");
}

#[test]
fn negative_missing_spin_fallback_is_rejected() {
    let regressed = "{ if ext2_lock_can_sleep() { park(); } unreachable!() }";
    let err =
        validate_park_capable_with_spin_fallback(regressed, "ext2_spin_wait_upgrade").unwrap_err();
    assert!(err.contains("spin fallback"), "unexpected message: {err}");
}

#[test]
fn negative_x86_regains_interrupts_enabled_is_rejected() {
    // The exact regression review B1 found: an interrupts_enabled() check
    // reintroduced into the x86 arm makes it unconditionally false in every
    // x86 syscall (IF=0 throughout), silently turning the fix back into a
    // no-op on the arch #728 was reported on.
    let mutated = r#"
fn ext2_lock_can_sleep() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        if !crate::arch_impl::aarch64::cpu::interrupts_enabled() { return false; }
        true
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        if !crate::arch_impl::x86_64::cpu::X86Cpu::interrupts_enabled() { return false; }
        crate::per_cpu::preempt_count() == 1
    }
}
"#;
    let err = validate_can_sleep_arch_split(mutated).unwrap_err();
    assert!(err.contains("regained an interrupts_enabled"), "unexpected message: {err}");
}

#[test]
fn negative_aarch64_loses_interrupts_enabled_is_rejected() {
    let mutated = r#"
fn ext2_lock_can_sleep() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        crate::per_cpu_aarch64::preempt_count() == 1
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        crate::per_cpu::preempt_count() == 1
    }
}
"#;
    let err = validate_can_sleep_arch_split(mutated).unwrap_err();
    assert!(err.contains("lost its interrupts_enabled"), "unexpected message: {err}");
}

#[test]
fn negative_wake_before_release_is_rejected() {
    let mutated = "fn drop(&mut self) {\n    self.waiters.wake_up();\n    self.inner = None;\n}";
    let err = validate_release_before_wake(mutated).unwrap_err();
    assert!(err.contains("wake_up"), "unexpected message: {err}");
}

#[test]
fn negative_missing_release_in_drop_is_rejected() {
    let mutated = "fn drop(&mut self) {\n    self.waiters.wake_up();\n}";
    let err = validate_release_before_wake(mutated).unwrap_err();
    assert!(err.contains("self.inner = None"), "unexpected message: {err}");
}

#[test]
fn negative_bare_prepare_to_wait_is_rejected() {
    let mutated = "{ waiters.prepare_to_wait_checked(state, None, cond); \
                     waiters.prepare_to_wait(state); }";
    let err = validate_checked_wait_used_exclusively(mutated).unwrap_err();
    assert!(err.contains("bare prepare_to_wait"), "unexpected message: {err}");
}

#[test]
fn negative_never_using_checked_wait_is_rejected() {
    let mutated = "{ waiters.prepare_to_wait(state); }";
    let err = validate_checked_wait_used_exclusively(mutated).unwrap_err();
    assert!(err.contains("never called"), "unexpected message: {err}");
}

#[test]
fn negative_raw_read_regression_is_rejected() {
    // A regression that reintroduces a raw .read() alongside the accessor
    // call must still be rejected -- the accessor call alone is not
    // sufficient proof the raw, blocking-spin path is gone.
    let regressed = "{\n    let _ = root_fs_read();\n    ROOT_EXT2.read().is_some()\n}";
    let err = validate_routed_through_accessor(regressed, "root_fs_read()").unwrap_err();
    assert!(err.contains("raw spin::RwLock"), "unexpected message: {err}");

    let regressed_home =
        "{\n    let _ = home_fs_read();\n    HOME_EXT2.read().as_ref().map(|fs| fs.mount_id)\n}";
    let err_home = validate_routed_through_accessor(regressed_home, "home_fs_read()").unwrap_err();
    assert!(err_home.contains("raw spin::RwLock"), "unexpected message: {err_home}");
}

#[test]
fn negative_missing_accessor_call_is_rejected() {
    let regressed = "{\n    true\n}";
    let err = validate_routed_through_accessor(regressed, "root_fs_read()").unwrap_err();
    assert!(err.contains("does not call"), "unexpected message: {err}");
}
