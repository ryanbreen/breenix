#![forbid(unsafe_code)]

use bas::{assemble_for, Target};

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
