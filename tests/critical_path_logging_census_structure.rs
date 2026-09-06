//! Structural ratchet for `scripts/check-critical-path-violations.sh`.
//!
//! The shell script greps a fixed file list for a fixed spelling list and
//! exits 1 today (135 distinct call sites across 9 files, `docs/planning/
//! green-program/gates/CRITICAL-PATH-DEBT-2026-09-06.md` §1-4). It is not
//! wired into any gate, so today the only thing that notices a NEW call
//! creeping in is a human rereading 274 lines of grep output. This suite
//! pins the census in Rust so a per-`(file, item-path)` INCREASE fails a
//! `cargo test` run, the same way `tests/serial_line_atomicity_structure.rs`
//! pins the raw-serial-primitive census and `tests/
//! capture_path_lock_free_structure.rs` pins the capture path's denylist.
//! The pin is a full `(file, item-path) -> count` exact match, not a
//! one-directional ratchet: a DECREASE at an existing anchor fails the same
//! run, with its own `~ <file> :: <item>  (expected N, found N-1)` diff
//! line, so a drain PR has to update the table consciously rather than
//! coast on a stale higher count. See "Direction" in `docs/planning/
//! green-program/gates/CRITICAL-PATH-DEBT-2026-09-06.md` for the fuller
//! statement of this.
//!
//! # Two censuses, on purpose
//!
//! `CRITICAL_PATH_LOG_ANCHORS` pins the shell script's ORIGINAL twelve
//! spellings at 135 -- the number the drain plan's PR ledger tracks PR by
//! PR. A second, WIDER set adds three spellings the original denylist
//! misses by construction (`serial_print!`, `log_serial_print!`,
//! `log::log!` -- each reaches the same blocking serial lock as the
//! `serial_println!`/`log::*!` families the narrow list already denies).
//! That wider census is 136 today: the 135 plus exactly one escaped site,
//! `kernel/src/arch_impl/aarch64/exception.rs :: fn sys_write`, a
//! `crate::serial_print!` call inside a per-BYTE loop. This same PR widens
//! `PROHIBITED_PATTERNS` in the shell script to carry the three new
//! spellings too, so `scripts/check-critical-path-violations.sh`'s own
//! output and this suite's wider set report the same shapes going forward;
//! `the_shell_and_this_suite_check_the_same_shapes` below is what keeps
//! them from drifting apart.
//!
//! # Census-anchored on item paths, not line numbers
//!
//! A line-number pin breaks on reflow above it -- an edit to an
//! unrelated line still shifts the pinned number. An
//! item-path pin (`kernel/src/task/scheduler.rs :: impl Scheduler::fn
//! schedule`) survives reflow and only breaks when the call itself is
//! added, removed, or moved to a different function -- the standing lesson
//! recorded against #549/#551/#527-r1 in this repository's project memory.
//!
//! # What zero-pinning the three clean files buys
//!
//! `arch_impl/aarch64/context.rs`, `interrupts/timer.rs` and
//! `arch_impl/aarch64/percpu.rs` are three of the fourteen checked files
//! that carry NO denylisted call today. 0 of the 135 rows in the anchor
//! table name them, so silence from the general census already catches a
//! first print there as an unexpected `+` row -- but a `+` row buried in a
//! sea of a 9-file diff is easy to misread as "one more example from an
//! already-dirty file". `zero_pin_files_stay_clean` gives that specific
//! claim -- these three files are clean, full stop -- its own assertion and
//! its own failure message.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

// ---------------------------------------------------------------------
// Comment- and string-aware source masking, plus the item-path machinery.
// Ported verbatim from `tests/serial_line_atomicity_structure.rs` (that
// file has no `pub` surface to import from -- structure suites are each
// compiled standalone by `scripts/run-structure-tests.sh`, see that
// script's own header for why).
// ---------------------------------------------------------------------

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

fn code_offsets(source: &str, mask: &[bool], needle: &str) -> Vec<usize> {
    source
        .match_indices(needle)
        .filter_map(|(offset, _)| mask.get(offset).copied().unwrap_or(false).then_some(offset))
        .collect()
}

fn next_code(source: &str, mask: &[bool], from: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    (from..bytes.len()).find(|index| mask[*index] && !bytes[*index].is_ascii_whitespace())
}

fn header_cfg(header: &str, mask: &[bool], keyword: usize) -> String {
    let bytes = header.as_bytes();
    let mut attributes = Vec::new();
    for offset in code_offsets(header, mask, "#[cfg") {
        if offset >= keyword {
            break;
        }
        let Some(paren) = next_code(header, mask, offset + "#[cfg".len()) else {
            continue;
        };
        if bytes[paren] != b'(' {
            continue;
        }
        let mut depth = 0usize;
        for close in offset..bytes.len() {
            match bytes[close] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        let compact: String = header[offset..close + 1]
                            .chars()
                            .filter(|character| !character.is_whitespace() && *character != '"')
                            .collect();
                        attributes.push(compact);
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    if attributes.is_empty() {
        String::new()
    } else {
        format!("{} ", attributes.join(" "))
    }
}

fn impl_segment(header: &str, mask: &[bool], keyword: usize) -> String {
    let kept: Vec<u8> = header.as_bytes()[keyword..]
        .iter()
        .zip(&mask[keyword..])
        .filter_map(|(byte, code)| code.then_some(*byte))
        .collect();
    let text = String::from_utf8_lossy(&kept)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match text.find(" where ") {
        Some(clause) => text[..clause].to_owned(),
        None => text,
    }
}

fn item_segment(header: &str, mask: &[bool]) -> Option<String> {
    let bytes = header.as_bytes();
    let named = |keyword: usize, length: usize| -> Option<String> {
        let mut cursor = keyword + length;
        while cursor < bytes.len() && (!mask[cursor] || bytes[cursor].is_ascii_whitespace()) {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && mask[cursor] && identifier_byte(bytes[cursor]) {
            cursor += 1;
        }
        (cursor > start).then(|| header[start..cursor].to_owned())
    };

    let declaration = identifier_offsets(header, mask, "fn")
        .into_iter()
        .filter_map(|offset| named(offset, "fn".len()).map(|name| (offset, format!("fn {name}"))))
        .next_back()
        .or_else(|| {
            ["impl", "mod", "trait", "struct"]
                .into_iter()
                .flat_map(|keyword| {
                    identifier_offsets(header, mask, keyword)
                        .into_iter()
                        .map(move |offset| (keyword, offset))
                })
                .max_by_key(|(_, offset)| *offset)
                .and_then(|(keyword, offset)| match keyword {
                    "impl" => Some((offset, impl_segment(header, mask, offset))),
                    _ => named(offset, keyword.len())
                        .map(|name| (offset, format!("{keyword} {name}"))),
                })
        });
    let (keyword, segment) = declaration?;
    Some(format!("{}{segment}", header_cfg(header, mask, keyword)))
}

fn item_spans(source: &str, mask: &[bool]) -> Vec<(usize, usize, String)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut header = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for index in 0..bytes.len() {
        if !mask[index] {
            continue;
        }
        match bytes[index] {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b'{' => {
                stack.push((index, header));
                header = index + 1;
            }
            b'}' => {
                if let Some((open, start)) = stack.pop() {
                    if let Some(segment) = item_segment(&source[start..open], &mask[start..open]) {
                        spans.push((open, index, segment));
                    }
                }
                header = index + 1;
            }
            b';' if paren_depth == 0 && bracket_depth == 0 => header = index + 1,
            _ => {}
        }
    }
    spans
}

fn rendered_item_spans(spans: &[(usize, usize, String)]) -> Vec<(usize, usize, String)> {
    let mut ordered = spans.to_vec();
    ordered.sort_by_key(|(open, _, _)| *open);

    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut rendered = Vec::with_capacity(ordered.len());
    for (open, close, segment) in ordered {
        while stack
            .last()
            .is_some_and(|(ancestor_close, _)| *ancestor_close < open)
        {
            stack.pop();
        }
        let path = match stack.last() {
            Some((_, parent)) => format!("{parent}::{segment}"),
            None => segment,
        };
        stack.push((close, path.clone()));
        rendered.push((open, close, path));
    }

    let mut path_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, _, path) in &rendered {
        *path_counts.entry(path.clone()).or_default() += 1;
    }
    for (_, _, path) in &mut rendered {
        if path_counts.get(path).is_some_and(|count| *count > 1) {
            path.push_str(" [duplicate item path]");
        }
    }
    rendered
}

fn item_path_at(spans: &[(usize, usize, String)], offset: usize) -> String {
    spans
        .iter()
        .filter(|(open, close, _)| *open <= offset && offset <= *close)
        .max_by_key(|(open, _, _)| *open)
        .map(|(_, _, path)| path.clone())
        .unwrap_or_default()
}

type Anchor = (String, String);
type Census = BTreeMap<Anchor, usize>;

fn expected_census(anchors: &[(&str, &str, usize)]) -> Census {
    let mut census = Census::new();
    for (path, item, count) in anchors {
        let anchor = ((*path).to_owned(), (*item).to_owned());
        assert!(
            census.insert(anchor, *count).is_none(),
            "duplicate census anchor {path} :: {item}"
        );
    }
    census
}

fn census_diff(actual: &Census, anchors: &[(&str, &str, usize)]) -> Vec<String> {
    let expected = expected_census(anchors);
    let mut diff = Vec::new();
    for (anchor, count) in actual {
        match expected.get(anchor) {
            None => diff.push(format!(
                "+ {} :: {}  ({count} occurrences, expected none)",
                anchor.0, anchor.1
            )),
            Some(want) if want != count => diff.push(format!(
                "~ {} :: {}  (expected {want}, found {count})",
                anchor.0, anchor.1
            )),
            Some(_) => {}
        }
    }
    for (anchor, count) in &expected {
        if !actual.contains_key(anchor) {
            diff.push(format!(
                "- {} :: {}  (expected {count}, found none)",
                anchor.0, anchor.1
            ));
        }
    }
    diff
}

fn validate_census(actual: &Census, anchors: &[(&str, &str, usize)]) -> Result<(), Vec<String>> {
    let diff = census_diff(actual, anchors);
    diff.is_empty().then_some(()).ok_or(diff)
}

/// Insert a `(path, source)` pair into a copy of `sources`, keeping the
/// input untouched. Mirrors `with_synthetic_source` in
/// `tests/serial_line_atomicity_structure.rs`: it is fine for `path` to
/// collide with a real entry already in `sources` -- `log_census` below
/// processes each `(path, source)` pair independently, so a synthetic
/// entry at a real path is additional violating text at that path, not a
/// replacement of it.
fn with_synthetic_source(
    sources: &[(String, String)],
    path: &str,
    synthetic_source: &str,
) -> Vec<(String, String)> {
    let mut perturbed = sources.to_vec();
    perturbed.push((path.to_owned(), synthetic_source.to_owned()));
    perturbed
}

// ---------------------------------------------------------------------
// The critical-path file list and denylist, mirrored from
// `scripts/check-critical-path-violations.sh`. `the_shell_and_this_suite_
// check_the_same_shapes` below reads the live script and asserts these
// two lists equal it byte-for-byte (modulo whitespace around each quoted
// entry), so a divergence between the shell array and this const fails a
// `cargo test` run instead of waiting for someone to notice by eye.
// ---------------------------------------------------------------------

/// `CRITICAL_FILES` as spelled in the shell script, in the same order.
/// `"capture/"` is the one directory entry -- `capture_rs_files` below
/// expands it the same way `check_all_critical_files` does: enumerate the
/// `.rs` files under `kernel/src/capture/` from disk, not from a list.
const CRITICAL_FILES: &[&str] = &[
    "arch_impl/aarch64/context_switch.rs",
    "arch_impl/aarch64/context.rs",
    "interrupts/context_switch.rs",
    "arch_impl/aarch64/timer_interrupt.rs",
    "arch_impl/aarch64/exception.rs",
    "interrupts/timer.rs",
    "interrupts/timer_entry.asm",
    "syscall/handler.rs",
    "syscall/entry.asm",
    "syscall/time.rs",
    "per_cpu.rs",
    "per_cpu_aarch64.rs",
    "arch_impl/aarch64/percpu.rs",
    "task/scheduler.rs",
    "capture/",
];

/// The original twelve spellings `PROHIBITED_PATTERNS` carried before this
/// PR. This is the set `CRITICAL_PATH_LOG_ANCHORS` (135) is computed
/// against -- the number the drain plan's PR-by-PR ledger tracks.
const SHARED_PATTERNS: &[&str] = &[
    "serial_println!",
    "log::debug!",
    "log::info!",
    "log::warn!",
    "log::error!",
    "log::trace!",
    "println!",
    "eprintln!",
    "format!",
    "write!",
    "writeln!",
    "crate::serial_println!",
];

/// The three spellings this PR adds to the shell's `PROHIBITED_PATTERNS`,
/// appended in this order after `crate::serial_println!`. Each reaches the
/// same blocking serial lock as a spelling the narrow list already denies:
/// `serial_print!` and `log_serial_print!` are the non-`ln` siblings of
/// `serial_println!`/`log_serial_println!`, and `log::log!` is the fourth
/// way to reach `CombinedLogger::log` alongside `log::{debug,info,warn,
/// error}!` (already denied) and `log::trace!` (already denied, though it
/// emits 0 bytes today -- see the round doc's H3 discussion).
const WIDER_EXTRA_PATTERNS: &[&str] = &["serial_print!", "log_serial_print!", "log::log!"];

/// The three checked files that carry no denylisted call today.
const ZERO_PIN_FILES: &[&str] = &[
    "kernel/src/arch_impl/aarch64/context.rs",
    "kernel/src/interrupts/timer.rs",
    "kernel/src/arch_impl/aarch64/percpu.rs",
];

/// The `.rs` files under `kernel/src/capture/`, as `kernel/src/...`
/// relative paths, in the same order `find ... -name '*.rs' | sort` would
/// produce (the shell script's own expansion for a directory entry).
fn capture_rs_files() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("kernel/src/capture must exist") {
            let path = entry.expect("readable capture dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut paths = Vec::new();
    walk(&root.join("kernel/src/capture"), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            path.strip_prefix(&root)
                .expect("capture file below repository root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

/// The exact set of `(path, source)` pairs `scripts/check-critical-path-
/// violations.sh` reads today: `CRITICAL_FILES` with `"capture/"` expanded
/// from disk, each as a `kernel/src/...`-relative path.
fn checked_sources() -> Vec<(String, String)> {
    let mut sources = Vec::new();
    for entry in CRITICAL_FILES {
        if *entry == "capture/" {
            for path in capture_rs_files() {
                let source = repo_text(&path);
                sources.push((path, source));
            }
            continue;
        }
        let path = format!("kernel/src/{entry}");
        let source = repo_text(&path);
        sources.push((path, source));
    }
    sources
}

fn line_number(source: &str, offset: usize) -> usize {
    source.as_bytes()[..offset].iter().filter(|byte| **byte == b'\n').count() + 1
}

/// One offset per violating LINE, deduplicated across overlapping pattern
/// spellings. Three of the twelve shared patterns are substrings of one
/// another (`println!` inside `serial_println!` inside
/// `crate::serial_println!`), which is why the shell script prints 274
/// lines for 135 call sites (`check-critical-path-violations.sh`'s report-
/// inflation bug, documented in the round doc §1). A call site is what the
/// census pins, not a (pattern, line) pair.
fn matched_line_offsets(source: &str, mask: &[bool], patterns: &[&str]) -> Vec<usize> {
    let mut earliest: BTreeMap<usize, usize> = BTreeMap::new();
    for pattern in patterns {
        for offset in code_offsets(source, mask, pattern) {
            let line = line_number(source, offset);
            earliest
                .entry(line)
                .and_modify(|existing| {
                    if offset < *existing {
                        *existing = offset;
                    }
                })
                .or_insert(offset);
        }
    }
    earliest.into_values().collect()
}

fn log_census(sources: &[(String, String)], patterns: &[&str]) -> Census {
    let mut census = Census::new();
    for (path, source) in sources {
        let mask = code_mask(source);
        let offsets = matched_line_offsets(source, &mask, patterns);
        if offsets.is_empty() {
            continue;
        }
        let spans = rendered_item_spans(&item_spans(source, &mask));
        for offset in offsets {
            *census
                .entry((path.clone(), item_path_at(&spans, offset)))
                .or_default() += 1;
        }
    }
    census
}

/// One row per `(file, item-path)` the narrow (pre-widen) denylist flags
/// today, summing to 135. Regenerated from the tree at `783a6a53`
/// (`docs/planning/green-program/gates/CRITICAL-PATH-DEBT-2026-09-06.md`'s
/// snapshot) by the same `code_mask`/`item_spans`/`item_path_at` pipeline
/// this file runs, so the anchors are provably what that pipeline computes
/// against that tree -- not hand-transcribed from the round doc's table.
const CRITICAL_PATH_LOG_ANCHORS: &[(&str, &str, usize)] = &[
    ("kernel/src/arch_impl/aarch64/context_switch.rs", "#[cfg(feature=boot_tests)] fn report_user_rsp_scratch_el_census", 1),
    ("kernel/src/arch_impl/aarch64/context_switch.rs", "fn drain_asm_resume_pc_refusals", 1),
    ("kernel/src/arch_impl/aarch64/context_switch.rs", "fn emit_resume_pc_census_locked", 2),
    ("kernel/src/arch_impl/aarch64/context_switch.rs", "fn record_resume_pc_refusal_locked", 1),
    ("kernel/src/arch_impl/aarch64/context_switch.rs", "fn report_foreign_resume_pc_refusal", 1),
    ("kernel/src/arch_impl/aarch64/exception.rs", "fn handle_sync_exception", 1),
    ("kernel/src/arch_impl/aarch64/exception.rs", "fn handle_syscall", 8),
    ("kernel/src/arch_impl/aarch64/timer_interrupt.rs", "fn dump_gic_state", 9),
    ("kernel/src/arch_impl/aarch64/timer_interrupt.rs", "fn init", 9),
    ("kernel/src/interrupts/context_switch.rs", "fn check_need_resched_and_switch", 2),
    ("kernel/src/interrupts/context_switch.rs", "fn restore_userspace_thread_context", 8),
    ("kernel/src/interrupts/context_switch.rs", "fn save_current_thread_context_with_guard", 4),
    ("kernel/src/interrupts/context_switch.rs", "fn save_kthread_context", 1),
    ("kernel/src/interrupts/context_switch.rs", "fn setup_first_userspace_entry", 3),
    ("kernel/src/interrupts/context_switch.rs", "fn setup_idle_return", 2),
    ("kernel/src/interrupts/context_switch.rs", "fn setup_kernel_thread_return", 1),
    ("kernel/src/interrupts/context_switch.rs", "fn switch_to_thread", 9),
    ("kernel/src/per_cpu.rs", "fn can_schedule", 1),
    ("kernel/src/per_cpu.rs", "fn init", 9),
    ("kernel/src/per_cpu.rs", "fn set_kernel_cr3", 2),
    ("kernel/src/per_cpu_aarch64.rs", "fn init", 5),
    ("kernel/src/per_cpu_aarch64.rs", "fn set_kernel_cr3", 1),
    ("kernel/src/syscall/handler.rs", "fn rust_syscall_handler", 2),
    ("kernel/src/syscall/time.rs", "fn sys_clock_settime", 1),
    ("kernel/src/task/scheduler.rs", "#[cfg(all(target_arch=aarch64,feature=boot_tests))] fn emit_pin_guard_oracle", 3),
    ("kernel/src/task/scheduler.rs", "#[cfg(all(target_arch=x86_64,feature=boot_tests))] fn emit_pin_guard_oracle", 1),
    ("kernel/src/task/scheduler.rs", "#[cfg(all(test,target_arch=x86_64))] mod tests::fn test_schedule_does_not_duplicate_ready_queue", 2),
    ("kernel/src/task/scheduler.rs", "#[cfg(all(test,target_arch=x86_64))] mod tests::fn test_unblock_does_not_duplicate_ready_queue", 2),
    ("kernel/src/task/scheduler.rs", "#[cfg(all(test,target_arch=x86_64))] mod tests::fn test_yield_current_does_not_modify_scheduler_state", 4),
    ("kernel/src/task/scheduler.rs", "#[cfg(feature=boot_tests)] fn probe_publication_lock_order_injection", 1),
    ("kernel/src/task/scheduler.rs", "#[cfg(target_arch=x86_64)] fn abort_dispatch_and_resume", 1),
    ("kernel/src/task/scheduler.rs", "#[cfg(target_arch=x86_64)] fn run_scheduler_tests", 3),
    ("kernel/src/task/scheduler.rs", "fn emit_pinned_placement_census", 1),
    ("kernel/src/task/scheduler.rs", "fn emit_wake_attribution_counters", 2),
    ("kernel/src/task/scheduler.rs", "fn init", 1),
    ("kernel/src/task/scheduler.rs", "fn init_with_current", 1),
    ("kernel/src/task/scheduler.rs", "fn note_scheduler_publication", 1),
    ("kernel/src/task/scheduler.rs", "fn switch_to_idle", 3),
    ("kernel/src/task/scheduler.rs", "impl ExecSchedCommit::fn apply", 4),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn add_thread_as_current", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn add_thread_inner", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current_for_child_exit", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn block_current_for_signal_with_context", 2),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn dump_thread_placement", 3),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn schedule", 5),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn unblock", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn unblock_for_child_exit", 1),
    ("kernel/src/task/scheduler.rs", "impl Scheduler::fn unblock_for_signal", 6),
];

/// The one call the narrow denylist misses: `crate::serial_print!` (no
/// `ln`) inside a per-byte write loop. `serial_print!` does not match any
/// of the twelve `SHARED_PATTERNS` -- it is a strict prefix removal of
/// `serial_println!`, not a substring of it -- so it is invisible to both
/// the shell script before this PR and to `CRITICAL_PATH_LOG_ANCHORS`
/// above. `critical_path_log_wider_census_is_pinned` below appends this
/// row to `CRITICAL_PATH_LOG_ANCHORS` and validates against
/// `SHARED_PATTERNS ++ WIDER_EXTRA_PATTERNS`, rather than duplicating 48
/// rows into a second const, so the "wider = narrow + this one escaped
/// site" relationship is visible in code instead of in two tables someone
/// has to notice are 47 rows identical.
const ESCAPED_SITE: (&str, &str, usize) = (
    "kernel/src/arch_impl/aarch64/exception.rs",
    "fn sys_write",
    1,
);

fn wider_anchors() -> Vec<(&'static str, &'static str, usize)> {
    let mut anchors = CRITICAL_PATH_LOG_ANCHORS.to_vec();
    anchors.push(ESCAPED_SITE);
    anchors
}

fn wider_patterns() -> Vec<&'static str> {
    SHARED_PATTERNS
        .iter()
        .chain(WIDER_EXTRA_PATTERNS.iter())
        .copied()
        .collect()
}

fn validate_zero_pin_files(sources: &[(String, String)]) -> Result<(), Vec<String>> {
    let mut violations = Vec::new();
    for zero_file in ZERO_PIN_FILES {
        for (path, source) in sources {
            if path != zero_file {
                continue;
            }
            let mask = code_mask(source);
            let offsets = matched_line_offsets(source, &mask, &wider_patterns());
            if offsets.is_empty() {
                continue;
            }
            let spans = rendered_item_spans(&item_spans(source, &mask));
            for offset in offsets {
                violations.push(format!(
                    "+ {path} :: {}  (zero-pin file carries a denylisted call)",
                    item_path_at(&spans, offset)
                ));
            }
        }
    }
    violations.is_empty().then_some(()).ok_or(violations)
}

// ---------------------------------------------------------------------
// Shell-script parity: `CRITICAL_FILES` and `PROHIBITED_PATTERNS` as
// spelled in `scripts/check-critical-path-violations.sh` must equal the
// Rust lists above, the same discipline
// `the_shell_guard_and_this_suite_deny_the_same_shapes` in
// `tests/capture_path_lock_free_structure.rs` applies to the capture-scoped
// denylist.
// ---------------------------------------------------------------------

const CRITICAL_PATH_SCRIPT: &str = "scripts/check-critical-path-violations.sh";

/// The quoted entries of a bash array declared as `<MARKER>\n    "a"\n
/// "b"\n)\n` (or with `'` quoting), skipping blank lines and `#`-prefixed
/// comment lines -- both `CRITICAL_FILES` and `PROHIBITED_PATTERNS` carry
/// explanatory comments between entries. The closing paren is found by its
/// OWN line reading exactly `)` after trimming, not by the first `)`
/// anywhere in the block: a `CRITICAL_FILES` comment line contains the
/// prose "(accessed in hot paths)", whose trailing `)` would otherwise
/// truncate the block early.
fn shell_array_entries(script: &str, marker: &str, quote: char) -> Vec<String> {
    let lines: Vec<&str> = script.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == marker)
        .unwrap_or_else(|| panic!("{CRITICAL_PATH_SCRIPT} has no `{marker}` array"));
    let mut entries = Vec::new();
    for line in &lines[start + 1..] {
        let trimmed = line.trim();
        if trimmed == ")" {
            return entries;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(open) = trimmed.find(quote) else {
            continue;
        };
        let Some(relative_close) = trimmed[open + 1..].find(quote) else {
            continue;
        };
        entries.push(trimmed[open + 1..open + 1 + relative_close].to_owned());
    }
    panic!("{CRITICAL_PATH_SCRIPT}'s `{marker}` array has no closing `)` line");
}

fn validate_shell_parity(script: &str) -> Result<(), String> {
    let shell_files = shell_array_entries(script, "CRITICAL_FILES=(", '"');
    let rust_files: Vec<String> = CRITICAL_FILES.iter().map(|entry| entry.to_string()).collect();
    if shell_files != rust_files {
        return Err(format!(
            "CRITICAL_FILES diverged -- shell {shell_files:?}, rust {rust_files:?}"
        ));
    }

    let shell_patterns = shell_array_entries(script, "PROHIBITED_PATTERNS=(", '\'');
    let rust_patterns: Vec<String> =
        wider_patterns().into_iter().map(|entry| entry.to_string()).collect();
    if shell_patterns != rust_patterns {
        return Err(format!(
            "PROHIBITED_PATTERNS diverged -- shell {shell_patterns:?}, rust {rust_patterns:?}"
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------
// Tests: real tree, then the five anti-vacuity mutation legs.
// ---------------------------------------------------------------------

#[test]
fn critical_path_log_census_is_pinned() {
    let census = log_census(&checked_sources(), SHARED_PATTERNS);
    assert_eq!(validate_census(&census, CRITICAL_PATH_LOG_ANCHORS), Ok(()));
}

#[test]
fn critical_path_log_wider_census_is_pinned() {
    let census = log_census(&checked_sources(), &wider_patterns());
    assert_eq!(validate_census(&census, &wider_anchors()), Ok(()));
}

#[test]
fn zero_pin_files_stay_clean() {
    assert_eq!(validate_zero_pin_files(&checked_sources()), Ok(()));
}

#[test]
fn the_shell_and_this_suite_check_the_same_shapes() {
    let script = repo_text(CRITICAL_PATH_SCRIPT);
    assert_eq!(validate_shell_parity(&script), Ok(()));
}

/// Mutation 1: a synthetic file at a checked path (`per_cpu.rs`) carrying
/// one `serial_println!` inside a brand-new function. Must redden the
/// general census with a `+` row for the synthetic site, not silently
/// merge into an existing anchor.
#[test]
fn census_validator_rejects_a_synthetic_violation_at_a_checked_path() {
    let sources = with_synthetic_source(
        &checked_sources(),
        "kernel/src/per_cpu.rs",
        r#"
            fn synthetic_critical_path_probe() {
                serial_println!("synthetic probe");
            }
        "#,
    );
    let census = log_census(&sources, SHARED_PATTERNS);
    assert_eq!(
        validate_census(&census, CRITICAL_PATH_LOG_ANCHORS),
        Err(vec![
            "+ kernel/src/per_cpu.rs :: fn synthetic_critical_path_probe  (1 occurrences, expected none)".to_owned()
        ])
    );
}

/// Mutation 2: delete one row from the anchor table. The real site is
/// still in the tree, so the census now carries an anchor the (shrunk)
/// table does not expect -- a `+` row naming exactly the deleted anchor.
#[test]
fn census_validator_rejects_a_deleted_anchor_row() {
    let deleted_anchor = ("kernel/src/syscall/handler.rs", "fn rust_syscall_handler", 2);
    let shrunk: Vec<(&str, &str, usize)> = CRITICAL_PATH_LOG_ANCHORS
        .iter()
        .copied()
        .filter(|anchor| *anchor != deleted_anchor)
        .collect();
    assert_eq!(shrunk.len(), CRITICAL_PATH_LOG_ANCHORS.len() - 1);

    let census = log_census(&checked_sources(), SHARED_PATTERNS);
    assert_eq!(
        validate_census(&census, &shrunk),
        Err(vec![format!(
            "+ {} :: {}  ({} occurrences, expected none)",
            deleted_anchor.0, deleted_anchor.1, deleted_anchor.2
        )])
    );
}

/// Mutation 3: decrement one row's expected count by one. The real tree
/// still carries the original count, so the diff must show a `~` line
/// naming the mismatch in both directions.
#[test]
fn census_validator_rejects_a_decremented_anchor_count() {
    let target = ("kernel/src/arch_impl/aarch64/exception.rs", "fn handle_syscall", 8usize);
    assert!(CRITICAL_PATH_LOG_ANCHORS.contains(&target));
    let decremented: Vec<(&str, &str, usize)> = CRITICAL_PATH_LOG_ANCHORS
        .iter()
        .copied()
        .map(|anchor| {
            if anchor == target {
                (anchor.0, anchor.1, anchor.2 - 1)
            } else {
                anchor
            }
        })
        .collect();

    let census = log_census(&checked_sources(), SHARED_PATTERNS);
    assert_eq!(
        validate_census(&census, &decremented),
        Err(vec![format!(
            "~ {} :: {}  (expected {}, found {})",
            target.0,
            target.1,
            target.2 - 1,
            target.2
        )])
    );
}

/// Mutation 4: a synthetic `interrupts/timer.rs` carrying one `log::info!`
/// -- one of the three files `ZERO_PIN_FILES` claims is clean. Must redden
/// the zero-pin validator specifically, independent of the general census.
#[test]
fn zero_pin_validator_rejects_a_synthetic_violation_in_timer_rs() {
    let sources = with_synthetic_source(
        &checked_sources(),
        "kernel/src/interrupts/timer.rs",
        r#"
            fn synthetic_timer_probe() {
                log::info!("synthetic timer log");
            }
        "#,
    );
    assert_eq!(
        validate_zero_pin_files(&sources),
        Err(vec![
            "+ kernel/src/interrupts/timer.rs :: fn synthetic_timer_probe  (zero-pin file carries a denylisted call)".to_owned()
        ])
    );
}

/// Mutation 5: remove one `CRITICAL_FILES` entry from an in-memory copy of
/// the shell script (the file on disk is untouched). The shell-parity
/// check must fail rather than silently accept a shrunk file list.
#[test]
fn shell_parity_validator_rejects_a_removed_critical_file() {
    let script = repo_text(CRITICAL_PATH_SCRIPT);
    let mutated = script.replacen("    \"task/scheduler.rs\"\n", "", 1);
    assert_ne!(mutated, script);
    match validate_shell_parity(&mutated) {
        Ok(()) => panic!("removing a CRITICAL_FILES entry must be caught"),
        Err(message) => assert!(
            message.contains("CRITICAL_FILES diverged"),
            "unexpected error: {message}"
        ),
    }
}
