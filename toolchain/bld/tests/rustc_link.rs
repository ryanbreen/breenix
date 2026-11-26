use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bobj::elf::{Elf, SHN_UNDEF, SHT_SYMTAB, STB_GLOBAL, STB_WEAK};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("bld-rustc-{}-{nanos}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(command: &mut Command) -> Vec<u8> {
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("{command:?}: {error}"));
    assert!(
        result.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

// Python's standard library reads the receipt and verifies its artifact hashes.
// ELF bytes and the linker's output are checked independently below.
const PREPARE_REPLAY: &str = r#"
import hashlib
import json
from pathlib import Path
import shutil
import sys

record, source, twin = map(Path, sys.argv[1:])
receipt = json.loads(record.read_text())
names = {"bas", "bld", "bar", "bnm", "bcc", "bmake", "panic-abort"}
links = {}
copied = set()

def verify(path, expected):
    with path.open("rb") as file:
        actual = hashlib.file_digest(file, "sha256").hexdigest()
    if actual != expected:
        sys.exit(f"{path}: SHA-256 {actual}, expected {expected}")

for name, line in receipt["links"].items():
    if not any(path.startswith("sysroot/lib/rustlib/x86_64-breenix/lib/libstd-")
               for path in line["inputs"]):
        continue
    links[name] = line
    for path in line["inputs"]:
        relative = Path(path)
        if relative.is_absolute() or ".." in relative.parts:
            sys.exit(f"{path}: expected a fixture-relative input")
        if path not in copied:
            verify(source / relative, receipt["inputs"][path]["sha256"])
            destination = twin / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / relative, destination)
            copied.add(path)

if set(links) != names:
    sys.exit(f"expected {sorted(names)}, found {sorted(links)}")

for name, line in links.items():
    argv = line["argv"].copy()
    output = argv.index("-o") + 1
    reference = argv[output]
    if reference != f"links/{name}/reference.elf":
        sys.exit(f"{name}: unexpected reference path {reference}")
    verify(source / reference, line["reference_sha256"])
    for index, arg in enumerate(argv):
        if arg == "-L":
            search = Path(argv[index + 1])
            if search.is_absolute() or ".." in search.parts:
                sys.exit(f"{search}: expected a fixture-relative search directory")
            if (source / search).is_dir():
                (twin / search).mkdir(parents=True, exist_ok=True)
    print("\0".join([reference, *argv]))
"#;

fn defined_globals(elf: &Elf<'_>) -> BTreeSet<Vec<u8>> {
    let table = (1..elf.section_count())
        .find(|&index| elf.section(index).unwrap().kind == SHT_SYMTAB)
        .expect("executable is missing its symbol table");
    let symbols = elf.symbols(table).unwrap();
    (1..symbols.len())
        .map(|index| symbols.get(index).unwrap())
        .filter(|symbol| {
            symbol.section != SHN_UNDEF && matches!(symbol.binding, STB_GLOBAL | STB_WEAK)
        })
        .map(|symbol| symbol.name.to_vec())
        .collect()
}

fn replay(root: &Path, output: &Path, args: &[&str]) -> Vec<u8> {
    let mut args: Vec<_> = args.iter().map(Path::new).collect();
    let destination = args.iter().position(|&arg| arg == Path::new("-o")).unwrap() + 1;
    args[destination] = output;
    run(Command::new(env!("CARGO_BIN_EXE_bld"))
        .env_clear()
        .current_dir(root)
        .args(args));
    fs::read(output).unwrap()
}

#[test]
fn rustc_link_line_recorded_on_mac_links() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = std::env::var_os("BREENIX_K6_FIXTURE")
        .map_or_else(|| repo.join("build/rustc-port/k6/fixture"), PathBuf::from)
        .canonicalize()
        .expect("missing K6 Mac twin link fixture");
    let scratch = Scratch::new();
    let twin = scratch.0.canonicalize().unwrap();
    let lines = run(Command::new("python3")
        .args(["-c", PREPARE_REPLAY])
        .arg(repo.join("toolchain/rust-host/k6/x86_64.json"))
        .arg(&fixture)
        .arg(&twin));
    for line in std::str::from_utf8(&lines).unwrap().lines() {
        let fields: Vec<_> = line.split('\0').collect();
        let [reference_path, args @ ..] = fields.as_slice() else {
            panic!("malformed replay line {line:?}");
        };
        let candidate = replay(&fixture, &twin.join("first.elf"), args);
        assert!(
            candidate == replay(&twin, &twin.join("second.elf"), args),
            "{reference_path}: link bytes depend on the fixture root"
        );
        let verdict = elfcheck::check_machine(
            &candidate,
            elfcheck::Kind::Executable,
            elfcheck::Machine::X86_64,
        );
        assert!(
            verdict.error.is_none() && verdict.findings.is_empty(),
            "{reference_path}: {verdict:?}"
        );
        let reference = fs::read(fixture.join(reference_path)).unwrap();
        let reference = Elf::parse(&reference).unwrap();
        let candidate = Elf::parse(&candidate).unwrap();
        assert_eq!(
            candidate.header().entry,
            reference.header().entry,
            "{reference_path}: entry address"
        );
        let expected = defined_globals(&reference);
        let actual = defined_globals(&candidate);
        let names = |names: Vec<&Vec<u8>>| -> Vec<String> {
            names
                .into_iter()
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .collect()
        };
        assert!(
            actual == expected,
            "{reference_path}: missing {:?}, extra {:?}",
            names(expected.difference(&actual).collect()),
            names(actual.difference(&expected).collect()),
        );
    }
}
