//! Run the sealed reference build's inputs against independently written expectations.

use breenix_suite::suite::{case, case_diagnostic, category, suite, CaseResult, Suite};
use libbreenix::crypto::sha256::{sha256, Sha256};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path};
use std::process::{Command, Stdio};

const DEV: &str = "opt/breenix-dev/25.2";
const CORPUS: &str = "usr/share/breenix/toolchain";
const RUNNER: &str = "sbin/suite-toolchain";
const MANIFEST: &str = "opt/breenix-dev/25.2/manifest.sha256";
const RECORD: &str = "opt/breenix-dev/25.2/build.json";
const CASES: &str = include_str!("../../../toolchain/corpus/cases");

static SUITE: Suite = suite(
    "toolchain",
    "Native C toolchain",
    &[category(
        "inputs",
        "The sysroot inventory and the cross-built reference corpus",
        &[
            case(
                "sysroot-manifest",
                "Staged toolchain inputs match this image's build record",
                sysroot_manifest,
            ),
            case(
                "reference-corpus-runs",
                "Each reference ELF produces its expected stdout and exit status",
                reference_corpus_runs,
            ),
        ],
    )],
);

fn main() {
    SUITE.run()
}

fn io_at<T>(path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{}: {error}", path.display()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn lower_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = io_at(path, File::open(path))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 16384];
    loop {
        let count = io_at(path, file.read(&mut buffer))?;
        if count == 0 {
            return Ok(hex(&digest.finalize()));
        }
        digest.update(&buffer[..count]);
    }
}

fn tree_files(path: &Path, files: &mut BTreeSet<String>) -> CaseResult {
    let kind = io_at(path, fs::symlink_metadata(path))?.file_type();
    if kind.is_dir() {
        for entry in io_at(path, fs::read_dir(path))? {
            tree_files(&io_at(path, entry)?.path(), files)?;
        }
    } else if kind.is_file() {
        let name = path
            .to_str()
            .and_then(|name| name.strip_prefix('/'))
            .ok_or_else(|| format!("invalid fixture path {}", path.display()))?;
        if name != MANIFEST && name != RECORD {
            files.insert(name.to_owned());
        }
    } else {
        return Err(format!("unexpected fixture file type at {}", path.display()).into());
    }
    Ok(())
}

fn fixture_path(name: &str) -> bool {
    let path = Path::new(name);
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        && (name == RUNNER
            || (path != Path::new(DEV) && path.starts_with(DEV))
            || (path != Path::new(CORPUS) && path.starts_with(CORPUS)))
        && name != MANIFEST
        && name != RECORD
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'+' | b'-')
        })
}

fn sysroot_manifest() -> CaseResult {
    let root = Path::new("/");
    let manifest_path = root.join(MANIFEST);
    let manifest = io_at(&manifest_path, fs::read_to_string(&manifest_path))?;
    let record_path = root.join(RECORD);
    let record: Value = serde_json::from_slice(&io_at(&record_path, fs::read(&record_path))?)
        .map_err(|error| format!("invalid toolchain image build record: {error}"))?;
    let commit = record["commit"]
        .as_str()
        .filter(|commit| lower_hex(commit, 40))
        .ok_or("missing toolchain image build commit")?;
    let arch = std::env::consts::ARCH;
    let hash = hex(&sha256(manifest.as_bytes()));
    if record["arch"] != arch || record["manifestSha256"] != hash {
        return Err("manifest digest or architecture differs from the image build record".into());
    }

    let mut listed = BTreeMap::new();
    for line in manifest.lines() {
        let (digest, name) = line.split_once("  ").ok_or("invalid manifest row")?;
        if !lower_hex(digest, 64) || !fixture_path(name) || listed.insert(name, digest).is_some() {
            return Err(format!("invalid or duplicate manifest path {name}").into());
        }
    }
    // This catalog is compiled into PID 1 from the independent source list.
    // Dropping a C file, expectation, executable or inventory from both the
    // staged tree and its generated manifest still fails here.
    for name in CASES.lines() {
        for suffix in [".c", ".expect", ".elf", ".inventory.json"] {
            let path = format!("{CORPUS}/{name}{suffix}");
            if !listed.contains_key(path.as_str()) {
                return Err(format!("manifest omits {path}").into());
            }
        }
    }
    for relative in ["lib/libc.a", "linker.ld"] {
        let path = format!("{DEV}/{relative}");
        if !listed.contains_key(path.as_str()) {
            return Err(format!("manifest omits {path}").into());
        }
    }

    let mut staged = BTreeSet::new();
    for path in [DEV, CORPUS, RUNNER] {
        tree_files(&root.join(path), &mut staged)?;
    }
    let declared: BTreeSet<_> = listed.keys().copied().collect();
    let present: BTreeSet<_> = staged.iter().map(String::as_str).collect();
    if let Some(path) = declared.symmetric_difference(&present).next() {
        return Err(format!("manifest and staged paths differ at {path}").into());
    }
    for (path, expected) in listed {
        if file_sha256(&root.join(path))? != expected {
            return Err(format!("SHA-256 differs for {path}").into());
        }
    }
    case_diagnostic(&format!(
        "TOOLCHAIN SYSROOT commit={commit} arch={arch} sha256={hash}"
    ));
    Ok(())
}

fn reference_corpus_runs() -> CaseResult {
    let corpus = Path::new("/").join(CORPUS);
    for name in CASES.lines() {
        let expect = corpus.join(format!("{name}.expect"));
        let bytes = io_at(&expect, fs::read(&expect))?;
        let newline = bytes
            .iter()
            .position(|&byte| byte == b'\n')
            .ok_or_else(|| format!("{name}: missing expected exit status"))?;
        let status = std::str::from_utf8(&bytes[..newline])
            .ok()
            .and_then(|status| status.parse::<u8>().ok())
            .ok_or_else(|| format!("{name}: invalid expected exit status"))?;
        let binary = corpus.join(format!("{name}.elf"));
        let result = io_at(
            &binary,
            Command::new(&binary)
                .env_clear()
                .stdin(Stdio::null())
                .output(),
        )?;
        if result.status.code() != Some(i32::from(status)) {
            return Err(format!("{name}: expected exit {status}, got {}", result.status).into());
        }
        let expected = &bytes[newline + 1..];
        if result.stdout != expected {
            return Err(format!(
                "{name}: stdout differs (expected {:?}, got {:?})",
                expected, result.stdout
            )
            .into());
        }
    }
    Ok(())
}
