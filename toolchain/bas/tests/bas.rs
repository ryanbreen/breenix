#![forbid(unsafe_code)]

use bas::{assemble, assemble_for, Target};

#[test]
fn aarch64_project_value_assembles() {
    let source = include_bytes!("../../corpus/project-aarch64/value.s");
    assemble_for(Target::Aarch64, source).expect("assemble the ARM64 project fixture");
}

fn aarch64_text(source: &str) -> Vec<u8> {
    let bytes = assemble_for(Target::Aarch64, source.as_bytes()).unwrap();
    let elf = bobj::elf::Elf::parse(&bytes).unwrap();
    let section = (0..elf.section_count())
        .find(|&index| elf.section_name(index).unwrap() == b".text")
        .unwrap();
    elf.section_data(section).unwrap().unwrap().to_vec()
}

#[test]
fn aarch64_binary128_load_store_encodings() {
    // DDI 0487 M.b §§C7.2.230, C7.2.387: size=00, opc=11/10 and
    // an unsigned offset scaled by 16, or an unscaled writeback immediate.
    let text = aarch64_text("ldr q7, [x9, #32]\nstr q0, [sp, #-16]!\nldr q1, [sp], #16");
    let expected = [0x3dc0_0927u32, 0x3c9f_0fe0, 0x3cc1_07e1];
    assert_eq!(text, expected.map(u32::to_le_bytes).concat());
}

#[test]
fn aarch64_reads_tpidr_el0() {
    // DDI 0487 M.b §§C6.2.286 and D24.2.203: MRS with
    // op0=3, op1=3, CRn=13, CRm=0, op2=2, and Rt in bits [4:0].
    let text = aarch64_text("mrs x0, tpidr_el0\nmrs x19, TPIDR_EL0\nmrs xzr, tpidr_el0");
    assert_eq!(
        text,
        [0xd53b_d040u32, 0xd53b_d053, 0xd53b_d05f]
            .map(u32::to_le_bytes)
            .concat()
    );
    for invalid in ["mrs w0, tpidr_el0", "mrs sp, tpidr_el0"] {
        assert!(assemble_for(Target::Aarch64, invalid.as_bytes()).is_err(), "{invalid}");
    }
}

#[test]
fn aarch64_mrs_fpcr_fpsr_encodings() {
    // DDI 0487 M.b C6.2.286 (MRS), C5.2.8 (FPCR) and
    // C5.2.10 (FPSR): op0=3, op1=3, CRn=4, CRm=4, op2=0/1.
    let text = aarch64_text("mrs x0, fpcr\nmrs x19, FPSR\nmrs xzr, fpcr");
    assert_eq!(text, [0xd53b_4400u32, 0xd53b_4433, 0xd53b_441f].map(u32::to_le_bytes).concat());
}

#[test]
fn aarch64_binary128_lane_move_encodings() {
    // DDI 0487 M.b §C7.2.157: sf=1, ftype=10, rmode=01,
    // opcode=110 (to X) or 111 (from X).
    let text = aarch64_text("fmov x3, v7.d[1]\nfmov v5.d[1], x12");
    let expected = [0x9eae_00e3u32, 0x9eaf_0185];
    assert_eq!(text, expected.map(u32::to_le_bytes).concat());
    for invalid in ["fmov w3, v7.d[1]", "fmov v5.d[0], x12", "fmov x3, v32.d[1]"] {
        assert!(assemble_for(Target::Aarch64, invalid.as_bytes()).is_err(), "{invalid}");
    }
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

#[test]
fn standalone_hash_comment_preserves_instruction_bytes() {
    for target in [bas::Target::X86_64, bas::Target::Aarch64] {
        let plain = bas::assemble_for(target, b".section .text\nnop\n").unwrap();
        let commented =
            bas::assemble_for(target, b".section .text\n  # a comment\n  nop\n").unwrap();
        assert_eq!(commented, plain);
    }
}
