//! The harness's site census, kept non-vacuous by construction.
//!
//! `sites_visited == sites_declared` is the pilot's anti-vacuity gate, and it is
//! only worth anything if both numbers are derived rather than written down. Two
//! ways that can quietly stop being true, both of which this file forbids:
//!
//!   * a `SiteId` variant that is declared but never PLACED — no `proof_point!`
//!     invocation names it — can never be visited, so a boot that reaches every
//!     real seam still fails the gate, and the pressure is then to lower the
//!     gate rather than place the site. (The inverse of the P3-B2 lesson: there,
//!     test wiring compiled but never executed, and running it exposed two real
//!     kernel bugs.)
//!   * a variant missing from `ALL`, or from `name()`, silently shrinks
//!     `DECLARED` and makes the gate's comparison pass while covering less.
//!
//! The checks are shape-based. Nothing here spells a site name or a site count:
//! the expected set is read out of the `SiteId` declaration itself, so adding a
//! thirteenth site requires no edit to this file. That is the census discipline
//! this campaign has had to relearn three times (#549, #551, #527-r1).
//!
//! It also pins the one safety rule the class annotation encodes: a `Masked`
//! site's seam sits inside a critical section that holds the scheduler lock with
//! interrupts masked, so a yield or a forced reschedule from there is a
//! harness-authored deadlock rather than a finding. Every seam placed inside
//! `scheduler.rs` must therefore be classified `Masked`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn sites_source() -> String {
    read("kernel/src/proof/sites.rs")
}

/// The variants of `enum SiteId`, read from the declaration.
fn declared_variants() -> Vec<String> {
    let source = sites_source();
    let start = source
        .find("pub enum SiteId {")
        .expect("kernel/src/proof/sites.rs declares `pub enum SiteId`");
    let body_start = start + "pub enum SiteId {".len();
    let body_end = body_start
        + source[body_start..]
            .find('}')
            .expect("the SiteId declaration is closed");
    let mut variants = Vec::new();
    for line in source[body_start..body_end].lines() {
        let trimmed = line.trim().trim_end_matches(',');
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        if trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            variants.push(trimmed.to_string());
        }
    }
    variants
}

/// Every `proof_point!(Variant)` invocation anywhere under `kernel/src`.
fn placed_variants() -> BTreeSet<String> {
    let mut placed = BTreeSet::new();
    walk(&repo_root().join("kernel/src"), &mut |path| {
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            return;
        }
        let text = fs::read_to_string(path).unwrap_or_default();
        collect_invocations(&text, &mut placed);
    });
    placed
}

/// `proof_point!` invocations inside the scheduler, whose seams all sit in
/// masked critical sections.
fn scheduler_placed_variants() -> BTreeSet<String> {
    let mut placed = BTreeSet::new();
    collect_invocations(&read("kernel/src/task/scheduler.rs"), &mut placed);
    placed
}

/// Collect `proof_point!(Site);` invocations — the STATEMENT form only.
///
/// The macro's own doc comment in `lib.rs` writes `proof_point!(SITE)` to
/// explain itself, and a comment is not a placement. Requiring the terminating
/// `);` distinguishes the two without needing a comment parser, and a real seam
/// is always a statement.
fn collect_invocations(text: &str, out: &mut BTreeSet<String>) {
    for (index, _) in text.match_indices("proof_point!(") {
        let rest = &text[index + "proof_point!(".len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end > 0 && rest[end..].starts_with(");") {
            out.insert(rest[..end].to_string());
        }
    }
}

fn walk(dir: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, visit);
        } else {
            visit(&path);
        }
    }
}

#[test]
fn every_declared_site_is_listed_and_named() {
    let variants = declared_variants();
    assert!(
        variants.len() >= 2,
        "the SiteId declaration parsed to {} variant(s); the parser has drifted \
         off the source it reads",
        variants.len()
    );
    let source = sites_source();
    for variant in &variants {
        assert!(
            source.contains(&format!("SiteId::{variant},")),
            "site {variant} is declared but is missing from ALL, so DECLARED \
             undercounts it and the anti-vacuity gate covers less than it claims"
        );
        assert!(
            source.contains(&format!("Self::{variant} =>")),
            "site {variant} has no arm in name(), so a violation record cannot \
             name it"
        );
        assert!(
            source.contains(&format!("Self::{variant}\n"))
                || source.contains(&format!("Self::{variant} "))
                || source.contains(&format!("Self::{variant}=>")),
            "site {variant} has no arm in class(), so its admissible actions are \
             unstated"
        );
    }
}

#[test]
fn every_declared_site_is_actually_placed() {
    let declared: BTreeSet<String> = declared_variants().into_iter().collect();
    let placed = placed_variants();
    let unplaced: Vec<_> = declared.difference(&placed).collect();
    assert!(
        unplaced.is_empty(),
        "these sites are declared but no proof_point! invocation names them, so \
         they can never be visited and sites_visited can never reach \
         sites_declared: {unplaced:?}"
    );
}

#[test]
fn every_placed_site_is_declared() {
    let declared: BTreeSet<String> = declared_variants().into_iter().collect();
    let placed = placed_variants();
    let undeclared: Vec<_> = placed.difference(&declared).collect();
    assert!(
        undeclared.is_empty(),
        "these proof_point! invocations name a site the census does not declare: \
         {undeclared:?}"
    );
}

#[test]
fn every_scheduler_seam_is_classified_masked() {
    let source = sites_source();
    let open_body = {
        let start = source
            .find("SiteClass::Open")
            .expect("class() has an Open arm");
        // The Open arm's match pattern is everything between the previous `=>`
        // boundary and this one.
        let head = &source[..start];
        let arm_start = head.rfind("=>").map(|i| i + 2).unwrap_or(0);
        head[arm_start..].to_string()
    };
    for variant in scheduler_placed_variants() {
        assert!(
            !open_body.contains(&format!("Self::{variant}")),
            "site {variant} is placed inside the scheduler's masked critical \
             sections but is classified Open, which would admit a yield or a \
             forced reschedule from a window that holds the scheduler lock with \
             interrupts masked — a harness-authored deadlock, not a finding"
        );
    }
}
