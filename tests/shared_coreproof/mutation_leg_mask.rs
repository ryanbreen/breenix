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
//! * only the POSITIVE attribute `#[cfg(feature = "coreproof_mut_…")]` is
//!   masked. The `#[cfg(not(feature = "coreproof_mut_…"))]` arm guards the
//!   PRODUCTION code these ratchets police and stays fully visible, so every
//!   law still applies to the code that ships;
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

/// The attribute that introduces a deliberately-compiled-out mutation leg.
pub const MUTATION_ATTR: &str = "#[cfg(feature = \"coreproof_mut_";

/// Spans covered by a `coreproof_mut_*` cfg attribute and the construct it
/// governs. `mask` must be the RAW code mask (comments and strings only) —
/// passing an already-exempted mask would always answer "nowhere".
pub fn mutation_gated_spans(source: &str, mask: &[bool]) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut search = 0usize;
    while let Some(found) = source[search..].find(MUTATION_ATTR) {
        let start = search + found;
        search = start + MUTATION_ATTR.len();
        if !mask.get(start).copied().unwrap_or(false) {
            continue;
        }
        let Some(attr_end) = source[start..].find(']').map(|offset| start + offset + 1) else {
            continue;
        };
        let mut index = attr_end;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let end = if bytes.get(index) == Some(&b'{') {
            let mut depth = 0usize;
            let mut cursor = index;
            loop {
                if cursor >= bytes.len() {
                    break cursor;
                }
                if mask.get(cursor).copied().unwrap_or(false) {
                    match bytes[cursor] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                break cursor + 1;
                            }
                        }
                        _ => {}
                    }
                }
                cursor += 1;
            }
        } else {
            let mut depth = 0isize;
            let mut cursor = index;
            loop {
                if cursor >= bytes.len() {
                    break cursor;
                }
                if mask.get(cursor).copied().unwrap_or(false) {
                    match bytes[cursor] {
                        b'{' | b'(' | b'[' => depth += 1,
                        b'}' | b')' | b']' => depth -= 1,
                        b';' if depth <= 0 => break cursor + 1,
                        _ => {}
                    }
                }
                cursor += 1;
            }
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
