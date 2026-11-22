//! `toolchain_workspace::lock_has_only_path_packages` (plan section 2).
//!
//! The toolchain workspace takes no dependency outside itself and std, so the
//! same sources build offline on any host, the box included (25.8). This
//! reads toolchain/Cargo.lock and the member manifests and requires:
//! - each locked package has no `source` and no `checksum` (a path package);
//! - the locked packages are exactly the workspace members, so no path
//!   dependency reaches outside the workspace;
//! - no member has a build script or is a proc macro.
//!
//! It lives in the elfcheck crate because a virtual workspace manifest cannot
//! hold tests; it checks the whole workspace, not elfcheck alone.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn toolchain() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("toolchain/elfcheck has a parent")
        .to_path_buf()
}

/// The quoted strings in `members = [...]` of the workspace manifest.
fn members(manifest: &str) -> Vec<String> {
    let start = manifest.find("members").expect("toolchain/Cargo.toml declares members");
    let rest = &manifest[start..];
    let open = rest.find('[').expect("members is an array");
    let close = rest[open..].find(']').expect("members array is closed") + open;
    rest[open + 1..close]
        .split(',')
        .map(|item| item.trim().trim_matches('"').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// The value of the first `key = "value"` line in a TOML table body.
fn string_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"'))
    })
}

#[test]
fn lock_has_only_path_packages() {
    let root = toolchain();
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read toolchain/Cargo.toml");
    let lock = fs::read_to_string(root.join("Cargo.lock")).expect("toolchain/Cargo.lock is committed");
    let mut problems = Vec::new();

    let mut member_names = BTreeSet::new();
    for member in members(&manifest) {
        let dir = root.join(&member);
        let text = fs::read_to_string(dir.join("Cargo.toml"))
            .unwrap_or_else(|e| panic!("read member {member}'s Cargo.toml: {e}"));
        // Each table body follows a "\n[" (the newline added for a table on line 1).
        let text = format!("\n{text}");
        let package = text.split("\n[").find(|t| t.starts_with("package]")).unwrap_or("");
        match string_value(package, "name") {
            Some(name) => {
                member_names.insert(name.to_string());
            }
            None => problems.push(format!("member {member} names no package")),
        }
        if dir.join("build.rs").exists() || string_value(package, "build").is_some() {
            problems.push(format!("member {member} has a build script"));
        }
        if text.contains("proc-macro") {
            problems.push(format!("member {member} is or names a proc macro"));
        }
    }

    let mut locked = BTreeSet::new();
    for block in lock.split("[[package]]").skip(1) {
        let name = string_value(block, "name").unwrap_or("<unnamed>").to_string();
        for key in ["source", "checksum"] {
            if string_value(block, key).is_some() {
                problems.push(format!("locked package {name} has a {key}: it is not a path package"));
            }
        }
        locked.insert(name);
    }

    if member_names.is_empty() {
        problems.push("the workspace has no members".to_string());
    }
    if locked != member_names {
        problems.push(format!(
            "locked packages {locked:?} are not exactly the workspace members {member_names:?}"
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
