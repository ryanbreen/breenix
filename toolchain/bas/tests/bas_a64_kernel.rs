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
