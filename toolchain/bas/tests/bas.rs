#![forbid(unsafe_code)]

use bas::{assemble, assemble_for, Target};

#[test]
fn aarch64_project_value_assembles() {
    let source = include_bytes!("../../corpus/project-aarch64/value.s");
    assemble_for(Target::Aarch64, source).expect("assemble the ARM64 project fixture");
}

#[test]
fn malformed_input_is_error_not_panic() {
    let instruction = b"mov rax, qword ptr fs:[r12 + r9 * 8 + 0x12]";
    assert!(assemble(instruction).is_ok());

    for end in 1..instruction.len() {
        let error = assemble(&instruction[..end]).expect_err("reject truncated operand");
        assert!(error.to_string().starts_with("1:"));
    }

    for index in 0..instruction.len() {
        let mut corrupt = *instruction;
        corrupt[index] = 0;
        let error = assemble(&corrupt).expect_err("reject NUL in source");
        assert_eq!(error.to_string(), format!("1:{}: NUL in source", index + 1));
    }

    for (source, expected) in [
        ("db 18446744073709551616", "1:4: integer out of range"),
        (
            "section .bss\nzero 18446744073709551615\ndb 0",
            "3:4: section size out of range",
        ),
        (
            "jmp external-9223372036854775808\nextern external",
            "1:5: relative relocation addend out of range",
        ),
    ] {
        let error = assemble(source.as_bytes()).expect_err("reject numeric overflow");
        assert_eq!(error.to_string(), expected);
    }

    let branch = b"tbz x0, #63, .Ltarget";
    let with_target = |line: &[u8]| [line, b"\n.Ltarget: nop"].concat();
    assert!(assemble_for(Target::Aarch64, &with_target(branch)).is_ok());

    for end in 1..branch.len() {
        let error = assemble_for(Target::Aarch64, &with_target(&branch[..end]))
            .expect_err("reject truncated AArch64 branch");
        assert!(error.to_string().starts_with("1:"));
    }
    for index in 0..branch.len() {
        let mut corrupt = *branch;
        corrupt[index] = 0;
        let error = assemble_for(Target::Aarch64, &with_target(&corrupt))
            .expect_err("reject NUL in AArch64 source");
        assert_eq!(error.to_string(), format!("1:{}: NUL in source", index + 1));
    }
    for source in [
        "movz x0, #18446744073709551616",
        "movz w0, #1, lsl #32",
        "tbz w0, #32, .Ltarget\n.Ltarget: nop",
        "b .Ltarget+9223372036854775808\n.Ltarget: nop",
    ] {
        assert!(assemble_for(Target::Aarch64, source.as_bytes())
            .expect_err("reject out-of-range AArch64 operand")
            .to_string()
            .starts_with("1:"));
    }
}
