use std::fs;
use std::path::{Path, PathBuf};

/// Adding a driver completion wait is expected to force explicit review of this
/// number rather than allowing a new interruptible wait to land silently.
const DRIVER_COMPLETION_WAIT_POPULATION: usize = 8;
const INTERRUPTIBLE_WAIT: &str = ".wait_timeout(";
const UNINTERRUPTIBLE_WAIT: &str = ".wait_timeout_uninterruptible(";
const BLOCK_EINTR_ORACLE_PREFIX: &str = "[BLOCK_EINTR_ORACLE:";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|_| panic!("read repository file {relative}"))
}

fn code_mask(source: &str) -> Vec<bool> {
    let bytes = source.as_bytes();
    let mut mask = vec![true; bytes.len()];
    let mut line_comment = false;
    let mut block_comment_depth = 0usize;
    let mut string = false;
    let mut character = false;
    let mut raw_string_hashes = None;
    let mut escaped = false;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if line_comment {
            if byte == b'\n' {
                line_comment = false;
            } else {
                mask[index] = false;
            }
            index += 1;
            continue;
        }
        if block_comment_depth != 0 {
            mask[index] = false;
            if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
                mask[index + 1] = false;
                block_comment_depth += 1;
                index += 2;
            } else if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                mask[index + 1] = false;
                block_comment_depth -= 1;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some(hashes) = raw_string_hashes {
            mask[index] = false;
            if byte == b'"'
                && bytes
                    .get(index + 1..index + 1 + hashes)
                    .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
            {
                mask[index + 1..=index + hashes].fill(false);
                raw_string_hashes = None;
                index += hashes + 1;
            }
            index += 1;
            continue;
        }
        if string || character {
            mask[index] = false;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if (string && byte == b'"') || (character && byte == b'\'') {
                string = false;
                character = false;
            }
            index += 1;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            mask[index] = false;
            mask[index + 1] = false;
            line_comment = true;
            index += 2;
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            mask[index] = false;
            mask[index + 1] = false;
            block_comment_depth = 1;
            index += 2;
            continue;
        }
        if byte == b'r' {
            let mut quote = index + 1;
            while bytes.get(quote) == Some(&b'#') {
                quote += 1;
            }
            if bytes.get(quote) == Some(&b'"') {
                mask[index..=quote].fill(false);
                raw_string_hashes = Some(quote - index - 1);
                index = quote + 1;
                continue;
            }
        }
        if byte == b'"' {
            mask[index] = false;
            string = true;
            index += 1;
            continue;
        }
        if byte == b'\'' {
            let plain_char = bytes.get(index + 2) == Some(&b'\'');
            let escaped_char =
                bytes.get(index + 1) == Some(&b'\\') && bytes.get(index + 3) == Some(&b'\'');
            if plain_char || escaped_char {
                mask[index] = false;
                character = true;
            }
        }
        index += 1;
    }
    mask
}

fn code_occurrences(source: &str, needle: &str) -> Vec<usize> {
    let mask = code_mask(source);
    source
        .match_indices(needle)
        .filter_map(|(offset, _)| {
            mask[offset..offset + needle.len()]
                .iter()
                .all(|is_code| *is_code)
                .then_some(offset)
        })
        .collect()
}

fn line_number(source: &str, offset: usize) -> usize {
    source[..offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn discover_files(root: &Path, extension: &str) -> Vec<PathBuf> {
    fn visit(directory: &Path, extension: &str, files: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(directory)
            .unwrap_or_else(|_| panic!("read repository directory {}", directory.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|_| panic!("read entry in {}", directory.display()))
                .path();
            if path.is_dir() {
                visit(&path, extension, files);
            } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, extension, &mut files);
    files.sort();
    files
}

#[derive(Default)]
struct CompletionWaitCensus {
    interruptible: usize,
    uninterruptible: usize,
    interruptible_sites: Vec<String>,
}

impl CompletionWaitCensus {
    fn total(&self) -> usize {
        self.interruptible + self.uninterruptible
    }
}

fn census_source(label: &str, source: &str) -> CompletionWaitCensus {
    let interruptible_offsets = code_occurrences(source, INTERRUPTIBLE_WAIT);
    CompletionWaitCensus {
        interruptible: interruptible_offsets.len(),
        uninterruptible: code_occurrences(source, UNINTERRUPTIBLE_WAIT).len(),
        interruptible_sites: interruptible_offsets
            .into_iter()
            .map(|offset| format!("{label}:{}", line_number(source, offset)))
            .collect(),
    }
}

fn driver_completion_wait_census() -> (CompletionWaitCensus, usize) {
    let drivers_root = repo_root().join("kernel/src/drivers");
    let files = discover_files(&drivers_root, "rs");
    let mut census = CompletionWaitCensus::default();

    for path in &files {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("read driver source {}", path.display()));
        let relative = path
            .strip_prefix(repo_root())
            .unwrap_or(path)
            .display()
            .to_string();
        let file_census = census_source(&relative, &source);
        census.interruptible += file_census.interruptible;
        census.uninterruptible += file_census.uninterruptible;
        census
            .interruptible_sites
            .extend(file_census.interruptible_sites);
    }

    (census, files.len())
}

fn validate_no_interruptible_waits(label: &str, source: &str) -> Result<(), String> {
    let census = census_source(label, source);
    if census.interruptible == 0 {
        Ok(())
    } else {
        Err(format!(
            "found {} interruptible completion wait(s): {}",
            census.interruptible,
            census.interruptible_sites.join(", ")
        ))
    }
}

fn discover_aarch64_oracle_gates() -> Vec<PathBuf> {
    let qemu_root = repo_root().join("docker/qemu");
    discover_files(&qemu_root, "sh")
        .into_iter()
        .filter(|path| {
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if !file_name.starts_with("run-aarch64-") {
                return false;
            }
            let source = fs::read_to_string(path)
                .unwrap_or_else(|_| panic!("read aarch64 gate {}", path.display()));
            let builds_boot_tests = source.contains("--features boot_tests");
            let consumes_full_test_kernel = file_name.contains("boot-test")
                && source.contains("target/aarch64-breenix-kernel/release/kernel-aarch64");
            builds_boot_tests || consumes_full_test_kernel
        })
        .collect()
}

#[test]
fn drivers_have_no_interruptible_completion_waits() {
    let (census, discovered_files) = driver_completion_wait_census();
    assert_eq!(
        census.interruptible,
        0,
        "discovered {discovered_files} driver Rust files with interruptible completion waits at: {}",
        census.interruptible_sites.join(", ")
    );
}

#[test]
fn driver_completion_wait_population_is_pinned() {
    let (census, discovered_files) = driver_completion_wait_census();
    assert_eq!(
        census.total(),
        DRIVER_COMPLETION_WAIT_POPULATION,
        "completion-wait census changed across {discovered_files} discovered driver Rust files: {} interruptible + {} uninterruptible",
        census.interruptible,
        census.uninterruptible
    );
}

#[test]
fn completion_wait_validator_rejects_interruptible_synthetic_source() {
    let synthetic = "self.completion.wait_timeout(token, TIMEOUT);";
    let error = validate_no_interruptible_waits("synthetic.rs", synthetic)
        .expect_err("synthetic interruptible wait must be rejected");
    assert!(
        error.contains("synthetic.rs:1") && error.contains("1 interruptible completion wait"),
        "validator returned the wrong diagnostic: {error}"
    );
}

#[test]
fn completion_wait_census_ignores_comments_and_string_literals() {
    let synthetic = r###"
        // self.completion.wait_timeout(token, TIMEOUT);
        /* device.wait_timeout(token, TIMEOUT); */
        const MESSAGE: &str = ".wait_timeout(";
        const RAW_MESSAGE: &str = r#".wait_timeout("#;
    "###;
    let census = census_source("synthetic.rs", synthetic);
    assert_eq!(
        census.total(),
        0,
        "comment/string-only waits were counted: {} interruptible + {} uninterruptible",
        census.interruptible,
        census.uninterruptible
    );
}

#[test]
fn block_eintr_oracle_marker_is_pinned_in_the_gates() {
    let oracle = repo_text("userspace/programs/src/block_eintr_oracle.rs");
    assert!(
        oracle.contains(BLOCK_EINTR_ORACLE_PREFIX),
        "userspace block EINTR oracle does not emit {BLOCK_EINTR_ORACLE_PREFIX}"
    );

    let gates = discover_aarch64_oracle_gates();
    let discovered_count = gates.len();
    assert!(
        discovered_count != 0,
        "discovered 0 aarch64 boot_tests/full-kernel gate scripts"
    );
    let missing: Vec<String> = gates
        .iter()
        .filter_map(|path| {
            let source = fs::read_to_string(path)
                .unwrap_or_else(|_| panic!("read aarch64 gate {}", path.display()));
            (!source.contains(BLOCK_EINTR_ORACLE_PREFIX)).then(|| {
                path.strip_prefix(repo_root())
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
        })
        .collect();
    assert!(
        missing.is_empty(),
        "oracle marker missing from {} of {discovered_count} discovered aarch64 gate scripts: {}",
        missing.len(),
        missing.join(", ")
    );
}
