use std::fs;
use std::path::Path;
use std::process::Command;

use bcc::{pp, sema};
use bobj::elf::amd64::R_TPOFF32;
use bobj::elf::{Elf, PT_TLS, SHF_TLS, SHT_NOBITS, SHT_PROGBITS, STB_LOCAL, STT_TLS};

// N1570 6.7.1 and 6.7.9, AMD64 psABI 3.1.2 and 10.3.2.
#[test]
fn tpoff32_offset_is_negative_aligned() {
    let work = std::env::temp_dir().join(format!("breenix-bcc-tls-{}", std::process::id()));
    fs::create_dir(&work).unwrap();
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(work);
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
            assert!(
                exe_bytes[start + 4..start + 32]
                    .iter()
                    .all(|&byte| byte == 0)
            );
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
        let operand = &input_text[..field];
        let access = if operand.ends_with(&[0x48, 0x8d, 0x80]) {
            assert!(
                operand.ends_with(&[0x64, 0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0, 0x48, 0x8d, 0x80])
            );
            "address"
        } else {
            assert_eq!(&operand[field - 2..], &[0x04, 0x25]);
            match operand[field - 4] {
                0x63 => {
                    assert!(operand.ends_with(&[0x64, 0x48, 0x63, 0x04, 0x25]));
                    "load"
                }
                opcode @ (0x8b | 0x89) => {
                    assert_eq!(operand[field - 5], 0x64);
                    if opcode == 0x8b { "load" } else { "store" }
                }
                opcode => panic!("unexpected TLS opcode {opcode:#x}"),
            }
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
