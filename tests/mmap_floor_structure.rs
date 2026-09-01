//! Structural ratchet for the mmap region's producer/validator agreement
//! (#742, PR #744 review B2).
//!
//! `layout.rs` asserts (compile-time) that `is_valid_user_range` accepts
//! `MMAP_REGION_START` and refuses `MMAP_REGION_START - 1`. That proves a
//! property of the *validator* only. Whether the real *allocators* ever hand
//! out an address the validator would refuse is a separate, source-level
//! fact: it holds only as long as every producer that descends `mmap_hint`
//! floors it against the named constant, and every producer that seeds
//! `mmap_hint` seeds it from the named constant, rather than an
//! independently hardcoded literal. Nothing in the type system enforces
//! that link -- #742 itself exists because six call sites drifted onto a
//! hardcoded `0x1000_0000` floor while the validator kept asserting
//! `MMAP_REGION_START` (0x7000_0000_0000), and #729's B4-a is the same
//! "validator anchored to a bound the allocator does not use" shape one
//! layer up (the mmap *window*, not just its floor).
//!
//! This file is the enforcement layer #742's own commit added prose for but
//! not a check for (review B2: `layout.rs` claimed "there is nowhere left
//! for either bound and the allocators' seed/floor to drift apart again" and
//! "a proof about the lowest address a real allocator can hand out" --
//! neither backed by anything that would fail a build). The law pinned here
//! is a **census**, not a universal claim: every known producer site's
//! *shape* -- a comparison or seed assignment naming
//! `crate::memory::vma::MMAP_REGION_START`/`_END` by path, not a literal --
//! is pinned by (file, enclosing function, occurrence count), the same
//! idiom `tests/teardown_structure.rs`'s `THREAD_STATE_CONSTRUCTIONS` and
//! `tests/preempt_bracket_structure.rs` use. A future edit that
//! re-hardcodes a literal, deletes a site, or adds an unrecognized new one
//! reddens this test; a future edit that only reflows or renames a local
//! variable does not (`comments_and_strings_do_not_feed_the_census` and
//! `reflowing_or_renaming_a_site_does_not_redden_the_census` below prove
//! both directions). Per the `[[gate-target-fidelity-528]]`-adjacent lesson
//! repeated at #549/#551/#527: the anchors below are shapes (file + function
//! + count), never a literal file/line list.
//!
//! What this file does **not** claim: it cannot see a producer that reads
//! `Process::mmap_hint` and floors or seeds it by some entirely different
//! textual shape than the two pinned below (a new syscall added tomorrow
//! that copies the old `0x1000_0000` habit without ever writing the string
//! `MMAP_REGION_START` would not be walked into either census). It is a
//! structural ratchet over today's known producers, not a proof that no
//! future producer could ever be written adrift again.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|error| panic!("read repository file {relative}: {error}"))
}

// === Comment/string-aware source scanning (same idiom as the other
// `_structure.rs` ratchets; duplicated locally per that established
// convention -- each structure test is its own independent test binary). ===

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
                    return Some(&source[start..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every `fn NAME(` definition in a module, keyed by name. Names may repeat
/// (`cfg`-split arch/feature variants, e.g. `sys_fbmmap`'s interactive-mode
/// stub); every body is kept, and callers sum across the group, so a rename
/// or a cfg split does not silently narrow what is being counted.
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
        // A signature terminated by `;` (trait requirement, extern block) has
        // no body; taking the next brace would attribute a foreign body to it.
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

/// The masked code of a fragment, whitespace runs collapsed to one space, so
/// a comparison split across lines by rustfmt (or reflowed by a future edit)
/// is counted the same as one written on a single line -- reflow is free,
/// per the file-level doc comment above.
fn normalized_code(fragment: &str) -> String {
    let mask = code_mask(fragment);
    let kept: Vec<u8> = fragment
        .bytes()
        .zip(mask)
        .filter_map(|(byte, code)| code.then_some(byte))
        .collect();
    String::from_utf8_lossy(&kept)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn needle_count(body: &str, needle: &str) -> usize {
    normalized_code(body).matches(needle).count()
}

// === The two shapes being ratcheted ===
//
// FLOOR: every site that descends `mmap_hint` (or, for `sys_mmap`'s
// `MAP_FIXED` arm, an address passed straight from userspace) checks it
// against the region's real lower bound. The comparison names the constant
// by its full path -- not the local variable, so a rename of `new_addr` or
// `start_addr` does not change what is pinned (`different_local_variable_
// names_still_count` below proves this).
const FLOOR_NEEDLE: &str = "< crate::memory::vma::MMAP_REGION_START";

// SEED: every site that (re)initializes `mmap_hint` seeds it from the
// region's real upper bound, in either of the two shapes Rust source
// actually uses for it -- a struct-literal field (`process.rs::new`) or a
// plain assignment (`manager.rs`'s exec paths, which reset mmap state on
// exec per POSIX).
const SEED_COLON_NEEDLE: &str = "mmap_hint: crate::memory::vma::MMAP_REGION_END";
const SEED_ASSIGN_NEEDLE: &str = "mmap_hint = crate::memory::vma::MMAP_REGION_END";

type Anchor = (String, String);
type Census = BTreeMap<Anchor, usize>;

fn file_census(path: &str, needles: &[&str]) -> Census {
    let source = repo_text(path);
    let bodies = module_function_bodies(&source);
    let mut census = Census::new();
    for (name, occurrences) in &bodies {
        let total: usize = occurrences
            .iter()
            .map(|body| needles.iter().map(|needle| needle_count(body, needle)).sum::<usize>())
            .sum();
        if total > 0 {
            census.insert((path.to_owned(), name.clone()), total);
        }
    }
    census
}

fn expected_census(anchors: &[(&str, &str, usize)]) -> Census {
    let mut census = Census::new();
    for (path, item, count) in anchors {
        let anchor = ((*path).to_owned(), (*item).to_owned());
        assert!(
            census.insert(anchor, *count).is_none(),
            "duplicate census anchor {path} :: {item}; the two entries would otherwise sum into one allowance"
        );
    }
    census
}

/// Every divergence between the observed and the pinned census, never just
/// the first: a legitimate re-anchor (a rename, a new arch/feature variant)
/// is done in one pass, and a regression that both drops a known site and
/// introduces an unrecognized new one is not half-reported.
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

/// The six producer sites #742's own fix-notes enumerate: `sys_mmap`'s
/// non-`MAP_FIXED` hint-descent floor plus its `MAP_FIXED` region-escape
/// check (both in one function, hence count 2), and the five
/// `graphics.rs` siblings the issue's own grep found beyond the two named
/// in #742's text.
const FLOOR_ANCHORS: &[(&str, &str, usize)] = &[
    ("kernel/src/syscall/mmap.rs", "sys_mmap", 2),
    ("kernel/src/syscall/graphics.rs", "handle_create_window_buffer", 1),
    ("kernel/src/syscall/graphics.rs", "handle_resize_window_buffer", 1),
    ("kernel/src/syscall/graphics.rs", "handle_map_window_buffer", 1),
    ("kernel/src/syscall/graphics.rs", "handle_map_compositor_texture", 1),
    ("kernel/src/syscall/graphics.rs", "sys_fbmmap", 1),
];

/// The five `mmap_hint` seed sites: the struct-literal init in
/// `Process::new` (grouped under the bare name `new` -- `process.rs` has
/// three functions literally named `new`; only one seeds `mmap_hint`, so
/// the pinned count still isolates it) and the exec-time resets in
/// `manager.rs`, each of which is itself arch-split into two bodies (one
/// seed site per body).
const SEED_ANCHORS: &[(&str, &str, usize)] = &[
    ("kernel/src/process/process.rs", "new", 1),
    ("kernel/src/process/manager.rs", "exec_process", 2),
    ("kernel/src/process/manager.rs", "exec_process_with_argv", 2),
];

fn floor_census() -> Census {
    let mut census = file_census("kernel/src/syscall/mmap.rs", &[FLOOR_NEEDLE]);
    census.extend(file_census("kernel/src/syscall/graphics.rs", &[FLOOR_NEEDLE]));
    census
}

fn seed_census() -> Census {
    let needles = [SEED_COLON_NEEDLE, SEED_ASSIGN_NEEDLE];
    let mut census = file_census("kernel/src/process/process.rs", &needles);
    census.extend(file_census("kernel/src/process/manager.rs", &needles));
    census
}

#[test]
fn mmap_floor_producers_match_the_pinned_census() {
    let diff = census_diff(&floor_census(), FLOOR_ANCHORS);
    assert!(
        diff.is_empty(),
        "producer floor-comparison census drifted from the pinned shape -- \
         either a site started using a literal instead of \
         `crate::memory::vma::MMAP_REGION_START`, a known site was deleted, \
         or a new one appeared uncounted (#742, PR #744 review B2):\n{}",
        diff.join("\n")
    );
}

#[test]
fn mmap_hint_seed_sites_match_the_pinned_census() {
    let diff = census_diff(&seed_census(), SEED_ANCHORS);
    assert!(
        diff.is_empty(),
        "mmap_hint seed census drifted from the pinned shape -- either a \
         site started seeding from a literal instead of \
         `crate::memory::vma::MMAP_REGION_END`, a known site was deleted, or \
         a new one appeared uncounted (PR #744 review B2):\n{}",
        diff.join("\n")
    );
}

#[test]
fn neither_census_is_empty() {
    // Anti-vacuity: a refactor that moves every producer out of these two
    // file pairs (or renames the constant so the needle no longer matches
    // anywhere) must fail loudly here rather than leave both censuses empty
    // and the two tests above trivially green.
    assert!(
        !floor_census().is_empty(),
        "the floor-comparison census found zero producer sites -- this ratchet is measuring nothing"
    );
    assert!(
        !seed_census().is_empty(),
        "the mmap_hint seed census found zero seed sites -- this ratchet is measuring nothing"
    );
}

#[test]
fn a_reintroduced_literal_floor_reddens_the_census() {
    // #742's exact regression shape: a site drifts from the named constant
    // back onto the stale hardcoded floor.
    let planted = r#"
pub fn sys_mmap(addr: u64, length: u64) -> SyscallResult {
    let new_addr = round_down_to_page(hint.saturating_sub(length));
    if new_addr < 0x1000_0000 {
        return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
    }
    process.mmap_hint = new_addr;
}
"#;
    let bodies = module_function_bodies(planted);
    let body = bodies
        .get("sys_mmap")
        .and_then(|bodies| bodies.first())
        .expect("synthetic source defines sys_mmap");
    assert_eq!(
        needle_count(body, FLOOR_NEEDLE),
        0,
        "a literal floor must not be counted as consulting MMAP_REGION_START"
    );
}

#[test]
fn a_deleted_floor_check_reddens_the_census() {
    // The other half of the same regression: the comparison is removed
    // entirely (e.g. "simplified away"), not merely re-literalized.
    let planted = r#"
pub fn sys_mmap(addr: u64, length: u64) -> SyscallResult {
    let new_addr = round_down_to_page(hint.saturating_sub(length));
    process.mmap_hint = new_addr;
}
"#;
    let bodies = module_function_bodies(planted);
    let body = bodies
        .get("sys_mmap")
        .and_then(|bodies| bodies.first())
        .expect("synthetic source defines sys_mmap");
    assert_eq!(needle_count(body, FLOOR_NEEDLE), 0);
}

#[test]
fn a_reintroduced_literal_seed_reddens_the_census() {
    let planted = r#"
impl Process {
    pub fn new(id: ProcessId) -> Self {
        Self {
            mmap_hint: 0x7FFF_FE00_0000,
        }
    }
}
"#;
    let bodies = module_function_bodies(planted);
    let body = bodies
        .get("new")
        .and_then(|bodies| bodies.first())
        .expect("synthetic source defines new");
    assert_eq!(
        needle_count(body, SEED_COLON_NEEDLE),
        0,
        "a literal seed must not be counted as consulting MMAP_REGION_END"
    );
}

#[test]
fn reflowing_or_renaming_a_site_does_not_redden_the_census() {
    // The other direction the review demanded: a legitimate refactor -- a
    // line-wrapped comparison, a renamed local variable -- must stay green.
    // This is what makes the census shape-based rather than a literal
    // file/line list (#549/#551/#527 lesson).
    let reflowed = r#"
pub fn sys_mmap(candidate_addr: u64, length: u64) -> SyscallResult {
    let descended
        = round_down_to_page(hint.saturating_sub(length));
    if descended
        < crate::memory::vma::MMAP_REGION_START
    {
        return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
    }
    process.mmap_hint = descended;
}
"#;
    let bodies = module_function_bodies(reflowed);
    let body = bodies
        .get("sys_mmap")
        .and_then(|bodies| bodies.first())
        .expect("synthetic source defines sys_mmap");
    assert_eq!(
        needle_count(body, FLOOR_NEEDLE),
        1,
        "reflowing a comparison across lines and renaming its local variable \
         must not change the count -- only the named constant matters"
    );
}

#[test]
fn comments_and_strings_do_not_feed_the_census() {
    // `layout.rs`, `mmap.rs`, and `graphics.rs` all carry prose that spells
    // out `MMAP_REGION_START`/`MMAP_REGION_END` repeatedly; a census that
    // counted prose would be unfalsifiable.
    let commented = r#"
pub fn sys_mmap(addr: u64, length: u64) -> SyscallResult {
    // Floors against `crate::memory::vma::MMAP_REGION_START`, same as the
    // validator: "< crate::memory::vma::MMAP_REGION_START" in a doc comment.
    let note = "< crate::memory::vma::MMAP_REGION_START";
    let new_addr = round_down_to_page(hint.saturating_sub(length));
    if new_addr < crate::memory::vma::MMAP_REGION_START {
        return SyscallResult::Err(ErrorCode::OutOfMemory as u64);
    }
    process.mmap_hint = new_addr;
}
"#;
    let bodies = module_function_bodies(commented);
    let body = bodies
        .get("sys_mmap")
        .and_then(|bodies| bodies.first())
        .expect("synthetic source defines sys_mmap");
    assert_eq!(
        needle_count(body, FLOOR_NEEDLE),
        1,
        "the doc comment and the string literal above must not be counted -- only the real code site"
    );
}

#[test]
fn no_producer_file_hardcodes_the_stale_mmap_floor_literal() {
    // Direct anti-regression check, independent of the census machinery
    // above: the pre-#742 literal must not appear anywhere in the *code*
    // (comments are exempt -- both files' fix-notes prose mentions it on
    // purpose, as history) of either producer file.
    for path in ["kernel/src/syscall/mmap.rs", "kernel/src/syscall/graphics.rs"] {
        let source = repo_text(path);
        let normalized = normalized_code(&source);
        assert!(
            !normalized.contains("0x1000_0000"),
            "{path} contains the stale pre-#742 hardcoded mmap floor literal \
             (0x1000_0000) outside of comments"
        );
    }
}
