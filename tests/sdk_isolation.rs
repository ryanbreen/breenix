//! The kernel and boot never depend on the Developer SDK (docs/suites/README.md,
//! "Developer SDK"). Only the SDK suite builds it: `scripts/sdk-suite.py` runs
//! `scripts/build-sdk.sh`, and the four hooks below run the suite's host side.
//!
//! A tracked file outside the SDK's own sources and the suite's measurement files fails
//! this test if it names `build-sdk.sh` or a path of the SDK's sources or build, and a
//! file other than the hooks and the suite's own fails it if it names `sdk-suite.py`.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The SDK's sources and the suite's measurement files: these may name anything SDK.
fn is_sdk_or_suite(path: &str) -> bool {
    const PREFIXES: &[&str] = &["sdk/", "docs/sdk/", "userspace/sdk-suite/"];
    const FILES: &[&str] = &[
        "scripts/build-sdk.sh",
        "scripts/sdk-suite.py",
        "docs/suites/README.md",
        "docs/suites/sdk.json",
        "userspace/programs/src/suite_sdk.rs",
        "tests/sdk_isolation.rs",
    ];
    PREFIXES.iter().any(|p| path.starts_with(p)) || FILES.contains(&path)
}

/// The boot paths that run the suite's host side, and the manifest check that reads it.
const HOOKS: &[&str] = &[
    "run.sh",
    "scripts/boot-interactive.sh",
    "docker/qemu/run-x86-gate.sh",
    "tests/suite_manifests.rs",
];

/// Paths of the SDK's sources, its build root and its installed layout, matched only where a
/// path component starts (so `wasi-sdk/bin/` is not `sdk/bin/`).
const SDK_PATHS: &[&str] = &[
    "sdk/build/",
    "sdk/builder/",
    "sdk/bin/",
    "sdk/libexec/",
    "sdk/templates/",
    "sdk/sdk.toml",
    "breenix-sdk-build",
    "share/breenix-sdk",
];

fn names_at_component(data: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    data.windows(needle.len()).enumerate().any(|(at, window)| {
        window == needle
            && (at == 0 || {
                let before = data[at - 1];
                !(before.is_ascii_alphanumeric() || matches!(before, b'-' | b'_' | b'.'))
            })
    })
}

fn contains(data: &[u8], needle: &str) -> bool {
    data.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// Tracked files holding "sdk", which every word the checks below look for contains; the
/// checks then read each one exactly.
fn candidate_files() -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(["grep", "-l", "-z", "-F", "-e", "sdk", "--"])
        .output()
        .expect("git grep runs");
    // git grep exits 1 when nothing matches.
    assert!(out.status.code().is_some_and(|c| c <= 1), "git grep failed");
    out.stdout
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

#[test]
fn only_the_sdk_suite_reaches_the_sdk() {
    let root = repo_root();
    let mut offenders = Vec::new();
    for path in candidate_files() {
        if is_sdk_or_suite(&path) {
            continue;
        }
        let full = root.join(&path);
        if full.is_symlink() || !full.is_file() {
            continue;
        }
        let data = std::fs::read(&full).expect("tracked file reads");
        if contains(&data, "build-sdk.sh") {
            offenders.push(format!("{path} names build-sdk.sh"));
        }
        for needle in SDK_PATHS {
            if names_at_component(&data, needle) {
                offenders.push(format!("{path} names the SDK path {needle}"));
            }
        }
        if !HOOKS.contains(&path.as_str()) && contains(&data, "sdk-suite.py") {
            offenders.push(format!("{path} runs or names scripts/sdk-suite.py but is not one of its hooks"));
        }
    }
    assert!(offenders.is_empty(), "files outside the SDK suite reach the SDK:\n{}", offenders.join("\n"));
}

#[test]
fn the_component_match_ignores_longer_names() {
    assert!(names_at_component(b"run sdk/bin/x", "sdk/bin/"));
    assert!(names_at_component(b"sdk/bin/x", "sdk/bin/"));
    assert!(!names_at_component(b"/opt/wasi-sdk/bin/clang", "sdk/bin/"));
    assert!(names_at_component(b"/opt/sdk/bin/clang", "sdk/bin/"));
}

#[test]
fn sdk_host_tools() {
    let root = repo_root();
    for test in ["sdk/tests/build_root_test.py", "sdk/tests/templates_test.py", "sdk/tests/image_test.py",
                 "sdk/tests/run_disk_test.py", "tests/libc_spawn_host.py"] {
        let status = Command::new("python3").arg(root.join(test)).current_dir(&root)
            .status().expect("python3 runs");
        assert!(status.success(), "{test} failed");
    }
}
