//! Check the sealed reference inputs and assemble objects with the staged bas.

use breenix_suite::suite::{case, case_diagnostic, category, suite, CaseResult, Suite};
use libbreenix::crypto::sha256::{sha256, Sha256};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

const DEV: &str = "opt/breenix-dev/25.2";
const CORPUS: &str = "usr/share/breenix/toolchain";
const RUNNER: &str = "sbin/suite-toolchain";
const MANIFEST: &str = "opt/breenix-dev/25.2/manifest.sha256";
const RECORD: &str = "opt/breenix-dev/25.2/build.json";
const BAS_PROGRAMS: &[&str] = &["sse-exit", "x87-exit"];
const BAS_INPUTS: &[&str] = &["encodings.s", "encodings.hex", "bad-input.s"];
const CASES: &str = include_str!("../../../toolchain/corpus/cases");

static SUITE: Suite = suite(
    "toolchain",
    "Native C toolchain",
    &[
        category(
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
        ),
        category(
            "assembler",
            "Instruction encodings and ELF objects from our assembler",
            &[
                case(
                    "encodings-match",
                    "Assembler output matches the instruction encoding table",
                    assembler_encodings_match,
                ),
                case(
                    "object-runs",
                    "Fresh objects match the recorded link inputs and both staged ELFs run",
                    assembler_object_runs,
                ),
                case(
                    "bad-input-located",
                    "Malformed assembly fails with its source file and line number",
                    assembler_bad_input_located,
                ),
            ],
        ),
    ],
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
    for relative in ["lib/libc.a", "linker.ld", "bin/bas"] {
        let path = format!("{DEV}/{relative}");
        if !listed.contains_key(path.as_str()) {
            return Err(format!("manifest omits {path}").into());
        }
    }

    if arch == "x86_64" {
        for name in BAS_INPUTS {
            let path = format!("{CORPUS}/assembler/{name}");
            if !listed.contains_key(path.as_str()) {
                return Err(format!("manifest omits {path}").into());
            }
        }
        for name in BAS_PROGRAMS {
            for suffix in [".s", ".expect", ".elf", ".link.json"] {
                let path = format!("{CORPUS}/assembler/{name}{suffix}");
                if !listed.contains_key(path.as_str()) {
                    return Err(format!("manifest omits {path}").into());
                }
            }
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

fn run_expected(directory: &Path, name: &str) -> CaseResult {
    let expect = directory.join(format!("{name}.expect"));
    let bytes = io_at(&expect, fs::read(&expect))?;
    let newline = bytes
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(|| format!("{name}: missing expected exit status"))?;
    let status = std::str::from_utf8(&bytes[..newline])
        .ok()
        .and_then(|status| status.parse::<u8>().ok())
        .ok_or_else(|| format!("{name}: invalid expected exit status"))?;
    let binary = directory.join(format!("{name}.elf"));
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
    Ok(())
}

fn reference_corpus_runs() -> CaseResult {
    let corpus = Path::new("/").join(CORPUS);
    for name in CASES.lines() {
        run_expected(&corpus, name)?;
    }
    Ok(())
}

struct AssemblyCase {
    corpus: PathBuf,
    scratch: PathBuf,
}

impl AssemblyCase {
    fn start() -> Result<Self, String> {
        if std::env::consts::ARCH != "x86_64" {
            return Err("AArch64 assembly and relocations are scheduled for A1".into());
        }
        let scratch = Path::new("/tmp").join(format!("toolchain-assembler-{}", std::process::id()));
        io_at(&scratch, fs::DirBuilder::new().mode(0o700).create(&scratch))?;
        Ok(Self {
            corpus: Path::new("/").join(CORPUS).join("assembler"),
            scratch,
        })
    }

    fn run(&self, source: &Path, object: &Path) -> Result<std::process::Output, String> {
        let bas = Path::new("/").join(DEV).join("bin/bas");
        io_at(
            &bas,
            Command::new(&bas)
                .arg("-o")
                .arg(object)
                .arg(source)
                .env_clear()
                .stdin(Stdio::null())
                .output(),
        )
    }

    fn assemble(&self, name: &str) -> Result<PathBuf, String> {
        let source = self.corpus.join(format!("{name}.s"));
        let object = self.scratch.join(format!("{name}.o"));
        let result = self.run(&source, &object)?;
        if !result.status.success() {
            return Err(format!(
                "{name}: bas exited {}: {}",
                result.status,
                String::from_utf8_lossy(&result.stderr).trim_end()
            ));
        }
        Ok(object)
    }
}

impl Drop for AssemblyCase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.scratch);
    }
}

fn elf_field<const N: usize>(bytes: &[u8], offset: usize) -> Option<[u8; N]> {
    bytes.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

// ELF gABI Listings 2.1 and 3.1. Read named section bytes independently of bas.
fn object_text(object: &[u8]) -> Option<&[u8]> {
    if !object.starts_with(b"\x7fELF\x02\x01\x01")
        || u16::from_le_bytes(elf_field(object, 16)?) != 1
        || u16::from_le_bytes(elf_field(object, 18)?) != 62
        || u16::from_le_bytes(elf_field(object, 58)?) != 64
    {
        return None;
    }
    let start = usize::try_from(u64::from_le_bytes(elf_field(object, 40)?)).ok()?;
    let count = usize::from(u16::from_le_bytes(elf_field(object, 60)?));
    let names = usize::from(u16::from_le_bytes(elf_field(object, 62)?));
    let table = object.get(start..start.checked_add(64 * count)?)?;
    let contents = |header: &[u8]| {
        let offset = usize::try_from(u64::from_le_bytes(elf_field(header, 24)?)).ok()?;
        let size = usize::try_from(u64::from_le_bytes(elf_field(header, 32)?)).ok()?;
        object.get(offset..offset.checked_add(size)?)
    };
    let strings = contents(table.chunks_exact(64).nth(names)?)?;
    let mut text = None;
    for header in table.chunks_exact(64) {
        let name = usize::try_from(u32::from_le_bytes(elf_field(header, 0)?)).ok()?;
        if strings.get(name..)?.starts_with(b".text\0")
            && (u32::from_le_bytes(elf_field(header, 4)?) != 1
                || text.replace(contents(header)?).is_some())
        {
            return None;
        }
    }
    text
}

fn assembler_encodings_match() -> CaseResult {
    let case = AssemblyCase::start()?;
    let object = case.assemble("encodings")?;
    let bytes = io_at(&object, fs::read(&object))?;
    let actual = object_text(&bytes).ok_or("bas object has no valid .text section")?;
    let path = case.corpus.join("encodings.hex");
    let hex_table = io_at(&path, fs::read_to_string(&path))?;
    let expected = hex_table
        .split_ascii_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if actual != expected || expected.is_empty() {
        return Err("bas .text differs from the Intel SDM encoding table".into());
    }
    Ok(())
}

fn assembler_object_runs() -> CaseResult {
    let case = AssemblyCase::start()?;
    for name in BAS_PROGRAMS {
        let object = case.assemble(name)?;
        let object_name = format!("{name}.o");
        let elf_name = format!("{name}.elf");
        let elf = case.corpus.join(&elf_name);
        let path = case.corpus.join(format!("{name}.link.json"));
        let receipt: Value = serde_json::from_slice(&io_at(&path, fs::read(&path))?)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let inputs = receipt["inputObjects"]
            .as_object()
            .filter(|inputs| inputs.len() == 1)
            .ok_or_else(|| format!("{name}: expected one object in link receipt"))?;
        let map = receipt["map"]
            .as_str()
            .ok_or_else(|| format!("{name}: missing lld map"))?;
        let map_input = format!("{object_name}:(.text)");
        if !map
            .lines()
            .any(|line| line.split_ascii_whitespace().last() == Some(map_input.as_str()))
        {
            return Err(format!("{name}: lld map does not name {object_name}").into());
        }
        if inputs.get(&object_name).and_then(Value::as_str) != Some(file_sha256(&object)?.as_str())
        {
            return Err(format!("{name}: fresh object differs from the recorded lld input").into());
        }
        if receipt["elf"]["name"] != elf_name || receipt["elf"]["sha256"] != file_sha256(&elf)? {
            return Err(format!("{name}: staged ELF differs from the link receipt").into());
        }
        run_expected(&case.corpus, name)?;
    }
    Ok(())
}

fn assembler_bad_input_located() -> CaseResult {
    let case = AssemblyCase::start()?;
    let source = case.corpus.join("bad-input.s");
    let object = case.scratch.join("bad-input.o");
    let result = case.run(&source, &object)?;
    let location = format!("{}:2:", source.display());
    if !result.status.code().is_some_and(|code| code != 0)
        || !String::from_utf8_lossy(&result.stderr).starts_with(&location)
        || object.exists()
    {
        return Err(format!(
            "expected nonzero exit and {location} with no output object, got {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim_end()
        )
        .into());
    }
    Ok(())
}
