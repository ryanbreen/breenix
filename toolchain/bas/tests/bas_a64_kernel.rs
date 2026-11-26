#![forbid(unsafe_code)]

use bas::{assemble_for, Target};
use bobj::elf::{Elf, SHN_UNDEF, STB_LOCAL, STT_NOTYPE};

fn section_index(elf: &Elf<'_>, name: &[u8]) -> usize {
    (1..elf.section_count())
        .find(|&index| elf.section_name(index).unwrap() == name)
        .unwrap_or_else(|| panic!("missing section {}", String::from_utf8_lossy(name)))
}

#[test]
fn privileged_a64_encodings() {
    let source = include_bytes!("../../corpus/kernel-asm/privileged-a64.s");
    let expected: Vec<u8> = include_str!("../../corpus/kernel-asm/privileged-a64.hex")
        .lines()
        .flat_map(|line| line.split('#').next().unwrap().split_ascii_whitespace())
        .map(|byte| u8::from_str_radix(byte, 16).unwrap())
        .collect();
    let actual = assemble_for(Target::Aarch64, source).expect("privileged A64 encodings");
    let elf = Elf::parse(&actual).unwrap();
    let text = elf
        .section_data(section_index(&elf, b".text"))
        .unwrap()
        .unwrap();
    assert_eq!(text, expected);
}

#[test]
fn literal_pool_load_relocates_abs64() {
    let source = b"
.extern external
.equ VALUE, (1 << 63) | 0x42
.section .text.boot
    ldr x8, =external + 16
    ldr x9, =VALUE
.section .text.syscall
    ldr x10, =local - 8
    ret
.section .text.boot
    ret
.section .rodata
local:
    .quad 0
";
    let actual = assemble_for(Target::Aarch64, source).expect("section-local literal pools");
    let elf = Elf::parse(&actual).unwrap();
    let boot = section_index(&elf, b".text.boot");
    let syscall = section_index(&elf, b".text.syscall");
    let rodata = section_index(&elf, b".rodata");
    let symbols = elf.static_symbols().unwrap().unwrap();
    // DDI 0487 M.b C6.2.216: imm19 is (slot - instruction) / 4.
    let cases = [
        (
            boot,
            b".rela.text.boot".as_slice(),
            [
                0x58000088_u32,
                0x580000a9,
                0xd65f03c0,
                0,
                0,
                0,
                0x42,
                0x80000000,
            ]
            .as_slice(),
            16,
            b"external".as_slice(),
            SHN_UNDEF,
            16,
            12,
        ),
        (
            syscall,
            b".rela.text.syscall".as_slice(),
            [0x5800004a_u32, 0xd65f03c0, 0, 0].as_slice(),
            8,
            b"local".as_slice(),
            rodata as u16,
            -8,
            8,
        ),
    ];
    for (section, rela_name, words, slot, symbol_name, symbol_section, addend, data_start) in cases
    {
        let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        assert_eq!(elf.section(section).unwrap().alignment, 8);
        assert_eq!(elf.section_data(section).unwrap().unwrap(), bytes);

        let rela_index = section_index(&elf, rela_name);
        assert_eq!(elf.section(rela_index).unwrap().info as usize, section);
        let relocations = elf.relocations(rela_index).unwrap();
        assert_eq!(relocations.len(), 1);
        let relocation = relocations.get(0).unwrap();
        // AAELF64 2025Q1 §5.7.5: R_AARCH64_ABS64 applies S + A to the slot.
        assert_eq!(
            (relocation.offset, relocation.kind, relocation.addend),
            (slot, 257, addend)
        );
        let symbol = symbols.get(relocation.symbol as usize).unwrap();
        assert_eq!((symbol.name, symbol.section), (symbol_name, symbol_section));

        // AAELF64 2025Q1 §5.6.5: the pool and its zero padding are data.
        let mappings: Vec<_> = (1..symbols.len())
            .map(|index| symbols.get(index).unwrap())
            .filter(|symbol| symbol.section as usize == section && symbol.name.starts_with(b"$"))
            .map(|symbol| (symbol.name, symbol.value, symbol.binding, symbol.kind))
            .collect();
        assert_eq!(
            mappings,
            [
                (b"$x".as_slice(), 0, STB_LOCAL, STT_NOTYPE),
                (b"$d".as_slice(), data_start, STB_LOCAL, STT_NOTYPE)
            ]
        );
    }

    let error = assemble_for(Target::Aarch64, b"nop\n    ldr x1, =1\n    .skip 1048576\n")
        .expect_err("literal pool must be within the signed imm19 displacement");
    assert!(error.to_string().starts_with("2:"), "{error}");
    assert!(
        error.to_string().contains("literal target out of range"),
        "{error}"
    );
}

#[test]
fn unsupported_a64_system_register_is_located() {
    for (instruction, column, message) in [
        ("mrs x0, unknown_el1", 11, "unsupported MRS system register"),
        ("msr unknown_el1, x0", 7, "unsupported MSR system register"),
        ("msr CurrentEL, x0", 7, "read-only system register"),
        ("msr MPIDR_EL1, x0", 7, "read-only system register"),
        ("msr CTR_EL0, x0", 7, "read-only system register"),
        ("msr ID_AA64MMFR1_EL1, x0", 7, "read-only system register"),
    ] {
        let source = format!("nop\n  {instruction}\n");
        let error = assemble_for(Target::Aarch64, source.as_bytes())
            .expect_err("unsupported register access")
            .to_string();
        assert!(
            error.starts_with(&format!("2:{column}:")) && error.contains(message),
            "{instruction}: {error}"
        );
    }
}

#[test]
fn equ_expressions_and_layout_directives() {
    let source = r#"
.equ KERNEL_BASE, 0xFFFF000000000000
.equ FIELD, (0b11 << 8) | (1 << 4)
.set VALUE, (FIELD >> 4) & 0xf
.section .text.boot
add x0, x0, #VALUE
.set VALUE, +(FIELD >> 4) + 2 - 1
mov x1, #VALUE
movz x2, #((KERNEL_BASE >> 48) & ~0b10)
.section .text.vectors
.rept 2
    .inst 0xd503201f
    .balign 16
.endr
.rept 0
    .rept 1
        .inst 0xd503201f
    .endr
.endr
.section .text.syscall
ret
.section .bss.boot
.balign 32
boot_stack:
.skip 1 << 5
stack_top:
.section .rodata
.quad KERNEL_BASE, ~0, 8 - 5 & 3
.skip 3, 0x55
.balign 8, 0xaa
"#;
    let expanded = r#"
.section .text.boot,ax,@progbits
add x0, x0, #1
mov x1, #50
movz x2, #65533
.section .text.vectors,ax,@progbits
.align 16
nop
nop
nop
nop
nop
nop
nop
nop
.section .text.syscall,ax,@progbits
ret
.section .bss.boot,aw,@nobits
.align 32
boot_stack:
.zero 32
stack_top:
.section .rodata,a,@progbits
.align 8
.quad 0xFFFF000000000000, -1, 7
.byte 0x55, 0x55, 0x55, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa
"#;
    let expected = assemble_for(Target::Aarch64, expanded.as_bytes()).expect("hand-expanded object");
    let actual = assemble_for(Target::Aarch64, source.as_bytes()).expect("GNU directives");
    assert!(actual == expected, "GNU and hand-expanded objects differ");
}

#[test]
fn gnu_expression_is_error_not_panic() {
    for source in [
        ".equ VALUE,",
        "mov x0, #(1 << 4",
        "mov x0, #(1 <<",
        ".quad (~)",
        ".quad 0b",
        ".quad 0x10000000000000000",
        ".quad (0xffffffffffffffff + 1)",
        ".quad (0x8000000000000000 << 1)",
        ".quad (1 << 64)",
        ".quad (-9223372036854775808 - 1)",
        ".skip -1",
        ".rept 2\n.inst 0xd503201f",
        ".endr",
    ] {
        let error = assemble_for(Target::Aarch64, source.as_bytes())
            .expect_err("malformed GNU source must return a diagnostic");
        assert!(error.to_string().starts_with("1:"), "{source}: {error}");
    }
}

#[test]
fn guarded_macro_expands_once() {
    let guard = r#"
.ifndef BREENIX_RESUME_PC_GUARD_INC
.set BREENIX_RESUME_PC_GUARD_INC, 1
.macro GUARD pc, resume
    cbz \pc, 8888f  // \comment is ignored.
    b \resume
8888: nop
    cbnz \pc, 8888b
.endm
.endif
"#;
    let source =
        format!("{guard}{guard}GUARD x0, 10f\nGUARD x1, 10f\n10: nop\nGUARD x2, 10b\nret\n");
    let expanded = r#"
    cbz x0, .Lfirst
    b .Lresume
.Lfirst: nop
    cbnz x0, .Lfirst
    cbz x1, .Lsecond
    b .Lresume
.Lsecond: nop
    cbnz x1, .Lsecond
.Lresume: nop
    cbz x2, .Lthird
    b .Lresume
.Lthird: nop
    cbnz x2, .Lthird
    ret
"#;
    let expected =
        assemble_for(Target::Aarch64, expanded.as_bytes()).expect("hand-expanded object");
    let actual = assemble_for(Target::Aarch64, source.as_bytes()).expect("guarded macro object");

    // Numeric and named labels can have different ELF symbol names. Compare
    // the instruction bytes and reject a branch relocation in .text.
    let text = |bytes: &[u8]| {
        let elf = bobj::elf::Elf::parse(bytes).unwrap();
        let mut text = None;
        for index in 0..elf.section_count() {
            match elf.section_name(index).unwrap() {
                b".text" => text = elf.section_data(index).unwrap().map(<[u8]>::to_vec),
                b".rela.text" => panic!("unexpected unresolved branch in .text"),
                _ => {}
            }
        }
        text.expect("missing .text")
    };
    assert_eq!(text(&actual), text(&expected));
}

#[test]
fn gnu_macro_is_error_not_panic() {
    let cases = [
        ("nop\n  .macro OPEN a\n    mov \\a, x0", "2:3:", ".endm"),
        (
            ".macro BAD a\n  mov \\arg, x0\n.endm\nBAD x1",
            "2:7:",
            "unknown macro parameter",
        ),
        ("nop\n  .endif", "2:3:", ".endif without"),
        ("nop\n  b 8888f", "2:5:", "no following numeric label"),
        (
            ".macro RECURSE\n    RECURSE\n.endm\nRECURSE",
            "2:5:",
            "macro expansion depth",
        ),
    ];

    for (source, location, message) in cases {
        let error = assemble_for(Target::Aarch64, source.as_bytes())
            .expect_err("malformed GNU macro or label must return a diagnostic")
            .to_string();
        assert!(
            error.starts_with(location) && error.contains(message),
            "{source}: {error}"
        );
    }
}
