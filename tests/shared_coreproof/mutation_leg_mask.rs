//! Masking the core-proof harness's compiled-out mutation legs.
//!
//! The harness plants six known-fixed defects back at their real sites, one
//! cargo feature each, so that it can be SHOWN to re-find them rather than
//! asserted to. Several of those sites live in files that this repository's
//! structural ratchets read, and those ratchets are TEXT-based: they cannot see
//! a `#[cfg]`. Without this, a leg that no production build compiles reddens
//! ratchets that exist to police what production compiles — the ratchet
//! reporting on source that is in no shipped binary.
//!
//! The exemption is deliberately narrow in three ways:
//!
//! * only a POSITIVE attribute is masked, and only when EVERY feature it
//!   names is `coreproof_`-prefixed. This covers both the six
//!   `coreproof_mut_*` mutation legs and the distinct `coreproof_component_c`
//!   proof-point driver, since `kernel/Cargo.toml` makes every one of them imply
//!   the base `coreproof` feature, and two dedicated ratchets —
//!   `check-coreproof-seams.sh` and `coreproof_production_clean.rs` — already
//!   prove none of it ships. The base `coreproof` feature itself remains
//!   visible: several ratchets intentionally inventory the proof harness's own
//!   APIs, so blanket-masking the whole harness would make those laws vacuous. A
//!   `#[cfg(not(feature = "coreproof_mut_…"))]` (or `not(feature =
//!   "coreproof_component_c")`) arm guards the PRODUCTION code these
//!   ratchets police and stays fully visible, so every law still applies to
//!   the code that ships. A predicate naming any non-coreproof feature
//!   alongside a coreproof one (e.g. `any(feature = "coreproof_mut_x",
//!   feature = "something_else")`) is likewise left fully visible — narrower
//!   than strictly necessary, deliberately, rather than guess at intent;
//! * only the construct that attribute governs is masked — one `;`-terminated
//!   statement or one balanced block — never a line range and never to end of
//!   file;
//! * every file that adopts it carries an anti-vacuity test proving that the
//!   SAME defect text without the attribute is still visible and still fires.
//!   Otherwise this would be a way to smuggle a real regression past a ratchet
//!   by dressing it up as a mutation leg.
//!
//! Included by `#[path]` from each ratchet file, so the three copies of
//! `code_mask` in this directory cannot drift apart on this rule.

/// Where a candidate cfg attribute opens. Every `#[cfg(...)]` is inspected;
/// whether it is actually masked is decided by `predicate_is_coreproof_only`
/// below, not by this literal — a single-feature-string prefix match couldn't
/// see the `any(feature = "coreproof_mut_masked_lock", feature =
/// "coreproof_mut_masked_lock_bare")` combinator form (added alongside PR
/// #645's M7 rung) or the differently-named `coreproof_component_c` driver,
/// and both are exactly the code this exemption exists to hide from a
/// text-based ratchet.
const CFG_ATTR_OPEN: &str = "#[cfg(";

/// True when a `#[cfg(...)]` predicate is built solely from positive
/// `feature = "coreproof_*"` atoms joined by `all(...)`/`any(...)`. Failing
/// closed on every other atom makes `not (...)`, a target predicate, and a
/// mixed feature predicate visible even when whitespace or nesting changes.
/// Plain `coreproof` is deliberately not an exempt feature; see the
/// module-level narrowness contract above.
fn predicate_is_coreproof_only(predicate: &str) -> bool {
    let predicate = predicate.trim();
    if let Some(after_feature) = predicate.strip_prefix("feature") {
        let Some(after_eq) = after_feature.trim_start().strip_prefix('=') else {
            return false;
        };
        let Some(after_quote) = after_eq.trim_start().strip_prefix('"') else {
            return false;
        };
        let Some(name_end) = after_quote.find('"') else {
            return false;
        };
        return after_quote[..name_end].starts_with("coreproof_")
            && after_quote[name_end + 1..].trim().is_empty();
    }

    for connective in ["all", "any"] {
        let Some(after_connective) = predicate.strip_prefix(connective) else {
            continue;
        };
        let Some(arguments) = after_connective
            .trim_start()
            .strip_prefix('(')
            .and_then(|arguments| arguments.strip_suffix(')'))
        else {
            return false;
        };
        let mut depth = 0usize;
        let mut start = 0usize;
        for (index, byte) in arguments.bytes().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' if depth != 0 => depth -= 1,
                b')' => return false,
                b',' if depth == 0 => {
                    let argument = arguments[start..index].trim();
                    if argument.is_empty() || !predicate_is_coreproof_only(argument) {
                        return false;
                    }
                    start = index + 1;
                }
                _ => {}
            }
        }
        if depth != 0 {
            return false;
        }
        let last = arguments[start..].trim();
        return !last.is_empty() && predicate_is_coreproof_only(last);
    }

    false
}

/// Spans covered by a coreproof-only cfg attribute and the construct it
/// governs. `mask` must be the RAW code mask (comments and strings only) —
/// passing an already-exempted mask would always answer "nowhere".
pub fn mutation_gated_spans(source: &str, mask: &[bool]) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut search = 0usize;
    while let Some(found) = source[search..].find(CFG_ATTR_OPEN) {
        let start = search + found;
        search = start + CFG_ATTR_OPEN.len();
        if !mask.get(start).copied().unwrap_or(false) {
            continue;
        }
        // Balanced-paren scan from the `(` right after `#[cfg`, so a nested
        // combinator (`any(...)`, `not(...)`) inside the predicate doesn't
        // end the search at its own first `)`.
        let predicate_start = start + CFG_ATTR_OPEN.len();
        let mut paren_depth = 1i32;
        let mut cursor = predicate_start;
        let predicate_end = loop {
            if cursor >= bytes.len() {
                break None;
            }
            if mask.get(cursor).copied().unwrap_or(false) {
                match bytes[cursor] {
                    b'(' => paren_depth += 1,
                    b')' => {
                        paren_depth -= 1;
                        if paren_depth == 0 {
                            break Some(cursor);
                        }
                    }
                    _ => {}
                }
            }
            cursor += 1;
        };
        let Some(predicate_end) = predicate_end else {
            continue;
        };
        // The attribute must close immediately after the predicate: `)]`.
        if bytes.get(predicate_end + 1) != Some(&b']') {
            continue;
        }
        if !predicate_is_coreproof_only(&source[predicate_start..predicate_end]) {
            continue;
        }
        let attr_end = predicate_end + 2;
        let mut index = attr_end;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        // Follow exactly one Rust construct. A cfg may govern a plain
        // semicolon-terminated statement, a naked block, or a braced item whose
        // opening `{` comes after a header (`pub fn`, `pub struct`, `impl`,
        // `macro_rules!`, and so on). The old scanner recognized a block only
        // when `{` was the first byte after the attribute. For a braced item it
        // therefore walked past the item's balanced `}` looking for a later
        // top-level `;`, masking unrelated declarations along the way.
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;
        let mut cursor = index;
        let end = loop {
            if cursor >= bytes.len() {
                break cursor;
            }
            if mask.get(cursor).copied().unwrap_or(false) {
                match bytes[cursor] {
                    b'(' => paren_depth += 1,
                    b')' => paren_depth = paren_depth.saturating_sub(1),
                    b'[' => bracket_depth += 1,
                    b']' => bracket_depth = bracket_depth.saturating_sub(1),
                    b'{' => brace_depth += 1,
                    b'}' if brace_depth != 0 => {
                        brace_depth -= 1;
                        if brace_depth == 0 && paren_depth == 0 && bracket_depth == 0 {
                            let block_end = cursor + 1;
                            let mut next = block_end;
                            while next < bytes.len()
                                && (bytes[next].is_ascii_whitespace()
                                    || !mask.get(next).copied().unwrap_or(false))
                            {
                                next += 1;
                            }

                            // A braced expression can still be part of the
                            // same statement (`if … {} else {}`, `Foo {}?`, or
                            // `Foo {}.method()`). Keep scanning only for those
                            // explicit continuations. Otherwise the balanced
                            // close is the end of the one governed construct.
                            if bytes.get(next) == Some(&b';') {
                                break next + 1;
                            }
                            let next_tail = &source[next..];
                            let identifier_boundary = |length: usize| {
                                bytes.get(next + length).is_none_or(|byte| {
                                    *byte != b'_' && !byte.is_ascii_alphanumeric()
                                })
                            };
                            let keyword_continuation = (next_tail.starts_with("else")
                                && identifier_boundary(4))
                                || (next_tail.starts_with("as") && identifier_boundary(2));
                            let punctuation_continuation = bytes.get(next).is_some_and(|byte| {
                                matches!(
                                    byte,
                                    b'.' | b'?'
                                        | b'+'
                                        | b'-'
                                        | b'*'
                                        | b'/'
                                        | b'%'
                                        | b'&'
                                        | b'|'
                                        | b'^'
                                        | b'<'
                                        | b'>'
                                        | b'='
                                        | b'!'
                                )
                            });
                            if !keyword_continuation && !punctuation_continuation {
                                break block_end;
                            }
                        }
                    }
                    b';' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                        break cursor + 1;
                    }
                    _ => {}
                }
            }
            cursor += 1;
        };
        spans.push((start, end));
    }
    spans
}

/// Clear `mask` over every compiled-out mutation leg in `source`.
pub fn apply(source: &str, mask: &mut [bool]) {
    let raw: Vec<bool> = mask.to_vec();
    for (start, end) in mutation_gated_spans(source, &raw) {
        for byte in mask.iter_mut().take(end.min(source.len())).skip(start) {
            *byte = false;
        }
    }
}
