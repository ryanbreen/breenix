//! Layout expectations come from linker-hosted.ld, GNU ld §3 and the ELF gABI.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bobj::elf::{Elf, ProgramHeader, SectionHeader, PF_R, PF_W, PT_LOAD, PT_TLS, SHN_UNDEF};

const PT_GNU_EH_FRAME: u32 = 0x6474_e550; // LSB Core 5.0 Table 11-1.

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("bld-hosted-{}-{nanos}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(command: &mut Command) -> Vec<u8> {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn section<'a>(elf: &Elf<'a>, name: &[u8]) -> (SectionHeader, &'a [u8]) {
    let index = (1..elf.section_count())
        .find(|&index| elf.section_name(index).unwrap() == name)
        .unwrap_or_else(|| panic!("missing section {}", String::from_utf8_lossy(name)));
    (
        elf.section(index).unwrap(),
        elf.section_data(index).unwrap().unwrap(),
    )
}

fn program(elf: &Elf<'_>, kind: u32, flags: u32) -> ProgramHeader {
    let matches: Vec<_> = (0..elf.program_count())
        .map(|index| elf.program(index).unwrap())
        .filter(|header| header.kind == kind && header.flags == flags)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected one program header {kind:#x}/{flags}"
    );
    matches[0]
}

fn check_hosted_elf(bytes: &[u8]) -> Elf<'_> {
    let verdict =
        elfcheck::check_machine(bytes, elfcheck::Kind::Executable, elfcheck::Machine::X86_64);
    assert!(
        verdict.error.is_none() && verdict.findings.is_empty(),
        "{verdict:?}"
    );
    let elf = Elf::parse(bytes).unwrap();
    // linker-hosted.ld:39-47 and LSB §10.6. The unwind index alone is in
    // PT_GNU_EH_FRAME, and both sections are in the read-only PT_LOAD.
    let unwind = program(&elf, PT_GNU_EH_FRAME, PF_R);
    let ro = program(&elf, PT_LOAD, PF_R);
    let rw = program(&elf, PT_LOAD, PF_R | PF_W);
    let (index, index_bytes) = section(&elf, b".eh_frame_hdr");
    let (frames, _) = section(&elf, b".eh_frame");
    assert_eq!(index_bytes[0], 1);
    assert_eq!(unwind.offset, index.offset);
    assert_eq!(unwind.virtual_address, index.address);
    assert_eq!(unwind.file_size, index.size);
    assert_eq!(unwind.memory_size, index.size);
    for header in [index, frames] {
        assert!(header.address >= ro.virtual_address);
        assert!(header.address + header.size <= ro.virtual_address + ro.memory_size);
    }
    // linker-hosted.ld:26-29, 53-56 and gABI §7.1.
    let ro_end = ro.virtual_address + ro.memory_size;
    assert_eq!(rw.virtual_address % 4096, 0);
    assert!(ro_end.div_ceil(4096) <= rw.virtual_address / 4096);
    elf
}

const FIXTURE: &str = r#"
.section .text._start,"ax",@progbits
.globl _start
.type _start,@function
_start:
.cfi_startproc
    movl %fs:tls_init@TPOFF, %eax
    movl %fs:tls_zero@TPOFF, %eax
    leaq bounds(%rip), %rax
    ret
.cfi_endproc

.section .text.calls,"ax",@progbits
.globl pre, low, high, last
pre:  ret
low:  ret
high: ret
last: ret

.section .preinit_array,"aw",@preinit_array
.quad pre

.section .init_array.100,"aw",@init_array
.quad high
.section .init_array,"aw",@init_array
.quad last
.section .init_array.20,"aw",@init_array
.quad low

.section .fini_array.100,"aw",@fini_array
.quad high
.section .fini_array,"aw",@fini_array
.quad last
.section .fini_array.20,"aw",@fini_array
.quad low

.section .data.bounds,"aw",@progbits
.globl bounds
bounds:
.quad __breenix_text_start, __breenix_got_start
.quad __preinit_array_start, __preinit_array_end
.quad __init_array_start, __init_array_end
.quad __fini_array_start, __fini_array_end
.quad __breenix_preinit_array_start, __breenix_preinit_array_end
.quad __breenix_init_array_start, __breenix_init_array_end
.quad __breenix_fini_array_start, __breenix_fini_array_end

.section .tdata,"awT",@progbits
.balign 8
tls_init: .quad 42
.section .tbss,"awT",@nobits
.balign 16
tls_zero: .zero 8
"#;

fn words(bytes: &[u8]) -> Vec<u64> {
    bytes
        .chunks_exact(8)
        .map(|word| u64::from_le_bytes(word.try_into().unwrap()))
        .collect()
}

#[test]
fn links_hosted_script_layout() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("fixture.s"), FIXTURE).unwrap();
    run(
        Command::new(std::env::var_os("CLANG").unwrap_or_else(|| "clang".into()))
            .current_dir(&scratch.0)
            .args([
                "-target",
                "x86_64-unknown-none",
                "-c",
                "fixture.s",
                "-o",
                "fixture.o",
            ]),
    );
    run(Command::new(env!("CARGO_BIN_EXE_bld"))
        .env_clear()
        .current_dir(&scratch.0)
        .args(["-flavor", "gnu", "--gc-sections", "--eh-frame-hdr", "-T"])
        .arg(repo().join("userspace/programs/linker-hosted.ld"))
        .args(["-o", "fixture.elf", "fixture.o"]));
    let bytes = fs::read(scratch.0.join("fixture.elf")).unwrap();
    let elf = check_hosted_elf(&bytes);
    let symbols = elf.static_symbols().unwrap().unwrap();
    let address = |name: &[u8]| {
        (1..symbols.len())
            .map(|i| symbols.get(i).unwrap())
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("missing symbol {}", String::from_utf8_lossy(name)))
            .value
    };
    let (text, _) = section(&elf, b".text");
    let (data, data_bytes) = section(&elf, b".data");
    let bounds = (address(b"bounds") - data.address) as usize;
    let pointers = words(&data_bytes[bounds..bounds + 14 * 8]);
    // Script line 22 and line 59. This fixture requests no GOT entries.
    assert_eq!(pointers[..2], [text.address, 0]);
    // Script lines 61-89 and gABI Table 3.8. The relocations name both
    // spellings of the bounds, including the hidden aliases the CRT uses.
    for (index, (name, kind, expected)) in [
        (b".preinit_array".as_slice(), 16, vec![address(b"pre")]),
        (
            b".init_array",
            14,
            vec![address(b"low"), address(b"high"), address(b"last")],
        ),
        (
            b".fini_array",
            15,
            vec![address(b"low"), address(b"high"), address(b"last")],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (header, contents) = section(&elf, name);
        assert_eq!(header.kind, kind);
        assert_eq!(header.entry_size, 8);
        assert_eq!(contents.len(), expected.len() * 8);
        assert_eq!(words(contents), expected);
        let expected = [header.address, header.address + header.size];
        let first = 2 + 2 * index;
        assert_eq!(pointers[first..first + 2], expected);
        assert_eq!(pointers[first + 6..first + 8], expected);
    }
    // Script lines 91-99 and gABI §7.7. Only .tdata has file bytes.
    let tls = program(&elf, PT_TLS, PF_R);
    let rw = program(&elf, PT_LOAD, PF_R | PF_W);
    let (init, _) = section(&elf, b".tdata");
    assert_eq!(tls.offset, init.offset);
    assert_eq!(tls.virtual_address, init.address);
    assert_eq!(tls.file_size, 8);
    assert_eq!(tls.memory_size, 24);
    assert_eq!(tls.alignment, 16);
    assert!(init.offset >= rw.offset);
    assert!(init.offset + init.size <= rw.offset + rw.file_size);
}

// Read the recorded argv and verify every input and the reference output
// before replaying it. Stage the same relative paths under a second root.
const PREPARE_REPLAY: &str = r#"
import hashlib
import json
from pathlib import Path
import shutil
import sys

record, source, twin = map(Path, sys.argv[1:])
receipt = json.loads(record.read_text())
line = receipt["links"]["catch-unwind"]
inputs = set(line["inputs"])
required = {"lib/hosted/libbunwind.a", "lib/hosted/libc.a", "lib/hosted/linker-hosted.ld"}
if not required <= inputs or not any(
    p.startswith("sysroot/lib/rustlib/x86_64-breenix-hosted/lib/libstd-") for p in inputs
):
    sys.exit("catch-unwind receipt is missing hosted link inputs")

def verify(path, expected):
    with path.open("rb") as file:
        actual = hashlib.file_digest(file, "sha256").hexdigest()
    if actual != expected:
        sys.exit(f"{path}: SHA-256 {actual}, expected {expected}")

def relative(path):
    path = Path(path)
    if path.is_absolute() or ".." in path.parts:
        sys.exit(f"{path}: expected fixture-relative path")
    return path

for path in line["inputs"]:
    destination = twin / relative(path)
    verify(source / path, receipt["inputs"][path]["sha256"])
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source / path, destination)

argv = line["argv"]
reference = argv[argv.index("-o") + 1]
if reference != "links/catch-unwind/reference.elf":
    sys.exit(f"unexpected reference path {reference}")
verify(source / reference, line["reference_sha256"])
for index, arg in enumerate(argv):
    if arg == "-L":
        search = relative(argv[index + 1])
        if (source / search).is_dir():
            (twin / search).mkdir(parents=True, exist_ok=True)
print("\0".join([reference, *argv]), end="")
"#;

fn defined_globals(elf: &Elf<'_>) -> BTreeSet<Vec<u8>> {
    use bobj::elf::{STB_GLOBAL, STB_WEAK};

    let symbols = elf.static_symbols().unwrap().unwrap();
    (1..symbols.len())
        .map(|index| symbols.get(index).unwrap())
        .filter(|sym| sym.section != SHN_UNDEF && matches!(sym.binding, STB_GLOBAL | STB_WEAK))
        .map(|sym| sym.name.to_vec())
        .collect()
}

#[test]
fn rustc_hosted_link_line_recorded_on_mac_links() {
    let fixture = std::env::var_os("BREENIX_K6_FIXTURE")
        .map_or_else(|| repo().join("build/rustc-port/k6/fixture"), PathBuf::from)
        .canonicalize()
        .expect("missing K6 Mac twin link fixture");
    let scratch = Scratch::new();
    let twin = scratch.0.join("replay");
    let line = run(Command::new("python3")
        .args(["-c", PREPARE_REPLAY])
        .arg(repo().join("toolchain/rust-host/k6/x86_64.json"))
        .arg(&fixture)
        .arg(&twin));
    let (reference, args) = std::str::from_utf8(&line)
        .unwrap()
        .split_once('\0')
        .unwrap();
    let mut args: Vec<_> = args.split('\0').map(Path::new).collect();
    let index = args.iter().position(|&arg| arg == Path::new("-o")).unwrap() + 1;
    let output = scratch.0.join("candidate.elf");
    args[index] = &output;
    let replay = |root: &Path| {
        run(Command::new(env!("CARGO_BIN_EXE_bld"))
            .env_clear()
            .current_dir(root)
            .args(&args));
        fs::read(&output).unwrap()
    };
    let bytes = replay(&fixture);
    assert!(
        bytes == replay(&twin),
        "hosted link bytes depend on fixture root"
    );
    let actual = check_hosted_elf(&bytes);
    let reference_bytes = fs::read(fixture.join(reference)).unwrap();
    let reference = Elf::parse(&reference_bytes).unwrap();
    assert_eq!(
        actual.header().entry,
        reference.header().entry,
        "entry address"
    );
    let expected = defined_globals(&reference);
    let actual = defined_globals(&actual);
    let names = |set: Vec<&Vec<u8>>| {
        set.into_iter()
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect::<Vec<_>>()
    };
    assert!(
        actual == expected,
        "missing {:?}, extra {:?}",
        names(expected.difference(&actual).collect()),
        names(actual.difference(&expected).collect()),
    );
}
