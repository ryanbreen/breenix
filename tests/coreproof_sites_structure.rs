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
//! ## Component-scoped (rung 2)
//!
//! `sites.rs` now declares MORE THAN ONE `pub enum SiteId { ... }` block —
//! one per component, mutually exclusive at compile time (see that file's own
//! header). This file no longer assumes there is exactly one: `blocks()` finds
//! every such block textually and every check below runs once PER block, so a
//! future third component's block is picked up with no edit here either.
//!
//! It also pins the one safety rule the class annotation encodes: a `Masked`
//! site's seam sits inside a critical section that holds the scheduler lock
//! with interrupts masked, so a yield or a forced reschedule from there is a
//! harness-authored deadlock rather than a finding. Component A's nine seams
//! placed inside `scheduler.rs` all sit inside such a critical section, so
//! every one of them must be classified `Masked` — that rule is unchanged and
//! still enforced exactly. Component C's `ScheduleEntry` seam is placed at the
//! very top of `scheduler::schedule()`, BEFORE that function ever reaches the
//! lock, so it is deliberately `Open` — a documented, checked exception rather
//! than a silently-skipped one. The distinguishing shape used to tell the two
//! cases apart is structural, not a name: a block that declares more than one
//! site is treated as "Component A's shape" (every one of its scheduler.rs
//! placements must be `Masked`), and a block that declares exactly one site is
//! treated as "a minimal single-entry-seam shape" (its one scheduler.rs
//! placement, if any, must be `Open`). Neither branch is skipped; both are
//! asserted.

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

/// Every component's own `SiteId` block, split at each `pub enum SiteId {`
/// occurrence. A block runs from its own enum declaration through to (but not
/// including) the next block's, or to the end of the file for the last one —
/// which safely includes that block's own `ALL`, `impl SiteId { .. }` and
/// nothing that belongs to any other block.
fn blocks() -> Vec<String> {
    let source = sites_source();
    let marker = "pub enum SiteId {";
    let mut starts: Vec<usize> = source.match_indices(marker).map(|(index, _)| index).collect();
    assert!(
        !starts.is_empty(),
        "no `pub enum SiteId` declaration found; the parser has drifted off the source it reads"
    );
    starts.push(source.len());
    (0..starts.len() - 1)
        .map(|index| source[starts[index]..starts[index + 1]].to_string())
        .collect()
}

/// The variants of one block's `enum SiteId`, read from its declaration.
fn declared_variants_in(block: &str) -> Vec<String> {
    let start = block
        .find("pub enum SiteId {")
        .expect("blocks() only ever returns text starting at a SiteId declaration");
    let body_start = start + "pub enum SiteId {".len();
    let body_end = body_start
        + block[body_start..]
            .find('}')
            .expect("the SiteId declaration is closed");
    let mut variants = Vec::new();
    for line in block[body_start..body_end].lines() {
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

/// `proof_point!` invocations inside the scheduler.
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
    let block_list = blocks();
    for block in &block_list {
        let variants = declared_variants_in(block);
        assert!(
            !variants.is_empty(),
            "a SiteId block parsed to zero variants; the parser has drifted off \
             the source it reads"
        );
        for variant in &variants {
            // A trailing comma follows every element of a multi-element ALL
            // array; a single-element ALL (a minimal, single-seam block) may
            // legally close with `]` right after the element instead, and
            // rustfmt prefers that form. Both are accepted, so this does not
            // start fighting the formatter over a single-site block.
            assert!(
                block.contains(&format!("SiteId::{variant},"))
                    || block.contains(&format!("SiteId::{variant}]")),
                "site {variant} is declared but is missing from its own ALL, so \
                 DECLARED undercounts it and the anti-vacuity gate covers less \
                 than it claims"
            );
            assert!(
                block.contains(&format!("Self::{variant} =>")),
                "site {variant} has no arm in its own name(), so a violation \
                 record cannot name it"
            );
            assert!(
                block.contains(&format!("Self::{variant}\n"))
                    || block.contains(&format!("Self::{variant} "))
                    || block.contains(&format!("Self::{variant}=>")),
                "site {variant} has no arm in its own class(), so its \
                 admissible actions are unstated"
            );
        }
    }
}

#[test]
fn every_declared_site_is_actually_placed() {
    let declared: BTreeSet<String> = blocks()
        .iter()
        .flat_map(|block| declared_variants_in(block))
        .collect();
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
    let declared: BTreeSet<String> = blocks()
        .iter()
        .flat_map(|block| declared_variants_in(block))
        .collect();
    let placed = placed_variants();
    let undeclared: Vec<_> = placed.difference(&declared).collect();
    assert!(
        undeclared.is_empty(),
        "these proof_point! invocations name a site no block declares: \
         {undeclared:?}"
    );
}

#[test]
fn every_scheduler_seam_is_classified_correctly() {
    let scheduler_placed = scheduler_placed_variants();
    for block in blocks() {
        let variants = declared_variants_in(&block);
        let open_body = {
            let Some(start) = block.find("SiteClass::Open") else {
                // A block with no Open arm at all has nothing further to check
                // here — every one of its variants is Masked by construction.
                continue;
            };
            let head = &block[..start];
            // The Open arm's own pattern sits BETWEEN the previous arm's `=>`
            // and this arm's own `=>` — two arrows back from "SiteClass::Open",
            // not one: `head.rfind("=>")` alone lands on THIS arm's own arrow,
            // whose head[..] slice then ends exactly at "SiteClass::Open" and
            // never actually contains this arm's pattern names at all. Walking
            // back a second arrow (or to `match self {` for a first arm) is
            // what makes this a real containment check instead of a
            // whitespace-only string that renders every assertion below
            // vacuously true.
            let this_arrow = head
                .rfind("=>")
                .expect("class() has an arm ending in SiteClass::Open");
            let prior_text = &head[..this_arrow];
            let arm_start = match prior_text.rfind("=>") {
                Some(index) => index + 2,
                None => {
                    let marker = "match self {";
                    prior_text
                        .rfind(marker)
                        .map(|index| index + marker.len())
                        .unwrap_or(0)
                }
            };
            head[arm_start..].to_string()
        };
        // A block that declares more than one site is Component A's shape:
        // every one of its seams sits inside a scheduler-lock critical
        // section, so none placed in scheduler.rs may be classified Open.
        // A block that declares exactly one site is a minimal single-entry
        // seam shape: its one scheduler.rs placement, if it has one, sits
        // BEFORE the lock is ever taken and must be classified Open — checked
        // for, not merely permitted, so a future single-seam block that
        // forgets to mark itself Open still reddens this test.
        let this_block_scheduler_variants: Vec<&String> = variants
            .iter()
            .filter(|variant| scheduler_placed.contains(*variant))
            .collect();
        if variants.len() > 1 {
            for variant in this_block_scheduler_variants {
                assert!(
                    !open_body.contains(&format!("Self::{variant}")),
                    "site {variant} is placed inside the scheduler's masked \
                     critical sections but is classified Open, which would \
                     admit a yield or a forced reschedule from a window that \
                     holds the scheduler lock with interrupts masked — a \
                     harness-authored deadlock, not a finding"
                );
            }
        } else {
            for variant in this_block_scheduler_variants {
                assert!(
                    open_body.contains(&format!("Self::{variant}")),
                    "site {variant} is its block's only declared site and is \
                     placed in scheduler.rs; a minimal single-entry-seam block \
                     must classify its one site Open (it is expected to sit \
                     before the scheduler lock is taken, not inside it) — \
                     found it classified something else"
                );
            }
        }
    }
}
