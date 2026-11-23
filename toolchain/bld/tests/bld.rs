//! ELF gABI §§3.9, 5 and 6 and AMD64 psABI §4.4.1, Table 4.9.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use bobj::ar::{self, IndexEntry};
use bobj::elf::{
    write_rel, Elf, RelSection, RelSymbol, Rela, EM_X86_64, ET_REL, PT_LOAD, SHF_ALLOC,
    SHF_EXECINSTR, SHF_GROUP, SHF_WRITE, SHT_GROUP, SHT_PROGBITS, SHT_RELA, STB_GLOBAL, STB_LOCAL,
    STT_NOTYPE, STT_SECTION,
};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("bld-r-{}-{nanos}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn link(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bld"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }

    fn linked(&self, args: &[&str], file: &str) -> Vec<u8> {
        let output = self.link(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::read(self.0.join(file)).unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn section<'a>(
    name: &'a [u8],
    alignment: u64,
    bytes: &'a [u8],
    relocations: &'a [Rela],
) -> RelSection<'a> {
    RelSection {
        name,
        kind: SHT_PROGBITS,
        flags: SHF_ALLOC
            | if name.starts_with(b".text") {
                SHF_EXECINSTR
            } else {
                SHF_WRITE
            },
        alignment,
        bytes,
        size: bytes.len() as u64,
        relocations,
    }
}

fn symbol(name: &[u8], section: u16, value: u64, binding: u8) -> RelSymbol<'_> {
    RelSymbol {
        other: 0,
        name,
        section,
        kind: STT_NOTYPE,
        binding,
        value,
        size: 0,
    }
}

fn absolute(offset: u64, symbol: u32, addend: i64) -> Rela {
    Rela {
        offset,
        symbol,
        kind: 1,
        addend,
    } // R_X86_64_64, S + A.
}

fn first(entry: &[u8]) -> Vec<u8> {
    write_rel(
        EM_X86_64,
        &[
            section(b".text", 8, &[0; 8], &[absolute(0, 1, 0)]),
            section(b".data", 8, &[0xa1; 8], &[]),
        ],
        &[
            symbol(entry, 1, 0, STB_GLOBAL),
            symbol(b"worker", 0, 0, STB_GLOBAL),
        ],
    )
    .unwrap()
}

fn worker(value: u64) -> Vec<u8> {
    write_rel(
        EM_X86_64,
        &[
            section(
                b".text",
                16,
                &[0; 16],
                &[absolute(0, 1, 2), absolute(8, 2, 3)],
            ),
            section(b".data", 8, &[0xb2; 8], &[]),
        ],
        &[
            symbol(b"worker", 1, 0, STB_GLOBAL),
            RelSymbol {
                kind: STT_SECTION,
                ..symbol(b"", 2, value, STB_LOCAL)
            },
            symbol(b"external", 0, 0, STB_GLOBAL),
        ],
    )
    .unwrap()
}

fn write_parts(root: &Scratch, entry: &[u8]) {
    fs::write(root.0.join("first.o"), first(entry)).unwrap();
    let worker = worker(0);
    let archive = ar::write(
        &[(b"worker.o", &worker), (b"unused.o", b"unused member")],
        &[IndexEntry {
            name: b"worker",
            member: 0,
        }],
    )
    .unwrap();
    fs::write(root.0.join("parts.a"), archive).unwrap();
}

fn named(elf: &Elf<'_>, name: &[u8]) -> usize {
    (1..elf.section_count())
        .find(|&i| elf.section_name(i).unwrap() == name)
        .unwrap()
}

#[test]
fn r_keeps_relocations_rebased() {
    let root = Scratch::new();
    write_parts(&root, b"first");
    // A partial link has no entry-point requirement.
    let partial = root.linked(
        &["-r", "-o", "partial.o", "first.o", "parts.a"],
        "partial.o",
    );
    let elf = Elf::parse(&partial).unwrap();
    assert_eq!(elf.header().file_type, ET_REL);
    assert_eq!(elf.header().entry, 0);
    assert_eq!(elf.program_count(), 0);
    let text = named(&elf, b".text");
    let data = named(&elf, b".data");
    assert_eq!(elf.section(text).unwrap().alignment, 16);
    assert_eq!(elf.section_data(text).unwrap().unwrap(), &[0; 32]);
    assert_eq!(
        elf.section_data(data).unwrap().unwrap(),
        &[0xa1; 8].into_iter().chain([0xb2; 8]).collect::<Vec<_>>()
    );
    let symbols = elf.static_symbols().unwrap().unwrap();
    let relas = elf.relocations(named(&elf, b".rela.text")).unwrap();
    assert_eq!(relas.len(), 3);
    for (index, (offset, kind, addend, name, binding, section, value)) in [
        (0, STT_NOTYPE, 0, &b"worker"[..], STB_GLOBAL, text, 16),
        (16, STT_SECTION, 2, &b""[..], STB_LOCAL, data, 8),
        (24, STT_NOTYPE, 3, &b"external"[..], STB_GLOBAL, 0, 0),
    ]
    .into_iter()
    .enumerate()
    {
        let rela = relas.get(index).unwrap();
        assert_eq!((rela.offset, rela.kind, rela.addend), (offset, 1, addend));
        let symbol = symbols.get(rela.symbol as usize).unwrap();
        assert_eq!(
            (
                symbol.name,
                symbol.binding,
                symbol.kind,
                usize::from(symbol.section),
                symbol.value
            ),
            (name, binding, kind, section, value)
        );
    }
}

// Make one group from a gABI §3.9 flag word and member indexes, using the
// ordinary writer only to supply the enclosing fixture's ELF records.
fn group(fill: u8) -> Vec<u8> {
    let words: Vec<_> = [1u32, 2, 3]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let mut bytes = write_rel(
        EM_X86_64,
        &[
            section(b".group", 4, &words, &[]),
            RelSection {
                flags: SHF_ALLOC | SHF_EXECINSTR | SHF_GROUP,
                ..section(b".text.group", 8, &[fill; 8], &[absolute(0, 0, 0)])
            },
        ],
        &[symbol(b"group_key", 2, 0, STB_GLOBAL)],
    )
    .unwrap();
    let shoff = Elf::parse(&bytes).unwrap().header().section_offset as usize;
    let group = shoff + 64;
    bytes[group + 4..group + 8].copy_from_slice(&SHT_GROUP.to_le_bytes());
    bytes[group + 8..group + 16].fill(0);
    bytes[group + 40..group + 44].copy_from_slice(&4u32.to_le_bytes()); // .symtab
    bytes[group + 44..group + 48].copy_from_slice(&1u32.to_le_bytes()); // signature
    bytes[group + 56..group + 64].copy_from_slice(&4u64.to_le_bytes());
    bytes[shoff + 3 * 64 + 9] |= 2; // SHF_GROUP on the relocation section.
    bytes
}

fn loads(data: &[u8]) -> Vec<(u64, u64, &[u8])> {
    let elf = Elf::parse(data).unwrap();
    (0..elf.program_count())
        .map(|i| elf.program(i).unwrap())
        .filter(|header| header.kind == PT_LOAD)
        .map(|header| {
            let start = header.offset as usize;
            (
                header.virtual_address,
                header.memory_size,
                &data[start..start + header.file_size as usize],
            )
        })
        .collect()
}

#[test]
fn r_output_links_like_its_parts() {
    let root = Scratch::new();
    write_parts(&root, b"_start");
    fs::write(root.0.join("group.o"), group(0xab)).unwrap();
    fs::write(root.0.join("duplicate.o"), group(0xcd)).unwrap();
    fs::write(
        root.0.join("last.o"),
        write_rel(
            EM_X86_64,
            &[section(b".data", 8, &[0xc3; 8], &[])],
            &[symbol(b"external", 1, 0, STB_GLOBAL)],
        )
        .unwrap(),
    )
    .unwrap();
    let partial = root.linked(
        &[
            "-r",
            "-o",
            "partial.o",
            "first.o",
            "parts.a",
            "group.o",
            "duplicate.o",
        ],
        "partial.o",
    );
    let verdict = elfcheck::check(&partial, elfcheck::Kind::Relocatable);
    assert!(
        verdict.error.is_none() && verdict.findings.is_empty(),
        "{verdict:?}"
    );
    let object = Elf::parse(&partial).unwrap();
    let groups: Vec<_> = (1..object.section_count())
        .map(|i| object.section(i).unwrap())
        .filter(|header| header.kind == SHT_GROUP)
        .collect();
    assert_eq!(groups.len(), 1);
    let group = groups[0];
    let signature = object
        .symbols(group.link as usize)
        .unwrap()
        .get(group.info as usize)
        .unwrap();
    assert_eq!(signature.name, b"group_key");
    let member = named(&object, b".text.group");
    assert_eq!(object.section_data(member).unwrap().unwrap(), &[0xab; 8]);
    assert_eq!(
        object
            .section(named(&object, b".rela.text.group"))
            .unwrap()
            .kind,
        SHT_RELA
    );

    // The same script controls both links; all PT_LOAD bytes include the
    // ELF and program headers, per userspace/programs/linker.ld (FILEHDR).
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../userspace/programs/linker.ld"
    );
    let direct = root.linked(
        &[
            "-T",
            script,
            "-o",
            "direct",
            "first.o",
            "parts.a",
            "group.o",
            "duplicate.o",
            "last.o",
        ],
        "direct",
    );
    let indirect = root.linked(
        &["-T", script, "-o", "indirect", "partial.o", "last.o"],
        "indirect",
    );
    assert_eq!(loads(&indirect), loads(&direct));

    // bld's static executables have no symbol table. The R_X86_64_64 fields
    // expose the final values of the global, section and COMDAT symbols.
    for bytes in [&direct, &indirect] {
        let elf = Elf::parse(bytes).unwrap();
        let text = named(&elf, b".text");
        let text_address = elf.section(text).unwrap().address;
        let data_address = elf.section(named(&elf, b".data")).unwrap().address;
        assert_eq!(elf.header().entry, text_address);
        let code = elf.section_data(text).unwrap().unwrap();
        let address =
            |offset: usize| u64::from_le_bytes(code[offset..offset + 8].try_into().unwrap());
        assert_eq!(address(0), text_address + 16);
        assert_eq!(address(16) - 2, data_address + 8);
        assert_eq!(address(24) - 3, data_address + 16);
        assert_eq!(address(32), text_address + 32);
    }
}

#[test]
fn r_malformed_input_is_error_not_panic() {
    let root = Scratch::new();
    fs::write(root.0.join("first.o"), first(b"_start")).unwrap();
    let prefix = worker(0);
    for (bytes, location) in [
        (&prefix[..prefix.len() - 1], "bad.o:"),
        (worker(9).as_slice(), "bad.o: .data:"),
    ] {
        fs::write(root.0.join("bad.o"), bytes).unwrap();
        let result = root.link(&["-r", "-o", "partial.o", "first.o", "bad.o"]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.code(), Some(1), "{stderr}");
        assert!(stderr.starts_with(location), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
        assert!(!root.0.join("partial.o").exists());
    }
}
