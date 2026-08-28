//! The mutation-site coverage census, kept equal to the planted-defect register.
//!
//! A catch from a window in which the mutated region never executed is not a
//! trial. This test therefore joins the mutation register, `MutSite`, and the
//! actual `proof_cover!` placements as one census in both directions. Nothing
//! here spells a mutation name: names are read from the register and from the
//! source that declares the coverage enum, so extending the existing register
//! makes this test demand the corresponding enum member and placement.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const MUTATION_PREFIX: &str = "coreproof_mut_";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn coverage_source() -> String {
    read("kernel/src/proof/coverage.rs")
}

fn mutation_names_in_order() -> Vec<String> {
    let register = read("kernel/src/proof/mutations.rs");
    register
        .lines()
        .filter_map(|line| {
            let value = line.trim().strip_prefix("feature: \"")?;
            let feature = value.strip_suffix("\",")?;
            feature.strip_prefix(MUTATION_PREFIX).map(ToOwned::to_owned)
        })
        .collect()
}

fn enum_variants() -> Vec<String> {
    let source = coverage_source();
    let start = source
        .find("pub enum MutSite {")
        .expect("coverage.rs declares `pub enum MutSite`");
    let body_start = start + "pub enum MutSite {".len();
    let body_end = body_start
        + source[body_start..]
            .find('}')
            .expect("the MutSite declaration is closed");
    source[body_start..body_end]
        .lines()
        .filter_map(|line| {
            let variant = line.trim().trim_end_matches(',');
            (!variant.is_empty()
                && !variant.starts_with('#')
                && variant
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_'))
            .then(|| variant.to_string())
        })
        .collect()
}

fn all_variants() -> Vec<String> {
    let source = coverage_source();
    let start = source
        .find("pub const ALL:")
        .expect("MutSite declares associated ALL");
    let end = start
        + source[start..]
            .find("];\n")
            .expect("MutSite::ALL is a closed array")
        + 2;
    collect_qualified_names(&source[start..end], "Self::")
}

fn names_in_enum_order() -> Vec<String> {
    let source = coverage_source();
    enum_variants()
        .iter()
        .map(|variant| {
            let arm = format!("Self::{variant} => \"");
            let start = source
                .find(&arm)
                .unwrap_or_else(|| panic!("MutSite::{variant} has no name() arm"))
                + arm.len();
            let end = source[start..]
                .find('"')
                .unwrap_or_else(|| panic!("MutSite::{variant}'s name is not closed"));
            source[start..start + end].to_string()
        })
        .collect()
}

fn harness_side_variants() -> BTreeSet<String> {
    let source = coverage_source();
    let start = source
        .find("pub const HARNESS_SIDE:")
        .expect("coverage.rs declares HARNESS_SIDE");
    let end = start
        + source[start..]
            .find("];\n")
            .expect("HARNESS_SIDE is a closed slice")
        + 2;
    collect_qualified_names(&source[start..end], "MutSite::")
        .into_iter()
        .collect()
}

fn placed_variants(root: &Path) -> BTreeSet<String> {
    let mut placed = BTreeSet::new();
    walk(root, &mut |path| {
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            return;
        }
        let text = fs::read_to_string(path).unwrap_or_default();
        collect_statement_invocations(&text, "proof_cover!(", &mut placed);
    });
    placed
}

fn harness_counted_variants() -> BTreeSet<String> {
    let mut counted = BTreeSet::new();
    walk(&repo_root().join("kernel/src/proof"), &mut |path| {
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            return;
        }
        let text = fs::read_to_string(path).unwrap_or_default();
        collect_statement_invocations(&text, "coverage::note(MutSite::", &mut counted);
    });
    counted
}

fn collect_statement_invocations(text: &str, prefix: &str, out: &mut BTreeSet<String>) {
    for (index, _) in text.match_indices(prefix) {
        let rest = &text[index + prefix.len()..];
        let end = rest
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(rest.len());
        if end > 0 && rest[end..].starts_with(");") {
            out.insert(rest[..end].to_string());
        }
    }
}

fn collect_qualified_names(text: &str, prefix: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(prefix) {
        let candidate = &rest[index + prefix.len()..];
        let end = candidate
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(candidate.len());
        if end > 0 {
            names.push(candidate[..end].to_string());
        }
        rest = &candidate[end..];
    }
    names
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
fn coverage_census_matches_the_mutation_register_in_order() {
    let register = mutation_names_in_order();
    assert!(
        !register.is_empty(),
        "the mutation register parsed to an empty census"
    );
    let variants = enum_variants();
    assert_eq!(
        variants,
        all_variants(),
        "MutSite and MutSite::ALL differ or are ordered differently"
    );
    assert_eq!(
        names_in_enum_order(),
        register,
        "MutSite::name() and mutations::REGISTER differ or are ordered differently"
    );
}

#[test]
fn every_non_harness_site_has_a_real_coverage_placement() {
    let declared: BTreeSet<_> = enum_variants().into_iter().collect();
    let harness_side = harness_side_variants();
    let placed = placed_variants(&repo_root().join("kernel/src"));
    let expected: BTreeSet<_> = declared.difference(&harness_side).cloned().collect();
    let missing: Vec<_> = expected.difference(&placed).collect();
    assert!(
        missing.is_empty(),
        "these non-harness mutation sites have no proof_cover! placement: {missing:?}"
    );
}

#[test]
fn every_coverage_placement_names_a_declared_site() {
    let declared: BTreeSet<_> = enum_variants().into_iter().collect();
    let placed = placed_variants(&repo_root().join("kernel/src"));
    let unknown: Vec<_> = placed.difference(&declared).collect();
    assert!(
        unknown.is_empty(),
        "these proof_cover! placements name no declared MutSite: {unknown:?}"
    );
}

#[test]
fn every_harness_side_exception_is_counted_inside_the_harness() {
    let harness_side = harness_side_variants();
    let counted = harness_counted_variants();
    let missing: Vec<_> = harness_side.difference(&counted).collect();
    assert!(
        missing.is_empty(),
        "these HARNESS_SIDE sites have no coverage::note call under kernel/src/proof: {missing:?}"
    );
}
