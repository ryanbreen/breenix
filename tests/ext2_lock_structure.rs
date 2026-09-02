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
//!
//! Added by the closure round to fix a blocking/major left open by the
//! round-2 review (`fix2-review.md`):
//!   6. `ext2_acquire` and `ext2_acquire_write` EACH individually call
//!      `prepare_to_wait_checked` — not merely somewhere in their combined
//!      bodies. Closes the M3 blind spot where deleting one function's
//!      entire park loop (keeping its can-sleep gate and spin fallback)
//!      still passed property 4 above because the *other* function's call
//!      satisfied it.
//!   7. No raw `_EXT2.read(` (as opposed to `.try_read()`) appears anywhere
//!      in the file, and exactly the two C5 mount-time `_EXT2.write(` sites
//!      exist — a file-wide census, not the three-function scope property 5
//!      uses, closing M3's second blind spot (a new raw `.read()`/`.write()`
//!      call site outside `is_mounted`/`is_home_mounted`/`home_mount_id`
//!      would not have been caught).
//!   8. `ext2_lock_can_sleep()`'s x86 arm checks `preempt_count() == 1`
//!      exactly, not `> 0` or `>= 1` — pinning the liveness dependency
//!      documented in `mod.rs` (review finding M2): a parked `BlockedOnIO`
//!      x86 thread can only be scheduled away via `can_schedule()`'s
//!      `preempt_count == 0` clause, which only `schedule_current_wait()`'s
//!      single `preempt_enable()` can reach from exactly 1.
//!
//! Added by the #748 second-best-fix round (x86 oracle pace is
//! pathological; this gives a park-path fact an early exit, not a
//! replacement for the full leg -- see `mod.rs`'s `ext2_record_park` doc
//! comment and `docker/qemu/run-ext2-lock-race-gate.sh`'s `--park-only`
//! flag):
//!   9. `ext2_acquire` and `ext2_acquire_write` EACH individually route
//!      their `Queued` arm through `ext2_record_park` rather than a bare
//!      `EXT2_LOCK_PARKS.fetch_add` -- mirroring property 6's per-function
//!      scope, so a future edit cannot silently drop the #748
//!      `EXT2_LOCK_PARK_FIRST` marker from one park loop while leaving it
//!      intact in the other.
//!
//! Added by the closure round (review finding F8: property 9 pins the
//! *routing* to `ext2_record_park` but was blind to deleting what that
//! routing actually depends on -- both gaps were confirmed open by a real-
//! source mutation that left property 9 green):
//!   10. `ext2_record_park` itself both calls `serial_println!` and prints
//!       the literal `EXT2_LOCK_PARK_FIRST` text -- catches deleting the
//!       marker print from inside the (still-called) helper, which
//!       property 9 cannot see since it only checks that callers reach
//!       `ext2_record_park`, not what `ext2_record_park` does once called.
//!   11. `run_one()` (`kernel/src/fs/ext2_lock_race.rs`) itself calls
//!       `ext2_reset_lock_park_first_marker()` -- catches the leg-scoping
//!       reset silently reverting to whole-boot-first semantics, which
//!       nothing in this file otherwise pins (the reset call lives outside
//!       `mod.rs` entirely).

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
    // A substring match on "wake_up" (not an identifier-bounded match) so
    // this covers both the broadcast wake_up() and the single-waiter
    // wake_up_one() the read side uses (review finding m1) -- either call
    // is a real wake, and both must come after the release.
    let wake = code_offsets(drop_body, &mask, "wake_up")
        .into_iter()
        .next()
        .ok_or_else(|| "missing a wake_up()/wake_up_one() call in Drop".to_string())?;
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

/// Closure-round property 6 (closes M3 blind spot 1): a single park-capable
/// function must itself call `prepare_to_wait_checked` — checked per
/// function, not on a combined body where a sibling function's call could
/// paper over this one's park loop having been deleted entirely.
fn validate_calls_checked_wait_itself(body: &str) -> Result<(), String> {
    let mask = code_mask(body);
    if identifier_offsets(body, &mask, "prepare_to_wait_checked").is_empty() {
        return Err("does not itself call prepare_to_wait_checked".to_string());
    }
    Ok(())
}

/// Closure-round property 7 (closes M3 blind spot 2): file-wide census, not
/// scoped to any particular function. No raw, non-yielding `_EXT2.read(`
/// may exist anywhere in the file (only `.try_read()`, which is a distinct
/// identifier — `_EXT2.read(` is not a substring of `_EXT2.try_read(`), and
/// exactly the two C5 mount-time `_EXT2.write(` initializers may exist —
/// any other raw write bypasses the park-capable write accessor.
fn validate_no_raw_ext2_rw_file_wide(source: &str) -> Result<(), String> {
    let mask = code_mask(source);
    if !code_offsets(source, &mask, "_EXT2.read(").is_empty() {
        return Err(
            "a raw spin::RwLock .read() on ROOT_EXT2/HOME_EXT2 exists somewhere in this file, \
             outside every park-capable accessor (#728 review M1/M3)"
                .to_string(),
        );
    }
    let write_sites = code_offsets(source, &mask, "_EXT2.write(").len();
    if write_sites != 2 {
        return Err(format!(
            "found {write_sites} raw _EXT2.write() call site(s), expected exactly 2 (the C5 \
             mount-time initializers) — any other count means a raw write was added or removed \
             outside the park-capable write accessor (#728 review M3)"
        ));
    }
    Ok(())
}

/// Closure-round property 8 (M2): the x86 arm's preempt_count check must be
/// the exact `== 1` the predicate's x86 liveness argument depends on (see
/// `ext2_lock_can_sleep`'s own doc comment in `mod.rs`) — `> 0`/`>= 1` would
/// still enter the park loop but could leave `preempt_count()` at 1 on
/// return from `schedule_current_wait()`'s single `preempt_enable()`, which
/// `can_schedule()`'s only reachable clause for a parked `BlockedOnIO`
/// thread requires to be exactly 0.
fn validate_x86_preempt_count_predicate_is_exact_one(fn_body: &str) -> Result<(), String> {
    let x86_block = block_after(fn_body, "#[cfg(not(target_arch = \"aarch64\"))]");
    let mask = code_mask(x86_block);
    if !code_offsets(x86_block, &mask, "preempt_count() == 1").is_empty() {
        return Ok(());
    }
    Err(
        "x86 arm's preempt_count() check is not the exact `== 1` this predicate's x86 liveness \
         argument depends on (review round-2 finding M2)"
            .to_string(),
    )
}

/// #748 property 9: a park-capable function must itself call
/// `ext2_record_park` (not a bare `EXT2_LOCK_PARKS.fetch_add`) in its
/// `Queued` arm, so the #748 `EXT2_LOCK_PARK_FIRST` marker fires the moment
/// either acquisition path actually parks. Checked per function (mirrors
/// property 6's per-function scope via `validate_calls_checked_wait_itself`)
/// so a future edit that restores a raw counter increment in one function
/// while leaving the other's `ext2_record_park` call intact cannot silently
/// drop the marker on a combined-body check.
fn validate_records_park_itself(body: &str) -> Result<(), String> {
    let mask = code_mask(body);
    if identifier_offsets(body, &mask, "ext2_record_park").is_empty() {
        return Err(
            "does not itself call ext2_record_park (the #748 first-park marker helper)"
                .to_string(),
        );
    }
    if !code_offsets(body, &mask, "EXT2_LOCK_PARKS.fetch_add").is_empty() {
        return Err(
            "calls EXT2_LOCK_PARKS.fetch_add directly instead of routing through \
             ext2_record_park -- this would silently drop the #748 EXT2_LOCK_PARK_FIRST marker"
                .to_string(),
        );
    }
    Ok(())
}

/// F8 property 10: `ext2_record_park` must both call `serial_println!` (as
/// real code, not merely mentioned in a comment -- hence `code_mask`) and
/// print the literal `EXT2_LOCK_PARK_FIRST` text. `.contains` (not
/// `code_offsets`) is deliberate for the second check: the marker text
/// lives inside a string literal, which `code_mask` masks OUT of "code" by
/// design (see the `string` arm above) -- `body` here is the raw,
/// unmasked source slice, so a direct substring search still finds it.
fn validate_record_park_prints_marker(body: &str) -> Result<(), String> {
    let mask = code_mask(body);
    if identifier_offsets(body, &mask, "serial_println").is_empty() {
        return Err(
            "does not itself call serial_println! -- the #748 EXT2_LOCK_PARK_FIRST marker \
             print would be silently gone even though ext2_record_park is still called"
                .to_string(),
        );
    }
    if !body.contains("EXT2_LOCK_PARK_FIRST") {
        return Err(
            "does not itself print the literal EXT2_LOCK_PARK_FIRST text -- a probe grepping \
             for it (docker/qemu/run-ext2-lock-race-gate.sh's --park-only) would find nothing \
             even though a park was recorded"
                .to_string(),
        );
    }
    Ok(())
}

/// F8 property 11: `run_one()` must itself call
/// `ext2_reset_lock_park_first_marker()` -- the #748 leg-scoping reset that
/// keeps the marker meaningful per race attempt instead of whole-boot-first.
fn validate_run_one_resets_park_marker(body: &str) -> Result<(), String> {
    let mask = code_mask(body);
    if identifier_offsets(body, &mask, "ext2_reset_lock_park_first_marker").is_empty() {
        return Err(
            "does not itself call ext2_reset_lock_park_first_marker() -- the #748 leg-scoping \
             reset would silently stop re-arming per race attempt, reverting to whole-boot- \
             first marker semantics"
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

#[test]
fn ext2_acquire_itself_calls_checked_wait() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    validate_calls_checked_wait_itself(function_body(&source, "ext2_acquire")).unwrap();
}

#[test]
fn ext2_acquire_write_itself_calls_checked_wait() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    validate_calls_checked_wait_itself(function_body(&source, "ext2_acquire_write")).unwrap();
}

#[test]
fn no_raw_ext2_read_write_exists_file_wide() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    validate_no_raw_ext2_rw_file_wide(&source).unwrap();
}

#[test]
fn x86_can_sleep_preempt_count_check_is_exact_one() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    let body = function_body(&source, "ext2_lock_can_sleep");
    validate_x86_preempt_count_predicate_is_exact_one(body).unwrap();
}

#[test]
fn ext2_acquire_itself_calls_record_park() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    validate_records_park_itself(function_body(&source, "ext2_acquire")).unwrap();
}

#[test]
fn ext2_acquire_write_itself_calls_record_park() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    validate_records_park_itself(function_body(&source, "ext2_acquire_write")).unwrap();
}

#[test]
fn ext2_record_park_itself_prints_the_marker() {
    let source = repo_text("kernel/src/fs/ext2/mod.rs");
    validate_record_park_prints_marker(function_body(&source, "ext2_record_park")).unwrap();
}

#[test]
fn run_one_itself_resets_the_park_marker() {
    let source = repo_text("kernel/src/fs/ext2_lock_race.rs");
    validate_run_one_resets_park_marker(function_body(&source, "run_one")).unwrap();
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

#[test]
fn negative_park_loop_deleted_from_one_function_is_rejected() {
    // The exact M3 blind spot: ext2_acquire's park loop deleted entirely
    // while keeping the can-sleep gate and the spin fallback. Property 1
    // (validate_park_capable_with_spin_fallback) does not catch this —
    // both identifiers it checks for are still present — but property 6
    // does, because ext2_acquire itself no longer calls
    // prepare_to_wait_checked.
    let regressed = r#"
fn ext2_acquire<T>() -> T {
    if ext2_lock_can_sleep() {
        // park loop deleted
    }
    spin_fallback()
}
"#;
    validate_park_capable_with_spin_fallback(regressed, "spin_fallback").unwrap();
    let err = validate_calls_checked_wait_itself(regressed).unwrap_err();
    assert!(
        err.contains("does not itself call"),
        "unexpected message: {err}"
    );
}

#[test]
fn negative_raw_read_added_outside_the_three_functions_is_rejected() {
    // The exact M3 blind spot: validate_routed_through_accessor only ever
    // sees the body of whichever function it's handed, so a brand-new raw
    // .read() call added to some OTHER function in the file (not
    // is_mounted/is_home_mounted/home_mount_id) is invisible to it. The
    // file-wide census (property 7) catches this because it scans the
    // whole file, not one function's body.
    let source = "fn some_new_helper() -> bool {\n    ROOT_EXT2.read().is_some()\n}\n";
    let err = validate_no_raw_ext2_rw_file_wide(source).unwrap_err();
    assert!(err.contains("raw spin::RwLock"), "unexpected message: {err}");
}

#[test]
fn negative_third_raw_write_site_is_rejected() {
    let source = "*ROOT_EXT2.write() = Some(fs);\n*HOME_EXT2.write() = Some(fs);\n\
                  *ROOT_EXT2.write() = None;\n";
    let err = validate_no_raw_ext2_rw_file_wide(source).unwrap_err();
    assert!(err.contains("found 3 raw"), "unexpected message: {err}");
}

#[test]
fn negative_missing_raw_write_site_is_rejected() {
    let source = "*ROOT_EXT2.write() = Some(fs);\n";
    let err = validate_no_raw_ext2_rw_file_wide(source).unwrap_err();
    assert!(err.contains("found 1 raw"), "unexpected message: {err}");
}

#[test]
fn negative_x86_preempt_count_weakened_to_greater_than_zero_is_rejected() {
    let mutated = r#"
fn ext2_lock_can_sleep() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        crate::per_cpu_aarch64::preempt_count() == 1
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        crate::per_cpu::preempt_count() > 0
    }
}
"#;
    let err = validate_x86_preempt_count_predicate_is_exact_one(mutated).unwrap_err();
    assert!(err.contains("exact"), "unexpected message: {err}");
}

#[test]
fn negative_missing_record_park_call_is_rejected() {
    // The #748 marker helper call deleted entirely -- e.g. a park loop
    // rewritten to drop straight into schedule_current_wait() without
    // recording anything.
    let regressed = "{\n    if let Some(v) = try_acquire() { return v; }\n    spin_fallback()\n}";
    let err = validate_records_park_itself(regressed).unwrap_err();
    assert!(
        err.contains("does not itself call ext2_record_park"),
        "unexpected message: {err}"
    );
}

#[test]
fn negative_raw_fetch_add_instead_of_record_park_is_rejected() {
    // The exact #748 regression this property exists to catch: a future
    // edit reverts to the pre-#748 bare atomic increment, silently losing
    // the EXT2_LOCK_PARK_FIRST marker while every other #728 property
    // (checked-wait usage, can-sleep gating, spin fallback) stays intact.
    let regressed = "{\n    EXT2_LOCK_PARKS.fetch_add(1, Ordering::Relaxed);\n    \
                      ext2_schedule_current_wait();\n}";
    let err = validate_records_park_itself(regressed).unwrap_err();
    assert!(
        err.contains("does not itself call ext2_record_park"),
        "unexpected message: {err}"
    );
}

#[test]
fn negative_raw_fetch_add_alongside_record_park_is_rejected() {
    // A stray direct increment reintroduced alongside the (still-present)
    // ext2_record_park call would double-count EXT2_LOCK_PARKS -- the
    // second check exists specifically to catch this even though the first
    // (missing-call) branch above cannot.
    let regressed = "{\n    ext2_record_park(lock_name);\n    \
                      EXT2_LOCK_PARKS.fetch_add(1, Ordering::Relaxed);\n}";
    let err = validate_records_park_itself(regressed).unwrap_err();
    assert!(
        err.contains("instead of routing through ext2_record_park"),
        "unexpected message: {err}"
    );
}

#[test]
fn negative_marker_print_deleted_from_record_park_is_rejected() {
    // F8 Mutation B: the review's real-source mutation that deleted
    // `crate::serial_println!("EXT2_LOCK_PARK_FIRST ...")` from
    // ext2_record_park while leaving the fetch_add and the
    // compare_exchange-gated `if` intact -- property 9 alone stayed green
    // for this because it only checks that callers reach ext2_record_park,
    // not what the helper does once reached.
    let regressed = "{\n    let parks = EXT2_LOCK_PARKS.fetch_add(1, Ordering::Relaxed) + 1;\n    \
                      if EXT2_LOCK_PARK_FIRST_LOGGED\n        .compare_exchange(false, true, \
                      Ordering::Relaxed, Ordering::Relaxed)\n        .is_ok()\n    {\n        \
                      let _ = parks;\n    }\n}";
    let err = validate_record_park_prints_marker(regressed).unwrap_err();
    assert!(err.contains("serial_println"), "unexpected message: {err}");
}

#[test]
fn negative_marker_text_changed_is_rejected() {
    // The print call survives but the literal marker text a gate script
    // greps for does not -- e.g. a well-meaning rename that forgot to
    // update the probe side.
    let regressed = "{\n    crate::serial_println!(\"lock parked lock={} parks={}\", \
                      lock_name, 1);\n}";
    let err = validate_record_park_prints_marker(regressed).unwrap_err();
    assert!(
        err.contains("EXT2_LOCK_PARK_FIRST"),
        "unexpected message: {err}"
    );
}

#[test]
fn negative_run_one_reset_call_deleted_is_rejected() {
    // F8 Mutation C: the review's real-source mutation that deleted
    // `ext2_reset_lock_park_first_marker()` from run_one() -- silently
    // reverting the #748 leg-scoping half of this round to whole-boot-first
    // marker semantics, with nothing else in the ratchet pinning it.
    let regressed = "{\n    let holder = spawn_holder(is_home);\n    \
                      let contender = spawn_contender(is_home);\n    \
                      join_and_classify(holder, contender)\n}";
    let err = validate_run_one_resets_park_marker(regressed).unwrap_err();
    assert!(
        err.contains("ext2_reset_lock_park_first_marker"),
        "unexpected message: {err}"
    );
}
