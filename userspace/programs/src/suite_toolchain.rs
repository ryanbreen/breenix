//! Check sealed reference inputs and exercise the native assembler, linker and compiler.

use breenix_suite::suite::{case, case_diagnostic, category, suite, CaseResult, Suite};
use libbreenix::crypto::sha256::{sha256, Sha256};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

#[path = "fixture/ssh_lease.rs"]
mod ssh_lease;

const DEV: &str = "opt/breenix-dev/25.2";
const CORPUS: &str = "usr/share/breenix/toolchain";
const RUNNER: &str = "sbin/suite-toolchain";
const MANIFEST: &str = "opt/breenix-dev/25.2/manifest.sha256";
const RECORD: &str = "opt/breenix-dev/25.2/build.json";
const BAS_PROGRAMS: &[&str] = &["sse-exit", "x87-exit"];
const BAS_INPUTS: &[&str] = &["encodings.s", "encodings.hex", "bad-input.s"];
const LINKER_OBJECTS: &[&str] = &[
    "got-main",
    "got-entry",
    "archive-value-with-long-name",
    "tls-template",
];
const ARCHIVE_MEMBERS: &[&str] = &["got-entry", "archive-value-with-long-name"];
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
        category(
            "linker",
            "Static links of our libc archive: GOT, archives, linker script and TLS",
            &[
                case(
                    "links-reference-objects",
                    "On-box links of the reference objects match their expectations",
                    links_reference_objects,
                ),
                case(
                    "got-and-archives",
                    "A fresh link resolves the GOT and pulls dependent archive members",
                    got_and_archives,
                ),
                case(
                    "tls-template",
                    "A fresh link initializes aligned TLS in the main thread and two new threads",
                    tls_template,
                ),
                case(
                    "archive-tools",
                    "Deterministic archives expose symbols and supply a runnable link",
                    archive_tools,
                ),
                case(
                    "undefined-located",
                    "Undefined references identify their relocation before a repaired link runs",
                    undefined_located,
                ),
                case(
                    "duplicate-located",
                    "Duplicate definitions identify both objects before a repaired link runs",
                    duplicate_located,
                ),
            ],
        ),
        category(
            "c-core",
            "Compile and run the core C subset",
            &[
                case("hello", "Native cc builds and runs hello", cc_hello),
                case(
                    "hello-over-ssh",
                    "The fixture account builds and runs hello over the restricted SSH lease",
                    cc_hello_over_ssh,
                ),
                case(
                    "corpus-three-producers",
                    "Reference, Mac candidate and native builds each match the expected results",
                    cc_corpus_three_producers,
                ),
                case(
                    "stages-agree",
                    "Explicit compiler stages match the direct build byte for byte",
                    cc_stages_agree,
                ),
                case(
                    "diagnostic-located",
                    "An undeclared identifier fails compilation with a file, line and column",
                    cc_diagnostic_located,
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
    // Dropping a C file, expectation, object, executable or inventory from both the
    // staged tree and its generated manifest still fails here.
    for name in CASES.lines() {
        for (prefix, suffix) in [
            ("", ".c"),
            ("", ".expect"),
            ("", ".elf"),
            ("", ".inventory.json"),
            ("objs/", ".o"),
        ] {
            let path = format!("{CORPUS}/{prefix}{name}{suffix}");
            if !listed.contains_key(path.as_str()) {
                return Err(format!("manifest omits {path}").into());
            }
        }
    }
    for relative in [
        "lib/libc.a",
        "linker.ld",
        "bin/bas",
        "bin/bld",
        "bin/bar",
        "bin/bnm",
        "bin/cc",
        "bin/as",
        "bin/ld",
    ] {
        let path = format!("{DEV}/{relative}");
        if !listed.contains_key(path.as_str()) {
            return Err(format!("manifest omits {path}").into());
        }
    }

    if arch == "x86_64" {
        for name in LINKER_OBJECTS {
            let path = format!("{CORPUS}/objs/{name}.o");
            if !listed.contains_key(path.as_str()) {
                return Err(format!("manifest omits {path}").into());
            }
        }
        for name in ["got-main", "tls-template"] {
            let path = format!("{CORPUS}/linker/{name}.expect");
            if !listed.contains_key(path.as_str()) {
                return Err(format!("manifest omits {path}").into());
            }
        }
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

fn expected_output(expect: &Path) -> Result<(u8, Vec<u8>), String> {
    let name = expect.display();
    let bytes = io_at(expect, fs::read(expect))?;
    let newline = bytes
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(|| format!("{name}: missing expected exit status"))?;
    let status = std::str::from_utf8(&bytes[..newline])
        .ok()
        .and_then(|status| status.parse::<u8>().ok())
        .ok_or_else(|| format!("{name}: invalid expected exit status"))?;
    Ok((status, bytes[newline + 1..].to_vec()))
}

fn run_expected(binary: &Path, expect: &Path) -> CaseResult {
    let name = binary.display();
    let (status, expected) = expected_output(expect)?;
    let result = io_at(
        binary,
        Command::new(binary)
            .env_clear()
            .stdin(Stdio::null())
            .output(),
    )?;
    if result.status.code() != Some(i32::from(status)) {
        return Err(format!("{name}: expected exit {status}, got {}", result.status).into());
    }
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
        run_expected(
            &corpus.join(format!("{name}.elf")),
            &corpus.join(format!("{name}.expect")),
        )?;
    }
    Ok(())
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(category: &str) -> Result<Self, String> {
        let path = Path::new("/tmp").join(format!("toolchain-{category}-{}", std::process::id()));
        io_at(&path, fs::DirBuilder::new().mode(0o700).create(&path))?;
        Ok(Self(path))
    }

    fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn native_cc_supported() -> CaseResult {
    if std::env::consts::ARCH != "x86_64" {
        return Err("AArch64 scalar C code generation is scheduled for A3".into());
    }
    Ok(())
}

fn c_core() -> PathBuf {
    Path::new("/").join(CORPUS).join("c-core")
}

fn cc_tool(
    scratch: &Scratch,
    tool: &str,
    args: &[&str],
) -> Result<std::process::Output, String> {
    let bin = Path::new("/").join(DEV).join("bin");
    let executable = bin.join(tool);
    io_at(
        &executable,
        Command::new(&executable)
            .args(args)
            .current_dir(&scratch.0)
            .env_clear()
            .env("PATH", format!("{}:/bin", bin.display()))
            .stdin(Stdio::null())
            .output(),
    )
}

fn cc_checked(scratch: &Scratch, tool: &str, args: &[&str]) -> CaseResult {
    let output = cc_tool(scratch, tool, args)?;
    if !output.status.success() {
        return Err(format!(
            "{tool} {}: {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim_end()
        )
        .into());
    }
    Ok(())
}

fn cc_hello() -> CaseResult {
    native_cc_supported()?;
    let scratch = Scratch::new("hello")?;
    let fixture = c_core();
    let source = fixture.join("hello.c");
    cc_checked(&scratch, "cc", &[source.to_str().unwrap(), "-o", "hello"])?;
    run_expected(&scratch.join("hello"), &fixture.join("hello.expect"))
}

fn cc_hello_over_ssh() -> CaseResult {
    native_cc_supported()?;
    ssh_lease::with_listener(
        "SUITE toolchain HOSTSSH c-core/hello-over-ssh",
        "TOOLCHAIN LEASE-END",
        |account, nonce| {
            let home = Path::new(&account.home);
            let directory = home.join(format!(".toolchain-{nonce}"));
            let binary = directory.join("hello");
            let receipt = directory.join("result.json");
            let record: Value = serde_json::from_slice(&io_at(&receipt, fs::read(&receipt))?)
                .map_err(|error| format!("invalid toolchain SSH result: {error}"))?;
            let (status, stdout) = expected_output(&c_core().join("hello.expect"))?;
            if record["nonce"] != nonce || record["uid"] != account.uid {
                return Err("toolchain SSH result has the wrong nonce or operator uid".into());
            }
            if record["path"].as_str() != binary.to_str() || record["sha256"] != file_sha256(&binary)? {
                return Err("toolchain SSH binary differs from the recorded path or digest".into());
            }
            if record["status"] != status || record["stdoutHex"] != hex(&stdout) {
                return Err("toolchain SSH program does not match hello.expect".into());
            }
            Ok(())
        },
    )
}

fn cc_corpus_three_producers() -> CaseResult {
    native_cc_supported()?;
    let scratch = Scratch::new("c-core")?;
    let corpus = Path::new("/").join(CORPUS);
    for name in CASES.lines() {
        let expected = corpus.join(format!("{name}.expect"));
        let source = corpus.join(format!("{name}.c"));
        let elf = format!("{name}.elf");
        run_expected(&corpus.join(&elf), &expected)?;
        run_expected(&c_core().join("mac").join(&elf), &expected)?;
        cc_checked(&scratch, "cc", &[source.to_str().unwrap(), "-o", &elf])?;
        run_expected(&scratch.join(&elf), &expected)?;
        let binary = scratch.join(&elf);
        io_at(&binary, fs::remove_file(&binary))?;
    }
    Ok(())
}

fn equal_stage(scratch: &Scratch, direct: &str, explicit: &str) -> CaseResult {
    let read = |name| {
        let path = scratch.join(name);
        io_at(&path, fs::read(&path))
    };
    if read(direct)? != read(explicit)? {
        return Err(format!("compiler stages differ at {direct} and {explicit}").into());
    }
    Ok(())
}

fn cc_stages_agree() -> CaseResult {
    native_cc_supported()?;
    let scratch = Scratch::new("cc-stages")?;
    let fixture = c_core();
    let source = fixture.join("hello.c");
    let source = source.to_str().unwrap();
    for (tool, args) in [
        ("cc", &["-E", "-o", "hello.i", source][..]),
        ("cc", &["-S", "-o", "direct.s", source]),
        ("cc", &["-S", "-o", "split.s", "hello.i"]),
    ] {
        cc_checked(&scratch, tool, args)?;
    }
    equal_stage(&scratch, "direct.s", "split.s")?;
    cc_checked(&scratch, "cc", &["-c", "-o", "direct.o", source])?;
    cc_checked(&scratch, "as", &["-o", "split.o", "split.s"])?;
    equal_stage(&scratch, "direct.o", "split.o")?;

    let dev = Path::new("/").join(DEV);
    let linker = dev.join("linker.ld");
    let archive = dev.join("lib/libc.a");
    cc_checked(
        &scratch,
        "ld",
        &[
            "-T", linker.to_str().unwrap(), "--gc-sections", "-o", "split",
            "split.o", archive.to_str().unwrap(),
        ],
    )?;
    cc_checked(&scratch, "cc", &["-o", "direct", source])?;
    equal_stage(&scratch, "direct", "split")?;
    run_expected(&scratch.join("split"), &fixture.join("hello.expect"))
}

fn cc_diagnostic_located() -> CaseResult {
    native_cc_supported()?;
    let scratch = Scratch::new("cc-diagnostic")?;
    let source = c_core().join("diagnostic.c");
    let source = source.to_str().unwrap();
    let output = cc_tool(&scratch, "cc", &[source, "-o", "unexpected"])?;
    let expected = format!("{source}:4:12: error: use of undeclared identifier");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !matches!(output.status.code(), Some(1))
        || scratch.join("unexpected").exists()
        || !stderr.lines().any(|line| line == expected)
    {
        return Err(format!("cc did not report the undeclared identifier at 4:12: {stderr}").into());
    }
    Ok(())
}

struct AssemblyCase {
    corpus: PathBuf,
    scratch: Scratch,
}

impl AssemblyCase {
    fn start() -> Result<Self, String> {
        if std::env::consts::ARCH != "x86_64" {
            return Err("AArch64 assembly and relocations are scheduled for A1".into());
        }
        Ok(Self {
            corpus: Path::new("/").join(CORPUS).join("assembler"),
            scratch: Scratch::new("assembler")?,
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
        run_expected(&elf, &case.corpus.join(format!("{name}.expect")))?;
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

struct LinkerCase {
    dev: PathBuf,
    corpus: PathBuf,
    scratch: Scratch,
}

impl LinkerCase {
    fn start() -> Result<Self, String> {
        if std::env::consts::ARCH != "x86_64" {
            return Err("AArch64 static linking is scheduled for A2".into());
        }
        Ok(Self {
            dev: Path::new("/").join(DEV),
            corpus: Path::new("/").join(CORPUS),
            scratch: Scratch::new("linker")?,
        })
    }

    fn object(&self, name: &str) -> PathBuf {
        self.corpus.join("objs").join(format!("{name}.o"))
    }

    fn tool(
        &self,
        tool: &str,
        args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
    ) -> Result<std::process::Output, String> {
        let binary = self.dev.join("bin").join(tool);
        io_at(
            &binary,
            Command::new(&binary)
                .args(args)
                .env_clear()
                .stdin(Stdio::null())
                .output(),
        )
    }

    fn link(&self, output: &Path, inputs: &[&Path]) -> Result<std::process::Output, String> {
        self.tool(
            "bld",
            [
                Path::new("-T"),
                &self.dev.join("linker.ld"),
                Path::new("--gc-sections"),
                Path::new("-o"),
                output,
            ]
            .into_iter()
            .chain(inputs.iter().copied())
            .chain([self.dev.join("lib/libc.a").as_path()]),
        )
    }

    fn link_and_run(&self, name: &str, expect: &Path, inputs: &[&Path]) -> CaseResult {
        let output = self.scratch.join(format!("{name}.elf"));
        require_success("bld", self.link(&output, inputs)?)?;
        run_expected(&output, expect)
    }

    fn make_archive(&self, name: &str) -> Result<PathBuf, String> {
        let archive = self.scratch.join(name);
        let members = ARCHIVE_MEMBERS.iter().map(|name| self.object(name));
        let args = [PathBuf::from("rcs"), archive.clone()]
            .into_iter()
            .chain(members);
        require_success("bar", self.tool("bar", args)?)?;
        Ok(archive)
    }

    fn run_got(&self, archive: &Path) -> CaseResult {
        self.link_and_run(
            "got-main",
            &self.corpus.join("linker/got-main.expect"),
            &[&self.object("got-main"), archive],
        )
    }
}

fn require_success(tool: &str, result: std::process::Output) -> Result<Vec<u8>, String> {
    if result.status.success() {
        Ok(result.stdout)
    } else {
        Err(format!(
            "{tool} exited {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim_end()
        ))
    }
}

fn links_reference_objects() -> CaseResult {
    let case = LinkerCase::start()?;
    for name in CASES.lines() {
        case.link_and_run(
            name,
            &case.corpus.join(format!("{name}.expect")),
            &[&case.object(name)],
        )?;
    }
    Ok(())
}

fn got_and_archives() -> CaseResult {
    let case = LinkerCase::start()?;
    let archive = case.make_archive("got.a")?;
    case.run_got(&archive)
}

fn tls_template() -> CaseResult {
    let case = LinkerCase::start()?;
    case.link_and_run(
        "tls-template",
        &case.corpus.join("linker/tls-template.expect"),
        &[&case.object("tls-template")],
    )
}

fn check_archive_index_rebuilds(case: &LinkerCase, archive: &Path, expected: &[u8]) -> CaseResult {
    // Solaris ar(3HEAD): the first member is the 32-bit symbol index. Keep
    // the long-name table, object members and their padding in both fixtures.
    const MAGIC: &[u8] = b"!<arch>\n";
    const INDEX_DATA: usize = MAGIC.len() + 60;
    let header = expected
        .strip_prefix(MAGIC)
        .and_then(|members| members.get(..60))
        .filter(|header| header.starts_with(b"/               ") && header.ends_with(b"`\n"))
        .ok_or("bar did not write a leading 32-bit index")?;
    let size = std::str::from_utf8(&header[48..58])
        .ok()
        .and_then(|field| field.trim_end().parse::<usize>().ok())
        .filter(|&size| (8..=expected.len() - INDEX_DATA).contains(&size))
        .ok_or("bar wrote an invalid index size")?;
    let remaining = expected
        .get(INDEX_DATA + size + size % 2..)
        .ok_or("bar wrote a truncated index member")?;
    let mut missing = MAGIC.to_vec();
    missing.extend_from_slice(remaining);
    let mut corrupt = expected.to_vec();
    // After the count, the first offset must locate an object member header.
    corrupt[INDEX_DATA + 4..INDEX_DATA + 8].fill(0);
    for (label, bytes) in [("missing", missing), ("corrupt", corrupt)] {
        if bytes == expected {
            return Err(format!("archive fixture did not create a {label} index").into());
        }
        io_at(archive, fs::write(archive, bytes))?;
        require_success(
            &format!("bar s ({label} index)"),
            case.tool("bar", [Path::new("s"), archive])?,
        )?;
        if io_at(archive, fs::read(archive))? != expected {
            return Err(format!("bar s did not rebuild the {label} index").into());
        }
    }
    Ok(())
}

fn archive_tools() -> CaseResult {
    let case = LinkerCase::start()?;
    let archive = case.make_archive("first.a")?;
    let second = case.make_archive("second.a")?;
    require_success("bar", case.tool("bar", [Path::new("s"), &second])?)?;
    let original = io_at(&archive, fs::read(&archive))?;
    if original != io_at(&second, fs::read(&second))? {
        return Err("bar produced different archive bytes after reindexing".into());
    }
    check_archive_index_rebuilds(&case, &second, &original)?;
    let symbols = require_success("bnm", case.tool("bnm", [Path::new("-Pg"), &second])?)?;
    let expected = format!(
        "{0}[got-entry.o]:\narchive_cell U\narchive_entry T 0 17\narchive_value U\n\
         {0}[archive-value-with-long-name.o]:\narchive_cell R 0 4\narchive_value T 0 6\n",
        second.display()
    );
    if symbols != expected.as_bytes() {
        return Err(format!(
            "bnm returned unexpected symbols: {}",
            String::from_utf8_lossy(&symbols)
        )
        .into());
    }
    case.run_got(&second)
}

fn check_link_failure(result: std::process::Output, output: &Path, diagnostic: &str) -> CaseResult {
    if result.status.code() != Some(1)
        || output.exists()
        || !String::from_utf8_lossy(&result.stderr)
            .lines()
            .any(|line| line.contains(diagnostic))
    {
        return Err(format!(
            "expected bld exit 1, no output, and {diagnostic:?}; got {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim_end()
        )
        .into());
    }
    Ok(())
}

fn undefined_located() -> CaseResult {
    let case = LinkerCase::start()?;
    let output = case.scratch.join("undefined.elf");
    let main = case.object("got-main");
    let result = case.link(&output, &[&main])?;
    let stderr = String::from_utf8_lossy(&result.stderr);
    let location = format!("{}: .text+0x", main.display());
    if !stderr.lines().any(|line| {
        line.starts_with(&location) && line.ends_with(" archive_entry: undefined symbol")
    }) {
        return Err(format!("bld did not locate the reference to archive_entry: {stderr}").into());
    }
    check_link_failure(result, &output, "R_X86_64_PLT32")?;
    let archive = case.make_archive("resolved.a")?;
    case.run_got(&archive)
}

fn duplicate_located() -> CaseResult {
    let case = LinkerCase::start()?;
    let output = case.scratch.join("duplicate.elf");
    let entry = case.object("got-entry");
    let duplicate = case.scratch.join("duplicate-entry.o");
    io_at(&duplicate, fs::copy(&entry, &duplicate))?;
    let result = case.link(&output, &[&case.object("got-main"), &entry, &duplicate])?;
    let diagnostic = format!(
        "{}: archive_entry: duplicate definition (first defined in {})",
        duplicate.display(),
        entry.display()
    );
    check_link_failure(result, &output, &diagnostic)?;
    let archive = case.make_archive("deduplicated.a")?;
    case.run_got(&archive)
}
