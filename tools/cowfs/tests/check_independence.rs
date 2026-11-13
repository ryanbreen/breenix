//! cowfs-check is written from docs/fs/cowfs-format.md alone. Its sources may share the
//! CRC-32C and SipHash primitives with the writer. This test refuses a checker source that
//! names a codec, B-tree or formatter path.

use std::fs;
use std::path::{Path, PathBuf};

fn checker_sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("src/bin/cowfs-check.rs")];
    let mut dirs = vec![root.join("src/check")];
    while let Some(dir) = dirs.pop() {
        let entries = fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The line with any `//` comment removed, so prose naming the format document is allowed.
fn code_of(line: &str) -> &str {
    match line.find("//") {
        Some(at) => &line[..at],
        None => line,
    }
}

#[test]
fn checker_sources_use_only_shared_primitives() {
    let files = checker_sources();
    assert!(files.len() >= 2, "checker sources missing: {files:?}");
    let mut violations = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        for (n, line) in text.lines().enumerate() {
            let code = code_of(line);
            let at = format!("{}:{}", file.display(), n + 1);
            for banned in ["format::", "btree::", "mkfs", "include!", "include_str!", "include_bytes!"] {
                if code.contains(banned) {
                    violations.push(format!("{at}: names `{banned}`: {line}"));
                }
            }
            let mut rest = code;
            while let Some(i) = rest.find("cowfs::") {
                let after = &rest[i + "cowfs::".len()..];
                if !(after.starts_with("crc32c") || after.starts_with("siphash")) {
                    violations.push(format!("{at}: cowfs path other than crc32c or siphash: {line}"));
                }
                rest = after;
            }
            if code.contains("#[path") {
                let target = code.split('"').nth(1).unwrap_or("");
                let inside_check = target == "../check/mod.rs"
                    || (!target.contains("..") && !target.starts_with('/'));
                if !inside_check {
                    violations.push(format!("{at}: module path leaves src/check: {line}"));
                }
            }
        }
    }
    assert!(violations.is_empty(), "checker reaches beyond the shared primitives:\n{}", violations.join("\n"));
}
