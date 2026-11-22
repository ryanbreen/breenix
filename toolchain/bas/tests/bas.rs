#![forbid(unsafe_code)]

use bas::assemble;

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
}
