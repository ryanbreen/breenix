//! Structural ratchet for the kernel's boot-path preempt bracket (#672).
//!
//! `preempt_disable()` and `preempt_enable()` are one protocol. On a boot path
//! they are taken hundreds of lines apart, which is exactly where a `#[cfg]` on
//! one half and not the other survives review: #672 was
//! `kernel/src/main.rs` taking the boot brake inside
//! `#[cfg(all(feature = "testing", not(feature = "interactive")))]` and
//! releasing it unconditionally, so every shipped (zero-feature) x86 boot
//! decremented a `preempt_count` that had never been incremented and wrapped it
//! to `0xFFFFFFFF`.
//!
//! The law pinned here is *symmetry*, not a line number and not a closed list of
//! feature names: for each boot-path source, the census of cfg conditions
//! governing its `preempt_disable()` sites must equal the census governing its
//! `preempt_enable()` sites. A future bracket that is deliberately cfg-gated as
//! a unit still passes; half a bracket never does. A second, stronger test pins
//! that every distinct bracket context in a file is a refinement of every other
//! (#673, B4: a file may hold more than one bracket, at more than one nesting
//! depth, once a nested bracket like #673's production-init one is legitimate) -
//! and the vacuity tests prove each check can still go red.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// Boot-path sources. Each owns a long-range preempt bracket taken once per
/// boot; these are the files where an asymmetric cfg is invisible to review.
const BOOT_PATH_SOURCES: [&str; 2] = ["kernel/src/main.rs", "kernel/src/main_aarch64.rs"];

const DISABLE: &str = "preempt_disable()";
const ENABLE: &str = "preempt_enable()";

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

/// The cfg conditions governing each call site, in source order.
///
/// A condition enters scope when a `#[cfg(..)]` attribute is seen and leaves it
/// when the item it decorates ends - the closing brace of the block it opens, or
/// the semicolon of the statement it decorates when there is no block. That
/// covers both shapes an asymmetry can take: the whole call wrapped in a
/// cfg-gated block (#672's shape) and the attribute written directly on the
/// call statement.
fn cfg_contexts(source: &str, call: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mask = code_mask(source);
    let mut scopes: Vec<Vec<String>> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    let mut contexts = Vec::new();

    let context_at = |scopes: &Vec<Vec<String>>, pending: &Vec<String>| -> String {
        let mut conditions: Vec<String> = scopes
            .iter()
            .flatten()
            .chain(pending.iter())
            .cloned()
            .collect();
        conditions.sort();
        conditions.join(" + ")
    };

    let mut index = 0usize;
    while index < bytes.len() {
        if !mask[index] {
            index += 1;
            continue;
        }
        if source[index..].starts_with(call) {
            contexts.push(context_at(&scopes, &pending));
            index += call.len();
            continue;
        }
        let byte = bytes[index];
        if byte == b'#' {
            // Outer `#[..]` and inner `#![..]` attributes alike: skip to the
            // matching bracket, harvesting the condition when it is a cfg.
            let open = if bytes.get(index + 1) == Some(&b'[') {
                index + 1
            } else if bytes.get(index + 1) == Some(&b'!') && bytes.get(index + 2) == Some(&b'[') {
                index + 2
            } else {
                index += 1;
                continue;
            };
            let mut depth = 0usize;
            let mut cursor = open;
            while cursor < bytes.len() {
                if mask[cursor] {
                    if bytes[cursor] == b'[' {
                        depth += 1;
                    } else if bytes[cursor] == b']' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                cursor += 1;
            }
            let inner = source[open + 1..cursor.min(bytes.len())].trim();
            if let Some(condition) = inner
                .strip_prefix("cfg(")
                .and_then(|rest| rest.strip_suffix(')'))
            {
                pending.push(condition.split_whitespace().collect::<Vec<_>>().join(" "));
            }
            index = cursor + 1;
            continue;
        }
        match byte {
            b'{' => {
                scopes.push(std::mem::take(&mut pending));
            }
            b'}' => {
                scopes.pop();
            }
            b';' => {
                pending.clear();
            }
            _ => {}
        }
        index += 1;
    }

    contexts
}

fn census(contexts: &[String]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for context in contexts {
        *counts.entry(context.clone()).or_insert(0) += 1;
    }
    counts
}

/// The whole law in one function so the vacuity tests can exercise it against
/// synthetic sources rather than only against the tree.
fn bracket_is_symmetric(source: &str) -> bool {
    census(&cfg_contexts(source, DISABLE)) == census(&cfg_contexts(source, ENABLE))
}

#[test]
fn boot_path_preempt_brackets_are_cfg_symmetric() {
    for path in BOOT_PATH_SOURCES {
        let source = repo_text(path);
        let disables = census(&cfg_contexts(&source, DISABLE));
        let enables = census(&cfg_contexts(&source, ENABLE));
        assert_eq!(
            disables, enables,
            "{path}: preempt_disable() and preempt_enable() sites carry different cfg conditions - \
             half a bracket compiles out and the other half underflows preempt_count (#672)"
        );
    }
}

#[test]
fn boot_path_preempt_sites_share_one_cfg_context() {
    // The strong form of the same law. A boot-path file's preempt sites may sit
    // under whatever cfg gates the item that contains them - `kernel_main_continue`
    // is itself `#[cfg(target_arch = "x86_64", ...)]`, and that is not an
    // asymmetry because both halves inherit it. What may never happen is one half
    // acquiring a condition the other does not have, which is #672 exactly.
    //
    // A file may legitimately hold more than one bracket, at more than one
    // nesting depth (#673 review, B4: the production block's bracket is a
    // deliberately MORE-nested one alongside the historical bracket, scoped
    // narrower and released after boot's own remaining sequential work; it
    // compiles into a strict SUBSET of the profiles the historical one does,
    // by lexical nesting inside the same function - not by feature-implication
    // reasoning about what any condition implies). "One context" here means
    // every distinct context is comparable to every other under set inclusion
    // - forms one nesting chain - so no two brackets can carry two genuinely
    // unrelated conditions. Exact per-context balance (disable count == enable
    // count for that exact context) is `boot_path_preempt_brackets_are_cfg_symmetric`'s
    // job above, not this one's.
    for path in BOOT_PATH_SOURCES {
        let source = repo_text(path);
        let mut contexts: Vec<String> = cfg_contexts(&source, DISABLE);
        contexts.extend(cfg_contexts(&source, ENABLE));
        let distinct = census(&contexts);
        let condition_sets: Vec<BTreeSet<&str>> = distinct
            .keys()
            .map(|context| context.split(" + ").filter(|c| !c.is_empty()).collect())
            .collect();
        for (i, a) in condition_sets.iter().enumerate() {
            for b in &condition_sets[i + 1..] {
                assert!(
                    a.is_subset(b) || b.is_subset(a),
                    "{path}: boot-path preempt sites carry two incomparable cfg contexts \
                     {a:?} and {b:?} (out of {} distinct contexts total) - every bracket must \
                     be a refinement of every other, not an unrelated condition (#672)",
                    distinct.len()
                );
            }
        }
    }
}

#[test]
fn boot_path_preempt_bracket_census_is_not_empty() {
    // Anti-vacuity: a rename or a refactor that moves these calls out of the
    // boot-path files must fail loudly here rather than leave both censuses
    // empty and every assertion above trivially true.
    for path in BOOT_PATH_SOURCES {
        let source = repo_text(path);
        assert!(
            !cfg_contexts(&source, DISABLE).is_empty(),
            "{path}: no preempt_disable() call site found - the ratchet is measuring nothing"
        );
        assert!(
            !cfg_contexts(&source, ENABLE).is_empty(),
            "{path}: no preempt_enable() call site found - the ratchet is measuring nothing"
        );
    }
}

#[test]
fn asymmetric_cfg_reddens_the_ratchet() {
    // #672's exact shape: the disable inside a testing-only block, the release
    // unconditional several hundred lines later.
    let planted = r#"
fn kernel_main_continue() -> ! {
    #[cfg(all(feature = "testing", not(feature = "interactive")))]
    {
        kernel::per_cpu::preempt_disable();
    }
    kernel::per_cpu::preempt_enable();
}
"#;
    assert!(
        !bracket_is_symmetric(planted),
        "a cfg on the disable and none on the enable must redden the symmetry ratchet"
    );

    // The attribute written straight onto the call statement is the same defect
    // with no block to see.
    let planted_statement = r#"
fn kernel_main_continue() -> ! {
    #[cfg(feature = "testing")]
    kernel::per_cpu::preempt_disable();
    kernel::per_cpu::preempt_enable();
}
"#;
    assert!(
        !bracket_is_symmetric(planted_statement),
        "a cfg attribute on the disable statement must redden the symmetry ratchet"
    );

    // And the mirror image: gating the release without gating the brake leaves
    // preemption disabled forever instead of underflowing.
    let planted_release = r#"
fn kernel_main_continue() -> ! {
    kernel::per_cpu::preempt_disable();
    #[cfg(feature = "testing")]
    kernel::per_cpu::preempt_enable();
}
"#;
    assert!(
        !bracket_is_symmetric(planted_release),
        "a cfg on the enable alone must redden the symmetry ratchet"
    );
}

#[test]
fn symmetric_cfg_and_unconditional_brackets_pass() {
    // The law is symmetry, not a ban on cfg: a bracket deliberately gated as a
    // unit is legal, which is what stops this ratchet from being a rule nobody
    // can satisfy.
    let paired = r#"
fn kernel_main_continue() -> ! {
    #[cfg(feature = "testing")]
    {
        kernel::per_cpu::preempt_disable();
        boot();
        kernel::per_cpu::preempt_enable();
    }
}
"#;
    assert!(
        bracket_is_symmetric(paired),
        "a bracket cfg-gated as a unit must satisfy the symmetry ratchet"
    );

    let unconditional = r#"
fn kernel_main_continue() -> ! {
    kernel::per_cpu::preempt_disable();
    #[cfg(feature = "testing")]
    {
        load_test_binaries();
    }
    kernel::per_cpu::preempt_enable();
}
"#;
    assert!(
        bracket_is_symmetric(unconditional),
        "the shipped shape - unconditional brake, cfg-gated work inside it - must pass"
    );
}

#[test]
fn comments_and_strings_do_not_feed_the_census() {
    // The fix's own comments name both calls repeatedly; a census that counted
    // them would be unfalsifiable.
    let commented = r#"
fn kernel_main_continue() -> ! {
    // preempt_disable() is the brake; the matching preempt_enable() is below.
    let note = "preempt_disable() / preempt_enable()";
    kernel::per_cpu::preempt_disable();
    kernel::per_cpu::preempt_enable();
}
"#;
    assert_eq!(
        cfg_contexts(commented, DISABLE).len(),
        1,
        "commented and quoted mentions must not count as call sites"
    );
    assert_eq!(cfg_contexts(commented, ENABLE).len(), 1);
}

#[test]
fn the_underflow_guard_stays_fail_closed() {
    // The bracket's other half of #672: an unpaired release must saturate and be
    // counted, never wrap. Pinned by shape - the guard consults the PREEMPT
    // bits, counts, and returns before the HAL decrement - so that deleting any
    // one of those three reddens here.
    let source = repo_text("kernel/src/per_cpu.rs");
    let mask = code_mask(&source);
    let enable = code_offsets(&source, &mask, "pub fn preempt_enable()")
        .first()
        .copied()
        .expect("per_cpu.rs must define preempt_enable()");
    let decrement = code_offsets(&source, &mask, "X86PerCpu::preempt_enable()")
        .into_iter()
        .find(|offset| *offset > enable)
        .expect("preempt_enable() must still reach the HAL decrement");
    let body = &source[enable..decrement];
    assert!(
        body.contains("PREEMPT_MASK"),
        "preempt_enable() must consult the PREEMPT bits before decrementing them (#672)"
    );
    assert!(
        body.contains("PREEMPT_UNDERFLOW_COUNT.fetch_add"),
        "preempt_enable() must count an unpaired release (#672)"
    );
    assert!(
        body.contains("return;"),
        "preempt_enable() must saturate at zero instead of wrapping the count (#672)"
    );
    assert!(
        code_offsets(&source, &mask, "pub fn preempt_underflow_count()").len() == 1,
        "the underflow count must stay readable at census time (#672)"
    );
}
