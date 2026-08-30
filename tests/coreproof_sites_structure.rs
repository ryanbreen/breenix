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
//! still enforced exactly. Component C's `ScheduleEntry` and `PreDispatchMask`
//! seams are both placed in `scheduler::schedule()` BEFORE that function ever
//! reaches the lock, so they are deliberately `Open` — documented, checked
//! exceptions rather than silently-skipped ones. The distinguishing fact is
//! each placement, not its block's cardinality: a scheduler seam inside
//! `impl Scheduler { .. }`, or
//! after a lock-taking construct in its own free function, must be `Masked`; a
//! seam genuinely before any lock in its own free function must be `Open`.
//! That handles Component C's now-multi-site block without forcing either of
//! its two genuinely `Open` scheduler seams to lie about themselves. Every placed
//! variant is checked even when its block has no `Open` arm at all, so absence
//! of that arm cannot skip the block and make the assertion vacuous (B3).

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
        // A floor of 2, not just non-empty: `main` asserted `>= 2` here (a
        // parser that silently stopped finding variants would still pass a
        // bare non-empty check on a block that used to have many). Rung 2's
        // first cut weakened this to `!is_empty()` to accommodate a
        // one-variant Component C block; this rung's own B1/M4a fixes give
        // Component C three variants, so the original floor is restored
        // rather than carried as a permanent weakening (rung 2 review, m3).
        assert!(
            variants.len() >= 2,
            "a SiteId block parsed to {} variant(s); the parser has drifted off \
             the source it reads (or a block has shrunk to a single seam, which \
             needs this floor revisited deliberately, not silently)",
            variants.len()
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

/// Whether a byte offset within `scheduler.rs`'s own source text sits inside a region that
/// runs with the scheduler lock already held.
///
/// This is the placement fact `every_scheduler_seam_is_classified_correctly` keys on now
/// (rung 2 review, M1) — not block cardinality, which has no causal relation to where a seam
/// actually lives and would wrongly force a future single-seam component's genuinely masked
/// placement to lie about itself as `Open` (or, as happened here, wrongly force a genuinely
/// `Open` multi-seam component's placements to lie about themselves as `Masked`). Two
/// structural signals, checked in order:
///
///   1. Every `impl Scheduler { .. }` method runs with the lock already held by ITS CALLER —
///      no method inside that block ever takes the lock itself; that is this codebase's own
///      calling convention (`Scheduler`'s methods are always reached via
///      `lock_scheduler()`/`without_interrupts` at the CALL site, never inside the method). A
///      call site lexically inside that block's own byte range is therefore Masked,
///      unconditionally.
///   2. A call site in a free (module-level) function is Masked only if a lock-taking
///      construct (`lock_scheduler(`, `try_lock_scheduler(`, or `without_interrupts(`) appears
///      textually earlier in THAT SAME function's own source than the call site does;
///      otherwise it precedes any lock the function ever takes, and is Open.
fn scheduler_seam_is_masked(source: &str, call_offset: usize) -> bool {
    let impl_marker = "\nimpl Scheduler {";
    let impl_start = source
        .find(impl_marker)
        .expect("scheduler.rs no longer declares `impl Scheduler { .. }`; the parser has drifted off the source it reads");
    // The block's own closing brace is the first line consisting of a bare `}` at column 0
    // after the block starts — every OTHER close inside it is indented (rustfmt's own
    // convention), so this cannot mistake a nested block's close for the impl's own.
    let close_rel = source[impl_start..]
        .find("\n}\n")
        .expect("`impl Scheduler { .. }` has no column-0 closing brace; the parser has drifted off the source it reads");
    let impl_end = impl_start + close_rel;
    if call_offset > impl_start && call_offset < impl_end {
        return true;
    }

    let head = &source[..call_offset];
    let fn_start = ["\nfn ", "\npub fn "]
        .iter()
        .filter_map(|marker| head.rfind(marker))
        .max()
        .unwrap_or(0);
    let body = &source[fn_start..call_offset];
    body.contains("lock_scheduler(")
        || body.contains("try_lock_scheduler(")
        || body.contains("without_interrupts(")
}

#[test]
fn every_scheduler_seam_is_classified_correctly() {
    let scheduler_source = read("kernel/src/task/scheduler.rs");
    let scheduler_placed = scheduler_placed_variants();
    for block in blocks() {
        let variants = declared_variants_in(&block);
        // The Open arm's own pattern text, used below to check that a variant this rung
        // requires to be `Open` is genuinely spelled inside a `SiteClass::Open` arm (not
        // merely absent from every OTHER arm — string containment, not exclusion, is the
        // actual check). A block with no `SiteClass::Open` arm at all yields an empty body
        // here rather than skipping the block (rung 2 review, B3): a variant this rung
        // requires to be `Open` then correctly fails the `assert!` below instead of having
        // its whole block silently skipped.
        let open_body: String = match block.find("SiteClass::Open") {
            Some(start) => {
                let head = &block[..start];
                // The Open arm's own pattern sits BETWEEN the previous arm's `=>` and this
                // arm's own `=>` — two arrows back from "SiteClass::Open", not one:
                // `head.rfind("=>")` alone lands on THIS arm's own arrow, whose head[..]
                // slice then ends exactly at "SiteClass::Open" and never actually contains
                // this arm's pattern names at all. Walking back a second arrow (or to
                // `match self {` for a first arm) is what makes this a real containment
                // check instead of a whitespace-only string that renders every assertion
                // below vacuously true.
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
            }
            None => String::new(),
        };

        for variant in variants
            .iter()
            .filter(|variant| scheduler_placed.contains(*variant))
        {
            let marker = format!("proof_point!({variant});");
            let call_offset = scheduler_source.find(&marker).unwrap_or_else(|| {
                panic!(
                    "{variant} is placed in scheduler.rs per proof_point! invocation \
                     scanning, but no `proof_point!({variant});` statement text was \
                     found — the parser has drifted off the source it reads"
                )
            });
            let must_be_masked = scheduler_seam_is_masked(&scheduler_source, call_offset);
            let is_open = open_body.contains(&format!("Self::{variant}"));
            if must_be_masked {
                assert!(
                    !is_open,
                    "site {variant} is placed inside the scheduler's masked critical \
                     sections (inside `impl Scheduler {{ .. }}`, or after a lock-taking \
                     construct in its own free function) but is classified Open, which \
                     would admit a yield or a forced reschedule from a window that holds \
                     the scheduler lock with interrupts masked — a harness-authored \
                     deadlock, not a finding"
                );
            } else {
                assert!(
                    is_open,
                    "site {variant} is placed in scheduler.rs BEFORE any lock-taking \
                     construct in its own free function (or outside `impl Scheduler {{ \
                     .. }}` entirely) but is not classified Open — a genuinely pre-lock \
                     seam must say so, not merely avoid saying it holds the lock"
                );
            }
        }
    }
}
