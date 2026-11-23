//! Exercise the bounds checks over deterministic truncations and corruptions,
//! and the AArch64 relocation verdicts (`aarch64_rel_verdicts`).

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("../..")
        .canonicalize()
        .expect("the repository root above toolchain/elfcheck exists")
}

/// A scratch directory removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = env::temp_dir().join(format!("elfcheck-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("create the scratch directory");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Relocatable,
    Exec,
}

struct Case {
    mode: Mode,
    name: String,
    path: String,
}

fn write_fixtures(root: &Path, directory: &Path) -> Vec<Case> {
    let script = root.join("toolchain/tests/elfcheck/write_fixtures.py");
    let output = Command::new("python3")
        .arg(&script)
        .arg(directory)
        .output()
        .expect("run python3 toolchain/tests/elfcheck/write_fixtures.py");
    assert!(
        output.status.success(),
        "write_fixtures.py failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("the manifest is UTF-8");
    text.lines()
        .map(|line| {
            let mut fields = line.split('\t');
            let mode = match fields.next() {
                Some("relocatable") => Mode::Relocatable,
                Some("exec") => Mode::Exec,
                _ => panic!("unknown fixture mode in {line:?}"),
            };
            let name = fields.next().expect("fixture name");
            let path = fields.next().expect("fixture path");
            assert!(fields.next().is_none(), "unexpected fixture line {line:?}");
            Case {
                mode,
                name: name.into(),
                path: path.into(),
            }
        })
        .collect()
}

/// Plan section 2: malformed input is a located error, not a panic.
///
/// Each fixture file is truncated (at each length for the two baselines, at
/// each 16th length for the controls), and each baseline is corrupted at each
/// byte (inverted) and at each 4- and 8-aligned offset (a field of 0xff bytes),
/// and checked in-process. A panic fails the test, and so does a
/// `ReadError`: that marks a read elfcheck did not bound first, which is a
/// defect in elfcheck, not in the input. A file shorter than the ELF header
/// must report `ehdr.size`.
#[test]
fn malformed_input_is_error_not_panic() {
    use elfcheck::{Check, Kind, Report};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    let root = repo_root();
    let scratch = Scratch::new("malformed");
    let cases = write_fixtures(&root, &scratch.0.join("fixtures"));
    let mut problems = Vec::new();
    let mut inputs = 0u64;

    let mut probe = |label: &str, what: String, data: &[u8], kind: Kind| {
        inputs += 1;
        match catch_unwind(AssertUnwindSafe(|| elfcheck::check(data, kind))) {
            Err(_) => problems.push(format!("{label}: {what}: panicked")),
            Ok(Report {
                error: Some(error), ..
            }) => problems.push(format!("{label}: {what}: {error}")),
            Ok(report) => {
                if data.len() < 64 && !report.findings.iter().any(|f| f.check == Check::EhdrSize) {
                    problems.push(format!("{label}: {what}: no ehdr.size finding"));
                }
            }
        }
    };

    for case in &cases {
        let data = fs::read(&case.path).unwrap_or_else(|e| panic!("read {}: {e}", case.path));
        let kind = match case.mode {
            Mode::Relocatable => Kind::Relocatable,
            Mode::Exec => Kind::Executable,
        };
        let label = format!("{:?} {}", case.mode, case.name);
        let baseline = case.name == "baseline";
        for length in (0..data.len()).step_by(if baseline { 1 } else { 16 }) {
            probe(
                &label,
                format!("truncated to {length} bytes"),
                &data[..length],
                kind,
            );
        }
        if !baseline {
            continue;
        }
        for at in 0..data.len() {
            let mut bad = data.clone();
            bad[at] ^= 0xff;
            probe(&label, format!("byte {at} inverted"), &bad, kind);
            for width in [4, 8] {
                if at % width == 0 && at + width <= data.len() {
                    let mut bad = data.clone();
                    bad[at..at + width].fill(0xff);
                    probe(
                        &label,
                        format!("{width} bytes at {at} set to 0xff"),
                        &bad,
                        kind,
                    );
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{} of {inputs} malformed inputs:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

// AArch64 relocatable fixtures for `aarch64_rel_verdicts`, built by hand from
// the ELF gABI (Listings 2.1, 3.1, 5.1 and 6.1) and AAELF64 2025Q1 (E25 ruling
// 29) alone.

/// AAELF64 5.2: e_machine.
const EM_AARCH64: u16 = 183;
/// gABI Table 2.5, the value the AArch64 negative control puts in e_machine.
const EM_X86_64: u16 = 62;

// AAELF64 tables in 5.7.5 (data), 5.7.6 (static AArch64), 5.7.11.4 (local
// exec TLS) and 5.7.13 (dynamic), ELF64 codes.
const R_AARCH64_ABS64: u32 = 257;
const R_AARCH64_ABS32: u32 = 258;
const R_AARCH64_PREL32: u32 = 261;
const R_AARCH64_LD_PREL_LO19: u32 = 273;
const R_AARCH64_ADR_PREL_PG_HI21: u32 = 275;
const R_AARCH64_ADD_ABS_LO12_NC: u32 = 277;
const R_AARCH64_LDST8_ABS_LO12_NC: u32 = 278;
const R_AARCH64_TSTBR14: u32 = 279;
const R_AARCH64_CONDBR19: u32 = 280;
/// No ELF64 relocation has code 281 (AAELF64 5.7.6 and 5.7.15).
const R_AARCH64_UNALLOCATED_281: u32 = 281;
const R_AARCH64_JUMP26: u32 = 282;
const R_AARCH64_CALL26: u32 = 283;
const R_AARCH64_LDST16_ABS_LO12_NC: u32 = 284;
const R_AARCH64_LDST32_ABS_LO12_NC: u32 = 285;
const R_AARCH64_LDST64_ABS_LO12_NC: u32 = 286;
const R_AARCH64_LDST128_ABS_LO12_NC: u32 = 299;
const R_AARCH64_ADR_GOT_PAGE: u32 = 311;
const R_AARCH64_LD64_GOT_LO12_NC: u32 = 312;
const R_AARCH64_TLSLE_ADD_TPREL_HI12: u32 = 549;
const R_AARCH64_TLSLE_ADD_TPREL_LO12_NC: u32 = 551;
const R_AARCH64_GLOB_DAT: u32 = 1025;

// Symbol indexes in the fixture's symbol table (see `aarch64_object`).
const SYM_FUNC: u32 = 3;
const SYM_OBJ: u32 = 4;
const SYM_TVAR: u32 = 5;
const SYM_EXT: u32 = 6;

/// The bytes of .text (16 A64 instructions), .data and .tdata.
const TEXT_SIZE: u64 = 64;
const DATA_SIZE: u64 = 16;
const TDATA_SIZE: u64 = 8;

#[derive(Clone, Copy)]
struct Rel {
    offset: u64,
    symbol: u32,
    kind: u32,
}

/// One AArch64 ET_REL object: its e_machine and the RELA entries against
/// .text and .data.
#[derive(Clone)]
struct A64Object {
    machine: u16,
    text: Vec<Rel>,
    data: Vec<Rel>,
}

impl A64Object {
    /// The positive fixture: the 19 relocations the A1 brief names, 16 on
    /// .text and 3 on .data, each on a place inside its section and the last
    /// of each section ending at the section's end.
    fn baseline() -> A64Object {
        let text = [
            (R_AARCH64_CALL26, SYM_EXT),
            (R_AARCH64_JUMP26, SYM_EXT),
            (R_AARCH64_CONDBR19, SYM_FUNC),
            (R_AARCH64_TSTBR14, SYM_FUNC),
            (R_AARCH64_LD_PREL_LO19, SYM_OBJ),
            (R_AARCH64_ADR_PREL_PG_HI21, SYM_OBJ),
            (R_AARCH64_ADD_ABS_LO12_NC, SYM_OBJ),
            (R_AARCH64_LDST8_ABS_LO12_NC, SYM_OBJ),
            (R_AARCH64_LDST16_ABS_LO12_NC, SYM_OBJ),
            (R_AARCH64_LDST32_ABS_LO12_NC, SYM_OBJ),
            (R_AARCH64_LDST64_ABS_LO12_NC, SYM_OBJ),
            (R_AARCH64_LDST128_ABS_LO12_NC, SYM_OBJ),
            (R_AARCH64_ADR_GOT_PAGE, SYM_EXT),
            (R_AARCH64_LD64_GOT_LO12_NC, SYM_EXT),
            (R_AARCH64_TLSLE_ADD_TPREL_HI12, SYM_TVAR),
            (R_AARCH64_TLSLE_ADD_TPREL_LO12_NC, SYM_TVAR),
        ];
        assert_eq!(text.len() as u64 * 4, TEXT_SIZE);
        A64Object {
            machine: EM_AARCH64,
            text: (0u64..)
                .zip(text)
                .map(|(i, (kind, symbol))| Rel {
                    offset: i * 4,
                    symbol,
                    kind,
                })
                .collect(),
            data: vec![
                Rel {
                    offset: 0,
                    symbol: SYM_FUNC,
                    kind: R_AARCH64_ABS64,
                },
                Rel {
                    offset: 8,
                    symbol: SYM_OBJ,
                    kind: R_AARCH64_ABS32,
                },
                Rel {
                    offset: 12,
                    symbol: SYM_FUNC,
                    kind: R_AARCH64_PREL32,
                },
            ],
        }
    }

    fn text_rel(&mut self, kind: u32) -> &mut Rel {
        self.text
            .iter_mut()
            .find(|r| r.kind == kind)
            .expect("the baseline has the relocation")
    }

    fn data_rel(&mut self, kind: u32) -> &mut Rel {
        self.data
            .iter_mut()
            .find(|r| r.kind == kind)
            .expect("the baseline has the relocation")
    }

    /// Lays the object out: the ELF header, .text, .data, .tdata, the two
    /// RELA tables, .symtab, .strtab, .shstrtab and the section header table.
    fn bytes(&self) -> Vec<u8> {
        fn put(out: &mut Vec<u8>, value: u64, width: usize) {
            out.extend_from_slice(&value.to_le_bytes()[..width]);
        }
        fn pad(out: &mut Vec<u8>, align: usize) {
            while out.len() % align != 0 {
                out.push(0);
            }
        }
        fn rela(rels: &[Rel]) -> Vec<u8> {
            let mut out = Vec::new();
            for r in rels {
                put(&mut out, r.offset, 8);
                put(&mut out, (u64::from(r.symbol) << 32) | u64::from(r.kind), 8);
                put(&mut out, 0, 8);
            }
            out
        }
        // gABI Tables 3.2, 3.3 and 5.2-5.4.
        const SHT_PROGBITS: u64 = 1;
        const SHT_SYMTAB: u64 = 2;
        const SHT_STRTAB: u64 = 3;
        const SHT_RELA: u64 = 4;
        const SHF_WRITE: u64 = 0x1;
        const SHF_ALLOC: u64 = 0x2;
        const SHF_EXECINSTR: u64 = 0x4;
        const SHF_INFO_LINK: u64 = 0x40;
        const SHF_TLS: u64 = 0x400;
        const STB_LOCAL: u8 = 0;
        const STB_GLOBAL: u8 = 1;
        const STT_NOTYPE: u8 = 0;
        const STT_OBJECT: u8 = 1;
        const STT_FUNC: u8 = 2;
        const STT_TLS: u8 = 6;

        let names = b"\0$x\0$d\0func\0obj\0tvar\0ext\0";
        // (name offset, binding, type, section, value, size), in index order;
        // SYM_FUNC, SYM_OBJ, SYM_TVAR and SYM_EXT name entries 3 to 6.
        let symbols: [(u64, u8, u8, u64, u64, u64); 6] = [
            (1, STB_LOCAL, STT_NOTYPE, 1, 0, 0), // $x: A64 code (AAELF64 5.6.5)
            (4, STB_LOCAL, STT_NOTYPE, 2, 0, 0), // $d: data
            (7, STB_GLOBAL, STT_FUNC, 1, 0, TEXT_SIZE),
            (12, STB_GLOBAL, STT_OBJECT, 2, 0, DATA_SIZE),
            (16, STB_GLOBAL, STT_TLS, 3, 0, TDATA_SIZE),
            (21, STB_GLOBAL, STT_NOTYPE, 0, 0, 0),
        ];
        let mut symtab = vec![0u8; 24];
        for (name, bind, kind, shndx, value, size) in symbols {
            put(&mut symtab, name, 4);
            symtab.push((bind << 4) | kind);
            symtab.push(0);
            put(&mut symtab, shndx, 2);
            put(&mut symtab, value, 8);
            put(&mut symtab, size, 8);
        }
        let shnames =
            b"\0.text\0.data\0.tdata\0.rela.text\0.rela.data\0.symtab\0.strtab\0.shstrtab\0";
        let nop = 0xd503_201f_u32.to_le_bytes();
        let text: Vec<u8> = nop
            .iter()
            .copied()
            .cycle()
            .take(TEXT_SIZE as usize)
            .collect();
        let contents: [(Vec<u8>, usize); 8] = [
            (text, 4),
            (vec![0; DATA_SIZE as usize], 8),
            (vec![0; TDATA_SIZE as usize], 8),
            (rela(&self.text), 8),
            (rela(&self.data), 8),
            (symtab, 8),
            (names.to_vec(), 1),
            (shnames.to_vec(), 1),
        ];

        let mut out = vec![0u8; 64];
        let mut placed = Vec::new();
        for (bytes, align) in &contents {
            pad(&mut out, *align);
            placed.push((out.len() as u64, bytes.len() as u64));
            out.extend_from_slice(bytes);
        }
        pad(&mut out, 8);
        let shoff = out.len() as u64;

        // (name, type, flags, link, info, align, entsize) for sections 1-8.
        let headers: [(u64, u64, u64, u64, u64, u64, u64); 8] = [
            (1, SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR, 0, 0, 4, 0),
            (7, SHT_PROGBITS, SHF_ALLOC | SHF_WRITE, 0, 0, 8, 0),
            (
                13,
                SHT_PROGBITS,
                SHF_ALLOC | SHF_WRITE | SHF_TLS,
                0,
                0,
                8,
                0,
            ),
            (20, SHT_RELA, SHF_INFO_LINK, 6, 1, 8, 24),
            (31, SHT_RELA, SHF_INFO_LINK, 6, 2, 8, 24),
            (42, SHT_SYMTAB, 0, 7, 3, 8, 24),
            (50, SHT_STRTAB, 0, 0, 0, 1, 0),
            (58, SHT_STRTAB, 0, 0, 0, 1, 0),
        ];
        out.extend_from_slice(&[0; 64]);
        for ((name, kind, flags, link, info, align, entsize), (offset, size)) in
            headers.into_iter().zip(placed)
        {
            put(&mut out, name, 4);
            put(&mut out, kind, 4);
            put(&mut out, flags, 8);
            put(&mut out, 0, 8);
            put(&mut out, offset, 8);
            put(&mut out, size, 8);
            put(&mut out, link, 4);
            put(&mut out, info, 4);
            put(&mut out, align, 8);
            put(&mut out, entsize, 8);
        }

        // gABI Listing 2.1 and Tables 2.1-2.6.
        let mut ehdr = vec![0x7f, b'E', b'L', b'F', 2, 1, 1];
        ehdr.resize(16, 0);
        put(&mut ehdr, 1, 2); // ET_REL
        put(&mut ehdr, u64::from(self.machine), 2);
        put(&mut ehdr, 1, 4); // EV_CURRENT
        put(&mut ehdr, 0, 8); // e_entry
        put(&mut ehdr, 0, 8); // e_phoff
        put(&mut ehdr, shoff, 8);
        put(&mut ehdr, 0, 4); // e_flags (AAELF64 5.2)
        put(&mut ehdr, 64, 2); // e_ehsize
        put(&mut ehdr, 0, 2); // e_phentsize
        put(&mut ehdr, 0, 2); // e_phnum
        put(&mut ehdr, 64, 2); // e_shentsize
        put(&mut ehdr, 9, 2); // e_shnum
        put(&mut ehdr, 8, 2); // e_shstrndx
        out[..64].copy_from_slice(&ehdr);
        out
    }
}

/// Runs the Cargo-built elfcheck on `path` with `args` before it and returns
/// its exit status, the check ids it reported for `path`, and its output.
fn run_elfcheck(args: &[&str], path: &Path) -> (Option<i32>, BTreeSet<String>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_elfcheck"))
        .args(args)
        .arg(path)
        .output()
        .expect("run the elfcheck binary");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let prefix = format!("{}: ", path.display());
    let found = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .filter(|rest| *rest != "ok")
        .filter_map(|rest| rest.split(": ").next())
        .map(str::to_owned)
        .collect();
    let text = format!("{stdout}{}", String::from_utf8_lossy(&output.stderr));
    (output.status.code(), found, text)
}

/// AAELF64 relocation verdicts for ET_REL objects (plan section 4.1, A1).
///
/// The positive fixture carries the 19 relocations the A1 brief names, on
/// .text, .data and an STT_TLS symbol, and must pass `--machine aarch64`
/// with no finding. Each negative control damages one property and must
/// fail exactly the one existing check named beside it. Checked without
/// `--machine`, the positive fixture must fail `ehdr.machine`: the default
/// stays x86-64.
#[test]
fn aarch64_rel_verdicts() {
    let scratch = Scratch::new("aarch64");
    let mut cases = vec![("baseline", A64Object::baseline(), BTreeSet::new())];
    let mut control = |name: &'static str, check: &str, damage: &dyn Fn(&mut A64Object)| {
        let mut object = A64Object::baseline();
        damage(&mut object);
        cases.push((name, object, BTreeSet::from([check.to_owned()])));
    };
    control("em-x86-64", "ehdr.machine", &|o| o.machine = EM_X86_64);
    control("dynamic-only-type", "rela.type", &|o| {
        o.data_rel(R_AARCH64_ABS64).kind = R_AARCH64_GLOB_DAT
    });
    control("unallocated-type", "rela.type", &|o| {
        o.text_rel(R_AARCH64_CONDBR19).kind = R_AARCH64_UNALLOCATED_281
    });
    control("tlsle-on-non-tls-symbol", "rela.tls", &|o| {
        o.text_rel(R_AARCH64_TLSLE_ADD_TPREL_HI12).symbol = SYM_OBJ
    });
    // ABS64 writes 8 bytes (AAELF64 5.7.5): at offset 12 of the 16-byte
    // .data it passes the end, where a 4-byte field would still fit.
    control("abs64-past-section-end", "rela.offset", &|o| {
        o.data_rel(R_AARCH64_ABS64).offset = DATA_SIZE - 4
    });

    let mut problems = Vec::new();
    for (name, object, expected) in &cases {
        let path = scratch.0.join(format!("{name}.o"));
        fs::write(&path, object.bytes()).expect("write the fixture");
        let (status, found, text) = run_elfcheck(&["--machine", "aarch64"], &path);
        let want = if expected.is_empty() { 0 } else { 1 };
        if status != Some(want) || &found != expected {
            problems.push(format!(
                "{name}: exit {status:?} (want {want}), found {found:?} (want {expected:?}):\n{text}"
            ));
        }
    }
    let baseline = scratch.0.join("baseline.o");
    let (status, found, text) = run_elfcheck(&[], &baseline);
    if status != Some(1) || !found.contains("ehdr.machine") {
        problems.push(format!(
            "baseline without --machine: exit {status:?}, found {found:?}, want ehdr.machine:\n{text}"
        ));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
