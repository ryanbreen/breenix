//! AAELF64 2025Q1 §§5.7.3.3, 5.7.5 and 5.7.6, and the Arm ARM
//! DDI 0487 M.b instruction encodings. Keep the fixture codes independent
//! of bobj's relocation constants.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use bobj::elf::{
    write_rel, Elf, RelSection, RelSymbol, Rela, EM_X86_64, SHF_ALLOC, SHF_EXECINSTR, SHT_PROGBITS,
    STB_GLOBAL, STT_FUNC, STT_OBJECT,
};

const EM_AARCH64: u16 = 183;
const CODE: [u32; 13] = [
    0x9000_0003, // adrp x3, target
    0x9100_0063, // add x3, x3, #0
    0x3940_0062, // ldrb w2, [x3]
    0x7940_0062, // ldrh w2, [x3]
    0xb940_0062, // ldr w2, [x3]
    0xf940_0062, // ldr x2, [x3]
    0x3dc0_0062, // ldr q2, [x3]
    0x1000_0004, // adr x4, target
    0x9400_0000, // bl _start
    0x1400_0000, // b _start
    0x5400_0001, // b.ne _start
    0x3628_0002, // tbz w2, #5, _start
    0x5800_0005, // ldr x5, target
];
const TARGET_OFFSET: u64 = 0xff0;
const PAGE_CROSSING_ADDEND: i64 = 0x130;

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("bld-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn link(&self, inputs: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bld"))
            .current_dir(&self.0)
            .args(["-o", "image.elf"])
            .args(inputs)
            .output()
            .unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn rela(offset: u64, kind: u32, symbol: u32, addend: i64) -> Rela {
    Rela {
        offset,
        kind,
        symbol,
        addend,
    }
}

fn object(machine: u16, text_relocs: &[Rela], data_relocs: &[Rela]) -> Vec<u8> {
    let text: Vec<u8> = CODE.into_iter().flat_map(u32::to_le_bytes).collect();
    let data = [0; 0x1140];
    write_rel(
        machine,
        &[
            RelSection {
                name: b".text",
                kind: SHT_PROGBITS,
                flags: SHF_ALLOC | SHF_EXECINSTR,
                alignment: 4,
                bytes: &text,
                size: text.len() as u64,
                relocations: text_relocs,
            },
            RelSection {
                name: b".rodata",
                kind: SHT_PROGBITS,
                flags: SHF_ALLOC,
                alignment: 16,
                bytes: &data,
                size: data.len() as u64,
                relocations: data_relocs,
            },
        ],
        &[
            RelSymbol {
                name: b"_start",
                section: 1,
                kind: STT_FUNC,
                binding: STB_GLOBAL,
                value: 0,
                size: text.len() as u64,
            },
            RelSymbol {
                name: b"target",
                section: 2,
                kind: STT_OBJECT,
                binding: STB_GLOBAL,
                value: TARGET_OFFSET,
                size: 16,
            },
        ],
    )
    .unwrap()
}

fn section<'a>(elf: &Elf<'a>, name: &[u8]) -> (i64, &'a [u8]) {
    let index = (1..elf.section_count())
        .find(|&index| elf.section_name(index).unwrap() == name)
        .unwrap();
    let address = elf.section(index).unwrap().address as i64;
    (address, elf.section_data(index).unwrap().unwrap())
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn signed(bits: u32, width: u32) -> i64 {
    let shift = 64 - width;
    (i64::from(bits) << shift) >> shift
}

fn adr_immediate(instruction: u32) -> i64 {
    signed(
        ((instruction >> 5) & 0x7ffff) << 2 | ((instruction >> 29) & 3),
        21,
    )
}

fn assert_error(output: Output, location: &str, reason: &str) {
    assert!(
        !output.status.success(),
        "{location}: link unexpectedly succeeded"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(location) && stderr.contains(reason),
        "{stderr}"
    );
}

// Build elfcheck in the same Cargo target/profile before running this check.
#[test]
fn aarch64_adrp_add_ldst_fields() {
    let scratch = Scratch::new("aarch64-fields");
    let mut text_relocs = [
        rela(0, 275, 1, PAGE_CROSSING_ADDEND),
        rela(4, 277, 1, PAGE_CROSSING_ADDEND),
        rela(8, 278, 1, PAGE_CROSSING_ADDEND),
        rela(12, 284, 1, PAGE_CROSSING_ADDEND),
        rela(16, 285, 1, PAGE_CROSSING_ADDEND),
        rela(20, 286, 1, PAGE_CROSSING_ADDEND),
        rela(24, 299, 1, PAGE_CROSSING_ADDEND),
        rela(28, 274, 1, -3),
        rela(32, 283, 0, 4),
        rela(36, 282, 0, 8),
        rela(40, 280, 0, 12),
        rela(44, 279, 0, 16),
        rela(48, 273, 1, 16),
    ];
    let data_relocs = [
        rela(0, 257, 1, -9),
        rela(8, 258, 1, 0x8000_0000),
        rela(12, 261, 0, 4),
    ];
    fs::write(
        scratch.0.join("fields.o"),
        object(EM_AARCH64, &text_relocs, &data_relocs),
    )
    .unwrap();
    let output = scratch.link(&["fields.o"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let path = scratch.0.join("image.elf");
    let bytes = fs::read(&path).unwrap();
    let elf = Elf::parse(&bytes).unwrap();
    let (text_address, text) = section(&elf, b".text");
    let (data_address, data) = section(&elf, b".rodata");
    let target = data_address + TARGET_OFFSET as i64;

    assert_eq!(
        u64::from_le_bytes(data[..8].try_into().unwrap()),
        (target - 9) as u64,
        "ABS64"
    );
    assert_eq!(word(data, 8), (target + 0x8000_0000) as u32, "ABS32");
    assert_eq!(
        word(data, 12) as i32 as i64,
        text_address + 4 - (data_address + 12),
        "PREL32"
    );

    // Independently decode the Arm ARM immediate layouts, also checking
    // that the relocation preserved the opcode, registers and condition.
    let relocated = target + PAGE_CROSSING_ADDEND;
    let page = |address: i64| address & !0xfff;
    let low = relocated & 0xfff;
    let fields = [
        (
            0x60ff_ffe0,
            adr_immediate(word(text, 0)) * 4096,
            page(relocated) - page(text_address),
        ),
        (0x003f_fc00, i64::from((word(text, 4) >> 10) & 0xfff), low),
        (0x003f_fc00, i64::from((word(text, 8) >> 10) & 0xfff), low),
        (
            0x003f_fc00,
            i64::from((word(text, 12) >> 10) & 0xfff),
            low / 2,
        ),
        (
            0x003f_fc00,
            i64::from((word(text, 16) >> 10) & 0xfff),
            low / 4,
        ),
        (
            0x003f_fc00,
            i64::from((word(text, 20) >> 10) & 0xfff),
            low / 8,
        ),
        (
            0x003f_fc00,
            i64::from((word(text, 24) >> 10) & 0xfff),
            low / 16,
        ),
        (
            0x60ff_ffe0,
            adr_immediate(word(text, 28)),
            target - 3 - (text_address + 28),
        ),
        (
            0x03ff_ffff,
            signed(word(text, 32) & 0x03ff_ffff, 26) * 4,
            4 - 32,
        ),
        (
            0x03ff_ffff,
            signed(word(text, 36) & 0x03ff_ffff, 26) * 4,
            8 - 36,
        ),
        (
            0x00ff_ffe0,
            signed((word(text, 40) >> 5) & 0x7ffff, 19) * 4,
            12 - 40,
        ),
        (
            0x0007_ffe0,
            signed((word(text, 44) >> 5) & 0x3fff, 14) * 4,
            16 - 44,
        ),
        (
            0x00ff_ffe0,
            signed((word(text, 48) >> 5) & 0x7ffff, 19) * 4,
            target + 16 - (text_address + 48),
        ),
    ];
    for (index, ((mask, actual, expected), original)) in fields.into_iter().zip(CODE).enumerate() {
        assert_eq!(
            actual, expected,
            "relocation {} immediate",
            text_relocs[index].kind
        );
        assert_eq!(
            word(text, index * 4) & !mask,
            original & !mask,
            "instruction {index}"
        );
    }

    let elfcheck = PathBuf::from(env!("CARGO_BIN_EXE_bld")).with_file_name("elfcheck");
    let output = Command::new(&elfcheck)
        .args(["--machine", "aarch64", "--exec"])
        .arg(path)
        .output()
        .unwrap_or_else(|error| panic!("run {}: {error}", elfcheck.display()));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    text_relocs[6].addend += 1;
    fs::write(
        scratch.0.join("misaligned.o"),
        object(EM_AARCH64, &text_relocs, &data_relocs),
    )
    .unwrap();
    assert_error(
        scratch.link(&["misaligned.o"]),
        "misaligned.o: .text+0x18: R_AARCH64_LDST128_ABS_LO12_NC",
        "aligned",
    );
}

#[test]
fn aarch64_unknown_relocation_named() {
    let scratch = Scratch::new("aarch64-diagnostics");
    // ABS16 is assigned by AAELF64 §5.7.5 but outside this slice.
    fs::write(
        scratch.0.join("unsupported.o"),
        object(EM_AARCH64, &[rela(0, 259, 1, 0)], &[]),
    )
    .unwrap();
    assert_error(
        scratch.link(&["unsupported.o"]),
        "unsupported.o: .text+0x0: R_AARCH64_ABS16 (259) target",
        "unsupported relocation",
    );

    fs::write(scratch.0.join("arm.o"), object(EM_AARCH64, &[], &[])).unwrap();
    fs::write(scratch.0.join("x86.o"), object(EM_X86_64, &[], &[])).unwrap();
    assert_error(scratch.link(&["arm.o", "x86.o"]), "x86.o:", "machine");
    assert_error(scratch.link(&["x86.o", "arm.o"]), "arm.o:", "machine");
}
