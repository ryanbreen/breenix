use std::fs;
use std::path::Path;
use std::process::Command;

use bcc::{pp, sema};
use bobj::elf::amd64::R_TPOFF32;
use bobj::elf::{
    Elf, PT_TLS, SHF_TLS, SHN_UNDEF, SHT_NOBITS, SHT_PROGBITS, STB_GLOBAL, STB_LOCAL, STT_TLS,
};

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let work = std::env::temp_dir().join(format!("breenix-bcc-{name}-{}", std::process::id()));
        fs::create_dir(&work).unwrap();
        Self(work)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// N1570 6.7.1 and 6.7.9, AMD64 psABI 3.1.2 and 10.3.2.
#[test]
fn tpoff32_offset_is_negative_aligned() {
    let scratch = Scratch::new("tls");
    let work = &scratch.0;
    let source = work.join("tls.c");
    let obj = work.join("tls.o");
    let executable = work.join("tls.elf");
    fs::write(
        &source,
        r"
struct Aligned { int value; long double alignment; };
static _Thread_local struct Aligned wide = { 7 };
int main(void) {
    _Thread_local extern struct Aligned wide;
    _Thread_local static int zero[3];
    wide.value = wide.value + 1;
    struct Aligned *address = &wide;
    return address->value + zero[0];
}
",
    )
    .unwrap();

    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let toolchain = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let root = toolchain.join("..");
    let libc = root.join("libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a");
    let script = root.join("userspace/programs/linker.ld");
    let assembly = work.join("tls.s");
    fs::write(&assembly, checked(Command::new(bcc).arg("-S").arg(source))).unwrap();
    checked(
        Command::new(bcc.with_file_name("bas"))
            .arg("-o")
            .arg(&obj)
            .arg(assembly),
    );
    checked(
        Command::new(bcc.with_file_name("bld"))
            .arg("-T")
            .arg(script)
            .args(["--gc-sections", "-o"])
            .arg(&executable)
            .arg(&obj)
            .arg(libc),
    );
    checked(
        Command::new(bcc.with_file_name("elfcheck"))
            .arg("--exec")
            .arg(&executable),
    );

    let obj_bytes = fs::read(obj).unwrap();
    let exe_bytes = fs::read(executable).unwrap();
    let obj = Elf::parse(&obj_bytes).unwrap();
    let exe = Elf::parse(&exe_bytes).unwrap();
    let tls = (0..exe.program_count())
        .map(|index| exe.program(index).unwrap())
        .find(|header| header.kind == PT_TLS)
        .expect("PT_TLS");
    assert!(tls.alignment.is_power_of_two() && tls.alignment >= 16);
    let span = tls.memory_size.next_multiple_of(tls.alignment);
    assert!(span > tls.memory_size, "fixture must exercise TLS rounding");

    let symbols = obj.static_symbols().unwrap().unwrap();
    let relocs = obj.relocations(section(&obj, b".rela.text")).unwrap();
    let input_text = obj.section_data(section(&obj, b".text")).unwrap().unwrap();
    let text = exe.section_data(section(&exe, b".text")).unwrap().unwrap();
    let mut accesses = Vec::new();
    for index in 0..relocs.len() {
        let reloc = relocs.get(index).unwrap();
        assert_eq!(reloc.kind, R_TPOFF32);
        assert_eq!(reloc.addend, 0);
        let symbol = symbols.get(reloc.symbol as usize).unwrap();
        assert_eq!(symbol.kind, STT_TLS);
        assert_eq!(symbol.binding, STB_LOCAL);
        let input = obj.section(usize::from(symbol.section)).unwrap();
        assert_ne!(input.flags & SHF_TLS, 0);
        let (name, size, alignment) = match input.kind {
            SHT_PROGBITS => (b".tdata".as_slice(), 32, 16),
            SHT_NOBITS => (b".tbss".as_slice(), 12, 4),
            kind => panic!("unexpected TLS section type {kind}"),
        };
        assert_eq!(obj.section_name(usize::from(symbol.section)).unwrap(), name);
        assert_eq!(symbol.size, size);
        assert!(input.alignment >= alignment);
        let output = exe.section(section(&exe, name)).unwrap();
        // The script places the input object's sections first in these output
        // sections. The linked TLS symbol value is relative to PT_TLS (gABI 5.2).
        let symbol_offset = output.address - tls.virtual_address + symbol.value;
        if name == b".tdata" {
            assert!(symbol_offset + size <= tls.file_size);
            let start = usize::try_from(tls.offset + symbol_offset).unwrap();
            assert_eq!(&exe_bytes[start..start + 4], &7_i32.to_le_bytes());
            assert!(exe_bytes[start + 4..start + 32]
                .iter()
                .all(|&byte| byte == 0));
        } else {
            assert!(symbol_offset >= tls.file_size && symbol_offset + size <= tls.memory_size);
            assert_eq!(output.kind, SHT_NOBITS);
        }
        let field = usize::try_from(reloc.offset).unwrap();
        let displacement = i64::from(i32::from_le_bytes(
            text[field..field + 4].try_into().unwrap(),
        ));
        assert!(displacement < 0);
        assert_eq!(displacement % i64::try_from(alignment).unwrap(), 0);
        assert_eq!(
            displacement,
            i64::try_from(symbol_offset).unwrap() - i64::try_from(span).unwrap()
        );
        // AMD64 psABI Tables 10.7 and 10.10. The four-byte displacement
        // follows either an absolute FS:SIB operand or a TP-based LEA.
        let operand = &text[..field];
        let encoding: &[u8] = if operand.ends_with(&[0x48, 0x8d, 0x80]) {
            &[0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0x48, 0x8d, 0x80]
        } else {
            match operand[field - 3] {
                0x63 => &[0x64, 0x48, 0x63, 0x04, 0x25],
                opcode @ (0x8b | 0x89) => &[0x64, opcode, 0x04, 0x25],
                opcode => panic!("unexpected TLS opcode {opcode:#x}"),
            }
        };
        assert!(operand.ends_with(encoding));
        assert!(input_text[..field].ends_with(encoding));
        let access = if encoding.ends_with(&[0x48, 0x8d, 0x80]) {
            "address"
        } else if operand[field - 3] == 0x89 {
            "store"
        } else {
            "load"
        };
        accesses.push((size, access));
    }
    accesses.sort_unstable();
    assert_eq!(
        accesses,
        [
            (12, "address"),
            (32, "address"),
            (32, "load"),
            (32, "store")
        ]
    );
}

#[test]
fn aarch64_tprel_is_tcb_plus_aligned_template() {
    let scratch = Scratch::new("arm-tls");
    let work = &scratch.0;
    let bcc = Path::new(env!("CARGO_BIN_EXE_bcc"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let libc = root.join("libs/libbreenix-libc/target/aarch64-breenix/release/libc.a");
    let script = root.join("userspace/programs/linker.ld");
    let executable = work.join("tls.elf");
    let compile = |stem: &str, source: &str| {
        let input = work.join(stem).with_extension("c");
        let asm = input.with_extension("s");
        let obj = input.with_extension("o");
        fs::write(&input, source).unwrap();
        checked(
            Command::new(bcc)
                .args(["--target=aarch64", "-S", "-o"])
                .arg(&asm)
                .arg(&input),
        );
        checked(
            Command::new(bcc.with_file_name("bas"))
                .args(["--target", "aarch64", "-o"])
                .arg(&obj)
                .arg(&asm),
        );
        obj
    };
    // N1570 §§6.7.1 and 6.7.9. The large zero-filled object makes the
    // next TLS access exercise the high 12-bit relocation as well.
    let main = compile(
        "main",
        r"
struct Aligned { int value; long double alignment; };
static _Thread_local struct Aligned wide = { 7 };
extern _Thread_local int data;
int main(void) {
    _Thread_local extern struct Aligned wide;
    _Thread_local static int zero[1025];
    _Thread_local static int tail;
    wide.value = wide.value + 1;
    struct Aligned *address = &wide;
    int *remote = &data;
    return address->value + zero[0] + tail + *remote;
}
",
    );
    let data = compile("data", "_Thread_local int data = 11;");
    checked(
        Command::new(bcc.with_file_name("bld"))
            .arg("-T")
            .arg(script)
            .args(["--gc-sections", "-o"])
            .arg(&executable)
            .arg(&main)
            .arg(&data)
            .arg(libc),
    );
    checked(
        Command::new(bcc.with_file_name("elfcheck"))
            .args(["--machine", "aarch64", "--exec"])
            .arg(&executable),
    );

    let exe_bytes = fs::read(executable).unwrap();
    let exe = Elf::parse(&exe_bytes).unwrap();
    let tls = (0..exe.program_count())
        .map(|index| exe.program(index).unwrap())
        .find(|header| header.kind == PT_TLS)
        .expect("PT_TLS");
    assert!(tls.alignment.is_power_of_two() && tls.alignment >= 16);

    // linker.ld:36–42 orders the inputs' .tdata before .tbss in PT_TLS.
    // gABI §§3.10 and 7.7 define the file-backed and zero-filled parts.
    let tdata_index = section(&exe, b".tdata");
    let tdata = exe.section(tdata_index).unwrap();
    let template = exe.section_data(tdata_index).unwrap().unwrap();
    assert_eq!(tdata.kind, SHT_PROGBITS);
    assert_ne!(tdata.flags & SHF_TLS, 0);
    assert_eq!(tdata.address, tls.virtual_address);
    assert_eq!(tdata.offset, tls.offset);
    let mut initialized = [0; 36];
    initialized[..4].copy_from_slice(&7_i32.to_le_bytes());
    initialized[32..].copy_from_slice(&11_i32.to_le_bytes());
    assert_eq!(&template[..36], &initialized);
    assert!(tls.file_size >= 36);
    let tbss_index = section(&exe, b".tbss");
    let tbss = exe.section(tbss_index).unwrap();
    let tbss_offset = tbss.address - tls.virtual_address;
    assert_eq!(tbss.kind, SHT_NOBITS);
    assert_ne!(tbss.flags & SHF_TLS, 0);
    assert!(exe.section_data(tbss_index).unwrap().is_none());
    assert!(tbss.size >= 4104);
    assert!(tbss_offset >= tls.file_size && tbss_offset + tbss.size <= tls.memory_size);

    let obj_bytes = fs::read(main).unwrap();
    let obj = Elf::parse(&obj_bytes).unwrap();
    let symbols = obj.static_symbols().unwrap().unwrap();
    let relocs = obj.relocations(section(&obj, b".rela.text")).unwrap();
    let text = exe.section_data(section(&exe, b".text")).unwrap().unwrap();
    let input_text = obj.section_data(section(&obj, b".text")).unwrap().unwrap();
    let word = |bytes: &[u8], offset: usize| {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    };
    let mut accesses = Vec::new();
    let mut starts = Vec::new();
    assert_eq!(relocs.len(), 12);
    for index in (0..relocs.len()).step_by(2) {
        let hi = relocs.get(index).unwrap();
        let lo = relocs.get(index + 1).unwrap();
        // AAELF64 2025Q1 §5.7.11.4.
        assert_eq!((hi.kind, lo.kind), (549, 551));
        assert_eq!((hi.addend, lo.addend), (0, 0));
        assert_eq!(hi.symbol, lo.symbol);
        assert_eq!(lo.offset, hi.offset + 4);
        let symbol = symbols.get(hi.symbol as usize).unwrap();
        assert_eq!(symbol.kind, STT_TLS);
        let symbol_offset = if symbol.section == SHN_UNDEF {
            assert_eq!(symbol.binding, STB_GLOBAL);
            assert_eq!(symbol.name, b"data");
            32
        } else {
            let input = usize::from(symbol.section);
            assert_ne!(obj.section(input).unwrap().flags & SHF_TLS, 0);
            assert_eq!(symbol.binding, STB_LOCAL);
            match (obj.section_name(input).unwrap(), symbol.size) {
                (b".tdata", 32) => {
                    assert_eq!(symbol.name, b"wide");
                    assert_eq!(symbol.value, 0);
                    0
                }
                (b".tbss", 4100) => {
                    assert_eq!(symbol.value, 0);
                    tbss_offset
                }
                (b".tbss", 4) => {
                    assert_eq!(symbol.value, 4100);
                    tbss_offset + 4100
                }
                _ => panic!("unexpected TLS symbol {symbol:?}"),
            }
        };
        accesses.push(symbol_offset);

        // DDI 0487 M.b §§C6.2.286 and D24.2.203 (MRS), §C6.2.4 (ADD).
        let field = usize::try_from(hi.offset).unwrap();
        let start = field - 4;
        starts.push(start);
        let mrs = word(input_text, start);
        let register = mrs & 31;
        assert!(register < 31);
        assert_eq!(mrs, 0xd53b_d040 | register);
        assert_eq!(word(text, start), mrs);
        let operands = (register << 5) | register;
        let hi_opcode = 0x9140_0000 | operands;
        let lo_opcode = 0x9100_0000 | operands;
        assert_eq!(word(input_text, field), hi_opcode);
        assert_eq!(word(input_text, field + 4), lo_opcode);
        let linked_hi = word(text, field);
        let linked_lo = word(text, field + 4);
        let immediate = |instruction: u32| u64::from((instruction >> 10) & 0xfff);
        assert_eq!(linked_hi & !(0xfff << 10), hi_opcode);
        assert_eq!(linked_lo & !(0xfff << 10), lo_opcode);
        // Breenix's variant-I TCB is 16 bytes, before the aligned template.
        assert_eq!(
            (immediate(linked_hi) << 12) | immediate(linked_lo),
            16_u64.next_multiple_of(tls.alignment) + symbol_offset,
        );
    }
    let reads: Vec<_> = (0..input_text.len())
        .step_by(4)
        .filter(|&offset| word(input_text, offset) & !31 == 0xd53b_d040)
        .collect();
    assert_eq!(starts, reads);
    accesses.sort_unstable();
    assert_eq!(accesses, [0, 0, 0, 32, tbss_offset, tbss_offset + 4100]);
}

#[test]
fn invalid_thread_local_has_located_diagnostic() {
    let path = Path::new("invalid-tls.c");
    let options = pp::Options::default();
    for source in [
        "int main(void) { _Thread_local int automatic; return 0; }",
        "_Thread_local int function(void);",
        "int function(_Thread_local int parameter);",
        "extern _Thread_local int shared; extern int shared;",
        "extern int shared; extern _Thread_local int shared;",
        "extern _Thread_local int shared; int main(void) { extern int shared; return shared; }",
        "typedef _Thread_local int alias;",
    ] {
        let input = pp::preprocess_source(path, source.as_bytes(), &options).unwrap();
        let error = sema::analyze(&input, path, options.target)
            .unwrap_err()
            .to_string();
        let error = error.strip_prefix("invalid-tls.c:1:").unwrap();
        let (column, message) = error.split_once(':').unwrap();
        assert!((1..=source.len()).contains(&column.parse().unwrap()));
        assert!(message.contains("_Thread_local"), "{source}\n{message}");
    }
}

fn section(elf: &Elf<'_>, name: &[u8]) -> usize {
    (0..elf.section_count())
        .find(|&index| elf.section_name(index).unwrap() == name)
        .unwrap_or_else(|| panic!("missing section {}", String::from_utf8_lossy(name)))
}

fn checked(command: &mut Command) -> Vec<u8> {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    output.stdout
}
