//! AAELF64 2025Q1 §§5.7.3.3, 5.7.5 and 5.7.6, and the Arm ARM
//! DDI 0487 M.b instruction encodings. Keep the fixture codes independent
//! of bobj's relocation constants.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use bobj::elf::{
    write_rel, Elf, RelSection, RelSymbol, Rela, EM_X86_64, PF_X, PT_LOAD, SHF_ALLOC,
    SHF_EXECINSTR, SHF_WRITE, SHN_ABS, SHT_PROGBITS, STB_GLOBAL, STT_FUNC, STT_OBJECT,
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

fn code_section<'a>(bytes: &'a [u8], relocations: &'a [Rela]) -> RelSection<'a> {
    RelSection {
        name: b".text",
        kind: SHT_PROGBITS,
        flags: SHF_ALLOC | SHF_EXECINSTR,
        alignment: 4,
        bytes,
        size: bytes.len() as u64,
        relocations,
    }
}

#[test]
fn aarch64_got_slot_holds_symbol_address() {
    let scratch = Scratch::new("aarch64-got");
    // AAELF64 §5.7.6: ADR_GOT_PAGE and LD64_GOT_LO12_NC. The LDR
    // uses an 8-byte slot at a nonzero offset in the addressed page.
    let code = [0x9000_0003u32, 0xf940_0062, 0xd65f_03c0]; // ADRP x3; LDR x2, [x3]; RET.
    let text: Vec<_> = code.into_iter().flat_map(u32::to_le_bytes).collect();
    let data = [0xabu8; 16];
    let object = write_rel(
        EM_AARCH64,
        &[
            code_section(&text, &[rela(0, 311, 1, 0), rela(4, 312, 1, 0)]),
            RelSection {
                name: b".data",
                kind: SHT_PROGBITS,
                flags: SHF_ALLOC | SHF_WRITE,
                alignment: 8,
                bytes: &data,
                size: data.len() as u64,
                relocations: &[],
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
                value: 8,
                size: 8,
            },
        ],
    )
    .unwrap();
    fs::write(scratch.0.join("got.o"), object).unwrap();
    let output = scratch.link(&["got.o"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(scratch.0.join("image.elf")).unwrap();
    let elf = Elf::parse(&bytes).unwrap();
    let (text_address, text) = section(&elf, b".text");
    let (data_address, _) = section(&elf, b".data");
    let (got_address, got) = section(&elf, b".got");
    let adrp = word(text, 0);
    let ldr = word(text, 4);
    assert_eq!(adrp & !0x60ff_ffe0, code[0]);
    assert_eq!(ldr & !0x003f_fc00, code[1]);
    assert_eq!(got_address & 7, 0);
    assert_ne!(got_address & 0xfff, 0);
    assert_eq!(
        (text_address & !0xfff) + adr_immediate(adrp) * 4096 + i64::from((ldr >> 10) & 0xfff) * 8,
        got_address
    );
    assert_eq!(got, (data_address + 8).to_le_bytes());
}

fn branch_object(code: &[u32], relocations: &[Rela], target_section: u16, target: u64) -> Vec<u8> {
    let text: Vec<_> = code.iter().copied().flat_map(u32::to_le_bytes).collect();
    write_rel(
        EM_AARCH64,
        &[code_section(&text, relocations)],
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
                section: target_section,
                kind: STT_FUNC,
                binding: STB_GLOBAL,
                value: target,
                size: 0,
            },
        ],
    )
    .unwrap()
}

#[test]
fn aarch64_call26_out_of_range_gets_veneer() {
    let scratch = Scratch::new("aarch64-veneer");
    let image = scratch.0.join("image.elf");
    let ret = 0xd65f_03c0;
    let bl = 0x9400_0000;
    let b = 0x1400_0000;
    // The default script's base is 0x40000000. AAELF64 §5.7.7 permits
    // veneers for CALL26 and JUMP26 to the absolute STT_FUNC definition.
    let far: u64 = 0x4000_0000 + (256 << 20);
    let link = |name, object| {
        fs::write(scratch.0.join(name), object).unwrap();
        scratch.link(&[name])
    };
    let linked_image = |name, object| {
        let output = link(name, object);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::read(&image).unwrap()
    };

    // Hand-encoding BL +4 gives a full-ELF comparison without branch relocation.
    let direct = linked_image("direct.o", branch_object(&[bl | 1, ret], &[], 1, 4));
    let near = linked_image(
        "near.o",
        branch_object(&[bl, ret], &[rela(0, 283, 1, 0)], 1, 4),
    );
    assert_eq!(near, direct);

    for (instruction, kind, relocation) in [
        (0x5400_0001, 280, "R_AARCH64_CONDBR19"),
        (0x3628_0002, 279, "R_AARCH64_TSTBR14"),
    ] {
        assert_error(
            link(
                "short.o",
                branch_object(&[instruction], &[rela(0, kind, 1, 0)], SHN_ABS, far),
            ),
            &format!("short.o: .text+0x0: {relocation} ({kind}) target"),
            "relocation overflow",
        );
    }

    let bytes = linked_image(
        "far.o",
        branch_object(
            &[bl, b],
            &[rela(0, 283, 1, 0), rela(4, 282, 1, 0x124)],
            SHN_ABS,
            far,
        ),
    );
    let elf = Elf::parse(&bytes).unwrap();
    let (text_address, text) = section(&elf, b".text");
    let exec = (0..elf.program_count())
        .map(|index| elf.program(index).unwrap())
        .find(|program| program.kind == PT_LOAD && program.flags & PF_X != 0)
        .unwrap();
    for (offset, opcode, target) in [(0, bl, far), (4, b, far + 0x124)] {
        let branch = word(text, offset);
        assert_eq!(branch & 0xfc00_0000, opcode);
        let veneer = text_address + offset as i64 + signed(branch & 0x03ff_ffff, 26) * 4;
        assert!(veneer >= text_address + 8);
        let load_offset = u64::try_from(veneer - exec.virtual_address as i64).unwrap();
        assert!(load_offset + 12 <= exec.file_size);
        let at = (exec.offset + load_offset) as usize;
        let adrp = word(&bytes, at);
        let add = word(&bytes, at + 4);
        // Arm ARM C6.2.5, C6.2.13 and C6.2.46, AAPCS64 §6.5.1 (IP0).
        assert_eq!(adrp & !0x60ff_ffe0, 0x9000_0010); // ADRP x16
        assert_eq!(add & !0x003f_fc00, 0x9100_0210); // ADD x16, x16, #imm12
        assert_eq!(word(&bytes, at + 8), 0xd61f_0200); // BR x16
        assert_eq!(
            (veneer & !0xfff) + adr_immediate(adrp) * 4096 + i64::from((add >> 10) & 0xfff),
            target as i64
        );
    }
}
