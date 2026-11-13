//! Require isolated image violations to name only the intended invariant.

#[path = "support/violation_images.rs"]
mod images;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

fn check(path: &Path) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_cowfs-check"))
        .arg("--full")
        .arg(path)
        .output()
        .expect("run the built cowfs-check binary");
    (
        output.status.code().expect("checker exited normally"),
        format!(
            "{}{}",
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap()
        ),
    )
}

#[test]
fn mkfs_images_check_clean_full() {
    let images = images::Corpus::new();
    for path in [&images.empty, &images.populated] {
        let (code, output) = check(path);
        assert_eq!(code, 0, "{}:\n{output}", path.display());
    }
}

#[test]
fn invariant_images_name_only_the_target() {
    // There are 27 invariant IDs; C1 has separate CRC-count and data-byte cases.
    let cases = [
        ("S1", "slot-reserved-byte"),
        ("S2", "ring-copy-disagreement"),
        ("S3", "no-matching-recovery-roots"),
        ("S4", "next-root-id"),
        ("N1", "root-pointer-crc"),
        ("N2", "future-node-generation"),
        ("N3", "child-level"),
        ("N4", "leaf-key-order"),
        ("N5", "unused-node-byte"),
        ("N6", "inode-reserved-byte"),
        ("A1", "uncovered-block"),
        ("A2", "adjacent-free-extents"),
        ("A3", "node-ref-kind"),
        ("A4", "node-refcount"),
        ("A5", "unreferenced-zero-refcount"),
        ("A6", "future-pending-generation"),
        ("C1", "data-crc-count"),
        ("C1", "data-byte"),
        ("F1", "root-parent"),
        ("F2", "object-without-inode"),
        ("F3", "directory-name-hash"),
        ("F4", "directory-index-bound"),
        ("F5", "file-link-count"),
        ("F6", "directory-parent-cycle"),
        ("F7", "nonzero-file-tail"),
        ("F8", "symlink-size"),
        ("F9", "duplicate-xattr-name"),
        ("F10", "next-inode"),
    ];
    let images = images::Corpus::new();
    let mut failures = Vec::new();
    for (id, name) in cases {
        let path = images.violation(name);
        let (code, output) = check(&path);
        let ids: BTreeSet<_> = output
            .lines()
            .filter_map(|line| line.strip_prefix("error "))
            .map(|line| line.split_whitespace().next().expect("error ID"))
            .collect();
        if code != 4 || ids != BTreeSet::from([id]) {
            failures.push(format!("{name}: expected exit 4 and only error {id}; exit {code}, IDs {ids:?}:\n{output}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
