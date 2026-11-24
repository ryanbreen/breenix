use crate::cfi::{CfaRule, Endian, Program, RegisterRule, Rules};
use std::vec::Vec;

#[test]
fn cfa_rules_from_dwarf5_table_6_3() {
    // DWARF 5 Appendix D.6, Tables D.5 and D.6, encoded using Table 7.29.
    // D.6 specifies advance_loc(12) at foo+20, which reaches foo+68
    // at code alignment 4. The comment's "44/<caf>" does not match it.
    let cie = [
        0x0c, 7, 0, // def_cfa R7, 0
        0x08, 0, // same_value R0
        0x07, 1, // undefined R1
        0x07, 2, // undefined R2
        0x07, 3, // undefined R3
        0x08, 4, // same_value R4
        0x08, 5, // same_value R5
        0x08, 6, // same_value R6
        0x08, 7, // same_value R7
        0x09, 8, 1, // register R8, R1
        0, 0, 0, // padding
    ];
    let fde = [
        0x41, 0x0e, 12, // +4: def_cfa_offset 12
        0x41, 0x88, 1, // +8: offset R8, -4
        0x41, 0x86, 2, // +12: offset R6, -8
        0x41, 0x0d, 6, // +16: def_cfa_register R6
        0x41, 0x84, 3, // +20: offset R4, -12
        0x4c, 0xc4, // +68: restore R4
        0x41, 0xc6, 0x0d, 7, // +72: restore R6, def_cfa_register R7
        0x41, 0xc8, // +76: restore R8
        0x41, 0x0e, 0, // +80: def_cfa_offset 0
        0, 0, // padding
    ];
    let foo = 0x1000;
    let program = Program {
        cie: &cie,
        fde: &fde,
        initial_location: foo,
        address_range: 84,
        code_alignment: 4,
        data_alignment: -4,
        address_size: 4,
        endian: Endian::Little,
    };
    let mut actual = Vec::new();
    program
        .rows(Rules::default(), &mut [], |row| {
            actual.push((row.start - foo, row.end - foo, *row.rules));
        })
        .unwrap();

    use RegisterRule::{Offset, Register, SameValue as Same, Undefined as Undef};
    let expected_row = |start, end, register, offset, r4, r6, r8| {
        (
            start,
            end,
            Rules {
                cfa: Some(CfaRule::RegisterOffset { register, offset }),
                // D.4 describes R7 architecturally. D.5 encodes its CFI
                // column as same_value and the FDE never changes it.
                registers: [Same, Undef, Undef, Undef, r4, Same, r6, Same, r8],
            },
        )
    };
    assert_eq!(
        actual,
        [
            expected_row(0, 4, 7, 0, Same, Same, Register(1)),
            expected_row(4, 8, 7, 12, Same, Same, Register(1)),
            expected_row(8, 12, 7, 12, Same, Same, Offset(-4)),
            expected_row(12, 16, 7, 12, Same, Offset(-8), Offset(-4)),
            expected_row(16, 20, 6, 12, Same, Offset(-8), Offset(-4)),
            // Also covers the unchanged row at +64 shown in Table D.4.
            expected_row(20, 68, 6, 12, Offset(-12), Offset(-8), Offset(-4)),
            expected_row(68, 72, 6, 12, Same, Offset(-8), Offset(-4)),
            expected_row(72, 76, 7, 12, Same, Same, Offset(-4)),
            expected_row(76, 80, 7, 12, Same, Same, Register(1)),
            expected_row(80, 84, 7, 0, Same, Same, Register(1)),
        ]
    );
}
