//! The planted-defect register, kept honest by census rather than by review.
//!
//! The harness's validation set is six known-fixed defects re-introduced one at
//! a time behind a cargo feature. That set is described in three places that can
//! drift apart independently:
//!
//!   * `kernel/Cargo.toml` — the features that can be turned on,
//!   * `kernel/src/proof/mutations.rs` — the register naming each one's issue,
//!     its fixing PR, the file it perturbs and the predicate expected to fire,
//!   * the `#[cfg(feature = "coreproof_mut_…")]` attributes at the real sites.
//!
//! A mutation that exists in Cargo.toml but perturbs nothing is a validation leg
//! that silently passes; a site that is cfg'd on a feature nobody declared does
//! not compile but also does not appear in any report. This test pins the three
//! as ONE set — equal in both directions — so adding a mutation is a three-place
//! edit or it is a red, and removing one cannot leave a stale entry behind.
//!
//! It is a census, not a list: the expected names are read out of Cargo.toml,
//! never spelled here. Adding a seventh mutation requires no edit to this file.
//! (Pinning literal name lists in a ratchet is the mistake this campaign has
//! made three times — #549, #551, #527-r1 — and it is not repeated.)

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

/// Every `coreproof_mut_*` feature declared in the kernel manifest's `[features]`
/// table, read from the left-hand side of each declaration.
fn declared_features() -> BTreeSet<String> {
    let manifest = read("kernel/Cargo.toml");
    let mut in_features = false;
    let mut found = BTreeSet::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_features = trimmed == "[features]";
            continue;
        }
        if !in_features {
            continue;
        }
        let Some((name, _)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.starts_with(MUTATION_PREFIX) {
            found.insert(name.to_string());
        }
    }
    found
}

/// Every mutation named by the in-kernel register.
fn registered_features() -> BTreeSet<String> {
    let register = read("kernel/src/proof/mutations.rs");
    scan_for_feature_names(&register)
}

/// Every mutation a `#[cfg(feature = "…")]` attribute anywhere under
/// `kernel/src` actually gates code on — excluding the register itself, which
/// only names them as data.
fn gated_features() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let src = repo_root().join("kernel/src");
    let register = src.join("proof/mutations.rs");
    walk(&src, &mut |path| {
        if path == register || path.extension().and_then(|e| e.to_str()) != Some("rs") {
            return;
        }
        let text = fs::read_to_string(path).unwrap_or_default();
        for (index, _) in text.match_indices("feature = \"") {
            let rest = &text[index + "feature = \"".len()..];
            if let Some(end) = rest.find('"') {
                let name = &rest[..end];
                if name.starts_with(MUTATION_PREFIX) {
                    found.insert(name.to_string());
                }
            }
        }
    });
    found
}

fn scan_for_feature_names(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(index) = rest.find(MUTATION_PREFIX) {
        let candidate = &rest[index..];
        let end = candidate
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(candidate.len());
        let name = &candidate[..end];
        // The register's own prose names the PREFIX when it explains the census.
        // A bare prefix is not a feature, and admitting it would make the
        // two-way comparison below permanently unequal.
        if name.len() > MUTATION_PREFIX.len() {
            found.insert(name.to_string());
        }
        rest = &candidate[end..];
    }
    found
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
fn every_declared_mutation_perturbs_a_real_site() {
    let declared = declared_features();
    assert!(
        !declared.is_empty(),
        "no coreproof_mut_* feature is declared, so this census proves nothing"
    );
    let gated = gated_features();
    let inert: Vec<_> = declared.difference(&gated).collect();
    assert!(
        inert.is_empty(),
        "these mutations can be enabled but gate no code, so their validation leg \
         would pass without perturbing anything: {inert:?}"
    );
}

#[test]
fn every_gated_mutation_is_declared() {
    let declared = declared_features();
    let gated = gated_features();
    let undeclared: Vec<_> = gated.difference(&declared).collect();
    assert!(
        undeclared.is_empty(),
        "these cfg attributes name a feature the manifest does not declare, so \
         nothing can ever turn them on: {undeclared:?}"
    );
}

#[test]
fn the_register_names_exactly_the_declared_mutations() {
    let declared = declared_features();
    let registered = registered_features();
    assert_eq!(
        declared, registered,
        "the manifest and kernel/src/proof/mutations.rs disagree about which \
         defects are planted; the register is what the run records cite, so a \
         drift there mislabels every validation result"
    );
}

/// Each register entry must carry the issue it re-introduces, the PR that fixed
/// it, and the predicate expected to fire — a mutation whose expected outcome is
/// unrecorded cannot be adjudicated, only argued about.
#[test]
fn every_register_entry_cites_its_issue_its_fix_and_its_predicate() {
    let register = read("kernel/src/proof/mutations.rs");
    let count = registered_features().len();
    for field in ["issue:", "fixed_by:", "site:", "predicate:"] {
        let occurrences = register.matches(field).count();
        assert!(
            occurrences >= count,
            "the register has {count} mutation(s) but only {occurrences} `{field}` \
             field(s); every entry must cite all four"
        );
    }
    for issue in ["#647", "#645", "#653", "#589", "#584", "#609"] {
        assert!(
            register.contains(issue),
            "the register does not cite {issue}, which the pilot's pass bar names \
             as one of its six planted defects"
        );
    }
}
